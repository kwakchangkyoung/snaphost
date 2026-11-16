pub mod merger;
pub mod parser;
pub mod profile;
pub mod proxy;
pub mod system;
pub mod updater;

pub use merger::{DomainConflict, MergeResult, merge_profiles};
pub use parser::{HostEntry, HostsLine, PortMapping, ValidationIssue, parse_hosts, validate_hosts};
pub use profile::Profile;
pub use proxy::{LocalReverseProxy, check_port_alive, extract_host_header};
pub use system::{ETC_HOSTS_PATH, SnapHostError, SnapHostManager, SystemStatus, flush_dns_cache};
pub use updater::{
    CURRENT_VERSION, UpdateInfo, check_for_updates, download_and_install_update, is_newer_version,
};
