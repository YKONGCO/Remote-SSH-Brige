use crate::{model::*, state::AppState};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex, Weak,
    },
    time::{Duration, Instant},
};
const CAPACITY: usize = 1024 * 1024;
const READ_LIMIT: usize = 64 * 1024;
const READ_BUDGET: Duration = Duration::from_millis(5);
const IDLE_WAIT: Duration = Duration::from_millis(50);
const INPUT_TIMEOUT: Duration = Duration::from_secs(30);
const RETENTION: Duration = Duration::from_secs(300);
const OPEN_WAIT: Duration = Duration::from_secs(90);
enum Input {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}
struct Buffer {
    bytes: VecDeque<u8>,
    total: u64,
    state: TerminalState,
    error: Option<String>,
    finished: Option<Instant>,
}
impl Buffer {
    fn read(&self, requested: u64) -> Result<Value> {
        anyhow::ensure!(
            requested <= self.total,
            "INVALID_ARGUMENT: cursor exceeds received output"
        );
        let start = self.total.saturating_sub(self.bytes.len() as u64);
        let cursor = requested.max(start);
        let mut bytes: Vec<_> = self
            .bytes
            .iter()
            .skip((cursor - start) as usize)
            .take(READ_LIMIT)
            .copied()
            .collect();
        if self.state.alive() || cursor + (bytes.len() as u64) < self.total {
            let mut checked = 0;
            while let Err(error) = std::str::from_utf8(&bytes[checked..]) {
                match error.error_len() {
                    Some(n) => checked += error.valid_up_to() + n,
                    None => {
                        bytes.truncate(checked + error.valid_up_to());
                        break;
                    }
                }
            }
        }
        let next = cursor + bytes.len() as u64;
        Ok(
            json!({"stdout":String::from_utf8_lossy(&bytes),"cursor":next,
            "alive":self.state.alive(),"state":self.state,"error":self.error,
            "truncated":requested < start,"has_more":next < self.total}),
        )
    }
}
struct WorkerSignal {
    generation: Mutex<u64>,
    changed: Condvar,
}
impl Default for WorkerSignal {
    fn default() -> Self {
        Self {
            generation: Mutex::new(0),
            changed: Condvar::new(),
        }
    }
}
impl WorkerSignal {
    fn notify(&self) {
        let mut generation = self.generation.lock().unwrap();
        *generation += 1;
        drop(generation);
        self.changed.notify_all();
    }
    fn wait(&self, timeout: Duration) {
        let generation = self.generation.lock().unwrap();
        let current = *generation;
        let _ = self
            .changed
            .wait_timeout_while(generation, timeout, |value| *value == current)
            .unwrap();
    }
}
#[derive(Default)]
struct TerminalTimings {
    reservation_ms: u64,
    pool_acquire_ms: u64,
    connect_slot_wait_ms: u64,
    dns_resolve_ms: u64,
    tcp_connect_ms: u64,
    handshake_ms: u64,
    authentication_ms: u64,
    channel_open_ms: u64,
    pty_request_ms: u64,
    shell_request_ms: u64,
    first_output_ms: Option<u64>,
    connection_reused: bool,
}
impl TerminalTimings {
    fn json(&self) -> Value {
        json!({
            "reservation_ms": self.reservation_ms,
            "pool_acquire_ms": self.pool_acquire_ms,
            "connect_slot_wait_ms": self.connect_slot_wait_ms,
            "dns_resolve_ms": self.dns_resolve_ms,
            "tcp_connect_ms": self.tcp_connect_ms,
            "handshake_ms": self.handshake_ms,
            "authentication_ms": self.authentication_ms,
            "channel_open_ms": self.channel_open_ms,
            "pty_request_ms": self.pty_request_ms,
            "shell_request_ms": self.shell_request_ms,
            "first_output_ms": self.first_output_ms,
            "connection_reused": self.connection_reused,
        })
    }
}
pub struct TerminalHandle {
    metadata: TerminalInfo,
    input: mpsc::SyncSender<Input>,
    buffer: Arc<Mutex<Buffer>>,
    wake: Arc<Condvar>,
    io: Arc<WorkerSignal>,
    closed: Arc<AtomicBool>,
    mcp: bool,
}
impl TerminalHandle {
    pub fn info(&self) -> TerminalInfo {
        let buffer = self.buffer.lock().unwrap();
        let mut info = self.metadata.clone();
        info.alive = buffer.state.alive();
        info.state = buffer.state;
        info.error = buffer.error.clone();
        info
    }
    fn state(&self) -> TerminalState {
        self.buffer.lock().unwrap().state
    }
    fn counts_toward_limit(&self) -> bool {
        matches!(
            self.state(),
            TerminalState::Connecting | TerminalState::Running | TerminalState::Closing
        )
    }
    fn close(&self) {
        let already = self.closed.swap(true, Ordering::AcqRel);
        let mut buffer = self.buffer.lock().unwrap();
        if matches!(
            buffer.state,
            TerminalState::Connecting | TerminalState::Running
        ) {
            buffer.state = TerminalState::Closing;
            buffer.error = None;
        }
        drop(buffer);
        self.wake.notify_all();
        self.io.notify();
        if !already {
            let _ = self.input.try_send(Input::Close);
        }
    }
    fn notify_input(&self) {
        self.io.notify();
    }
    fn begin_running(&self) -> bool {
        let mut buffer = self.buffer.lock().unwrap();
        if self.closed.load(Ordering::Acquire) || buffer.state != TerminalState::Connecting {
            return false;
        }
        buffer.state = TerminalState::Running;
        drop(buffer);
        self.wake.notify_all();
        true
    }
    fn finish(&self, state: TerminalState, error: Option<String>) -> TerminalState {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.state = if buffer.state == TerminalState::Closing || self.closed.load(Ordering::Acquire)
        {
            match state {
                TerminalState::Failed | TerminalState::Disconnected => TerminalState::Closed,
                other => other,
            }
        } else {
            state
        };
        buffer.error = if matches!(
            buffer.state,
            TerminalState::Failed | TerminalState::Disconnected
        ) {
            error
        } else {
            None
        };
        buffer.finished = Some(Instant::now());
        let final_state = buffer.state;
        drop(buffer);
        self.wake.notify_all();
        self.io.notify();
        final_state
    }
    fn read(&self, cursor: u64, wait: Duration) -> Result<Value> {
        let deadline = Instant::now() + wait;
        let mut buffer = self.buffer.lock().unwrap();
        loop {
            let value = buffer.read(cursor)?;
            let empty = value["stdout"].as_str().is_some_and(|text| text.is_empty());
            let now = Instant::now();
            if !empty
                || !buffer.state.alive()
                || wait.is_zero()
                || now >= deadline
            {
                return Ok(value);
            }
            let remaining = deadline.saturating_duration_since(now);
            buffer = self
                .wake
                .wait_timeout(buffer, remaining)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|error| error.into_inner().0);
        }
    }
    fn wait_ready(&self, timeout: Duration) -> Result<TerminalInfo> {
        let deadline = Instant::now() + timeout;
        let mut buffer = self.buffer.lock().unwrap();
        loop {
            match buffer.state {
                TerminalState::Running => {
                    let mut info = self.metadata.clone();
                    info.alive = true;
                    info.state = TerminalState::Running;
                    info.error = None;
                    return Ok(info);
                }
                TerminalState::Connecting => {}
                state => {
                    let reason = buffer.error.clone();
                    drop(buffer);
                    return Err(crate::operation_error::error_with_details(
                        "TERMINAL_OPEN_FAILED",
                        "Terminal failed to start",
                        json!({"state":state,"reason":reason}),
                    ));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                drop(buffer);
                return Err(crate::operation_error::error_with_details(
                    "SSH_TIMEOUT",
                    "Terminal did not become ready in time",
                    json!({"state":TerminalState::Connecting}),
                ));
            }
            let remaining = deadline.saturating_duration_since(now);
            buffer = self
                .wake
                .wait_timeout(buffer, remaining)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|error| error.into_inner().0);
        }
    }
}
fn append(buffer: &Arc<Mutex<Buffer>>, wake: &Arc<Condvar>, bytes: &[u8]) {
    let mut b = buffer.lock().unwrap();
    b.bytes.extend(bytes);
    b.total += bytes.len() as u64;
    let excess = b.bytes.len().saturating_sub(CAPACITY);
    b.bytes.drain(..excess);
    drop(b);
    wake.notify_all();
}
pub fn operate(state: &Arc<AppState>, server: &Server, op: &Operation, source: &str) -> Result<Value> {
    if op.kind == "terminal_open" {
        return open(state, server, op, source);
    }
    let handle = state
        .terminals
        .lock()
        .unwrap()
        .get(&op.session_id)
        .cloned();
    let Some(handle) = handle else {
        if op.kind == "terminal_close" {
            return Ok(json!({"closed":true,"state":TerminalState::Closed}));
        }
        bail!("Terminal session does not exist")
    };
    if handle.metadata.server_id != server.id {
        bail!("Terminal session belongs to a different server")
    }
    match op.kind.as_str() {
        "terminal_read" => {
            anyhow::ensure!(
                op.wait_secs <= 30,
                "INVALID_ARGUMENT: wait_secs must be 0-30"
            );
            handle.read(op.cursor, Duration::from_secs(op.wait_secs))
        }
        "terminal_write" => {
            if op.content.len() > 65536 {
                bail!("Terminal input exceeds 64 KiB")
            }
            let state = handle.state();
            anyhow::ensure!(
                state == TerminalState::Running,
                "Terminal is not running (state: {state:?}); input was not sent"
            );
            handle
                .input
                .try_send(Input::Data(op.content.as_bytes().to_vec()))
                .context("Terminal input queue is full or the session is closing")?;
            handle.notify_input();
            Ok(json!({"sent":true}))
        }
        "terminal_resize" => {
            handle
                .input
                .try_send(Input::Resize(
                    op.cols.clamp(20, 500),
                    op.rows.clamp(5, 200),
                ))
                .context("Terminal session is closing")?;
            handle.notify_input();
            Ok(json!({"resized":true}))
        }
        "terminal_close" => {
            handle.close();
            let lifecycle = handle.state();
            if !matches!(
                lifecycle,
                TerminalState::Connecting | TerminalState::Running | TerminalState::Closing
            ) {
                state.terminals.lock().unwrap().remove(&op.session_id);
            }
            Ok(json!({"closed":true,"state":lifecycle}))
        }
        _ => bail!("Unsupported terminal action"),
    }
}
fn open(state: &Arc<AppState>, server: &Server, op: &Operation, source: &str) -> Result<Value> {
    let started = Instant::now();
    let settings = state.settings.lock().unwrap().clone();
    let session_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::sync_channel(256);
    let buffer = Arc::new(Mutex::new(Buffer {
        bytes: VecDeque::new(),
        total: 0,
        state: TerminalState::Connecting,
        error: None,
        finished: None,
    }));
    let wake = Arc::new(Condvar::new());
    let closed = Arc::new(AtomicBool::new(false));
    let handle = Arc::new(TerminalHandle {
        metadata: TerminalInfo {
            id: session_id.clone(),
            server_id: server.id.clone(),
            name: format!("{} · 终端", server.name),
            created_at: chrono::Utc::now().to_rfc3339(),
            alive: true,
            state: TerminalState::Connecting,
            error: None,
        },
        input: tx,
        buffer: buffer.clone(),
        wake: wake.clone(),
        io: Arc::new(WorkerSignal::default()),
        closed: closed.clone(),
        mcp: source == "mcp",
    });
    {
        let mut terminals = state.terminals.lock().unwrap();
        let active = terminals
            .values()
            .filter(|handle| handle.counts_toward_limit())
            .count();
        if active >= settings.max_terminals {
            bail!(
                "Terminal session limit of {} reached; close an unused session",
                settings.max_terminals
            )
        }
        terminals.insert(session_id.clone(), handle.clone());
    }
    let reservation_ms = started.elapsed().as_millis() as u64;
    let worker_state = Arc::downgrade(state);
    let worker_handle = handle.clone();
    let worker_server = server.clone();
    let worker_settings = settings.clone();
    let worker_id = session_id.clone();
    let cols = op.cols.clamp(20, 500);
    let rows = op.rows.clamp(5, 200);
    std::thread::Builder::new()
        .name(format!("terminal-{session_id}"))
        .spawn(move || {
            run_worker(
                worker_state,
                worker_handle,
                worker_server,
                worker_settings,
                worker_id,
                cols,
                rows,
                reservation_ms,
                started,
                rx,
                closed,
                buffer,
                wake,
            )
        })?;
    if source == "mcp" {
        match handle.wait_ready(OPEN_WAIT) {
            Ok(info) => Ok(json!({"session_id":session_id,"terminal":info})),
            Err(error) => {
                handle.close();
                Err(error)
            }
        }
    } else {
        Ok(json!({"session_id":session_id,"terminal":handle.info()}))
    }
}
#[allow(clippy::too_many_arguments)]
fn run_worker(
    state: Weak<AppState>,
    handle: Arc<TerminalHandle>,
    server: Server,
    settings: RuntimeSettings,
    id: String,
    cols: u32,
    rows: u32,
    reservation_ms: u64,
    started: Instant,
    rx: mpsc::Receiver<Input>,
    closed: Arc<AtomicBool>,
    buffer: Arc<Mutex<Buffer>>,
    wake: Arc<Condvar>,
) {
    let mut timings = TerminalTimings {
        reservation_ms,
        ..Default::default()
    };
    let cancelled = || closed.load(Ordering::Acquire);
    let acquire_start = Instant::now();
    let lease = state
        .upgrade()
        .context("Application state is no longer available")
        .and_then(|state| state.sessions.acquire_terminal(&server, &settings, &cancelled));
    timings.pool_acquire_ms = acquire_start.elapsed().as_millis() as u64;
    let mut lease = match lease {
        Ok(lease) => lease,
        Err(error) => {
            let message = crate::operation_error::error_message(&error);
            let final_state = handle.finish(
                if cancelled() {
                    TerminalState::Closed
                } else {
                    TerminalState::Failed
                },
                Some(message.clone()),
            );
            eprintln!(
                "terminal {id} {final_state:?} timing={} error={message}",
                timings.json()
            );
            cleanup_closed(&state, &id, cancelled());
            return;
        }
    };
    let stats = lease.stats();
    timings.connect_slot_wait_ms = stats.slot_wait_ms;
    timings.dns_resolve_ms = stats.dns_resolve_ms;
    timings.tcp_connect_ms = stats.tcp_connect_ms;
    timings.handshake_ms = stats.handshake_ms;
    timings.authentication_ms = stats.authentication_ms;
    timings.connection_reused = lease.reused();
    let mut size = (cols, rows);
    let mut preconnect_cancelled = false;
    loop {
        match rx.try_recv() {
            Ok(Input::Resize(c, r)) => size = (c, r),
            Ok(Input::Close) => {
                preconnect_cancelled = true;
                break;
            }
            Ok(Input::Data(_)) => {}
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                preconnect_cancelled = true;
                break;
            }
        }
    }
    let mut running = false;
    let result = (|| -> Result<()> {
        if preconnect_cancelled || cancelled() {
            return Ok(());
        }
        let session = lease.session();
        let channel_start = Instant::now();
        let mut channel = session.channel_session()?;
        timings.channel_open_ms = channel_start.elapsed().as_millis() as u64;
        if cancelled() {
            let _ = channel.close();
            return Ok(());
        }
        let pty_start = Instant::now();
        channel.request_pty("xterm-256color", None, Some((size.0, size.1, 0, 0)))?;
        timings.pty_request_ms = pty_start.elapsed().as_millis() as u64;
        if cancelled() {
            let _ = channel.close();
            return Ok(());
        }
        let shell_start = Instant::now();
        channel.shell()?;
        timings.shell_request_ms = shell_start.elapsed().as_millis() as u64;
        if cancelled() {
            let _ = channel.close();
            return Ok(());
        }
        session.set_blocking(false);
        if !handle.begin_running() {
            let _ = channel.close();
            return Ok(());
        }
        running = true;
        let mut pending = VecDeque::new();
        let mut resize = None;
        let mut last_progress = Instant::now();
        let mut data = [0u8; 16384];
        loop {
            if cancelled() {
                let _ = channel.close();
                return Ok(());
            }
            let mut worked = false;
            while pending.len() < CAPACITY {
                match rx.try_recv() {
                    Ok(Input::Close) => {
                        let _ = channel.close();
                        return Ok(());
                    }
                    Ok(Input::Data(bytes)) => {
                        if pending.is_empty() {
                            last_progress = Instant::now();
                        }
                        pending.extend(bytes);
                        worked = true;
                    }
                    Ok(Input::Resize(c, r)) => {
                        resize = Some((c, r));
                        worked = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let _ = channel.close();
                        return Ok(());
                    }
                }
            }
            if let Some((c, r)) = resize.take() {
                match channel.request_pty_size(c, r, None, None) {
                    Ok(_) => {}
                    Err(error) if error.code() == ssh2::ErrorCode::Session(-37) => {}
                    Err(error) => return Err(error.into()),
                }
                worked = true;
            }
            if !pending.is_empty() {
                match channel.write(pending.make_contiguous()) {
                    Ok(n) => {
                        pending.drain(..n);
                        if n > 0 {
                            last_progress = Instant::now();
                            worked = true;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
                if !pending.is_empty() && last_progress.elapsed() > INPUT_TIMEOUT {
                    bail!("Terminal input timed out")
                }
            }
            let deadline = Instant::now() + READ_BUDGET;
            let mut budget = READ_LIMIT;
            while budget > 0 && Instant::now() < deadline {
                let read_len = budget.min(data.len());
                match channel.read(&mut data[..read_len]) {
                    Ok(0) => break,
                    Ok(n) => {
                        if timings.first_output_ms.is_none() {
                            timings.first_output_ms =
                                Some(started.elapsed().as_millis() as u64);
                        }
                        append(&buffer, &wake, &data[..n]);
                        budget = budget.saturating_sub(n);
                        worked = true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error.into()),
                }
            }
            if channel.eof() {
                break;
            }
            if cancelled() {
                let _ = channel.close();
                return Ok(());
            }
            if !worked {
                handle.io.wait(IDLE_WAIT);
            }
        }
        Ok(())
    })();
    let was_cancelled = cancelled();
    let (outcome, error) = match (&result, running, was_cancelled) {
        (Err(_), _, true) => (TerminalState::Closed, None),
        (Err(error), true, false) => {
            let message = crate::operation_error::error_message(error);
            append(
                &buffer,
                &wake,
                format!("\r\n[Connection closed: {message}]\r\n").as_bytes(),
            );
            (TerminalState::Disconnected, Some(message))
        }
        (Err(error), false, false) => (
            TerminalState::Failed,
            Some(crate::operation_error::error_message(error)),
        ),
        (Ok(()), _, _) => (TerminalState::Closed, None),
    };
    lease.release(false);
    let final_state = handle.finish(outcome, error);
    eprintln!(
        "terminal {id} {final_state:?} timing={}",
        timings.json()
    );
    cleanup_closed(&state, &id, was_cancelled);
}
fn cleanup_closed(state: &Weak<AppState>, id: &str, user_closed: bool) {
    if !user_closed {
        return;
    }
    if let Some(state) = state.upgrade() {
        state.terminals.lock().unwrap().remove(id);
    }
}
pub fn stop_mcp(terminals: &Mutex<HashMap<String, Arc<TerminalHandle>>>) {
    let sessions: Vec<_> = terminals
        .lock()
        .unwrap()
        .values()
        .filter(|handle| handle.mcp)
        .cloned()
        .collect();
    for handle in sessions {
        handle.close();
    }
}
pub fn close_server(terminals: &Mutex<HashMap<String, Arc<TerminalHandle>>>, server_id: &str) {
    let sessions: Vec<_> = terminals
        .lock()
        .unwrap()
        .values()
        .filter(|handle| handle.metadata.server_id.as_str() == server_id)
        .cloned()
        .collect();
    for handle in sessions {
        handle.close();
    }
}
pub fn remove_server(terminals: &Mutex<HashMap<String, Arc<TerminalHandle>>>, server_id: &str) {
    terminals
        .lock()
        .unwrap()
        .retain(|_, handle| handle.metadata.server_id.as_str() != server_id);
}
pub fn prune(terminals: &Mutex<HashMap<String, Arc<TerminalHandle>>>) {
    let expired: Vec<_> = terminals
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, handle)| {
            let buffer = handle.buffer.lock().unwrap();
            !buffer.state.alive()
                && buffer
                    .finished
                    .is_some_and(|finished| finished.elapsed() > RETENTION)
        })
        .map(|(id, _)| id.clone())
        .collect();
    let mut terminals = terminals.lock().unwrap();
    for id in expired {
        terminals.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(bytes: &[u8], state: TerminalState) -> Buffer {
        Buffer {
            bytes: bytes.iter().copied().collect(),
            total: bytes.len() as u64,
            state,
            error: None,
            finished: None,
        }
    }

    fn handle() -> Arc<TerminalHandle> {
        let (input, _receiver) = mpsc::sync_channel(1);
        Arc::new(TerminalHandle {
            metadata: TerminalInfo {
                id: "t1".into(),
                server_id: "s1".into(),
                name: "server · 终端".into(),
                created_at: "2026-11-26T00:00:00Z".into(),
                alive: true,
                state: TerminalState::Connecting,
                error: None,
            },
            input,
            buffer: Arc::new(Mutex::new(buffer(b"", TerminalState::Connecting))),
            wake: Arc::new(Condvar::new()),
            io: Arc::new(WorkerSignal::default()),
            closed: Arc::new(AtomicBool::new(false)),
            mcp: false,
        })
    }

    #[test]
    fn cancelled_terminals_cannot_become_running_again() {
        let terminal = handle();
        assert!(terminal.begin_running());
        assert_eq!(terminal.state(), TerminalState::Running);
        terminal.close();
        assert_eq!(terminal.state(), TerminalState::Closing);
        assert!(!terminal.begin_running());
        terminal.finish(TerminalState::Closed, None);
        assert_eq!(terminal.state(), TerminalState::Closed);
        assert!(!terminal.info().alive);
    }

    #[test]
    fn failed_creation_exposes_the_error_without_holding_a_slot() {
        let terminal = handle();
        assert!(terminal.counts_toward_limit());
        terminal.finish(TerminalState::Failed, Some("authentication failed".into()));
        let info = terminal.info();
        assert_eq!(info.state, TerminalState::Failed);
        assert_eq!(info.error.as_deref(), Some("authentication failed"));
        assert!(!info.alive);
        assert!(!terminal.counts_toward_limit());
    }

    #[test]
    fn text_reads_preserve_utf8_across_arrivals_and_flush_invalid_eof() {
        let mut b = buffer(&[b'a', 0xe4, 0xb8], TerminalState::Running);
        let first = b.read(0).unwrap();
        assert_eq!(first["stdout"], "a");
        assert_eq!(first["cursor"], 1);
        assert_eq!(first["state"], "running");
        assert!(first.get("base64").is_none());
        b.bytes.push_back(0xad);
        b.total += 1;
        assert_eq!(b.read(1).unwrap()["stdout"], "中");
        let invalid = buffer(&[0xff, 0xe4], TerminalState::Closed).read(0).unwrap();
        assert_eq!(invalid["stdout"], "��");
        assert_eq!(invalid["cursor"], 2);
        assert_eq!(invalid["has_more"], false);
        assert_eq!(invalid["state"], "closed");
    }

    #[test]
    fn connecting_sessions_are_readable_and_report_their_lifecycle_state() {
        let b = buffer(b"", TerminalState::Connecting);
        let result = b.read(0).unwrap();
        assert_eq!(result["stdout"], "");
        assert_eq!(result["alive"], true);
        assert_eq!(result["state"], "connecting");
    }

    #[test]
    fn pages_use_byte_cursors_and_drain_after_eof() {
        let mut bytes = vec![b'x'; READ_LIMIT - 1];
        bytes.extend_from_slice("中文".as_bytes());
        let b = buffer(&bytes, TerminalState::Closed);
        let first = b.read(0).unwrap();
        assert_eq!(first["stdout"].as_str().unwrap().len(), READ_LIMIT - 1);
        assert_eq!(first["has_more"], true);
        assert_eq!(first["alive"], false);
        let second = b.read(first["cursor"].as_u64().unwrap()).unwrap();
        assert_eq!(second["stdout"], "中文");
        assert_eq!(second["cursor"], bytes.len());
        assert_eq!(second["has_more"], false);
    }

    #[test]
    fn overwritten_output_is_marked_and_future_cursors_are_rejected() {
        let mut b = buffer(b"tail", TerminalState::Running);
        b.total = 100;
        let result = b.read(0).unwrap();
        assert_eq!(result["stdout"], "tail");
        assert_eq!(result["cursor"], 100);
        assert_eq!(result["truncated"], true);
        assert_eq!(b.read(100).unwrap()["stdout"], "");
        assert!(b.read(101).is_err());
    }
}
