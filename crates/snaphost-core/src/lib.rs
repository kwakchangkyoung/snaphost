pub mod merger;
pub mod parser;
pub mod profile;
pub mod proxy;
pub mod system;

pub use merger::{DomainConflict, MergeResult, merge_profiles};
pub use parser::{HostEntry, HostsLine, PortMapping, ValidationIssue, parse_hosts, validate_hosts};
pub use profile::Profile;
pub use proxy::{LocalReverseProxy, check_port_alive, extract_host_header};
pub use system::{ETC_HOSTS_PATH, SnapHostError, SnapHostManager, SystemStatus, flush_dns_cache};
