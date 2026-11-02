//! Local TCP forwarding. Each client owns its SSH session so slow clients cannot
//! hold libssh2's shared session lock and stall other clients.
use crate::{
    model::{Operation, Server},
    state::AppState,
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};

const MAX_TUNNELS: usize = 32;
const MAX_CONNECTIONS: usize = 16;

#[derive(Clone, Serialize)]
pub struct TunnelInfo {
    pub tunnel_id: String,
    pub server_id: String,
    pub server_name: String,
    pub source: String,
    pub local_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub status: String,
    pub created_at: String,
    pub connections: usize,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub error: Option<String>,
}
struct Tunnel {
    info: Mutex<TunnelInfo>,
    cancel: AtomicBool,
    sockets: Mutex<HashMap<String, TcpStream>>,
}
impl Tunnel {
    fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let mut info = self.info.lock().unwrap();
        if info.status == "running" {
            info.status = "closing".into();
        }
        drop(info);
        for socket in self.sockets.lock().unwrap().values() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}
#[derive(Default)]
pub struct TunnelManager {
    entries: Mutex<HashMap<String, Arc<Tunnel>>>,
    mcp_generation: AtomicU64,
}
impl Drop for TunnelManager {
    fn drop(&mut self) {
        for tunnel in self.entries.lock().unwrap().values() {
            tunnel.stop();
        }
    }
}
impl TunnelManager {
    pub fn list(&self, server_id: Option<&str>) -> Vec<TunnelInfo> {
        let mut items: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .values()
            .map(|t| t.info.lock().unwrap().clone())
            .filter(|i| server_id.is_none_or(|id| id == i.server_id))
            .filter(|i| matches!(i.status.as_str(), "running" | "closing"))
            .collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        items
    }
    pub fn stop_server(&self, id: &str) {
        for t in self.entries.lock().unwrap().values() {
            if t.info.lock().unwrap().server_id == id {
                t.stop();
            }
        }
    }
    pub fn stop_mcp(&self) {
        let entries = self.entries.lock().unwrap();
        self.mcp_generation.fetch_add(1, Ordering::SeqCst);
        for t in entries.values() {
            if t.info.lock().unwrap().source == "mcp" {
                t.stop();
            }
        }
    }
    pub fn stop_all(&self) {
        for t in self.entries.lock().unwrap().values() {
            t.stop();
        }
    }
}

pub fn validate(op: &Operation) -> Result<()> {
    match op.kind.as_str() {
        "port_forward_open" => {
            anyhow::ensure!(
                op.remote_port != 0,
                "INVALID_ARGUMENT: remote_port must be 1-65535"
            );
            // First version intentionally limits targets to the SSH server itself.
            anyhow::ensure!(
                matches!(op.remote_host.as_str(), "127.0.0.1" | "::1"),
                "INVALID_ARGUMENT: remote_host must be 127.0.0.1 or ::1"
            );
        }
        "port_forward_read" | "port_forward_close" => {
            anyhow::ensure!(
                !op.tunnel_id.is_empty() && op.tunnel_id.len() <= 128,
                "INVALID_ARGUMENT: tunnel_id is required"
            );
        }
        "port_forward_list" => {}
        _ => bail!("INVALID_ARGUMENT: Unsupported port-forward action"),
    }
    Ok(())
}

pub fn operate(
    state: &Arc<AppState>,
    server: &Server,
    op: &Operation,
    source: &str,
) -> Result<Value> {
    validate(op)?;
    if op.kind == "port_forward_list" {
        return Ok(json!({"tunnels":state.tunnels.list(Some(&server.id))}));
    }
    if op.kind != "port_forward_open" {
        let entries = state.tunnels.entries.lock().unwrap();
        let tunnel = entries
            .get(&op.tunnel_id)
            .context("NOT_FOUND: Tunnel does not exist or has expired")?;
        anyhow::ensure!(
            tunnel.info.lock().unwrap().server_id == server.id,
            "PERMISSION_DENIED: Tunnel belongs to a different server"
        );
        if op.kind == "port_forward_close" {
            tunnel.stop();
        }
        return Ok(json!(tunnel.info.lock().unwrap().clone()));
    }
    let settings = state.settings.lock().unwrap().clone();
    let generation = state.tunnels.mcp_generation.load(Ordering::SeqCst);
    // Verify host key and credentials before publishing a listener.
    let session = crate::ssh::connect_with_settings(server, &settings)?;
    drop(session);
    let listener = TcpListener::bind(("127.0.0.1", op.local_port))
        .context("Cannot bind local port; it may already be in use")?;
    listener.set_nonblocking(true)?;
    let id = uuid::Uuid::new_v4().to_string();
    let tunnel = Arc::new(Tunnel {
        info: Mutex::new(TunnelInfo {
            tunnel_id: id.clone(),
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            source: source.into(),
            local_host: "127.0.0.1".into(),
            local_port: listener.local_addr()?.port(),
            remote_host: op.remote_host.clone(),
            remote_port: op.remote_port,
            status: "running".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            connections: 0,
            bytes_sent: 0,
            bytes_received: 0,
            error: None,
        }),
        cancel: AtomicBool::new(false),
        sockets: Mutex::new(HashMap::new()),
    });
    let weak = Arc::downgrade(state);
    let original = server.clone();
    let mut entries = state.tunnels.entries.lock().unwrap();
    if source == "mcp" {
        anyhow::ensure!(
            generation == state.tunnels.mcp_generation.load(Ordering::SeqCst)
                && state
                    .cancel
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|c| !c.is_cancelled()),
            "MCP server stopped; recreate the tunnel"
        );
    }
    let active = entries
        .values()
        .filter(|t| {
            matches!(
                t.info.lock().unwrap().status.as_str(),
                "running" | "closing"
            )
        })
        .count();
    anyhow::ensure!(
        active < MAX_TUNNELS,
        "Maximum of {MAX_TUNNELS} active tunnels reached"
    );
    // Bound retained history without discarding running tasks.
    if entries.len() >= 128 {
        entries.retain(|_, t| {
            matches!(
                t.info.lock().unwrap().status.as_str(),
                "running" | "closing"
            )
        });
    }
    let worker = tunnel.clone();
    std::thread::Builder::new()
        .name("ssh-forward".into())
        .spawn(move || {
            run_listener(listener, &worker, weak, original);
        })?;
    entries.insert(id, tunnel.clone());
    let info = tunnel.info.lock().unwrap().clone();
    Ok(json!(info))
}

fn still_allowed(state: &AppState, server: &Server, source: &str) -> bool {
    !state.settings.lock().unwrap().remote_read_only
        && state.server(&server.id).is_ok_and(|current| {
            current.enabled
                && serde_json::to_value(&current).ok() == serde_json::to_value(server).ok()
                && (source != "mcp" || current.permits_mcp("port_forward_open"))
        })
}

fn run_listener(listener: TcpListener, tunnel: &Arc<Tunnel>, weak: Weak<AppState>, server: Server) {
    let source = tunnel.info.lock().unwrap().source.clone();
    let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();
    while !tunnel.cancel.load(Ordering::SeqCst) {
        let Some(state) = weak.upgrade() else { break };
        if !still_allowed(&state, &server, &source) {
            tunnel.info.lock().unwrap().error =
                Some("Server configuration or permissions changed; tunnel stopped".into());
            break;
        }
        let settings = state.settings.lock().unwrap().clone();
        drop(state);
        workers.retain(|w| !w.is_finished());
        match listener.accept() {
            Ok((socket, _)) => {
                if workers.len() >= MAX_CONNECTIONS {
                    tunnel.info.lock().unwrap().error = Some(format!(
                        "Tunnel connection limit of {MAX_CONNECTIONS} reached"
                    ));
                    continue;
                }
                let id = uuid::Uuid::new_v4().to_string();
                let Ok(control) = socket.try_clone() else {
                    continue;
                };
                tunnel.sockets.lock().unwrap().insert(id.clone(), control);
                tunnel.info.lock().unwrap().connections += 1;
                let t = tunnel.clone();
                let s = server.clone();
                let cleanup_id = id.clone();
                match std::thread::Builder::new()
                    .name("ssh-forward-client".into())
                    .spawn(move || {
                        let result = forward(socket, &t, &s, &settings);
                        t.sockets.lock().unwrap().remove(&id);
                        let mut info = t.info.lock().unwrap();
                        info.connections -= 1;
                        if let Err(e) = result {
                            if !t.cancel.load(Ordering::SeqCst) {
                                info.error = Some(crate::operation_error::error_message(&e));
                            }
                        }
                    }) {
                    Ok(w) => workers.push(w),
                    Err(e) => {
                        tunnel.sockets.lock().unwrap().remove(&cleanup_id);
                        let mut info = tunnel.info.lock().unwrap();
                        info.connections -= 1;
                        info.error = Some(crate::operation_error::error_message(&e.into()));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25))
            }
            Err(e) => {
                tunnel.info.lock().unwrap().error =
                    Some(crate::operation_error::error_message(&e.into()));
                break;
            }
        }
    }
    drop(listener); // Free the listening port before waiting for in-flight SSH handshakes.
    tunnel.stop();
    for w in workers {
        let _ = w.join();
    }
    tunnel.info.lock().unwrap().status = "closed".into();
    if let Some(state) = weak.upgrade() {
        let info = tunnel.info.lock().unwrap().clone();
        let op = Operation {
            kind: "port_forward_close".into(),
            server_id: server.id.clone(),
            tunnel_id: info.tunnel_id.clone(),
            ..Default::default()
        };
        let _ = state.log(&server, &source, &op, &Ok(json!(info)), 0);
    }
}

fn retry(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}
// One bounded buffer per direction; never accumulate arbitrary stream contents.
#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    offset: usize,
    eof: bool,
}
impl Pending {
    fn pump(&mut self, from: &mut impl Read, to: &mut impl Write) -> std::io::Result<u64> {
        if self.offset == self.bytes.len() {
            self.bytes.clear();
            self.offset = 0;
            if !self.eof {
                let mut buffer = [0; 32768];
                match from.read(&mut buffer) {
                    Ok(0) => self.eof = true,
                    Ok(n) => self.bytes.extend_from_slice(&buffer[..n]),
                    Err(e) if retry(&e) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        if self.offset < self.bytes.len() {
            match to.write(&self.bytes[self.offset..]) {
                Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.offset += n;
                    return Ok(n as u64);
                }
                Err(e) if retry(&e) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(0)
    }
    fn done(&self) -> bool {
        self.eof && self.offset == self.bytes.len()
    }
}
fn forward(
    mut socket: TcpStream,
    tunnel: &Tunnel,
    server: &Server,
    settings: &crate::model::RuntimeSettings,
) -> Result<()> {
    if tunnel.cancel.load(Ordering::SeqCst) {
        return Ok(());
    }
    let session = crate::ssh::connect_with_settings(server, settings)?;
    if tunnel.cancel.load(Ordering::SeqCst) {
        return Ok(());
    }
    let (host, port) = {
        let i = tunnel.info.lock().unwrap();
        (i.remote_host.clone(), i.remote_port)
    };
    let mut channel = session
        .channel_direct_tcpip(&host, port, None)
        .context("Remote port connection failed or SSH forwarding was denied")?;
    socket.set_nonblocking(true)?;
    session.set_blocking(false);
    session.set_keepalive(true, 15);
    let mut up = Pending::default();
    let mut down = Pending::default();
    let mut sent_eof = false;
    let mut received_eof = false;
    let mut keepalive = Instant::now();
    while !tunnel.cancel.load(Ordering::SeqCst) {
        let sent = up.pump(&mut socket, &mut channel)?;
        let received = down.pump(&mut channel, &mut socket)?;
        if sent + received > 0 {
            let mut i = tunnel.info.lock().unwrap();
            i.bytes_sent += sent;
            i.bytes_received += received;
        }
        if up.done() && !sent_eof {
            match channel.send_eof() {
                Ok(()) => sent_eof = true,
                Err(e) if e.code() == ssh2::ErrorCode::Session(-37) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if down.done() && !received_eof {
            socket.shutdown(Shutdown::Write)?;
            received_eof = true;
        }
        if sent_eof && received_eof {
            break;
        }
        if keepalive.elapsed() >= Duration::from_secs(15) {
            match session.keepalive_send() {
                Ok(_) => keepalive = Instant::now(),
                Err(e) if e.code() == ssh2::ErrorCode::Session(-37) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if sent + received == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let _ = socket.shutdown(Shutdown::Both);
    let _ = channel.close();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn permissions_read_only_and_approval_are_enforced_before_network() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().into()).unwrap();
        let mut server = state
            .save_server(
                Server {
                    name: "test".into(),
                    host: "127.0.0.1".into(),
                    auth: "agent".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let op = Operation {
            kind: "port_forward_open".into(),
            server_id: server.id.clone(),
            remote_host: "127.0.0.1".into(),
            remote_port: 8080,
            ..Default::default()
        };
        assert!(crate::state::execute(state.clone(), op.clone(), "mcp")
            .await
            .unwrap_err()
            .to_string()
            .contains("not authorized"));
        server.mcp_permissions = Some(vec!["port_forward_open".into()]);
        state.save_server(server.clone(), None).unwrap();
        let mut settings = state.settings.lock().unwrap().clone();
        settings.remote_read_only = true;
        state.save_settings(settings.clone()).unwrap();
        assert!(crate::state::execute(state.clone(), op.clone(), "mcp")
            .await
            .is_err());
        assert!(crate::security::sensitive(&op, "desktop"));
        settings.remote_read_only = false;
        settings.approval_mode = "mcp".into();
        state.save_settings(settings).unwrap();
        let pending = tokio::spawn(crate::state::execute(state.clone(), op, "mcp"));
        let view = loop {
            if let Some(view) = state.approvals.list().first() {
                break view.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        state.resolve_approval(&view.id, false).unwrap();
        assert!(pending.await.unwrap().is_err());
        assert!(state.tunnels.list(None).is_empty());
    }

    #[test]
    fn revocation_and_mcp_stop_release_listeners() {
        for mode in ["permission", "readonly", "stop", "delete", "close"] {
            let dir = tempfile::tempdir().unwrap();
            let state = AppState::open(dir.path().into()).unwrap();
            let server = state
                .save_server(
                    Server {
                        name: "test".into(),
                        host: "127.0.0.1".into(),
                        auth: "agent".into(),
                        mcp_permissions: Some(vec!["port_forward_open".into()]),
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let tunnel = Arc::new(Tunnel {
                info: Mutex::new(TunnelInfo {
                    tunnel_id: "test".into(),
                    server_id: server.id.clone(),
                    server_name: server.name.clone(),
                    source: "mcp".into(),
                    local_host: "127.0.0.1".into(),
                    local_port: port,
                    remote_host: "127.0.0.1".into(),
                    remote_port: 8080,
                    status: "running".into(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    connections: 0,
                    bytes_sent: 0,
                    bytes_received: 0,
                    error: None,
                }),
                cancel: AtomicBool::new(false),
                sockets: Mutex::new(HashMap::new()),
            });
            state
                .tunnels
                .entries
                .lock()
                .unwrap()
                .insert("test".into(), tunnel.clone());
            let t = tunnel.clone();
            let weak = Arc::downgrade(&state);
            let original = server.clone();
            let worker = std::thread::spawn(move || run_listener(listener, &t, weak, original));
            match mode {
                "permission" => {
                    let mut s = server.clone();
                    s.mcp_permissions = Some(vec![]);
                    state.save_server(s, None).unwrap();
                }
                "readonly" => {
                    let mut settings = state.settings.lock().unwrap().clone();
                    settings.remote_read_only = true;
                    state.save_settings(settings).unwrap();
                }
                "stop" => state.tunnels.stop_mcp(),
                "delete" => state.remove_server(&server.id).unwrap(),
                _ => {
                    let op = Operation {
                        kind: "port_forward_close".into(),
                        tunnel_id: "test".into(),
                        ..Default::default()
                    };
                    operate(&state, &server, &op, "desktop").unwrap();
                }
            }
            worker.join().unwrap();
            assert_eq!(tunnel.info.lock().unwrap().status, "closed", "{mode}");
            assert!(TcpListener::bind(("127.0.0.1", port)).is_ok(), "{mode}");
        }
    }
    #[test]
    fn rejects_non_loopback_targets_and_missing_ids() {
        let mut op = Operation {
            kind: "port_forward_open".into(),
            remote_host: "127.0.0.1".into(),
            remote_port: 8080,
            ..Default::default()
        };
        assert!(validate(&op).is_ok());
        op.remote_host = "10.0.0.1".into();
        assert!(validate(&op).is_err());
        op.kind = "port_forward_close".into();
        assert!(validate(&op).is_err());
    }
    #[test]
    fn pump_preserves_partial_writes_and_half_close() {
        struct Slow(Vec<u8>);
        impl Write for Slow {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                let n = b.len().min(3);
                self.0.extend_from_slice(&b[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let data = vec![42; 100_000];
        let mut input = std::io::Cursor::new(data.clone());
        let mut output = Slow(Vec::new());
        let mut pending = Pending::default();
        while !pending.done() {
            pending.pump(&mut input, &mut output).unwrap();
            assert!(pending.bytes.len() <= 32768);
        }
        assert_eq!(output.0, data);
    }
}
