//! Local execution sessions. Only the worker touches the SSH session/channel.
use crate::{
    model::{Operation, RuntimeSettings, Server},
    operation_error::{error, error_with_details},
    state::AppState,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::Read,
    sync::{Arc, Condvar, Mutex, Weak},
    time::{Duration, Instant},
};

const CAPACITY: usize = 4 * 1024 * 1024;
const RETENTION: Duration = Duration::from_secs(30 * 60);
pub type Progress = Option<tokio::sync::mpsc::Sender<Value>>;

#[derive(Default)]
struct Buffer {
    bytes: VecDeque<u8>,
    end: u64,
}
impl Buffer {
    fn append(&mut self, bytes: &[u8]) {
        self.end += bytes.len() as u64;
        let bytes = &bytes[bytes.len().saturating_sub(CAPACITY)..];
        let excess = (self.bytes.len() + bytes.len()).saturating_sub(CAPACITY);
        self.bytes.drain(..excess);
        self.bytes.extend(bytes);
    }
    fn read(&self, cursor: u64, limit: usize, finished: bool) -> (String, u64, bool) {
        let start = self.end - self.bytes.len() as u64;
        let offset = cursor.max(start).min(self.end);
        let mut bytes: Vec<u8> = self
            .bytes
            .iter()
            .skip((offset - start) as usize)
            .take(limit)
            .copied()
            .collect();
        // Keep an incomplete trailing UTF-8 character for the next read. Cursors
        // still refer to raw bytes; invalid remote text is decoded lossily.
        if !finished || offset + (bytes.len() as u64) < self.end {
            let mut checked = 0;
            while let Err(e) = std::str::from_utf8(&bytes[checked..]) {
                match e.error_len() {
                    Some(n) => checked += e.valid_up_to() + n,
                    None => {
                        bytes.truncate(checked + e.valid_up_to());
                        break;
                    }
                }
            }
        }
        (
            String::from_utf8_lossy(&bytes).into_owned(),
            offset + bytes.len() as u64,
            cursor < start,
        )
    }
}
struct Data {
    stdout: Buffer,
    stderr: Buffer,
    status: &'static str,
    exit_code: Option<i32>,
    error: Option<String>,
    finished: Option<Instant>,
    retained: bool,
    close: bool,
    decided: bool,
}
impl Default for Data {
    fn default() -> Self {
        Self {
            stdout: Buffer::default(),
            stderr: Buffer::default(),
            status: "running",
            exit_code: None,
            error: None,
            finished: None,
            retained: false,
            close: false,
            decided: false,
        }
    }
}
pub struct Job {
    id: String,
    server_id: String,
    server_name: String,
    command: String,
    source: String,
    created_at: String,
    started: Instant,
    data: Mutex<Data>,
    changed: Condvar,
}
impl Job {
    fn new(server_id: &str) -> Self {
        Self {
            id: format!("job_{}", uuid::Uuid::new_v4().simple()),
            server_id: server_id.into(),
            server_name: String::new(),
            command: String::new(),
            source: String::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
            started: Instant::now(),
            data: Mutex::new(Data::default()),
            changed: Condvar::new(),
        }
    }
    fn info(&self) -> Value {
        let d = self.data.lock().unwrap();
        let elapsed = d
            .finished
            .map(|end| end.duration_since(self.started))
            .unwrap_or_else(|| self.started.elapsed());
        json!({"job_id":self.id,"server_id":self.server_id,"server_name":self.server_name,"command":self.command,"source":self.source,"created_at":self.created_at,"status":d.status,"exit_code":d.exit_code,"error":d.error,"duration_ms":elapsed.as_millis() as u64,"stdout_end_cursor":d.stdout.end,"stderr_end_cursor":d.stderr.end})
    }
    fn snapshot(&self, out: u64, err: u64, limit: usize) -> Result<Value> {
        let d = self.data.lock().unwrap();
        anyhow::ensure!(
            out <= d.stdout.end && err <= d.stderr.end,
            "INVALID_ARGUMENT: cursor exceeds received output"
        );
        let (stdout, next_out, out_truncated) = d.stdout.read(out, limit, d.status != "running");
        let (stderr, next_err, err_truncated) = d.stderr.read(err, limit, d.status != "running");
        Ok(
            json!({"job_id":self.id,"server_id":self.server_id,"created_at":self.created_at,"status":d.status,"exit_code":d.exit_code,"error":d.error,
            "stdout":stdout,"stderr":stderr,"next_stdout_cursor":next_out,"next_stderr_cursor":next_err,
            "stdout_start_cursor":d.stdout.end - d.stdout.bytes.len() as u64,"stderr_start_cursor":d.stderr.end - d.stderr.bytes.len() as u64,
            "stdout_end_cursor":d.stdout.end,"stderr_end_cursor":d.stderr.end,
            "stdout_truncated":out_truncated,"stderr_truncated":err_truncated,"truncated":out_truncated || err_truncated,
            "stdout_has_more":next_out < d.stdout.end,"stderr_has_more":next_err < d.stderr.end}),
        )
    }
    fn wait(&self, out: u64, err: u64, duration: Duration) {
        let d = self.data.lock().unwrap();
        let _ = self
            .changed
            .wait_timeout_while(d, duration, |d| {
                d.status == "running" && !d.close && d.stdout.end == out && d.stderr.end == err
            })
            .unwrap();
    }
    fn finish(&self, status: &'static str, code: Option<i32>, error: Option<String>) {
        let mut d = self.data.lock().unwrap();
        d.status = status;
        d.exit_code = code;
        d.error = error;
        d.finished = Some(Instant::now());
        self.changed.notify_all();
    }
    fn close(&self) -> bool {
        let mut d = self.data.lock().unwrap();
        if d.status == "running" {
            d.close = true;
            self.changed.notify_all();
        }
        let (d, _) = self
            .changed
            .wait_timeout_while(d, Duration::from_secs(2), |d| d.status == "running")
            .unwrap();
        d.status != "running"
    }
}

#[derive(Default)]
pub struct JobManager {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}
impl JobManager {
    pub fn list(&self) -> Vec<Value> {
        let mut jobs = self.jobs.lock().unwrap();
        Self::prune(&mut jobs);
        let mut items: Vec<_> = jobs.values().map(|j| j.info()).collect();
        items.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str()));
        items
    }
    fn prune(jobs: &mut HashMap<String, Arc<Job>>) {
        jobs.retain(|_, j| {
            j.data
                .lock()
                .unwrap()
                .finished
                .is_none_or(|t| t.elapsed() < RETENTION)
        });
    }
    fn retain(&self, job: &Arc<Job>) -> Result<()> {
        let mut jobs = self.jobs.lock().unwrap();
        Self::prune(&mut jobs);
        let active = jobs
            .values()
            .filter(|j| j.server_id == job.server_id && j.data.lock().unwrap().status == "running")
            .count();
        if active >= 4 && job.data.lock().unwrap().status == "running" {
            return Err(error(
                "JOB_LIMIT",
                "Cannot retain session: server already has 4 active jobs",
            ));
        }
        job.data.lock().unwrap().retained = true;
        jobs.insert(job.id.clone(), job.clone());
        Ok(())
    }
    fn get(&self, server: &str, id: &str) -> Result<Arc<Job>> {
        let mut jobs = self.jobs.lock().unwrap();
        Self::prune(&mut jobs);
        jobs.get(id)
            .filter(|j| j.server_id == server)
            .cloned()
            .ok_or_else(|| error("JOB_NOT_FOUND", "Job does not exist or has expired"))
    }
    pub fn prune_expired(&self) {
        Self::prune(&mut self.jobs.lock().unwrap());
    }
    pub fn operate(&self, op: &Operation, progress: &Progress) -> Result<Value> {
        self.operate_with_limit(op, progress, CAPACITY)
    }
    pub fn operate_desktop(&self, op: &Operation) -> Result<Value> {
        anyhow::ensure!(
            matches!(op.job_action.as_str(), "read" | "close")
                && !op.server_id.is_empty()
                && op.server_id.len() <= 128
                && !op.job_id.is_empty()
                && op.job_id.len() <= 128,
            "INVALID_ARGUMENT: server_id, job_id and read/close action are required"
        );
        self.operate_with_limit(op, &None, 64 * 1024)
    }
    fn operate_with_limit(
        &self,
        op: &Operation,
        progress: &Progress,
        limit: usize,
    ) -> Result<Value> {
        if op.job_action == "list" {
            let items: Vec<_> = self
                .list()
                .into_iter()
                .filter(|j| j["server_id"] == op.server_id)
                .collect();
            return Ok(json!({"jobs":items}));
        }
        let job = self.get(&op.server_id, &op.job_id)?;
        if op.job_action == "close" {
            if !job.close() {
                return Err(error_with_details(
                    "JOB_CLOSE_PENDING",
                    "Close requested but not confirmed; check job.read",
                    job.snapshot(0, 0, CAPACITY)?,
                ));
            }
            return job.snapshot(0, 0, limit);
        }
        let deadline = Instant::now() + Duration::from_secs(op.wait_secs);
        loop {
            let value = job.snapshot(op.stdout_cursor, op.stderr_cursor, limit)?;
            if value["status"] != "running"
                || value["next_stdout_cursor"] != op.stdout_cursor
                || value["next_stderr_cursor"] != op.stderr_cursor
                || Instant::now() >= deadline
            {
                send_progress(progress, &job, op.stdout_cursor, op.stderr_cursor);
                return Ok(value);
            }
            job.wait(
                value["stdout_end_cursor"].as_u64().unwrap(),
                value["stderr_end_cursor"].as_u64().unwrap(),
                deadline.saturating_duration_since(Instant::now()),
            );
        }
    }
    pub fn close_all(&self) {
        for job in self.jobs.lock().unwrap().values() {
            job.data.lock().unwrap().close = true;
            job.changed.notify_all();
        }
    }
}
impl Drop for JobManager {
    fn drop(&mut self) {
        self.close_all();
    }
}

fn send_progress(progress: &Progress, job: &Job, out: u64, err: u64) -> (u64, u64) {
    if let Some(tx) = progress {
        if let Ok(value) = job.snapshot(out, err, 8192) {
            if value["next_stdout_cursor"] != out || value["next_stderr_cursor"] != err {
                let next = (
                    value["next_stdout_cursor"].as_u64().unwrap(),
                    value["next_stderr_cursor"].as_u64().unwrap(),
                );
                if tx.try_send(value).is_ok() {
                    return next;
                }
            }
        }
    }
    (out, err)
}

pub fn execute(
    state: &Arc<AppState>,
    server: &Server,
    op: &Operation,
    settings: &RuntimeSettings,
    source: &'static str,
    progress: &Progress,
) -> Result<Value> {
    anyhow::ensure!(!op.command.trim().is_empty(), "command must not be empty");
    let session = crate::ssh::connect_with_settings(server, settings)?;
    let mut channel = session.channel_session()?;
    channel.exec(&crate::ssh::command_with_login_path(server, &op.command))?; // Exactly once; this channel is moved intact to the worker.
    session.set_blocking(false);
    let mut execution = Job::new(&server.id);
    execution.server_name = server.name.clone();
    execution.command = op.command.clone();
    execution.source = source.into();
    let job = Arc::new(execution);
    // Cancellation/error paths must release a foreground worker as well.
    struct Foreground(Arc<Job>);
    impl Drop for Foreground {
        fn drop(&mut self) {
            let mut d = self.0.data.lock().unwrap();
            d.decided = true;
            if !d.retained {
                d.close = true;
            }
            self.0.changed.notify_all();
        }
    }
    let _foreground = Foreground(job.clone());
    let worker_job = job.clone();
    let weak = Arc::downgrade(state);
    let server = server.clone();
    let operation = op.clone();
    std::thread::Builder::new()
        .name("ssh-exec".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> Result<Option<i32>> {
                    let mut bytes = [0; 8192];
                    loop {
                        if worker_job.data.lock().unwrap().close {
                            let _ = channel.close();
                            let _ = session.disconnect(None, "Execution session closed", None);
                            return Ok(None);
                        }
                        // Bound work per stream so continuous stdout cannot starve stderr or close.
                        for stream_id in [0, 1] {
                            let mut stream = channel.stream(stream_id);
                            for _ in 0..16 {
                                match stream.read(&mut bytes) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        let mut d = worker_job.data.lock().unwrap();
                                        if stream_id == 0 {
                                            d.stdout.append(&bytes[..n]);
                                        } else {
                                            d.stderr.append(&bytes[..n]);
                                        }
                                        worker_job.changed.notify_all();
                                    }
                                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                                    Err(e) => return Err(e.into()),
                                }
                            }
                        }
                        if channel.eof() {
                            match channel.wait_close() {
                                Ok(()) => {
                                    // libssh2 defaults exit_status to zero when the server sends none.
                                    // Inspect exit signal as well; never invent success for a signal.
                                    if channel.exit_signal()?.exit_signal.is_some() {
                                        anyhow::bail!(
                                            "SSH command ended by signal; exit code unknown"
                                        );
                                    }
                                    return Ok(Some(channel.exit_status()?));
                                }
                                Err(e) if e.code() == ssh2::ErrorCode::Session(-37) => {}
                                Err(e) => return Err(e.into()),
                            }
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                },
            ));
            drop(channel);
            drop(session);
            match outcome {
                Ok(Ok(Some(code))) => worker_job.finish("exited", Some(code), None),
                Ok(Ok(None)) => worker_job.finish("closed", None, None),
                Ok(Err(e)) => worker_job.finish(
                    "disconnected",
                    None,
                    Some(crate::operation_error::error_message(&e)),
                ),
                Err(_) => worker_job.finish(
                    "disconnected",
                    None,
                    Some("SSH worker stopped unexpectedly".into()),
                ),
            }
            // The foreground decides whether this was handed over before final audit is written.
            let mut d = worker_job.data.lock().unwrap();
            while !d.decided {
                d = worker_job.changed.wait(d).unwrap();
            }
            let retained = d.retained;
            drop(d);
            if retained {
                audit_final(weak, &server, &operation, source, &worker_job);
            }
        })
        .context("Cannot start SSH worker")?;
    let timeout = Duration::from_secs(if op.timeout_secs == 0 {
        settings.command_timeout_secs
    } else {
        op.timeout_secs.clamp(1, 300)
    });
    let deadline = Instant::now() + timeout;
    let (mut out, mut err) = (0, 0);
    let mut last_progress = Instant::now() - Duration::from_millis(100);
    loop {
        if last_progress.elapsed() >= Duration::from_millis(100) {
            (out, err) = send_progress(progress, &job, out, err);
            last_progress = Instant::now();
        }
        let mut d = job.data.lock().unwrap();
        if d.status != "running" {
            d.close = true;
            job.changed.notify_all();
            drop(d);
            let mut value = job.snapshot(0, 0, CAPACITY)?;
            if value["status"] == "exited" {
                value.as_object_mut().unwrap().remove("job_id");
                return Ok(value);
            }
            return Err(error_with_details(
                "SSH_DISCONNECTED",
                "SSH disconnected; final exit status is unknown",
                value,
            ));
        }
        if Instant::now() >= deadline {
            drop(d);
            let failure = if !settings.exec_timeout_to_job {
                Some((
                    "EXEC_TIMEOUT",
                    "Command timed out; job retention is disabled",
                ))
            } else if !state.server(&op.server_id)?.permits_mcp("job") {
                Some((
                    "JOB_PERMISSION_DENIED",
                    "Command timed out; missing job permission prevents retention",
                ))
            } else {
                state.jobs.retain(&job).err().map(|_| {
                    (
                        "JOB_LIMIT",
                        "Cannot retain session: server already has 4 active jobs",
                    )
                })
            };
            if let Some((code, reason)) = failure {
                let close_confirmed = job.close();
                let mut value = job.snapshot(0, 0, CAPACITY)?;
                value["close_confirmed"] = json!(close_confirmed);
                return Err(error_with_details(code, reason, value));
            }
            let value = job.snapshot(0, 0, CAPACITY)?;
            // Serialize handoff audit before the worker's final audit.
            state.log(
                &state.server(&op.server_id)?,
                source,
                op,
                &Ok(value.clone()),
                timeout.as_millis() as u64,
            )?;
            job.changed.notify_all();
            return Ok(value);
        }
        let _guard = job
            .changed
            .wait_timeout(d, Duration::from_millis(100))
            .unwrap();
    }
}

fn audit_final(state: Weak<AppState>, server: &Server, op: &Operation, source: &str, job: &Job) {
    if let Some(state) = state.upgrade() {
        let mut op = op.clone();
        op.kind = "job".into();
        op.job_action = "finish".into();
        op.job_id = job.id.clone();
        let result = job.snapshot(0, 0, 16384);
        if let Err(e) = state.log(
            server,
            source,
            &op,
            &result,
            job.started.elapsed().as_millis() as u64,
        ) {
            eprintln!("Job final audit failed: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_list_exposes_metadata_and_reads_bounded_pages_without_replaying_output() {
        let manager = JobManager::default();
        let mut item = Job::new("server");
        item.server_name = "Production".into();
        item.command = "tail -f /var/log/app.log".into();
        item.source = "mcp".into();
        item.data
            .lock()
            .unwrap()
            .stdout
            .append(&vec![b'x'; 70 * 1024]);
        let job = Arc::new(item);
        manager.retain(&job).unwrap();
        let list = manager.list();
        assert_eq!(list[0]["command"], "tail -f /var/log/app.log");
        assert_eq!(list[0]["server_name"], "Production");
        assert!(list[0].get("stdout").is_none());
        let op = Operation {
            server_id: "server".into(),
            job_id: job.id.clone(),
            job_action: "read".into(),
            ..Default::default()
        };
        let first = manager.operate_desktop(&op).unwrap();
        assert_eq!(first["stdout"].as_str().unwrap().len(), 64 * 1024);
        assert_eq!(first["stdout_has_more"], true);
        let second = manager
            .operate_desktop(&Operation {
                stdout_cursor: first["next_stdout_cursor"].as_u64().unwrap(),
                ..op.clone()
            })
            .unwrap();
        assert_eq!(second["stdout"].as_str().unwrap().len(), 6 * 1024);
        assert_eq!(second["stdout_has_more"], false);
        assert!(manager
            .operate_desktop(&Operation {
                server_id: "other-server".into(),
                ..op.clone()
            })
            .is_err());
        job.finish("exited", Some(7), None);
        let closed = manager
            .operate_desktop(&Operation {
                job_action: "close".into(),
                ..op
            })
            .unwrap();
        assert_eq!(closed["status"], "exited");
        assert_eq!(closed["exit_code"], 7);
    }

    #[test]
    fn buffer_is_bounded_and_cursors_report_overwrites() {
        let mut buffer = Buffer::default();
        buffer.append(&vec![b'a'; CAPACITY + 123]);
        buffer.append(b"tail");
        let (text, next, truncated) = buffer.read(0, CAPACITY, true);
        assert!(truncated);
        assert_eq!(text.len(), CAPACITY);
        assert!(text.ends_with("tail"));
        assert_eq!(next, CAPACITY as u64 + 127);
        assert!(buffer.bytes.capacity() <= CAPACITY);
        assert_eq!(
            buffer.read(next, CAPACITY, true),
            (String::new(), next, false)
        );
    }

    #[test]
    fn unicode_split_across_reads_is_not_replaced_or_lost() {
        let mut buffer = Buffer::default();
        let bytes = "中🙂".as_bytes();
        buffer.append(&bytes[..5]);
        assert_eq!(buffer.read(0, CAPACITY, false), ("中".into(), 3, false));
        buffer.append(&bytes[5..]);
        assert_eq!(buffer.read(3, CAPACITY, false), ("🙂".into(), 7, false));
        assert_eq!(buffer.read(0, 5, true), ("中".into(), 3, false));
    }

    #[test]
    fn retention_limits_expiry_and_server_isolation() {
        let manager = JobManager::default();
        let jobs: Vec<_> = (0..4).map(|_| Arc::new(Job::new("server"))).collect();
        for job in &jobs {
            manager.retain(job).unwrap();
        }
        assert!(manager.retain(&Arc::new(Job::new("server"))).is_err());
        assert!(manager.retain(&Arc::new(Job::new("other"))).is_ok());
        assert!(manager.get("other", &jobs[0].id).is_err());
        jobs[0].finish("exited", Some(7), None);
        assert!(manager.get("server", &jobs[0].id).is_ok());
        assert!(manager.retain(&Arc::new(Job::new("server"))).is_ok());
        jobs[0].data.lock().unwrap().finished = Some(Instant::now() - RETENTION);
        manager.prune_expired();
        assert!(manager.get("server", &jobs[0].id).is_err());
        assert!(JobManager::default().get("server", &jobs[1].id).is_err());
    }

    #[test]
    fn waiting_reader_wakes_on_output_and_on_completion() {
        let job = Arc::new(Job::new("server"));
        let writer = job.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            writer.data.lock().unwrap().stderr.append(b"error");
            writer.changed.notify_all();
        });
        job.wait(0, 0, Duration::from_secs(2));
        thread.join().unwrap();
        assert_eq!(job.snapshot(0, 0, CAPACITY).unwrap()["stderr"], "error");
        let writer = job.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            writer.finish("disconnected", None, Some("network lost".into()));
        });
        job.wait(0, 5, Duration::from_secs(2));
        thread.join().unwrap();
        let final_result = job.snapshot(0, 5, CAPACITY).unwrap();
        assert_eq!(final_result["status"], "disconnected");
        assert!(final_result["exit_code"].is_null());
    }
}
