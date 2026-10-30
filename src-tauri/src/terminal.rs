use crate::{model::*, state::AppState};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};
const CAPACITY: usize = 1024 * 1024;
enum Input {
    Data(Vec<u8>),
    Resize(u32, u32),
    Close,
}
struct Buffer {
    bytes: VecDeque<u8>,
    total: u64,
    alive: bool,
}
pub struct TerminalHandle {
    metadata: TerminalInfo,
    input: mpsc::SyncSender<Input>,
    buffer: Arc<Mutex<Buffer>>,
}
impl TerminalHandle {
    pub fn info(&self) -> TerminalInfo {
        let mut i = self.metadata.clone();
        i.alive = self.buffer.lock().unwrap().alive;
        i
    }
}
fn append(buffer: &Arc<Mutex<Buffer>>, bytes: &[u8]) {
    let mut b = buffer.lock().unwrap();
    b.bytes.extend(bytes);
    b.total += bytes.len() as u64;
    let excess = b.bytes.len().saturating_sub(CAPACITY);
    b.bytes.drain(..excess);
}
pub fn operate(state: &AppState, server: &Server, op: &Operation) -> Result<Value> {
    if op.kind == "terminal_open" {
        let _opening = state.terminal_open_lock.lock().unwrap();
        let settings = state.settings.lock().unwrap().clone();
        if state.terminals.lock().unwrap().len() >= settings.max_terminals {
            bail!(
                "最多保留 {} 个会话，请关闭并移除不用的会话",
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
        }));
        let handle = Arc::new(TerminalHandle {
            metadata: TerminalInfo {
                id: id.clone(),
                server_id: server.id.clone(),
                name: format!("{} · 终端", server.name),
                created_at: chrono::Utc::now().to_rfc3339(),
                alive: true,
            },
            input: tx,
            buffer: buffer.clone(),
        });
        state.terminals.lock().unwrap().insert(id.clone(), handle);
        std::thread::spawn(move || {
            let mut pending = VecDeque::new();
            let mut data = [0; 16384];
            let mut resize = None;
            let mut pending_since = Instant::now();
            let result = (|| -> Result<()> {
                loop {
                    // Bound unsent input when a remote process stops reading.
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
                            bail!("终端输入等待超时")
                        }
                    }
                    loop {
                        match channel.read(&mut data) {
                            Ok(0) => break,
                            Ok(n) => append(&buffer, &data[..n]),
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
                append(&buffer, format!("\r\n[连接结束：{e}]\r\n").as_bytes());
            }
            buffer.lock().unwrap().alive = false;
        });
        return Ok(json!({"session_id":id}));
    }
    let handle = state
        .terminals
        .lock()
        .unwrap()
        .get(&op.session_id)
        .cloned()
        .context("终端会话不存在")?;
    if handle.metadata.server_id != server.id {
        bail!("终端不属于此服务器")
    }
    match op.kind.as_str() {
        "terminal_read" => {
            let b = handle.buffer.lock().unwrap();
            let start = b.total.saturating_sub(b.bytes.len() as u64);
            let cursor = op.cursor.max(start).min(b.total);
            let skip = (cursor - start) as usize;
            let bytes: Vec<_> = b.bytes.iter().skip(skip).copied().collect();
            Ok(
                json!({"base64":STANDARD.encode(bytes), "cursor":b.total, "alive":b.alive, "truncated":op.cursor < start}),
            )
        }
        "terminal_write" => {
            if op.content.len() > 65536 {
                bail!("单次终端输入不得超过 64 KiB")
            }
            handle
                .input
                .try_send(Input::Data(op.content.as_bytes().to_vec()))
                .context("终端输入队列已满或会话已关闭")?;
            Ok(json!({"sent":true}))
        }
        "terminal_resize" => {
            handle
                .input
                .try_send(Input::Resize(op.cols.clamp(20, 500), op.rows.clamp(5, 200)))?;
            Ok(json!({"resized":true}))
        }
        "terminal_close" => {
            let _ = handle.input.try_send(Input::Close);
            state.terminals.lock().unwrap().remove(&op.session_id);
            Ok(json!({"closed":true}))
        }
        _ => bail!("未知终端操作"),
    }
}
