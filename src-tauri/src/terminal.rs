use crate::{model::*, state::AppState};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
const CAPACITY: usize = 1024 * 1024;
const READ_LIMIT: usize = 64 * 1024;
const RETENTION: Duration = Duration::from_secs(300);
enum Input {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}
struct Buffer {
    bytes: VecDeque<u8>,
    total: u64,
    alive: bool,
    state: String,
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
        if self.alive || cursor + (bytes.len() as u64) < self.total {
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
            "alive":self.alive,"state":self.state,"truncated":requested < start,
            "has_more":next < self.total}),
        )
    }
}
pub struct TerminalHandle {
    metadata: TerminalInfo,
    input: mpsc::SyncSender<Input>,
    buffer: Arc<Mutex<Buffer>>,
    wake: Arc<Condvar>,
    closed: Arc<AtomicBool>,
    mcp: bool,
}
impl TerminalHandle {
    pub fn info(&self) -> TerminalInfo {
        let buffer = self.buffer.lock().unwrap();
        let mut info = self.metadata.clone();
        info.alive = buffer.alive;
        info.state = buffer.state.clone();
        info
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.buffer.lock().unwrap().state = "closing".into();
        self.wake.notify_all();
        let _ = self.input.try_send(Input::Close);
    }
    fn read(&self, cursor: u64, wait: Duration) -> Result<Value> {
        let deadline = Instant::now() + wait;
        let mut buffer = self.buffer.lock().unwrap();
        loop {
            let value = buffer.read(cursor)?;
            let empty = value["stdout"].as_str().is_some_and(|text| text.is_empty());
            let now = Instant::now();
            if !empty
                || !buffer.alive
                || buffer.state != "running"
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
fn finish(buffer: &Arc<Mutex<Buffer>>, wake: &Arc<Condvar>, state: &str) {
    let mut b = buffer.lock().unwrap();
    b.alive = false;
    b.state = state.into();
    b.finished = Some(Instant::now());
    drop(b);
    wake.notify_all();
}
pub fn operate(state: &AppState, server: &Server, op: &Operation, source: &str) -> Result<Value> {
    if op.kind == "terminal_open" {
        let _opening = state.terminal_open_lock.lock().unwrap();
        let settings = state.settings.lock().unwrap().clone();
        let active = state
            .terminals
            .lock()
            .unwrap()
            .values()
            .filter(|handle| handle.info().alive)
            .count();
        if active >= settings.max_terminals {
            bail!(
                "Terminal session limit of {} reached; close an unused session",
                settings.max_terminals
            )
        }
        let session = crate::ssh::connect_with_settings(server, &settings)?;
        let mut channel = session.channel_session()?;
        channel.request_pty(
            "xterm-256color",
            None,
            Some((op.cols.clamp(20, 500), op.rows.clamp(5, 200), 0, 0)),
        )?;
        channel.shell()?;
        session.set_blocking(false);
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::sync_channel(256);
        let buffer = Arc::new(Mutex::new(Buffer {
            bytes: VecDeque::new(),
            total: 0,
            alive: true,
            state: "running".into(),
            finished: None,
        }));
        let wake = Arc::new(Condvar::new());
        let closed = Arc::new(AtomicBool::new(false));
        let handle = Arc::new(TerminalHandle {
            metadata: TerminalInfo {
                id: id.clone(),
                server_id: server.id.clone(),
                name: format!("{} · 终端", server.name),
                created_at: chrono::Utc::now().to_rfc3339(),
                alive: true,
                state: "running".into(),
            },
            input: tx,
            buffer: buffer.clone(),
            wake: wake.clone(),
            closed: closed.clone(),
            mcp: source == "mcp",
        });
        state.terminals.lock().unwrap().insert(id.clone(), handle);
        let worker_buffer = buffer.clone();
        let worker_wake = wake.clone();
        std::thread::spawn(move || {
            let mut pending = VecDeque::new();
            let mut data = [0; 16384];
            let mut resize = None;
            let mut pending_since = Instant::now();
            let result = (|| -> Result<()> {
                loop {
                    if closed.load(Ordering::Acquire) {
                        let _ = channel.close();
                        return Ok(());
                    }
                    while pending.len() < CAPACITY {
                        match rx.try_recv() {
                            Ok(Input::Close) => {
                                let _ = channel.close();
                                return Ok(());
                            }
                            Ok(Input::Data(bytes)) => {
                                if pending.is_empty() {
                                    pending_since = Instant::now();
                                }
                                pending.extend(bytes);
                            }
                            Ok(Input::Resize(c, r)) => resize = Some((c, r)),
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                        }
                    }
                    if let Some((c, r)) = resize {
                        match channel.request_pty_size(c, r, None, None) {
                            Ok(_) => resize = None,
                            Err(e) if e.code() == ssh2::ErrorCode::Session(-37) => {}
                            Err(e) => return Err(e.into()),
                        }
                    }
                    if !pending.is_empty() {
                        match channel.write(pending.make_contiguous()) {
                            Ok(n) => {
                                pending.drain(..n);
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(e) => return Err(e.into()),
                        }
                        if pending_since.elapsed() > Duration::from_secs(30) {
                            bail!("Terminal input timed out")
                        }
                    }
                    loop {
                        match channel.read(&mut data) {
                            Ok(0) => break,
                            Ok(n) => append(&worker_buffer, &worker_wake, &data[..n]),
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(e) => return Err(e.into()),
                        }
                    }
                    if channel.eof() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
                Ok(())
            })();
            if let Err(e) = result {
                append(
                    &worker_buffer,
                    &worker_wake,
                    format!("\r\n[Connection closed: {e}]\r\n").as_bytes(),
                );
                finish(&worker_buffer, &worker_wake, "disconnected");
            } else {
                finish(&worker_buffer, &worker_wake, "closed");
            }
        });
        return Ok(json!({"session_id":id}));
    }
    let handle = state
        .terminals
        .lock()
        .unwrap()
        .get(&op.session_id)
        .cloned()
        .context("Terminal session does not exist")?;
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
            handle
                .input
                .try_send(Input::Data(op.content.as_bytes().to_vec()))
                .context("Terminal input queue is full or the session is closed")?;
            Ok(json!({"sent":true}))
        }
        "terminal_resize" => {
            handle
                .input
                .try_send(Input::Resize(op.cols.clamp(20, 500), op.rows.clamp(5, 200)))?;
            Ok(json!({"resized":true}))
        }
        "terminal_close" => {
            handle.close();
            state.terminals.lock().unwrap().remove(&op.session_id);
            Ok(json!({"closed":true}))
        }
        _ => bail!("Unsupported terminal action"),
    }
}
pub fn stop_mcp(terminals: &Mutex<HashMap<String, Arc<TerminalHandle>>>) {
    let sessions: Vec<_> = terminals
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, handle)| handle.mcp)
        .map(|(id, handle)| (id.clone(), handle.clone()))
        .collect();
    for (id, handle) in sessions {
        handle.close();
        terminals.lock().unwrap().remove(&id);
    }
}
pub fn prune(terminals: &Mutex<HashMap<String, Arc<TerminalHandle>>>) {
    let expired: Vec<_> = terminals
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, handle)| {
            let buffer = handle.buffer.lock().unwrap();
            !buffer.alive
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

    fn buffer(bytes: &[u8], alive: bool) -> Buffer {
        Buffer {
            bytes: bytes.iter().copied().collect(),
            total: bytes.len() as u64,
            alive,
            state: if alive { "running" } else { "closed" }.into(),
            finished: None,
        }
    }

    #[test]
    fn text_reads_preserve_utf8_across_arrivals_and_flush_invalid_eof() {
        let mut b = buffer(&[b'a', 0xe4, 0xb8], true);
        let first = b.read(0).unwrap();
        assert_eq!(first["stdout"], "a");
        assert_eq!(first["cursor"], 1);
        assert!(first.get("base64").is_none());
        b.bytes.push_back(0xad);
        b.total += 1;
        assert_eq!(b.read(1).unwrap()["stdout"], "中");
        let invalid = buffer(&[0xff, 0xe4], false).read(0).unwrap();
        assert_eq!(invalid["stdout"], "��");
        assert_eq!(invalid["cursor"], 2);
        assert_eq!(invalid["has_more"], false);
        assert_eq!(invalid["state"], "closed");
    }

    #[test]
    fn pages_use_byte_cursors_and_drain_after_eof() {
        let mut bytes = vec![b'x'; READ_LIMIT - 1];
        bytes.extend_from_slice("中文".as_bytes());
        let b = buffer(&bytes, false);
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
        let mut b = buffer(b"tail", true);
        b.total = 100;
        let result = b.read(0).unwrap();
        assert_eq!(result["stdout"], "tail");
        assert_eq!(result["cursor"], 100);
        assert_eq!(result["truncated"], true);
        assert_eq!(b.read(100).unwrap()["stdout"], "");
        assert!(b.read(101).is_err());
    }
}
