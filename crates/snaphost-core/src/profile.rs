use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// True if this profile is the required macOS system base (`localhost`, `broadcasthost`, etc.)
    pub is_system_base: bool,
    pub content: String,
    pub updated_at: DateTime<Utc>,
}

impl Profile {
    pub fn new(
        name: impl Into<String>,
        content: impl Into<String>,
        enabled: bool,
        is_system_base: bool,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            enabled,
            is_system_base,
            content: content.into(),
            updated_at: Utc::now(),
        }
    }

    pub fn default_macos_base() -> Self {
        let default_content = "\
##
# Host Database
#
# localhost is used to configure the loopback interface
# when the system is booting.  Do not change this entry.
##
127.0.0.1       localhost
255.255.255.255 broadcasthost
::1             localhost
";
        Self::new("macOS Default (Base)", default_content, true, true)
    }
}
