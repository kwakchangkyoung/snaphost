use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// A single parsed line from a hosts profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostsLine {
    /// Blank line or whitespace-only line
    Empty,
    /// Full-line comment (starts with `#`)
    Comment(String),
    /// Valid host entry (supports both standard IP and SnapHost `IP:PORT` syntax)
    Entry(HostEntry),
    /// Line that failed validation
    Invalid {
        line_number: usize,
        raw: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    pub line_number: usize,
    pub ip: IpAddr,
    /// Optional port for SnapHost local reverse proxy (e.g., `127.0.0.1:3000 api.local`)
    pub port: Option<u16>,
    pub domains: Vec<String>,
    pub inline_comment: Option<String>,
}

/// Extracted local port-forwarding rule for the built-in reverse proxy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMapping {
    pub domain: String,
    pub target_ip: IpAddr,
    pub target_port: u16,
    pub profile_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub line_number: usize,
    pub raw: String,
    pub reason: String,
}

/// Parse a single line of hosts content.
pub fn parse_line(line_number: usize, raw_line: &str) -> HostsLine {
    let trimmed = raw_line.trim();
    if trimmed.is_empty() {
        return HostsLine::Empty;
    }
    if trimmed.starts_with('#') {
        return HostsLine::Comment(trimmed.to_string());
    }

    // Split inline comment if present
    let (code_part, inline_comment) = match trimmed.split_once('#') {
        Some((code, comment)) => (code.trim(), Some(comment.trim().to_string())),
        None => (trimmed, None),
    };

    let mut tokens = code_part.split_whitespace();
    let Some(target_token) = tokens.next() else {
        return HostsLine::Empty;
    };

    let (ip, port) = match parse_ip_and_optional_port(target_token) {
        Ok(res) => res,
        Err(reason) => {
            return HostsLine::Invalid {
                line_number,
                raw: raw_line.to_string(),
                reason,
            };
        }
    };

    let domains: Vec<String> = tokens.map(|s| s.to_string()).collect();
    if domains.is_empty() {
        return HostsLine::Invalid {
            line_number,
            raw: raw_line.to_string(),
            reason: "IP 주소 뒤에 최소 1개 이상의 도메인이 필요합니다.".to_string(),
        };
    }

    for domain in &domains {
        if !is_valid_domain(domain) {
            return HostsLine::Invalid {
                line_number,
                raw: raw_line.to_string(),
                reason: format!("유효하지 않은 도메인 형식입니다: '{domain}'"),
            };
        }
    }

    HostsLine::Entry(HostEntry {
        line_number,
        ip,
        port,
        domains,
        inline_comment,
    })
}

/// Parse an entire hosts file string into structured lines.
pub fn parse_hosts(content: &str) -> Vec<HostsLine> {
    content
        .lines()
        .enumerate()
        .map(|(idx, line)| parse_line(idx + 1, line))
        .collect()
}

/// Validate hosts text and return any syntax errors with line numbers.
pub fn validate_hosts(content: &str) -> Vec<ValidationIssue> {
    parse_hosts(content)
        .into_iter()
        .filter_map(|line| match line {
            HostsLine::Invalid {
                line_number,
                raw,
                reason,
            } => Some(ValidationIssue {
                line_number,
                raw,
                reason,
            }),
            _ => None,
        })
        .collect()
}

fn parse_ip_and_optional_port(token: &str) -> Result<(IpAddr, Option<u16>), String> {
    // 1. Try standard IP first (handles both IPv4 and IPv6 like `::1` or `fe80::1%lo0`)
    let clean_token = token.split('%').next().unwrap_or(token);
    if let Ok(ip) = clean_token.parse::<IpAddr>() {
        return Ok((ip, None));
    }

    // 2. Bracketed IPv6 with port: `[::1]:3000`
    if let Some(rest) = token.strip_prefix('[') {
        if let Some((ip_part, port_part)) = rest.split_once("]:") {
            let ip = ip_part
                .parse::<IpAddr>()
                .map_err(|_| format!("유효하지 않은 IPv6 주소입니다: '{ip_part}'"))?;
            let port = parse_port(port_part)?;
            return Ok((ip, Some(port)));
        }
    }

    // 3. IPv4 with port: `127.0.0.1:3000`
    if let Some((ip_part, port_part)) = token.rsplit_once(':') {
        if !ip_part.contains(':') {
            let ip = ip_part
                .parse::<IpAddr>()
                .map_err(|_| format!("유효하지 않은 IP 주소입니다: '{ip_part}'"))?;
            let port = parse_port(port_part)?;
            return Ok((ip, Some(port)));
        }
    }

    Err(format!("유효하지 않은 IP 또는 IP:PORT 형식입니다: '{token}'"))
}

fn parse_port(port_str: &str) -> Result<u16, String> {
    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("유효하지 않은 포트 번호입니다: '{port_str}'"))?;
    if port == 0 {
        return Err("포트 번호는 1 ~ 65535 사이여야 합니다.".to_string());
    }
    Ok(port)
}

fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    domain
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
        && !domain.starts_with('-')
        && !domain.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_standard_hosts_line() {
        let parsed = parse_line(1, "127.0.0.1   localhost my.local # local dev");
        assert_eq!(
            parsed,
            HostsLine::Entry(HostEntry {
                line_number: 1,
                ip: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                port: None,
                domains: vec!["localhost".to_string(), "my.local".to_string()],
                inline_comment: Some("local dev".to_string()),
            })
        );
    }

    #[test]
    fn test_extended_port_syntax() {
        let parsed = parse_line(2, "127.0.0.1:3000 web.myapp.local api.myapp.local");
        assert_eq!(
            parsed,
            HostsLine::Entry(HostEntry {
                line_number: 2,
                ip: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                port: Some(3000),
                domains: vec!["web.myapp.local".to_string(), "api.myapp.local".to_string()],
                inline_comment: None,
            })
        );
    }

    #[test]
    fn test_ipv6_and_zone_index() {
        let parsed = parse_line(3, "fe80::1%lo0 localhost");
        assert_eq!(
            parsed,
            HostsLine::Entry(HostEntry {
                line_number: 3,
                ip: IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
                port: None,
                domains: vec!["localhost".to_string()],
                inline_comment: None,
            })
        );
    }

    #[test]
    fn test_invalid_line_reporting() {
        let issues = validate_hosts("999.999.999.999 bad.domain\n127.0.0.1");
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].line_number, 1);
        assert_eq!(issues[1].line_number, 2);
    }
}
