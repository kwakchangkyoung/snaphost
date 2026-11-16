use crate::parser::PortMapping;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

/// Ultra-lightweight Host-header Reverse Proxy (supports HTTP/1.1 + WebSocket upgrades for Vite/Next.js HMR)
#[derive(Clone)]
pub struct LocalReverseProxy {
    routes: Arc<RwLock<HashMap<String, SocketAddr>>>,
    running: Arc<AtomicBool>,
    listen_port: Arc<RwLock<u16>>,
}

impl Default for LocalReverseProxy {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalReverseProxy {
    pub fn new() -> Self {
        Self {
            routes: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(AtomicBool::new(false)),
            listen_port: Arc::new(RwLock::new(80)),
        }
    }

    /// Update active domain -> `IP:PORT` routing table in 0ms without restarting the listener.
    pub fn update_mappings(&self, mappings: &[PortMapping]) {
        if let Ok(mut table) = self.routes.write() {
            table.clear();
            for m in mappings {
                table.insert(
                    m.domain.to_lowercase(),
                    SocketAddr::new(m.target_ip, m.target_port),
                );
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn current_port(&self) -> u16 {
        *self.listen_port.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Start listening on `127.0.0.1:<port>` (defaults to 80, falls back to 8880 if port 80 requires root).
    pub fn start(&self, preferred_port: u16) -> Result<u16, String> {
        if self.is_running() {
            return Ok(self.current_port());
        }

        let listener = TcpListener::bind(("127.0.0.1", preferred_port))
            .or_else(|_| TcpListener::bind(("127.0.0.1", 8880)))
            .map_err(|e| format!("프록시 포트 바인딩 실패: {e}"))?;

        let bound_port = listener
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(preferred_port);
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Non-blocking 설정 실패: {e}"))?;

        if let Ok(mut p) = self.listen_port.write() {
            *p = bound_port;
        }
        self.running.store(true, Ordering::SeqCst);

        let routes = Arc::clone(&self.routes);
        let running = Arc::clone(&self.running);

        thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((client, _)) => {
                        let routes_ref = Arc::clone(&routes);
                        thread::spawn(move || {
                            let _ = handle_client(client, routes_ref);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(15));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(bound_port)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

fn handle_client(
    mut client: TcpStream,
    routes: Arc<RwLock<HashMap<String, SocketAddr>>>,
) -> std::io::Result<()> {
    let _ = client.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = [0u8; 8192];
    let n = client.read(&mut buffer)?;
    if n == 0 {
        return Ok(());
    }

    let req_head = String::from_utf8_lossy(&buffer[..n]);
    let Some(host) = extract_host_header(&req_head) else {
        let _ = client.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 16\r\n\r\nMissing Host hdr");
        return Ok(());
    };

    let target_addr = routes
        .read()
        .ok()
        .and_then(|table| table.get(&host.to_lowercase()).copied());

    let Some(addr) = target_addr else {
        let body = format!(
            "SnapHost Proxy: No active port mapping found for host '{host}'\n"
        );
        let resp = format!(
            "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = client.write_all(resp.as_bytes());
        return Ok(());
    };

    let mut upstream = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(s) => s,
        Err(e) => {
            let body = format!(
                "SnapHost Proxy: Could not connect to {host} -> {addr} ({e})\n"
            );
            let resp = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = client.write_all(resp.as_bytes());
            return Ok(());
        }
    };

    // Forward initial request bytes
    upstream.write_all(&buffer[..n])?;

    // Bi-directional TCP stream relay (supports HTTP & WebSockets)
    let mut client_clone = client.try_clone()?;
    let mut upstream_clone = upstream.try_clone()?;

    let t1 = thread::spawn(move || std::io::copy(&mut client_clone, &mut upstream_clone));
    let _ = std::io::copy(&mut upstream, &mut client);
    let _ = t1.join();

    Ok(())
}

pub fn extract_host_header(raw_http: &str) -> Option<String> {
    for line in raw_http.lines() {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("host") {
                let host_only = v.trim().split(':').next().unwrap_or("").trim();
                if !host_only.is_empty() {
                    return Some(host_only.to_string());
                }
            }
        }
    }
    None
}

/// Check if a local TCP port (`IP:PORT`) is currently accepting connections.
pub fn check_port_alive(ip: IpAddr, port: u16) -> bool {
    let addr = SocketAddr::new(ip, port);
    TcpStream::connect_timeout(&addr, Duration::from_millis(80)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_host_header() {
        let req = "GET /index.html HTTP/1.1\r\nHost: web.myapp.dev:8880\r\nUser-Agent: curl/8.0\r\n\r\n";
        assert_eq!(
            extract_host_header(req),
            Some("web.myapp.dev".to_string())
        );
    }
}
