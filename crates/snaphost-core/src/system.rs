use crate::merger::{MergeResult, merge_profiles};
use crate::profile::Profile;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

pub const ETC_HOSTS_PATH: &str = "/private/etc/hosts";

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
        self.save_state()
    }

    pub fn status(&self) -> SystemStatus {
        SystemStatus {
            hosts_writable: is_file_writable(&self.hosts_path),
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

        profile.name = name;
        profile.content = content;
        profile.updated_at = chrono::Utc::now();
        let updated = profile.clone();
        self.save_state()?;
        Ok(updated)
    }

    pub fn delete_profile(&mut self, id: &str) -> Result<(), SnapHostError> {
        let idx = self
            .state
            .profiles
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| SnapHostError::ProfileNotFound(id.to_string()))?;

        if self.state.profiles[idx].is_system_base {
            return Err(SnapHostError::CannotDeleteSystemBase);
        }

        self.state.profiles.remove(idx);
        self.save_state()?;
        Ok(())
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
        let _ = flush_dns_cache();
        Ok(content)
    }

    /// Request one-time macOS administrator privilege so `/private/etc/hosts` becomes writable by the current user
    /// for instant 0ms switching without repeated password prompts.
    pub fn grant_one_time_permission(&self) -> Result<(), SnapHostError> {
        let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
        let target = self.hosts_path.display().to_string();
        let script = format!(
            "do shell script \"chown {user}:staff '{target}' && chmod 664 '{target}'\" with administrator privileges"
        );

        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(SnapHostError::PrivilegeError(stderr));
        }
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

fn write_hosts_content(hosts_path: &Path, content: &str) -> Result<(), SnapHostError> {
    // 1. Fast path: direct write if user already has write access
    if fs::write(hosts_path, content).is_ok() {
        return Ok(());
    }

    // 2. Fallback: write temp file and move/copy via osascript administrator prompt
    let tmp_path = std::env::temp_dir().join("snaphost_staged_hosts");
    fs::write(&tmp_path, content)?;

    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let target = hosts_path.display().to_string();
    let staged = tmp_path.display().to_string();

    // Also grant ownership during fallback so subsequent switches are instant (0ms)
    let script = format!(
        "do shell script \"cp '{staged}' '{target}' && chown {user}:staff '{target}' && chmod 664 '{target}'\" with administrator privileges"
    );

    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()?;

    let _ = fs::remove_file(&tmp_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(SnapHostError::PrivilegeError(stderr));
    }

    Ok(())
}

/// Flush macOS DNS caches (`dscacheutil` + `mDNSResponder`)
pub fn flush_dns_cache() -> Result<(), SnapHostError> {
    let _ = Command::new("dscacheutil").arg("-flushcache").output();
    let _ = Command::new("killall")
        .args(["-HUP", "mDNSResponder"])
        .output();
    Ok(())
}
