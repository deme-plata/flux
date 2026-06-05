// flux_portal — Single-port multiplexer: one port to rule them all
//
// Problem: fluxc-serve (:8084), fluxmux webhook (:9099), P2P swarm (:9003),
// dashboard SSE — every service grabs a port. Port conflicts, firewall holes,
// and "address already in use" are constant friction.
//
// Solution: Flux Portal listens on ONE master port and routes internally
// via Unix domain sockets + path-based dispatch. Services never bind ports
// directly — they register with Portal and receive connections through it.
//
// Architecture:
//   :8084 (master) ─┬─ /api/*      → fluxc HTTP API
//                   ├─ /sse        → SSE event stream
//                   ├─ /webhook    → fluxmux webhook receiver
//                   ├─ /p2p/*      → P2P swarm RPC (via Unix socket)
//                   ├─ /dashboard  → Dashboard HTML
//                   └─ /portal/*   → Portal management API
//
// Internal services communicate via:
//   /tmp/flux-portal/{api,sse,webhook,p2p}.sock  (Unix domain sockets)
//
// Auto-discovery: services register by creating their socket file.
// Portal scans /tmp/flux-portal/ and auto-routes to active services.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;

fn is_private_bind_host(host: &str) -> bool {
    let h = host.trim();
    let parts: Vec<&str> = h.split('.').collect();
    let is_172_private = parts.len() == 4
        && parts[0] == "172"
        && parts[1].parse::<u8>().map(|b| (16..=31).contains(&b)).unwrap_or(false);
    h == "127.0.0.1"
        || h == "localhost"
        || h == "::1"
        || h.starts_with("10.")
        || h.starts_with("192.168.")
        || h.starts_with("fd")
        || h.starts_with("fc")
        || is_172_private
}

fn harden_socket_dir(path: &PathBuf) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
}

/// Configuration for the Flux Portal.
#[derive(Clone, Debug)]
pub struct PortalConfig {
    /// TCP host to bind. Defaults to localhost; set FLUX_PORTAL_HOST to opt in.
    pub bind_host: String,
    /// Master port — the only TCP port exposed.
    pub master_port: u16,
    /// Directory for Unix domain sockets.
    pub socket_dir: PathBuf,
    /// Enable auto-discovery of services.
    pub auto_discover: bool,
}

impl Default for PortalConfig {
    fn default() -> Self {
        PortalConfig {
            bind_host: std::env::var("FLUX_PORTAL_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            master_port: 8084,
            socket_dir: PathBuf::from("/tmp/flux-portal"),
            auto_discover: true,
        }
    }
}

/// A registered service in the portal.
#[derive(Clone, Debug)]
struct ServiceRegistration {
    name: String,
    socket_path: PathBuf,
    path_prefix: String,
    active: bool,
}

/// The Flux Portal — single-port master multiplexer.
pub struct FluxPortal {
    config: PortalConfig,
    services: Arc<RwLock<HashMap<String, ServiceRegistration>>>,
}

impl FluxPortal {
    /// Create a new portal with the given config.
    pub fn new(config: PortalConfig) -> Self {
        std::fs::create_dir_all(&config.socket_dir).ok();
        harden_socket_dir(&config.socket_dir);
        FluxPortal {
            config,
            services: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a service with the portal.
    /// The service should listen on its Unix socket at `socket_path`.
    pub fn register(&self, name: &str, socket_path: PathBuf, path_prefix: &str) {
        let reg = ServiceRegistration {
            name: name.to_string(),
            socket_path,
            path_prefix: path_prefix.to_string(),
            active: true,
        };
        self.services.write().insert(name.to_string(), reg);
        eprintln!("Portal: service {} registered at {}", name, path_prefix);
    }

    /// Start the portal — blocks on the master port, routing connections.
    pub fn start(&self) -> Result<(), String> {
        if !is_private_bind_host(&self.config.bind_host)
            && std::env::var("FLUX_PORTAL_ALLOW_PUBLIC").ok().as_deref() != Some("1")
        {
            return Err(format!(
                "Portal refusing public bind {} without FLUX_PORTAL_ALLOW_PUBLIC=1",
                self.config.bind_host
            ));
        }
        let addr = format!("{}:{}", self.config.bind_host, self.config.master_port);
        let listener = TcpListener::bind(&addr)
            .map_err(|e| format!("Portal bind {}: {}", addr, e))?;

        println!("🔮 Flux Portal v0.9.14 — one port to rule them all");
        eprintln!("   http://{}:{}", self.config.bind_host, self.config.master_port);
        eprintln!("   {} services registered", self.services.read().len());
        eprintln!("   Sockets: {}", self.config.socket_dir.display());

        for stream in listener.incoming() {
            match stream {
                Ok(mut tcp) => {
                    let services = self.services.clone();
                    let socket_dir = self.config.socket_dir.clone();
                    std::thread::spawn(move || {
                        Self::handle_connection(&mut tcp, &services, &socket_dir);
                    });
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }

    fn handle_connection(
        tcp: &mut TcpStream,
        services: &Arc<RwLock<HashMap<String, ServiceRegistration>>>,
        socket_dir: &PathBuf,
    ) {
        let mut buf = [0u8; 4096];
        let n = match tcp.read(&mut buf) {
            Ok(n) if n > 0 => n,
            _ => return,
        };

        let raw = String::from_utf8_lossy(&buf[..n]);
        let first_line = raw.lines().next().unwrap_or("");
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() < 2 { return; }

        let path = parts[1].to_string();

        // Auto-discover: scan for new socket files
        if let Ok(entries) = std::fs::read_dir(socket_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".sock") && !services.read().contains_key(&name) {
                    let svc_name = name.trim_end_matches(".sock");
                    services.write().insert(name.clone(), ServiceRegistration {
                        name: svc_name.to_string(),
                        socket_path: entry.path(),
                        path_prefix: format!("/{}", svc_name),
                        active: true,
                    });
                }
            }
        }

        // Route to matching service
        let svcs = services.read();
        for (_, svc) in svcs.iter() {
            if path.starts_with(&svc.path_prefix) {
                // Forward to Unix socket
                match UnixStream::connect(&svc.socket_path) {
                    Ok(mut unix) => {
                        let _ = unix.write_all(buf[..n].as_ref());
                        let mut resp = Vec::new();
                        let _ = unix.read_to_end(&mut resp);
                        let _ = tcp.write_all(&resp);
                        return;
                    }
                    Err(_) => {
                        // Service socket not available — fall through to default
                    }
                }
            }
        }

        // Default: portal status page
        let status = serde_json::json!({
            "portal": "Flux Portal v0.9.14",
            "master_port": 8084,
            "services": svcs.len(),
            "auto_discover": true,
            "routes": svcs.values().map(|s| {
                serde_json::json!({"name": s.name, "prefix": s.path_prefix, "active": s.active})
            }).collect::<Vec<_>>(),
        });
        let body = serde_json::to_string_pretty(&status).unwrap_or_default();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
            body.len(), body
        );
        let _ = tcp.write_all(response.as_bytes());
    }
}

/// Quick-start: launch portal with auto-discover on port 8084.
/// All flux services (serve, webhook, P2P) route through this single port.
pub fn quick_portal(port: u16) -> FluxPortal {
    let config = PortalConfig {
        master_port: port,
        ..Default::default()
    };
    FluxPortal::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_portal_create() {
        let portal = quick_portal(18084);
        portal.register("test", PathBuf::from("/tmp/flux-portal/test.sock"), "/test");
        assert_eq!(portal.services.read().len(), 1);
    }

    #[test]
    fn test_register_multiple() {
        let portal = quick_portal(18085);
        portal.register("api", PathBuf::from("/tmp/flux-portal/api.sock"), "/api");
        portal.register("p2p", PathBuf::from("/tmp/flux-portal/p2p.sock"), "/p2p");
        portal.register("webhook", PathBuf::from("/tmp/flux-portal/webhook.sock"), "/webhook");
        assert_eq!(portal.services.read().len(), 3);
    }

    #[test]
    fn test_portal_default_config() {
        let config = PortalConfig::default();
        assert_eq!(config.master_port, 8084);
        assert_eq!(config.socket_dir, PathBuf::from("/tmp/flux-portal"));
        assert!(config.auto_discover);
    }
}
