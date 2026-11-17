use crate::parser::PortMapping;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
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
                table
                    .entry(m.domain.to_lowercase())
                    .or_insert_with(|| SocketAddr::new(m.target_ip, m.target_port));
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn current_port(&self) -> u16 {
        *self.listen_port.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Start listening on `127.0.0.1:<port>` (and `[::1]:<port>` if available).
    /// Defaults to 80, falls back to 8880 if port 80 requires root.
    pub fn start(&self, preferred_port: u16) -> Result<u16, String> {
        if self.is_running() {
            return Ok(self.current_port());
        }

        let listener_v4 = TcpListener::bind(("127.0.0.1", preferred_port))
            .or_else(|_| TcpListener::bind(("127.0.0.1", 8880)))
            .map_err(|e| format!("프록시 포트 바인딩 실패: {e}"))?;

        let bound_port = listener_v4
            .local_addr()
            .map(|a| a.port())
            .unwrap_or(preferred_port);

        if let Ok(mut p) = self.listen_port.write() {
            *p = bound_port;
        }
        self.running.store(true, Ordering::SeqCst);

        Self::spawn_accept_loop(
            listener_v4,
            Arc::clone(&self.routes),
            Arc::clone(&self.running),
        );

        // Best-effort IPv6 loopback listener on the same port
        if let Ok(listener_v6) = TcpListener::bind(("::1", bound_port)) {
            Self::spawn_accept_loop(
                listener_v6,
                Arc::clone(&self.routes),
                Arc::clone(&self.running),
            );
        }

        Ok(bound_port)
    }

    fn spawn_accept_loop(
        listener: TcpListener,
        routes: Arc<RwLock<HashMap<String, SocketAddr>>>,
        running: Arc<AtomicBool>,
    ) {
        thread::spawn(move || {
            for incoming in listener.incoming() {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                match incoming {
                    Ok(client) => {
                        let routes_ref = Arc::clone(&routes);
                        thread::spawn(move || {
                            let _ = handle_client(client, routes_ref);
                        });
                    }
                    Err(_) => {
                        if !running.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                }
            }
        });
    }

    pub fn stop(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            let port = self.current_port();
            // Wake up blocking accept() loops cleanly
            let _ = TcpStream::connect_timeout(
                &SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_millis(50),
            );
            let _ = TcpStream::connect_timeout(
                &SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
                Duration::from_millis(50),
            );
        }
    }
}

fn read_http_head(client: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    const MAX_HEADER_BYTES: usize = 32 * 1024;

    loop {
        let n = client.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n")
            || buf.windows(2).any(|w| w == b"\n\n")
            || buf.len() >= MAX_HEADER_BYTES
        {
            break;
        }
    }
    Ok(buf)
}

fn handle_client(
    mut client: TcpStream,
    routes: Arc<RwLock<HashMap<String, SocketAddr>>>,
) -> std::io::Result<()> {
    let _ = client.set_read_timeout(Some(Duration::from_secs(5)));
    let initial_bytes = read_http_head(&mut client)?;
    if initial_bytes.is_empty() {
        return Ok(());
    }

    // Clear read timeout so long-lived WebSocket (HMR) or Keep-Alive connections don't drop
    let _ = client.set_read_timeout(None);

    let req_head = String::from_utf8_lossy(&initial_bytes);
    let Some(host) = extract_host_header(&req_head) else {
        let _ = client.write_all(
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 16\r\nConnection: close\r\n\r\nMissing Host hdr",
        );
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
            "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
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
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = client.write_all(resp.as_bytes());
            return Ok(());
        }
    };

    // Forward initial request bytes
    upstream.write_all(&initial_bytes)?;

    // Bi-directional TCP stream relay (supports HTTP & WebSockets)
    let mut client_clone = client.try_clone()?;
    let mut upstream_clone = upstream.try_clone()?;

    let t1 = thread::spawn(move || {
        let res = std::io::copy(&mut client_clone, &mut upstream_clone);
        let _ = upstream_clone.shutdown(Shutdown::Write);
        res
    });
    let _ = std::io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Both);
    let _ = upstream.shutdown(Shutdown::Both);
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
                let val = v.trim();
                let host_only = if let Some(rest) = val.strip_prefix('[') {
                    rest.split(']').next().unwrap_or("").trim()
                } else {
                    val.split(':').next().unwrap_or("").trim()
                };
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

        let req_v6 = "GET / HTTP/1.1\r\nHost: [::1]:8880\r\n\r\n";
        assert_eq!(extract_host_header(req_v6), Some("::1".to_string()));
    }
}
