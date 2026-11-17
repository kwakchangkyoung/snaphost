use crate::merger::{MergeResult, merge_profiles};
use crate::profile::Profile;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use uuid::Uuid;

pub const ETC_HOSTS_PATH: &str = "/private/etc/hosts";
pub const SUDOERS_SNAPHOST_PATH: &str = "/private/etc/sudoers.d/snaphost";

#[derive(Debug, Error)]
pub enum SnapHostError {
    #[error("입출력 오류: {0}")]
    Io(#[from] std::io::Error),
    #[error("직렬화 오류: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("프로필을 찾을 수 없습니다: {0}")]
    ProfileNotFound(String),
    #[error("기본 시스템 프로필은 삭제할 수 없습니다.")]
    CannotDeleteSystemBase,
    #[error("권한 획득 또는 시스템 명령 실행 실패: {0}")]
    PrivilegeError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    pub hosts_writable: bool,
    pub has_original_backup: bool,
    pub app_data_dir: String,
    pub etc_hosts_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredState {
    pub profiles: Vec<Profile>,
    /// If true, selecting a non-base profile turns off other non-base profiles automatically (Single Switch Mode).
    /// If false, multiple profiles can be toggled concurrently (Combined Mode).
    pub single_switch_mode: bool,
}

pub struct SnapHostManager {
    data_dir: PathBuf,
    hosts_path: PathBuf,
    state: StoredState,
}

impl SnapHostManager {
    /// Initialize SnapHostManager using standard macOS Application Support directory
    pub fn init_default() -> Result<Self, SnapHostError> {
        let base_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("com.snaphost.app");
        Self::init_with_paths(base_dir, PathBuf::from(ETC_HOSTS_PATH))
    }

    pub fn init_with_paths(data_dir: PathBuf, hosts_path: PathBuf) -> Result<Self, SnapHostError> {
        fs::create_dir_all(&data_dir)?;

        // 1. Create permanent backup of original /etc/hosts on very first launch
        let backup_path = data_dir.join("original_hosts.bak");
        if !backup_path.exists() && hosts_path.exists() {
            if let Ok(current_hosts) = fs::read_to_string(&hosts_path) {
                let _ = fs::write(&backup_path, current_hosts);
            }
        }

        // 2. Load or initialize profiles.json
        let state_path = data_dir.join("profiles.json");
        let state = if state_path.exists() {
            let raw = fs::read_to_string(&state_path)?;
            serde_json::from_str::<StoredState>(&raw)?
        } else {
            let mut initial_profiles = vec![Profile::default_macos_base()];

            // Import existing /etc/hosts as a profile so user loses nothing
            if let Ok(existing) = fs::read_to_string(&hosts_path) {
                initial_profiles.push(Profile::new(
                    "기존 호스트 백업 (Imported)",
                    existing,
                    false,
                    false,
                ));
            }

            // Add a helpful sample profile
            initial_profiles.push(Profile::new(
                "로컬 개발 환경 (Sample)",
                "# SnapHost 샘플 프로필\n# 일반 호스트 매핑\n127.0.0.1       local.myapp.dev\n\n# SnapHost 확장 포트 매핑 (리버스 프록시 연동용)\n127.0.0.1:3000  web.myapp.dev\n127.0.0.1:8080  api.myapp.dev\n",
                false,
                false,
            ));

            let initial_state = StoredState {
                profiles: initial_profiles,
                single_switch_mode: false,
            };
            let serialized = serde_json::to_string_pretty(&initial_state)?;
            fs::write(&state_path, serialized)?;
            initial_state
        };

        Ok(Self {
            data_dir,
            hosts_path,
            state,
        })
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.state.profiles
    }

    pub fn single_switch_mode(&self) -> bool {
        self.state.single_switch_mode
    }

    pub fn set_single_switch_mode(&mut self, single_mode: bool) -> Result<(), SnapHostError> {
        self.state.single_switch_mode = single_mode;

        let mut modified_active = false;
        if single_mode {
            for p in &mut self.state.profiles {
                if !p.is_system_base && p.enabled {
                    p.enabled = false;
                    modified_active = true;
                }
            }
        }

        self.save_state()?;
        if modified_active {
            let _ = self.apply_to_system();
        }
        Ok(())
    }

    pub fn status(&self) -> SystemStatus {
        let sudoers_ok = if self.hosts_path == Path::new(ETC_HOSTS_PATH) {
            Path::new(SUDOERS_SNAPHOST_PATH).exists()
        } else {
            true
        };
        SystemStatus {
            hosts_writable: is_file_writable(&self.hosts_path) && sudoers_ok,
            has_original_backup: self.data_dir.join("original_hosts.bak").exists(),
            app_data_dir: self.data_dir.display().to_string(),
            etc_hosts_path: self.hosts_path.display().to_string(),
        }
    }

    pub fn create_profile(
        &mut self,
        name: String,
        content: String,
    ) -> Result<Profile, SnapHostError> {
        let profile = Profile::new(name, content, false, false);
        self.state.profiles.push(profile.clone());
        self.save_state()?;
        Ok(profile)
    }

    pub fn update_profile(
        &mut self,
        id: &str,
        name: String,
        content: String,
    ) -> Result<Profile, SnapHostError> {
        let profile = self
            .state
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| SnapHostError::ProfileNotFound(id.to_string()))?;

        if !profile.is_system_base {
            profile.name = name;
        }
        profile.content = content;
        profile.updated_at = chrono::Utc::now();
        let updated = profile.clone();
        self.save_state()?;
        Ok(updated)
    }

    pub fn delete_profile(&mut self, id: &str) -> Result<bool, SnapHostError> {
        let idx = self
            .state
            .profiles
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| SnapHostError::ProfileNotFound(id.to_string()))?;

        if self.state.profiles[idx].is_system_base {
            return Err(SnapHostError::CannotDeleteSystemBase);
        }

        let removed = self.state.profiles.remove(idx);
        self.save_state()?;
        Ok(removed.enabled)
    }

    /// Toggle a profile on/off and immediately apply the merged result to `/etc/hosts`.
    pub fn toggle_profile(&mut self, id: &str, enabled: bool) -> Result<MergeResult, SnapHostError> {
        let is_base = self
            .state
            .profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| SnapHostError::ProfileNotFound(id.to_string()))?
            .is_system_base;

        if self.state.single_switch_mode && enabled && !is_base {
            for p in &mut self.state.profiles {
                if !p.is_system_base {
                    p.enabled = p.id == id;
                }
            }
        } else if let Some(p) = self.state.profiles.iter_mut().find(|p| p.id == id) {
            p.enabled = enabled;
        }

        self.save_state()?;
        self.apply_to_system()
    }

    /// Generate the merged `/etc/hosts` preview without writing to disk.
    pub fn preview_merge(&self) -> MergeResult {
        merge_profiles(&self.state.profiles)
    }

    /// Write merged profiles to `/etc/hosts` and flush macOS DNS cache.
    pub fn apply_to_system(&self) -> Result<MergeResult, SnapHostError> {
        let merged = self.preview_merge();
        write_hosts_content(&self.hosts_path, &merged.hosts_content)?;
        let _ = flush_dns_cache();
        Ok(merged)
    }

    /// Restore original `/etc/hosts` backup (Panic Restore).
    pub fn restore_original_backup(&mut self) -> Result<String, SnapHostError> {
        let backup_path = self.data_dir.join("original_hosts.bak");
        let content = if backup_path.exists() {
            fs::read_to_string(&backup_path)?
        } else {
            Profile::default_macos_base().content
        };

        write_hosts_content(&self.hosts_path, &content)?;
        for p in &mut self.state.profiles {
            p.enabled = p.is_system_base;
        }
        self.save_state()?;
        let _ = flush_dns_cache();
        Ok(content)
    }

    /// Request one-time macOS administrator privilege so `/private/etc/hosts` becomes writable by the current user
    /// and `/private/etc/sudoers.d/snaphost` allows passwordless `mDNSResponder` DNS cache flush for instant 0ms switching.
    pub fn grant_one_time_permission(&self) -> Result<(), SnapHostError> {
        let user = sanitized_username();
        let target = escape_for_applescript_shell(&self.hosts_path.display().to_string());
        let sudoers_staged_path =
            std::env::temp_dir().join(format!("snaphost_sudoers_{}", Uuid::new_v4()));
        let sudoers_rule = format!(
            "# SnapHost passwordless DNS cache flush\n{user} ALL=(root) NOPASSWD: /usr/bin/killall -HUP mDNSResponder, /usr/bin/dscacheutil -flushcache\n"
        );
        fs::write(&sudoers_staged_path, sudoers_rule)?;
        let staged_sudoers =
            escape_for_applescript_shell(&sudoers_staged_path.display().to_string());
        let sudoers_dest = escape_for_applescript_shell(SUDOERS_SNAPHOST_PATH);

        let script = format!(
            "do shell script \"chown {user}:staff '{target}' && chmod 644 '{target}' && mkdir -p /private/etc/sudoers.d && cp '{staged_sudoers}' '{sudoers_dest}' && chown root:wheel '{sudoers_dest}' && chmod 440 '{sudoers_dest}' && (/usr/sbin/visudo -cf '{sudoers_dest}' >/dev/null 2>&1 || rm -f '{sudoers_dest}')\" with administrator privileges"
        );

        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output();

        let _ = fs::remove_file(&sudoers_staged_path);
        let output = output?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(SnapHostError::PrivilegeError(stderr));
        }
        let _ = flush_dns_cache();
        Ok(())
    }

    fn save_state(&self) -> Result<(), SnapHostError> {
        let state_path = self.data_dir.join("profiles.json");
        let serialized = serde_json::to_string_pretty(&self.state)?;
        fs::write(state_path, serialized)?;
        Ok(())
    }
}

fn is_file_writable(path: &Path) -> bool {
    fs::OpenOptions::new().write(true).open(path).is_ok()
}

fn sanitized_username() -> String {
    let raw = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect();
    if cleaned.is_empty() {
        "root".to_string()
    } else {
        cleaned
    }
}

/// Escape a string so it can be safely embedded inside single quotes `'...'` within an AppleScript `"..."` string.
pub(crate) fn escape_for_applescript_shell(input: &str) -> String {
    let shell_escaped = input.replace('\'', "'\\''");
    shell_escaped.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_hosts_content(hosts_path: &Path, content: &str) -> Result<(), SnapHostError> {
    // 1. Fast path: direct write + fsync if user already has write access
    if let Ok(mut file) = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(hosts_path)
    {
        if file.write_all(content.as_bytes()).is_ok() {
            let _ = file.sync_all();
            return Ok(());
        }
    }

    // 2. Fallback: write unique temp file and copy via osascript administrator prompt
    let tmp_path = std::env::temp_dir().join(format!("snaphost_staged_{}", Uuid::new_v4()));
    fs::write(&tmp_path, content)?;

    let user = sanitized_username();
    let target = escape_for_applescript_shell(&hosts_path.display().to_string());
    let staged = escape_for_applescript_shell(&tmp_path.display().to_string());

    let sudoers_staged_path =
        std::env::temp_dir().join(format!("snaphost_sudoers_{}", Uuid::new_v4()));
    let sudoers_rule = format!(
        "# SnapHost passwordless DNS cache flush\n{user} ALL=(root) NOPASSWD: /usr/bin/killall -HUP mDNSResponder, /usr/bin/dscacheutil -flushcache\n"
    );
    let _ = fs::write(&sudoers_staged_path, sudoers_rule);
    let staged_sudoers =
        escape_for_applescript_shell(&sudoers_staged_path.display().to_string());
    let sudoers_dest = escape_for_applescript_shell(SUDOERS_SNAPHOST_PATH);

    // Also grant ownership + sudoers rule during fallback so subsequent switches and DNS flushes are instant (0ms)
    let script = format!(
        "do shell script \"cp '{staged}' '{target}' && chown {user}:staff '{target}' && chmod 644 '{target}' && mkdir -p /private/etc/sudoers.d && cp '{staged_sudoers}' '{sudoers_dest}' && chown root:wheel '{sudoers_dest}' && chmod 440 '{sudoers_dest}' && (/usr/sbin/visudo -cf '{sudoers_dest}' >/dev/null 2>&1 || rm -f '{sudoers_dest}')\" with administrator privileges"
    );

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output();

    let _ = fs::remove_file(&tmp_path);
    let _ = fs::remove_file(&sudoers_staged_path);
    let output = output?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(SnapHostError::PrivilegeError(stderr));
    }

    Ok(())
}

/// Flush macOS DNS caches (`dscacheutil` + `mDNSResponder`)
pub fn flush_dns_cache() -> Result<(), SnapHostError> {
    let _ = Command::new("/usr/bin/dscacheutil")
        .arg("-flushcache")
        .output();
    let _ = Command::new("/usr/bin/sudo")
        .args(["-n", "/usr/bin/dscacheutil", "-flushcache"])
        .output();

    // Send SIGHUP to mDNSResponder via passwordless sudo if configured, falling back to direct call
    let sudo_hup = Command::new("/usr/bin/sudo")
        .args(["-n", "/usr/bin/killall", "-HUP", "mDNSResponder"])
        .output();
    if !sudo_hup.map(|o| o.status.success()).unwrap_or(false) {
        let _ = Command::new("/usr/bin/killall")
            .args(["-HUP", "mDNSResponder"])
            .output();
    }
    Ok(())
}
