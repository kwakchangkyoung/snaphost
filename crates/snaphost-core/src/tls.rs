//! HTTPS front for the reverse proxy: terminates TLS with certificates issued by the local CA
//! and relays the decrypted stream to the domain's mapped `IP:PORT`.

use crate::cert::LocalCa;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

type Routes = Arc<RwLock<HashMap<String, SocketAddr>>>;

/// Running HTTPS listener; dropping it does not stop it, call [`TlsListener::stop`].
pub struct TlsListener {
    pub port: u16,
    shutdown: watch::Sender<bool>,
}

impl TlsListener {
    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
    }
}

/// Picks a certificate by SNI, only for domains that currently have a port mapping.
#[derive(Debug)]
struct SniResolver {
    ca: Arc<LocalCa>,
    routes: Routes,
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = client_hello.server_name()?.to_lowercase();
        let known = self.routes.read().ok()?.contains_key(&name);
        if !known {
            return None;
        }
        self.ca.certified_key_for(&name).ok()
    }
}

fn bind(host: &str, port: u16) -> std::io::Result<StdTcpListener> {
    let l = StdTcpListener::bind((host, port))?;
    l.set_nonblocking(true)?;
    Ok(l)
}

/// Bind `preferred_port` (falling back to 8443) and serve HTTPS on a background tokio runtime.
/// Like the HTTP proxy, it binds the wildcard address when macOS denies loopback binds below 1024
/// and drops non-loopback peers.
pub fn start(preferred_port: u16, ca: Arc<LocalCa>, routes: Routes) -> Result<TlsListener, String> {
    let v4 = bind("127.0.0.1", preferred_port)
        .or_else(|_| bind("0.0.0.0", preferred_port))
        .or_else(|_| bind("127.0.0.1", 8443))
        .map_err(|e| format!("HTTPS 포트 바인딩 실패: {e}"))?;
    let port = v4.local_addr().map(|a| a.port()).unwrap_or(preferred_port);
    let v6 = bind("::1", port).or_else(|_| bind("::", port)).ok();

    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(SniResolver {
        ca,
        routes: Arc::clone(&routes),
    }));
    // The relay is a raw byte pipe, so only offer HTTP/1.1 (keeps WebSocket upgrades working).
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let (tx, rx) = watch::channel(false);

    std::thread::spawn(move || {
        runtime.block_on(async move {
            let mut loops = Vec::new();
            for std_listener in std::iter::once(v4).chain(v6) {
                if let Ok(listener) = TcpListener::from_std(std_listener) {
                    loops.push(tokio::spawn(accept_loop(
                        listener,
                        acceptor.clone(),
                        Arc::clone(&routes),
                        rx.clone(),
                    )));
                }
            }
            for l in loops {
                let _ = l.await;
            }
        });
        // Dropping the runtime here also cancels in-flight connections.
    });

    Ok(TlsListener { port, shutdown: tx })
}

async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    routes: Routes,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else { continue };
                if !peer.ip().to_canonical().is_loopback() {
                    continue;
                }
                let acceptor = acceptor.clone();
                let routes = Arc::clone(&routes);
                tokio::spawn(async move {
                    let _ = handle_tls_client(stream, acceptor, routes).await;
                });
            }
        }
    }
}

async fn handle_tls_client(
    stream: TcpStream,
    acceptor: TlsAcceptor,
    routes: Routes,
) -> std::io::Result<()> {
    let mut tls = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream)).await??;

    let host = tls.get_ref().1.server_name().unwrap_or_default().to_lowercase();
    let target = routes.read().ok().and_then(|t| t.get(&host).copied());
    let Some(addr) = target else {
        return Ok(());
    };

    let mut upstream =
        match tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr)).await {
            Ok(Ok(s)) => s,
            res => {
                let err = match res {
                    Ok(Err(e)) => e.to_string(),
                    _ => "timed out".to_string(),
                };
                let body = format!("SnapHost Proxy: Could not connect to {host} -> {addr} ({err})\n");
                let resp = format!(
                    "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = tls.write_all(resp.as_bytes()).await;
                let _ = tls.shutdown().await;
                return Ok(());
            }
        };

    let _ = tokio::io::copy_bidirectional(&mut tls, &mut upstream).await;
    Ok(())
}
