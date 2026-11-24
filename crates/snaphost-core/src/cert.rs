use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use time::{Duration, OffsetDateTime};

pub const CA_COMMON_NAME: &str = "SnapHost Local CA";
const CA_CERT_FILE: &str = "snaphost-ca.pem";
const CA_KEY_FILE: &str = "snaphost-ca-key.pem";

/// Local root CA used to issue per-domain certificates for the HTTPS proxy.
pub struct LocalCa {
    cert_path: PathBuf,
    issuer: Issuer<'static, KeyPair>,
    leaf_cache: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl std::fmt::Debug for LocalCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalCa").field("cert_path", &self.cert_path).finish()
    }
}

fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, CA_COMMON_NAME);
    dn.push(DnType::OrganizationName, "SnapHost");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params
}

fn ca_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("ca")
}

impl LocalCa {
    pub fn exists(data_dir: &Path) -> bool {
        let dir = ca_dir(data_dir);
        dir.join(CA_CERT_FILE).exists() && dir.join(CA_KEY_FILE).exists()
    }

    /// Load the CA from `<data_dir>/ca`, generating a new one on first use.
    pub fn load_or_create(data_dir: &Path) -> Result<Self, String> {
        let dir = ca_dir(data_dir);
        let cert_path = dir.join(CA_CERT_FILE);
        let key_path = dir.join(CA_KEY_FILE);

        let key = if Self::exists(data_dir) {
            let pem = fs::read_to_string(&key_path).map_err(|e| format!("CA 키 읽기 실패: {e}"))?;
            KeyPair::from_pem(&pem).map_err(|e| format!("CA 키 파싱 실패: {e}"))?
        } else {
            fs::create_dir_all(&dir).map_err(|e| format!("CA 디렉터리 생성 실패: {e}"))?;
            let key = KeyPair::generate().map_err(|e| format!("CA 키 생성 실패: {e}"))?;
            let mut params = ca_params();
            let now = OffsetDateTime::now_utc();
            params.not_before = now - Duration::days(1);
            params.not_after = now + Duration::days(3650);
            let cert = params
                .self_signed(&key)
                .map_err(|e| format!("CA 인증서 생성 실패: {e}"))?;
            write_private(&key_path, key.serialize_pem().as_bytes())?;
            fs::write(&cert_path, cert.pem()).map_err(|e| format!("CA 인증서 저장 실패: {e}"))?;
            key
        };

        Ok(Self {
            cert_path,
            issuer: Issuer::new(ca_params(), key),
            leaf_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Path of the CA certificate inside `data_dir` (whether or not it exists yet).
    pub fn cert_path_in(data_dir: &Path) -> PathBuf {
        ca_dir(data_dir).join(CA_CERT_FILE)
    }

    pub fn cert_path(&self) -> &Path {
        &self.cert_path
    }

    /// Return a (cached) certificate for `domain`, signed by this CA.
    pub fn certified_key_for(&self, domain: &str) -> Result<Arc<CertifiedKey>, String> {
        let domain = domain.to_lowercase();
        let mut cache = self.leaf_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ck) = cache.get(&domain) {
            return Ok(Arc::clone(ck));
        }

        let key = KeyPair::generate().map_err(|e| e.to_string())?;
        let mut params = CertificateParams::new(vec![domain.clone()]).map_err(|e| e.to_string())?;
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, domain.as_str());
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        let now = OffsetDateTime::now_utc();
        // Apple rejects TLS server certificates valid for more than 825 days.
        params.not_before = now - Duration::days(1);
        params.not_after = now + Duration::days(365);
        let cert = params
            .signed_by(&key, &self.issuer)
            .map_err(|e| e.to_string())?;

        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let signing_key =
            rustls::crypto::ring::sign::any_supported_type(&key_der).map_err(|e| e.to_string())?;
        let ck = Arc::new(CertifiedKey::new(
            vec![CertificateDer::from(cert.der().to_vec())],
            signing_key,
        ));
        cache.insert(domain, Arc::clone(&ck));
        Ok(ck)
    }
}

fn write_private(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("CA 키 저장 실패: {e}"))?;
    f.write_all(contents).map_err(|e| format!("CA 키 저장 실패: {e}"))
}

fn login_keychain() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join("Library/Keychains/login.keychain-db")
}

/// Whether macOS currently trusts the CA certificate at `cert_path` for TLS.
pub fn is_ca_trusted(cert_path: &Path) -> bool {
    Command::new("security")
        .args(["verify-cert", "-L", "-p", "ssl", "-c"])
        .arg(cert_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Add the CA to the user's login keychain as a trusted root (macOS shows a password prompt).
pub fn install_ca_trust(cert_path: &Path) -> Result<(), String> {
    let out = Command::new("security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-p", "ssl", "-k"])
        .arg(login_keychain())
        .arg(cert_path)
        .output()
        .map_err(|e| format!("security 실행 실패: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "인증서 신뢰 등록 실패: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Remove the CA's trust settings and keychain entry, then delete the CA files.
pub fn remove_ca(data_dir: &Path) -> Result<(), String> {
    let dir = ca_dir(data_dir);
    let cert_path = dir.join(CA_CERT_FILE);
    if cert_path.exists() {
        let _ = Command::new("security")
            .arg("remove-trusted-cert")
            .arg(&cert_path)
            .output();
    }
    // Remove every keychain copy of the CA (add-trusted-cert imports it into the login keychain).
    for _ in 0..8 {
        let ok = Command::new("security")
            .args(["delete-certificate", "-c", CA_COMMON_NAME])
            .arg(login_keychain())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            break;
        }
    }
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("CA 파일 삭제 실패: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_ca_and_reloads_with_same_key() {
        let dir = std::env::temp_dir().join(format!("snaphost-ca-test-{}", uuid::Uuid::new_v4()));
        let ca = LocalCa::load_or_create(&dir).unwrap();
        let pem1 = fs::read_to_string(ca.cert_path()).unwrap();
        let mode = fs::metadata(dir.join("ca").join(CA_KEY_FILE)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.permissions().mode() & 0o777, 0o600);

        let ca2 = LocalCa::load_or_create(&dir).unwrap();
        assert_eq!(pem1, fs::read_to_string(ca2.cert_path()).unwrap());

        let a = ca2.certified_key_for("Dream.Example.com").unwrap();
        let b = ca2.certified_key_for("dream.example.com").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let _ = fs::remove_dir_all(&dir);
    }
}
