use serde::{Deserialize, Serialize};
use std::process::Command;

pub const GITHUB_REPO: &str = "kwakchangkyoung/snaphost";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_notes: String,
    pub release_url: String,
    pub download_url: Option<String>,
    pub asset_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Query GitHub Releases API for the latest version of SnapHost.
pub fn check_for_updates() -> Result<UpdateInfo, String> {
    let api_url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let output = Command::new("/usr/bin/curl")
        .args([
            "-sSL",
            "--max-time",
            "8",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: SnapHost-Updater",
            &api_url,
        ])
        .output()
        .map_err(|e| format!("네트워크 요청 실행 실패: {e}"))?;

    if !output.status.success() {
        return Err("GitHub Releases 정보를 가져올 수 없습니다.".to_string());
    }

    let body_str = String::from_utf8_lossy(&output.stdout);
    // If no release exists yet on GitHub, API returns `{"message": "Not Found", ...}`
    if body_str.contains("\"message\":\"Not Found\"") || body_str.contains("\"message\": \"Not Found\"") {
        return Ok(UpdateInfo {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: CURRENT_VERSION.to_string(),
            update_available: false,
            release_notes: "아직 게시된 GitHub 릴리즈가 없습니다.".to_string(),
            release_url: format!("https://github.com/{GITHUB_REPO}/releases"),
            download_url: None,
            asset_name: None,
        });
    }

    let release: GithubRelease = serde_json::from_str(&body_str)
        .map_err(|e| format!("릴리즈 응답 파싱 실패: {e}"))?;

    let latest_clean = release.tag_name.trim_start_matches('v').to_string();
    let update_available = is_newer_version(CURRENT_VERSION, &latest_clean);

    // Find best macOS installer asset (.app.tar.gz or .dmg)
    let chosen_asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(".app.tar.gz"))
        .or_else(|| release.assets.iter().find(|a| a.name.ends_with(".dmg")));

    Ok(UpdateInfo {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: latest_clean,
        update_available,
        release_notes: release.body.unwrap_or_default(),
        release_url: release.html_url,
        download_url: chosen_asset.map(|a| a.browser_download_url.clone()),
        asset_name: chosen_asset.map(|a| a.name.clone()),
    })
}

/// Download the latest `.dmg` or `.app.tar.gz` asset from GitHub Releases, replace `/Applications/SnapHost.app`,
/// clear macOS quarantine flags (`xattr -cr`), and relaunch the app.
pub fn download_and_install_update(download_url: &str, asset_name: &str) -> Result<(), String> {
    let tmp_dir = std::env::temp_dir().join("snaphost_updater");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;

    let archive_path = tmp_dir.join(asset_name);
    let archive_str = archive_path.display().to_string();

    // 1. Download asset via curl
    let status = Command::new("/usr/bin/curl")
        .args(["-fL", "--max-time", "120", "-o", &archive_str, download_url])
        .status()
        .map_err(|e| format!("업데이트 파일 다운로드 실패: {e}"))?;

    if !status.success() {
        return Err("업데이트 파일 다운로드 중 오류가 발생했습니다.".to_string());
    }

    // 2. Install depending on asset type (.app.tar.gz vs .dmg)
    if asset_name.ends_with(".app.tar.gz") {
        let extract_status = Command::new("/usr/bin/tar")
            .args(["-xzf", &archive_str, "-C", &tmp_dir.display().to_string()])
            .status()
            .map_err(|e| format!("압축 해제 실패: {e}"))?;

        if !extract_status.success() {
            return Err("업데이트 패키지 압축 해제에 실패했습니다.".to_string());
        }

        let extracted_app = tmp_dir.join("SnapHost.app");
        install_app_bundle(&extracted_app)?;
    } else if asset_name.ends_with(".dmg") {
        let mount_point = tmp_dir.join("mnt");
        std::fs::create_dir_all(&mount_point).map_err(|e| e.to_string())?;
        let mount_str = mount_point.display().to_string();

        let attach = Command::new("/usr/bin/hdiutil")
            .args([
                "attach",
                &archive_str,
                "-mountpoint",
                &mount_str,
                "-nobrowse",
                "-readonly",
                "-quiet",
            ])
            .status()
            .map_err(|e| format!("DMG 마운트 실패: {e}"))?;

        if !attach.success() {
            return Err("DMG 파일을 마운트할 수 없습니다.".to_string());
        }

        let mounted_app = mount_point.join("SnapHost.app");
        let install_res = install_app_bundle(&mounted_app);

        let _ = Command::new("/usr/bin/hdiutil")
            .args(["detach", &mount_str, "-quiet", "-force"])
            .status();

        install_res?;
    } else {
        return Err(format!("지원하지 않는 업데이트 파일 형식입니다: {asset_name}"));
    }

    // 3. Relaunch updated app from /Applications/SnapHost.app after 1 second
    let _ = Command::new("/bin/sh")
        .args([
            "-c",
            "sleep 1 && open -n /Applications/SnapHost.app",
        ])
        .spawn();

    Ok(())
}

fn install_app_bundle(source_app: &std::path::Path) -> Result<(), String> {
    let dest_app = "/Applications/SnapHost.app";
    let src_str = source_app.display().to_string();

    // Try direct copy first
    let direct = Command::new("/bin/sh")
        .args([
            "-c",
            &format!("rm -rf '{dest_app}' && cp -R '{src_str}' '{dest_app}' && xattr -cr '{dest_app}'"),
        ])
        .status();

    if let Ok(s) = direct {
        if s.success() {
            return Ok(());
        }
    }

    // Fallback: request macOS admin privileges via osascript
    let script = format!(
        "do shell script \"rm -rf '{dest_app}' && cp -R '{src_str}' '{dest_app}' && xattr -cr '{dest_app}'\" with administrator privileges"
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("설치 권한 요청 실패: {e}"))?;

    if !output.status.success() {
        return Err("Applications 폴더에 새 버전을 복사하지 못했습니다.".to_string());
    }
    Ok(())
}

/// Compare two semantic version strings (e.g. `0.1.0` vs `0.2.0`).
/// Returns `true` if `latest` is strictly greater than `current`.
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse_ver = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0))
            .collect()
    };

    let c = parse_ver(current);
    let l = parse_ver(latest);
    let max_len = c.len().max(l.len());

    for i in 0..max_len {
        let cv = c.get(i).copied().unwrap_or(0);
        let lv = l.get(i).copied().unwrap_or(0);
        if lv > cv {
            return true;
        } else if lv < cv {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_comparison() {
        assert!(is_newer_version("0.1.0", "0.1.1"));
        assert!(is_newer_version("0.1.0", "v0.2.0"));
        assert!(is_newer_version("0.1.9", "1.0.0"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.2.0", "0.1.9"));
    }
}
