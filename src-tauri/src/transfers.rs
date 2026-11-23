use crate::transfer_scheduler::{RunPermit, TransferScheduler};
use crate::{
    model::{Operation, RuntimeSettings, Server},
    operation_error::error,
    state::AppState,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub const CHUNK_BYTES: usize = 1024 * 1024;
const RETENTION: Duration = Duration::from_secs(7 * 86400);
#[cfg(test)]
pub(crate) fn testing_handle() -> Arc<Handle> {
    tests::handle()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TransferSettings {
    pub max_concurrent: usize,
    pub per_server_concurrent: usize,
    pub max_retries: u32,
    pub retry_base_secs: u64,
    pub io_timeout_secs: u64,
    pub idle_timeout_secs: u64,
    pub bandwidth_kib: u64,
}
impl Default for TransferSettings {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            per_server_concurrent: 2,
            max_retries: 5,
            retry_base_secs: 1,
            io_timeout_secs: 30,
            idle_timeout_secs: 120,
            bandwidth_kib: 0,
        }
    }
}
impl TransferSettings {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (1..=16).contains(&self.max_concurrent)
                && (1..=8).contains(&self.per_server_concurrent),
            "Transfer concurrency must be 1-16 globally and 1-8 per server"
        );
        anyhow::ensure!(
            self.max_retries <= 10 && (1..=30).contains(&self.retry_base_secs),
            "Transfer retries must be 0-10; retry delay must be 1-30 seconds"
        );
        anyhow::ensure!(
            (1..=300).contains(&self.io_timeout_secs)
                && (1..=3600).contains(&self.idle_timeout_secs),
            "Invalid transfer I/O or idle timeout"
        );
        anyhow::ensure!(
            self.bandwidth_kib <= 1_048_576,
            "Transfer bandwidth limit exceeds 1 GiB/s"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferSpec {
    pub local_path: String,
    pub remote_path: String,
    pub overwrite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default)]
    pub create_parents: bool,
    #[serde(default)]
    pub skip_existing: bool,
    #[serde(default)]
    pub directory: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub transfer_id: String,
    pub server_id: String,
    pub direction: String,
    pub local_path: String,
    pub remote_path: String,
    pub overwrite: bool,
    pub status: String,
    pub phase: String,
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub bytes_per_second: u64,
    pub elapsed_ms: u64,
    pub version: u64,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub error: Option<Value>,
    pub audit_error: Option<String>,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub next_retry_at: Option<String>,
    #[serde(default)]
    pub resumed_bytes: u64,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub bandwidth_kib: u64,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub verified_bytes: u64,
    #[serde(default)]
    pub last_progress_at: Option<String>,
    #[serde(default)]
    pub retry_stage: Option<String>,
    #[serde(default)]
    pub failure_category: Option<String>,
    #[serde(default)]
    pub session_reuse_count: u64,
    #[serde(default)]
    pub effective_throughput: u64,
    #[serde(default)]
    pub queued_ms: u64,
    #[serde(default)]
    pub transfer_ms: u64,
    #[serde(default)]
    pub verify_ms: u64,
    #[serde(default)]
    pub retry_wait_ms: u64,
    #[serde(default)]
    pub batch_policy: Option<String>,
}

pub struct Handle {
    snapshot: Mutex<Snapshot>,
    notify: tokio::sync::Notify,
    cancel: CancellationToken,

    control: AtomicU8,
    confirmed_bytes: AtomicU64,
    destination_key: String,
    created: Instant,
    finished: Mutex<Option<Instant>>,
    state: Mutex<Weak<AppState>>,
    pub(crate) operation: Mutex<Operation>,
    origin: Mutex<String>,
    pub(crate) checkpoint: Mutex<crate::transfer_engine::Checkpoint>,
    worker_active: std::sync::atomic::AtomicBool,
    server_identity: Mutex<Option<Server>>,
    journal_lock: Mutex<()>,
    lifecycle: tokio::sync::Mutex<()>,
    pub(crate) window: Mutex<crate::transfer_engine::AdaptiveWindow>,
    pub(crate) last_progress: Mutex<Instant>,
    phase_started: Mutex<Option<(String, Instant)>>,
    queued_recorded: std::sync::atomic::AtomicBool,
}
impl Handle {
    fn snapshot(&self) -> Snapshot {
        let mut view = self.snapshot.lock().unwrap().clone();
        if !terminal(&view.status) {
            view.elapsed_ms = self.created.elapsed().as_millis() as u64;
            view.transferred_bytes = self.confirmed_bytes.load(Ordering::Acquire);
        }
        view
    }
    fn update(&self, update: impl FnOnce(&mut Snapshot)) {
        let mut view = self.snapshot.lock().unwrap();
        update(&mut view);
        view.version += 1;
        drop(view);
        self.notify.notify_waiters();
    }
    pub(crate) fn mark_running(&self) {
        if !self.queued_recorded.swap(true, Ordering::AcqRel) {
            let queued = self.created.elapsed().as_millis() as u64;
            self.update(|s| s.queued_ms = queued);
        }
    }
    pub(crate) fn retry_wait(&self, delay: Duration) {
        self.update(|s| {
            s.retry_wait_ms = s.retry_wait_ms.saturating_add(delay.as_millis() as u64);
        });
    }
    pub(crate) fn verified(&self, bytes: u64) {
        self.update(|s| s.verified_bytes = bytes);
    }
    pub(crate) fn reset_checkpoint(&self) {
        {
            let mut checkpoint = self.checkpoint.lock().unwrap();
            checkpoint.hashes.clear();
            checkpoint.verified_bytes = 0;
            checkpoint.temp_size = None;
        }
        let _ = self.persist();
    }
    pub(crate) fn session_reused(&self, reused: bool) {
        if reused {
            self.update(|s| s.session_reuse_count = s.session_reuse_count.saturating_add(1));
        }
    }
    pub(crate) fn failure(&self, error: &anyhow::Error) {
        let code = crate::operation_error::error_code(error);
        let category = failure_category(code);
        let stage = crate::operation_error::error_response(error, &Operation::default())["stage"]
            .as_str()
            .unwrap_or("execution")
            .to_string();
        self.update(|s| {
            s.failure_category = Some(category.to_string());
            s.retry_stage = Some(stage);
        });
    }
    pub fn check_cancelled(&self) -> Result<()> {
        if self.control.load(Ordering::Acquire) == 4 {
            return Err(error("TRANSFER_PAUSED", "Transfer paused"));
        }
        if self.control.load(Ordering::Acquire) == 1 {
            return Err(error("TRANSFER_CANCELLED", "Transfer cancelled"));
        }
        Ok(())
    }
    pub fn begin_commit(&self) -> Result<()> {
        self.control
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| error("TRANSFER_CANCELLED", "Transfer cancelled"))?;
        self.phase("committing");
        Ok(())
    }
    fn request_cancel(&self) -> bool {
        match self
            .control
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                self.cancel.cancel();
                self.phase("cancelling");
                true
            }
            Err(1) => true,
            _ => false,
        }
    }
    fn progress(&self, count: u64, total: u64, elapsed: Duration) {
        self.confirmed_bytes.store(count, Ordering::Release);
        let throughput = (count as f64 / elapsed.as_secs_f64().max(0.001)) as u64;
        let now = chrono::Utc::now().to_rfc3339();
        *self.last_progress.lock().unwrap() = Instant::now();
        self.update(|s| {
            s.transferred_bytes = count;
            s.total_bytes = Some(total);
            s.bytes_per_second = throughput;
            s.effective_throughput = throughput;
            s.last_progress_at = Some(now);
            if s.phase == "stalled" {
                s.phase = "transferring".into();
            }
        });
    }
    fn complete(&self, result: &Result<Value>) {
        let previous = self.control.swap(3, Ordering::AcqRel);
        let cancelled = previous == 1 && result.is_err();
        let paused = previous == 4 && result.is_err();
        *self.finished.lock().unwrap() = Some(Instant::now());
        self.update(|s| {
            s.status = match result {
                Ok(_) => "completed",
                Err(_) if cancelled => "cancelled",
                Err(_) if paused => "paused",
                Err(_) => "failed",
            }
            .into();

            s.phase = s.status.clone();
            s.next_retry_at = None;
            if result.is_ok() {
                s.error = None;
            }
            if result.as_ref().ok().is_some_and(|v| v["skipped"] == true) {
                s.phase = "skipped".into();
            }
            s.transferred_bytes = self.confirmed_bytes.load(Ordering::Acquire);
            s.elapsed_ms = self.created.elapsed().as_millis() as u64;
            s.finished_at = Some(chrono::Utc::now().to_rfc3339());
            if let Err(e) = result {
                let cancellation = error("TRANSFER_CANCELLED", "Transfer cancelled");
                let operation = Operation {
                    kind: if s.direction == "upload" {
                        "upload_file"
                    } else {
                        "download_file"
                    }
                    .into(),
                    server_id: s.server_id.clone(),
                    transfer_id: s.transfer_id.clone(),
                    path: s.remote_path.clone(),
                    ..Default::default()
                };
                let response =
                    transfer_error_response(if cancelled { &cancellation } else { e }, &operation);
                s.failure_category =
                    Some(failure_category(response["code"].as_str().unwrap_or("")).to_string());
                s.retry_stage = response["stage"].as_str().map(str::to_string);
                s.error = Some(response);
            }
            if paused {
                s.error = None;
                s.finished_at = None;
            }
        });
        let _ = self.persist();
    }
}
fn same_request(a: &Operation, b: &Operation) -> bool {
    match (&a.transfer_request, &b.transfer_request) {
        (Some(a), Some(b)) => {
            a.local_path == b.local_path
                && a.remote_path == b.remote_path
                && a.overwrite == b.overwrite
                && a.skip_existing == b.skip_existing
                && a.directory == b.directory
                && a.create_parents == b.create_parents
        }
        _ => false,
    }
}
fn terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

pub(crate) fn transfer_error_response(error: &anyhow::Error, op: &Operation) -> Value {
    let mut response = crate::operation_error::error_response(error, op);
    let code = response["code"].as_str().unwrap_or("").to_string();
    response["retryable"] = json!(crate::operation_error::retryable(error));
    response["reconnect_required"] =
        json!(matches!(failure_category(&code), "connection" | "network"));
    response["recovery"] = json!(match failure_category(&code) {
        "connection" => "retry after the SSH connection is available",
        "network" => "retry with backoff; keep the checkpoint",
        "remote_storage" => "free remote space or fix permissions before retrying",
        "permission" => "ask the server owner for the required permission",
        "file" => "inspect the source and destination, then create a new transfer",
        "control" => "resume or create a new transfer",
        _ => "inspect the transfer status before retrying",
    });
    response
}

fn batch_policy_name(policy: crate::transfer_planner::BatchErrorPolicy) -> &'static str {
    match policy {
        crate::transfer_planner::BatchErrorPolicy::Continue => "continue_on_error",
        crate::transfer_planner::BatchErrorPolicy::FailFast => "fail_fast",
    }
}

fn failure_category(code: &str) -> &'static str {
    match code {
        "SSH_CONNECT_FAILED"
        | "SSH_AUTHENTICATION_FAILED"
        | "SSH_HOST_UNTRUSTED"
        | "SSH_HOST_KEY_MISMATCH"
        | "SSH_SESSION_BUSY" => "connection",
        "SSH_TIMEOUT" | "IO_TIMEOUT" | "CONNECTION_LOST" | "CONNECTION_REFUSED" | "SSH_ERROR" => {
            "network"
        }
        "NO_SPACE" | "QUOTA_EXCEEDED" | "SFTP_FAILURE" | "REMOTE_IO" => "remote_storage",
        "PERMISSION_DENIED" | "WRITE_PROTECTED" | "LOCAL_PATH_DENIED" | "PATH_OUTSIDE_ROOT" => {
            "permission"
        }
        "SOURCE_CHANGED" | "NOT_FOUND" | "ALREADY_EXISTS" | "CHECKSUM_MISMATCH" => "file",
        "TRANSFER_CANCELLED" | "TRANSFER_PAUSED" => "control",
        _ => "unknown",
    }
}

include!("transfer_tasks.rs");

pub struct TransferManager {
    items: Mutex<HashMap<String, Arc<Handle>>>,
    scheduler: Arc<TransferScheduler>,
    group_policies: Mutex<HashMap<String, crate::transfer_planner::BatchErrorPolicy>>,
    state: Mutex<Weak<AppState>>,
}

impl Default for TransferManager {
    fn default() -> Self {
        Self {
            items: Mutex::new(HashMap::new()),
            scheduler: TransferScheduler::new(),
            group_policies: Mutex::new(HashMap::new()),
            state: Mutex::new(Weak::new()),
        }
    }
}
impl TransferManager {
    pub fn prune(&self) {
        let mut items = self.items.lock().unwrap();
        let expired: Vec<_> = items
            .iter()
            .filter(|(_, h)| {
                let v = h.snapshot();
                v.status == "completed"
                    && chrono::DateTime::parse_from_rfc3339(
                        v.finished_at.as_deref().unwrap_or(&v.created_at),
                    )
                    .is_ok_and(|t| {
                        chrono::Utc::now().signed_duration_since(t).num_seconds()
                            > RETENTION.as_secs() as i64
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(state) = self.state.lock().unwrap().upgrade() {
                if state.database.lock().unwrap().delete_transfer(&id).is_err() {
                    continue;
                }
            }
            items.remove(&id);
        }
        let live: HashSet<String> = items
            .values()
            .filter_map(|handle| handle.snapshot().group_id)
            .collect();
        drop(items);
        self.group_policies
            .lock()
            .unwrap()
            .retain(|group, _| live.contains(group));
    }
    pub fn completed_local_file(&self, server_id: &str, transfer_id: &str) -> Result<PathBuf> {
        let item = self
            .items
            .lock()
            .unwrap()
            .get(transfer_id)
            .filter(|handle| handle.snapshot().server_id == server_id)
            .cloned()
            .ok_or_else(|| {
                error(
                    "TRANSFER_NOT_FOUND",
                    "Transfer does not exist or has expired",
                )
            })?;
        let snapshot = item.snapshot();
        anyhow::ensure!(
            snapshot.status == "completed",
            "TRANSFER_NOT_COMPLETED: Transfer has not completed"
        );
        let path = PathBuf::from(snapshot.local_path);
        anyhow::ensure!(
            path.is_absolute(),
            "INVALID_ARGUMENT: Local path is not absolute"
        );
        let path = path
            .canonicalize()
            .context("Completed transfer file no longer exists")?;
        anyhow::ensure!(
            path.is_file()
                || (path.is_dir()
                    && item
                        .operation
                        .lock()
                        .unwrap()
                        .transfer_request
                        .as_ref()
                        .is_some_and(|spec| spec.directory)),
            "Completed transfer path is not a file or transferred directory"
        );
        Ok(path)
    }
    pub(crate) fn register_group(
        &self,
        group: &str,
        policy: crate::transfer_planner::BatchErrorPolicy,
    ) {
        let name = batch_policy_name(policy);
        {
            let mut policies = self.group_policies.lock().unwrap();
            policies.insert(group.to_string(), policy);
        }
        for handle in self.items.lock().unwrap().values() {
            if handle.snapshot().group_id.as_deref() == Some(group) {
                handle.update(|snapshot| snapshot.batch_policy = Some(name.to_string()));
            }
        }
    }

    pub(crate) fn group_terminal(&self, group: &str, status: &str) {
        if !matches!(status, "failed" | "cancelled") {
            return;
        }
        let policy = self.group_policies.lock().unwrap().get(group).copied();
        if policy != Some(crate::transfer_planner::BatchErrorPolicy::FailFast) {
            return;
        }
        let handles: Vec<_> = self
            .items
            .lock()
            .unwrap()
            .values()
            .filter(|handle| handle.snapshot().group_id.as_deref() == Some(group))
            .cloned()
            .collect();
        for handle in handles {
            if !terminal(&handle.snapshot().status) {
                handle.request_cancel();
                self.scheduler.cancel(&handle.snapshot().transfer_id);
            }
        }
    }

    pub(crate) fn group_exists(&self, server_id: &str, group: &str) -> bool {
        self.items.lock().unwrap().values().any(|handle| {
            let view = handle.snapshot();
            view.server_id == server_id && view.group_id.as_deref() == Some(group)
        })
    }

    pub(crate) fn cancel_group(&self, server_id: &str, group: &str) -> usize {
        let handles: Vec<_> = self
            .items
            .lock()
            .unwrap()
            .values()
            .filter(|handle| {
                let view = handle.snapshot();
                view.server_id == server_id && view.group_id.as_deref() == Some(group)
            })
            .cloned()
            .collect();
        let mut count = 0;
        for handle in handles {
            if !terminal(&handle.snapshot().status) {
                handle.request_cancel();
                self.scheduler.cancel(&handle.snapshot().transfer_id);
                count += 1;
            }
        }
        count
    }

    pub fn group_summary(&self, server_id: &str, group: &str) -> Value {
        self.group_summaries(server_id)
            .into_iter()
            .find(|summary| summary["group_id"] == group)
            .unwrap_or_else(|| {
                json!({
                    "group_id":group,
                    "total":0,
                    "completed":0,
                    "failed":0,
                    "cancelled":0,
                    "paused":0,
                    "queued":0,
                    "running":0,
                    "retrying":0,
                    "transferred_bytes":0,
                    "verified_bytes":0,
                    "status":"empty",
                })
            })
    }

    pub fn group_summaries(&self, server_id: &str) -> Vec<Value> {
        let policies = self.group_policies.lock().unwrap().clone();
        let items = self.items.lock().unwrap();
        let mut groups: HashMap<String, Vec<Snapshot>> = HashMap::new();
        for handle in items.values() {
            let view = handle.snapshot();
            if view.server_id == server_id {
                if let Some(group) = &view.group_id {
                    groups.entry(group.clone()).or_default().push(view);
                }
            }
        }
        let mut summaries: Vec<_> = groups
            .into_iter()
            .map(|(group, views)| {
                let mut summary = json!({
                    "group_id": group,
                    "total": views.len(),
                    "completed": 0,
                    "failed": 0,
                    "cancelled": 0,
                    "paused": 0,
                    "queued": 0,
                    "running": 0,
                    "retrying": 0,
                    "transferred_bytes": 0,
                    "verified_bytes": 0,
                    "status": "completed",
                    "batch_policy": policies
                        .get(&group)
                        .copied()
                        .map(batch_policy_name),
                });
                let mut active = false;
                let mut failed = false;
                let mut cancelled = false;
                let mut paused = false;
                for view in &views {
                    summary["transferred_bytes"] = json!(
                        summary["transferred_bytes"].as_u64().unwrap_or(0) + view.transferred_bytes
                    );
                    summary["verified_bytes"] = json!(
                        summary["verified_bytes"].as_u64().unwrap_or(0) + view.verified_bytes
                    );
                    match view.status.as_str() {
                        "completed" => {
                            summary["completed"] =
                                json!(summary["completed"].as_u64().unwrap_or(0) + 1)
                        }
                        "failed" => {
                            failed = true;
                            summary["failed"] = json!(summary["failed"].as_u64().unwrap_or(0) + 1)
                        }
                        "cancelled" => {
                            cancelled = true;
                            summary["cancelled"] =
                                json!(summary["cancelled"].as_u64().unwrap_or(0) + 1)
                        }
                        "paused" => {
                            paused = true;
                            summary["paused"] = json!(summary["paused"].as_u64().unwrap_or(0) + 1)
                        }
                        "running" => {
                            active = true;
                            summary["running"] = json!(summary["running"].as_u64().unwrap_or(0) + 1)
                        }
                        "retrying" => {
                            active = true;
                            summary["retrying"] =
                                json!(summary["retrying"].as_u64().unwrap_or(0) + 1)
                        }
                        _ => {
                            active = true;
                            summary["queued"] = json!(summary["queued"].as_u64().unwrap_or(0) + 1)
                        }
                    }
                }
                summary["status"] = json!(if active {
                    "running"
                } else if paused {
                    "paused"
                } else if failed {
                    "failed"
                } else if cancelled {
                    "cancelled"
                } else {
                    "completed"
                });
                summary
            })
            .collect();
        summaries.sort_by(|a, b| b["group_id"].as_str().cmp(&a["group_id"].as_str()));
        summaries
    }

    fn insert(&self, server: &Server, op: &Operation) -> Result<Arc<Handle>> {
        let batch_policy = op.transfer_group.as_ref().and_then(|group| {
            self.group_policies
                .lock()
                .unwrap()
                .get(group)
                .copied()
                .map(batch_policy_name)
                .map(str::to_string)
        });
        self.prune();
        let mut items = self.items.lock().unwrap();
        anyhow::ensure!(
            items.len() < 1024,
            "TRANSFER_LIMIT: Transfer history is full; wait for older records to expire"
        );
        let spec = op
            .transfer_request
            .as_ref()
            .context("Missing transfer parameters")?;
        for h in items.values() {
            let original = h.operation.lock().unwrap();
            if original.server_id == op.server_id && original.kind == op.kind {
                if op.request_id.is_some() && original.request_id == op.request_id {
                    anyhow::ensure!(
                        same_request(&original, op),
                        "INVALID_ARGUMENT: request_id was used for different transfer parameters"
                    );
                    return Ok(h.clone());
                }
                if !terminal(&h.snapshot().status)
                    && serde_json::to_value(&original.transfer_request)?
                        == serde_json::to_value(&op.transfer_request)?
                {
                    return Ok(h.clone());
                }
            }
        }
        let destination_key = destination_key(Some(server), op);
        if items
            .values()
            .any(|h| h.destination_key == destination_key && !terminal(&h.snapshot().status))
        {
            return Err(error(
                "TRANSFER_CONFLICT",
                "A transfer to this destination is already active",
            ));
        }
        let id = format!("transfer_{}", uuid::Uuid::new_v4().simple());
        let h = Arc::new(Handle {
            snapshot: Mutex::new(Snapshot {
                transfer_id: id.clone(),
                server_id: server.id.clone(),
                direction: if op.kind == "upload_file" {
                    "upload"
                } else {
                    "download"
                }
                .into(),
                local_path: spec.local_path.clone(),
                remote_path: spec.remote_path.clone(),
                overwrite: spec.overwrite,
                status: "queued".into(),
                phase: "queued".into(),
                transferred_bytes: 0,
                total_bytes: spec.size,
                bytes_per_second: 0,
                elapsed_ms: 0,
                version: 1,
                created_at: chrono::Utc::now().to_rfc3339(),
                finished_at: None,
                error: None,
                audit_error: None,
                retry_count: 0,
                next_retry_at: None,
                resumed_bytes: 0,
                priority: 0,
                bandwidth_kib: 0,
                sha256: None,
                group_id: op.transfer_group.clone(),
                verified_bytes: 0,
                last_progress_at: None,
                retry_stage: None,
                failure_category: None,
                session_reuse_count: 0,
                effective_throughput: 0,
                queued_ms: 0,
                transfer_ms: 0,
                verify_ms: 0,
                retry_wait_ms: 0,
                batch_policy,
            }),
            notify: tokio::sync::Notify::new(),
            cancel: CancellationToken::new(),
            control: AtomicU8::new(0),
            confirmed_bytes: AtomicU64::new(0),
            destination_key,
            created: Instant::now(),
            finished: Mutex::new(None),
            state: Mutex::new(self.state.lock().unwrap().clone()),
            operation: Mutex::new(op.clone()),
            origin: Mutex::new("desktop".into()),
            checkpoint: Mutex::new(Default::default()),
            worker_active: std::sync::atomic::AtomicBool::new(false),
            server_identity: Mutex::new(Some(server.clone())),
            journal_lock: Mutex::new(()),
            lifecycle: tokio::sync::Mutex::new(()),
            window: Mutex::new(Default::default()),
            last_progress: Mutex::new(Instant::now()),
            phase_started: Mutex::new(None),
            queued_recorded: std::sync::atomic::AtomicBool::new(false),
        });
        items.insert(id, h.clone());
        Ok(h)
    }
    pub fn cancel_all(&self) {
        for h in self.items.lock().unwrap().values() {
            h.request_cancel();
        }
        self.scheduler.cancel_all();
    }
    pub async fn operate(
        &self,
        state: &Arc<AppState>,
        op: &Operation,
        source: &str,
    ) -> Result<Value> {
        let server = state.server(&op.server_id)?;
        authorize(&server, op, source)?;
        let allowed = if source == "desktop" {
            matches!(
                op.transfer_action.as_str(),
                "list" | "read" | "cancel" | "pause" | "resume" | "cleanup" | "configure"
            )
        } else {
            matches!(
                op.transfer_action.as_str(),
                "list" | "read" | "cancel" | "pause" | "resume"
            )
        };
        anyhow::ensure!(
            allowed && op.wait_secs <= 30,
            "INVALID_ARGUMENT: invalid transfer action or wait_secs"
        );
        self.prune();
        if op.transfer_action == "list" {
            let mut items: Vec<_> = self
                .items
                .lock()
                .unwrap()
                .values()
                .map(|h| h.snapshot())
                .filter(|s| s.server_id == server.id)
                .collect();
            items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            return Ok(json!({"transfers":items,"groups":self.group_summaries(&server.id)}));
        }
        let group = op
            .transfer_group
            .as_deref()
            .filter(|group| !group.is_empty());
        anyhow::ensure!(
            !(op.transfer_id.is_empty() && group.is_none()),
            "INVALID_ARGUMENT: transfer_id or group_id is required"
        );
        anyhow::ensure!(
            op.transfer_id.is_empty() || group.is_none(),
            "INVALID_ARGUMENT: provide either transfer_id or group_id"
        );
        anyhow::ensure!(
            op.transfer_id.len() <= 128 && group.is_none_or(|group| group.len() <= 128),
            "INVALID_ARGUMENT: invalid transfer_id or group_id"
        );
        if let Some(group) = group {
            return self
                .operate_group(state, op, source, &server.id, group)
                .await;
        }
        let h = self
            .items
            .lock()
            .unwrap()
            .get(&op.transfer_id)
            .filter(|h| h.snapshot().server_id == server.id)
            .cloned()
            .ok_or_else(|| {
                error(
                    "TRANSFER_NOT_FOUND",
                    "Transfer does not exist or has expired",
                )
            })?;
        if op.transfer_action == "cancel" {
            if !h.worker_active.load(Ordering::Acquire) && h.snapshot().status == "paused" {
                h.control.store(1, Ordering::Release);
                h.complete(&Err(error("TRANSFER_CANCELLED", "Transfer cancelled")));
                let mut value = serde_json::to_value(h.snapshot())?;
                value["cancel_requested"] = json!(true);
                return Ok(value);
            }
            let accepted = h.request_cancel();
            self.scheduler.cancel(&op.transfer_id);
            let mut value = serde_json::to_value(h.snapshot())?;
            value["cancel_requested"] = json!(accepted);
            return Ok(value);
        }
        if op.transfer_action != "read" {
            return self.control_task(state, op, source, &h).await;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(op.wait_secs);
        loop {
            let notified = h.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let view = h.snapshot();
            if let Some(version) = op.version {
                anyhow::ensure!(
                    version <= view.version,
                    "INVALID_ARGUMENT: version exceeds current transfer version"
                );
            }

            if op.version.is_some_and(|v| v != view.version)
                || terminal(&view.status)
                || view.status == "paused"
                || tokio::time::Instant::now() >= deadline
            {
                authorize(&state.server(&op.server_id)?, op, source)?;
                return Ok(serde_json::to_value(view)?);
            }
            let _ = tokio::time::timeout_at(deadline, notified).await;
        }
    }

    async fn operate_group(
        &self,
        state: &Arc<AppState>,
        op: &Operation,
        source: &str,
        server_id: &str,
        group: &str,
    ) -> Result<Value> {
        anyhow::ensure!(
            self.group_exists(server_id, group),
            "TRANSFER_NOT_FOUND: Transfer group does not exist or has expired"
        );
        match op.transfer_action.as_str() {
            "read" => {
                let mut tasks: Vec<_> = self
                    .items
                    .lock()
                    .unwrap()
                    .values()
                    .map(|handle| handle.snapshot())
                    .filter(|view| {
                        view.server_id == server_id && view.group_id.as_deref() == Some(group)
                    })
                    .collect();
                tasks.sort_by(|a, b| a.remote_path.cmp(&b.remote_path));
                Ok(json!({
                    "group_id":group,
                    "group":self.group_summary(server_id, group),
                    "tasks":tasks,
                }))
            }
            "cancel" => {
                let cancelled = self.cancel_group(server_id, group);
                Ok(json!({
                    "group_id":group,
                    "cancel_requested":true,
                    "cancelled":cancelled,
                }))
            }
            "pause" => {
                let handles = self.group_handles(server_id, group);
                let mut paused = 0;
                for handle in handles {
                    if !terminal(&handle.snapshot().status) && self.pause_handle(&handle) {
                        paused += 1;
                    }
                }
                Ok(json!({
                    "group_id":group,
                    "pause_requested":paused > 0,
                    "paused":paused,
                    "group":self.group_summary(server_id, group),
                }))
            }
            "resume" => {
                let handles = self.group_handles(server_id, group);
                let mut resumed = 0;
                let mut failed = Vec::new();
                for handle in handles {
                    if !matches!(handle.snapshot().status.as_str(), "paused" | "failed") {
                        continue;
                    }
                    match self.resume_handle(state, &handle, source).await {
                        Ok(()) => resumed += 1,
                        Err(error) => failed.push(json!({
                            "transfer_id":handle.snapshot().transfer_id,
                            "error":crate::operation_error::error_response(&error, op),
                        })),
                    }
                }
                Ok(json!({
                    "group_id":group,
                    "resumed":resumed,
                    "failed":failed,
                    "group":self.group_summary(server_id, group),
                }))
            }
            _ => Err(error(
                "INVALID_ARGUMENT",
                "This transfer action does not support group_id",
            )),
        }
    }

    fn group_handles(&self, server_id: &str, group: &str) -> Vec<Arc<Handle>> {
        self.items
            .lock()
            .unwrap()
            .values()
            .filter(|handle| {
                let view = handle.snapshot();
                view.server_id == server_id && view.group_id.as_deref() == Some(group)
            })
            .cloned()
            .collect()
    }
}
impl Drop for TransferManager {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

fn authorize(server: &Server, op: &Operation, source: &str) -> Result<()> {
    anyhow::ensure!(
        !op.server_id.is_empty() && op.server_id.len() <= 128,
        "INVALID_ARGUMENT: server_id is required"
    );
    anyhow::ensure!(
        server.enabled || (source == "desktop" && op.transfer_action == "cancel"),
        "Server is disabled"
    );
    if source == "mcp" {
        let mut required = vec![op.kind.as_str()];
        if op.kind != "transfer" {
            required.push("transfer");
        }
        crate::file_tools::require_mcp_permissions(server, &required)
            .map_err(|e| crate::operation_error::with_stage(e, "authorization"))?;
    }
    Ok(())
}

pub fn local_path(
    spec: &TransferSpec,
    upload: bool,
    settings: &RuntimeSettings,
    source: &str,
) -> Result<PathBuf> {
    if source == "mcp"
        && (if upload {
            &settings.local_upload_roots
        } else {
            &settings.local_download_roots
        })
        .is_empty()
    {
        return Err(error(
            "LOCAL_PATH_DENIED",
            "No local file roots are configured for this transfer direction",
        ));
    }
    let input = Path::new(&spec.local_path);
    anyhow::ensure!(
        spec.local_path.len() <= 8192
            && input.is_absolute()
            && !spec.local_path.contains('\0')
            && !input
                .components()
                .any(|c| matches!(c, Component::ParentDir)),
        "INVALID_ARGUMENT: local_path must be an absolute file path without .."
    );
    let resolved = if upload {
        input.canonicalize()?
    } else {
        if let Ok(meta) = std::fs::symlink_metadata(input) {
            anyhow::ensure!(
                (meta.is_file() || (spec.directory && meta.is_dir()))
                    && !meta.file_type().is_symlink(),
                "INVALID_ARGUMENT: download destination must be a regular file, not a symlink"
            );
            anyhow::ensure!(
                spec.overwrite || spec.skip_existing || spec.directory,
                "ALREADY_EXISTS: Local target already exists"
            );
        }
        let mut parent = input.parent().context("Invalid local parent")?;
        let mut suffix = vec![input
            .file_name()
            .context("local_path must name a file")?
            .to_os_string()];
        while !parent.exists() {
            anyhow::ensure!(
                spec.create_parents,
                "PARENT_NOT_FOUND: Local parent directory does not exist"
            );
            suffix.push(
                parent
                    .file_name()
                    .context("Invalid local parent")?
                    .to_os_string(),
            );
            parent = parent.parent().context("Invalid local parent")?;
        }
        let mut path = parent.canonicalize()?;
        anyhow::ensure!(
            path.is_dir(),
            "NOT_A_DIRECTORY: Local parent is not a directory"
        );
        for name in suffix.into_iter().rev() {
            path.push(name);
        }
        path
    };
    if source == "mcp" {
        let roots = if upload {
            &settings.local_upload_roots
        } else {
            &settings.local_download_roots
        };
        let inside = roots
            .iter()
            .filter_map(|r| Path::new(r).canonicalize().ok())
            .any(|r| r.is_dir() && (resolved != r || spec.directory) && resolved.starts_with(r));
        if !inside {
            return Err(error(
                "LOCAL_PATH_DENIED",
                "Local path is outside the allowed transfer roots",
            ));
        }
    }
    if upload {
        anyhow::ensure!(
            resolved.is_file() || (spec.directory && resolved.is_dir()),
            "INVALID_ARGUMENT: upload source must be a regular file"
        );
    }
    Ok(resolved)
}

pub async fn start(state: Arc<AppState>, mut op: Operation, source: &'static str) -> Result<Value> {
    let result = start_inner(state.clone(), op.clone(), source).await;
    if result.is_err() {
        if let Ok(server) = state.server(&op.server_id) {
            op.path = op
                .transfer_request
                .as_ref()
                .map(|s| s.remote_path.clone())
                .unwrap_or_default();
            state.log(&server, source, &op, &result, 0)?;
        }
    }
    result
}

async fn start_inner(
    state: Arc<AppState>,
    mut op: Operation,
    source: &'static str,
) -> Result<Value> {
    anyhow::ensure!(
        op.request_id
            .as_ref()
            .is_none_or(|id| !id.is_empty() && id.len() <= 128),
        "INVALID_ARGUMENT: request_id must be 1-128 bytes"
    );
    let server = state.server(&op.server_id)?;
    authorize(&server, &op, source)?;
    anyhow::ensure!(
        matches!(op.kind.as_str(), "upload_file" | "download_file"),
        "INVALID_ARGUMENT: invalid transfer direction"
    );
    let spec = op
        .transfer_request
        .as_ref()
        .context("INVALID_ARGUMENT: transfer parameters required")?;
    anyhow::ensure!(
        !spec.remote_path.is_empty()
            && spec.remote_path.len() <= 8192
            && spec.remote_path.starts_with('/')
            && !spec.remote_path.contains(['\0', '\\'])
            && !spec.remote_path.split('/').any(|p| p == ".."),
        "INVALID_ARGUMENT: remote_path must be an absolute POSIX file path without .."
    );
    let settings = state.settings.lock().unwrap().clone();
    crate::security::check(&settings, &op, source)?;
    if let Some(request_id) = &op.request_id {
        let existing = state
            .transfers
            .items
            .lock()
            .unwrap()
            .values()
            .find(|h| {
                let original = h.operation.lock().unwrap();
                original.server_id == op.server_id
                    && original.kind == op.kind
                    && original.request_id.as_ref() == Some(request_id)
            })
            .cloned();
        if let Some(existing) = existing {
            anyhow::ensure!(
                same_request(&existing.operation.lock().unwrap(), &op),
                "INVALID_ARGUMENT: request_id was used for different transfer parameters"
            );
            let mut validation = spec.clone();
            validation.overwrite = true;
            local_path(&validation, op.kind == "upload_file", &settings, source)?;
            return Ok(serde_json::to_value(existing.snapshot())?);
        }
    }
    let upload = op.kind == "upload_file";
    let selected = spec.clone();
    let kind = op.kind.clone();
    let (resolved, local_file, size) = tokio::task::spawn_blocking(move || -> Result<_> {
        let resolved = local_path(&selected, upload, &settings, source)?;
        let file = if upload && !selected.directory {
            Some(File::open(&resolved)?)
        } else {
            None
        };
        let size = file
            .as_ref()
            .map(|f| f.metadata().map(|m| m.len()))
            .transpose()?;
        if let Some(size) = size {
            anyhow::ensure!(
                size <= settings.transfer_limit(&kind),
                "PAYLOAD_TOO_LARGE: File exceeds the upload limit"
            );
        }
        Ok((resolved, file, size))
    })
    .await??;
    op.transfer_request.as_mut().unwrap().size = size;
    op.path = op.transfer_request.as_ref().unwrap().remote_path.clone();
    op.target = resolved.to_string_lossy().into_owned();
    let h = state.transfers.insert(&server, &op)?;
    op.transfer_id = h.snapshot().transfer_id.clone();
    if terminal(&h.snapshot().status)
        || h.snapshot().status == "paused"
        || h.worker_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Ok(serde_json::to_value(h.snapshot())?);
    }
    *h.operation.lock().unwrap() = op;
    *h.origin.lock().unwrap() = source.into();
    h.worker_active.store(true, Ordering::Release);
    if let Err(e) = h.persist() {
        state
            .transfers
            .items
            .lock()
            .unwrap()
            .remove(&h.snapshot().transfer_id);
        return Err(e);
    }
    let initial = serde_json::to_value(h.snapshot())?;
    drop(local_file);
    spawn_worker(state, h);
    Ok(initial)
}

pub fn copy_file(
    reader: &mut impl Read,
    writer: &mut impl Write,
    total: u64,
    limit: u64,
    handle: &Handle,
) -> Result<String> {
    if total > limit {
        return Err(error(
            "PAYLOAD_TOO_LARGE",
            "File exceeds the transfer size limit",
        ));
    }
    let mut bytes = vec![0; CHUNK_BYTES];
    let mut count = 0_u64;
    let mut hash = Sha256::new();
    let start = Instant::now();
    let mut last = start;
    handle.progress(0, total, Duration::ZERO);
    loop {
        handle.check_cancelled()?;
        let n = reader.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        let next = count.checked_add(n as u64).context("File size overflow")?;
        if next > total || next > limit {
            return Err(error(
                "SOURCE_CHANGED",
                "Source file size changed during transfer",
            ));
        }
        let mut written = 0;
        while written < n {
            handle.check_cancelled()?;
            match writer.write(&bytes[written..n]) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "File stream stopped accepting bytes",
                    )
                    .into())
                }
                Ok(amount) => {
                    written += amount;
                    handle
                        .confirmed_bytes
                        .store(count + written as u64, Ordering::Release);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        hash.update(&bytes[..n]);
        count = next;
        if last.elapsed() >= Duration::from_millis(200) {
            handle.progress(count, total, start.elapsed());
            last = Instant::now();
        }
    }
    if count != total {
        return Err(error(
            "SOURCE_CHANGED",
            "Source file was truncated during transfer",
        ));
    }
    handle.check_cancelled()?;
    handle.progress(count, total, start.elapsed());
    Ok(format!("{:x}", hash.finalize()))
}

pub fn validate_commit(
    state: &AppState,
    server: &Server,
    op: &Operation,
    source: &str,
    resolved: &Path,
) -> Result<()> {
    let live = state.server(&server.id)?;
    authorize(&live, op, source)?;
    anyhow::ensure!(
        serde_json::to_value(&live)? == serde_json::to_value(server)?,
        "Server configuration changed; transfer was not committed"
    );
    let settings = state.settings.lock().unwrap().clone();
    crate::security::check(&settings, op, source)?;
    anyhow::ensure!(
        local_path(
            op.transfer_request.as_ref().unwrap(),
            op.kind == "upload_file",
            &settings,
            source
        )? == resolved,
        "Local path changed; transfer was not committed"
    );
    Ok(())
}

pub fn unchanged(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.len() == after.len() && before.modified().ok() == after.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cleanup_removes_only_the_owned_partial_file_and_task_record() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().join("db")).unwrap();
        let server = state
            .save_server(
                Server {
                    name: "fixture".into(),
                    host: "localhost".into(),
                    auth: "agent".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let target = dir.path().join("keep.bin");
        std::fs::write(&target, b"keep original").unwrap();
        let mut selected = spec(&target);
        selected.overwrite = true;
        let op = Operation {
            kind: "download_file".into(),
            server_id: server.id.clone(),
            transfer_request: Some(selected),
            ..Default::default()
        };
        let h = state.transfers.insert(&server, &op).unwrap();
        let id = h.snapshot().transfer_id;
        h.operation.lock().unwrap().transfer_id = id.clone();
        let partial = dir.path().join(format!(".remote-ssh-brige-{id}.part"));
        std::fs::write(&partial, b"partial").unwrap();
        h.checkpoint.lock().unwrap().temp = partial.to_string_lossy().into_owned();
        h.control.store(4, Ordering::Release);
        h.complete(&Err(error("TRANSFER_PAUSED", "paused")));
        state
            .transfers
            .operate(
                &state,
                &Operation {
                    kind: "transfer".into(),
                    server_id: server.id,
                    transfer_id: id.clone(),
                    transfer_action: "cleanup".into(),
                    ..Default::default()
                },
                "desktop",
            )
            .await
            .unwrap();
        assert!(!partial.exists());
        assert_eq!(std::fs::read(target).unwrap(), b"keep original");
        assert!(!state.transfers.items.lock().unwrap().contains_key(&id));
        assert!(state
            .database
            .lock()
            .unwrap()
            .transfers()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn persisted_tasks_restore_paused_with_authorization_origin_and_request_id() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().to_path_buf()).unwrap();
        let server = Server {
            id: "s".into(),
            host: "localhost".into(),
            ..Default::default()
        };
        let op = Operation {
            kind: "upload_file".into(),
            server_id: "s".into(),
            path: "/workspace/test.bin".into(),
            request_id: Some("request".into()),
            transfer_request: Some(spec(&dir.path().join("source"))),
            ..Default::default()
        };
        let h = state.transfers.insert(&server, &op).unwrap();
        let id = h.snapshot().transfer_id;
        h.operation.lock().unwrap().transfer_id = id.clone();
        *h.origin.lock().unwrap() = "mcp".into();
        h.progress(4096, 8192, Duration::from_secs(1));
        h.persist().unwrap();
        drop(h);
        drop(state);
        let restored = AppState::open(dir.path().to_path_buf()).unwrap();
        let handles = restored.transfers.items.lock().unwrap();
        let h = handles.get(&id).unwrap();
        assert_eq!(h.snapshot().status, "paused");
        assert_eq!(h.snapshot().transferred_bytes, 4096);
        assert_eq!(
            h.operation.lock().unwrap().request_id.as_deref(),
            Some("request")
        );
        assert!(h.operation.lock().unwrap().transfer_request.is_some());
        assert_eq!(*h.origin.lock().unwrap(), "mcp");
    }
    #[tokio::test]
    async fn more_than_four_tasks_queue_and_aliases_share_endpoint_limit() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().to_path_buf()).unwrap();
        let mut handles = Vec::new();
        for i in 0..6 {
            let server = Server {
                id: format!("alias{i}"),
                host: "same-host".into(),
                ..Default::default()
            };
            let mut spec = spec(&dir.path().join("source"));
            spec.remote_path = format!("/workspace/{i}");
            let h = state
                .transfers
                .insert(
                    &server,
                    &Operation {
                        kind: "upload_file".into(),
                        server_id: server.id.clone(),
                        transfer_request: Some(spec),
                        ..Default::default()
                    },
                )
                .unwrap();
            h.worker_active.store(true, Ordering::Release);
            handles.push(h);
        }
        assert_eq!(state.transfers.items.lock().unwrap().len(), 6);
        let a = state.transfers.acquire(&state, &handles[0]).await.unwrap();
        let b = state.transfers.acquire(&state, &handles[1]).await.unwrap();
        assert!(tokio::time::timeout(
            Duration::from_millis(120),
            state.transfers.acquire(&state, &handles[2])
        )
        .await
        .is_err());
        drop(a);
        drop(b);
        handles[2].update(|s| s.priority = 10);
        let _c = state.transfers.acquire(&state, &handles[2]).await.unwrap();
    }
    fn spec(local: &Path) -> TransferSpec {
        TransferSpec {
            local_path: local.to_string_lossy().into_owned(),
            remote_path: "/workspace/test.bin".into(),
            overwrite: false,
            size: None,
            sha256: None,
            create_parents: false,
            skip_existing: false,
            directory: false,
        }
    }
    pub(super) fn handle() -> Arc<Handle> {
        let manager = TransferManager::default();
        let handle = manager
            .insert(
                &Server {
                    id: "test".into(),
                    ..Default::default()
                },
                &Operation {
                    kind: "upload_file".into(),
                    transfer_request: Some(spec(Path::new("/tmp/source"))),
                    ..Default::default()
                },
            )
            .unwrap();
        manager.items.lock().unwrap().clear();
        handle
    }
    #[derive(Default)]
    struct CountWriter {
        count: u64,
        max_chunk: usize,
    }
    impl Write for CountWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.count += bytes.len() as u64;
            self.max_chunk = self.max_chunk.max(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn gigabyte_stream_keeps_a_fixed_buffer_and_is_not_complete_until_commit() {
        let h = handle();
        let total = 1024 * 1024 * 1024 + 17;
        let mut source = std::io::repeat(0x53).take(total);
        let mut output = CountWriter::default();
        assert_eq!(
            copy_file(&mut source, &mut output, total, u64::MAX, &h)
                .unwrap()
                .len(),
            64
        );
        assert_eq!(output.count, total);
        assert!(output.max_chunk <= CHUNK_BYTES);
        assert_eq!(h.snapshot().transferred_bytes, total);
        assert_ne!(h.snapshot().status, "completed");
        h.begin_commit().unwrap();
        assert!(!h.request_cancel());
        h.complete(&Ok(json!({})));
        assert_eq!(h.snapshot().status, "completed");
    }
    #[test]
    fn completed_local_file_only_returns_an_existing_completed_transfer_file() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("download.bin");
        std::fs::write(&file, b"complete").unwrap();
        let manager = TransferManager::default();
        let handle = manager
            .insert(
                &Server {
                    id: "server".into(),
                    ..Default::default()
                },
                &Operation {
                    kind: "download_file".into(),
                    transfer_request: Some(spec(&file)),
                    ..Default::default()
                },
            )
            .unwrap();
        let id = handle.snapshot().transfer_id;
        assert!(manager.completed_local_file("server", &id).is_err());
        handle.complete(&Ok(json!({})));
        assert_eq!(
            manager.completed_local_file("server", &id).unwrap(),
            file.canonicalize().unwrap()
        );
        assert!(manager.completed_local_file("other", &id).is_err());
    }
    #[test]
    fn cancellation_and_source_changes_never_enter_commit() {
        let h = handle();
        assert!(h.request_cancel());
        let mut output = CountWriter::default();
        let error =
            copy_file(&mut std::io::repeat(0).take(10), &mut output, 10, 10, &h).unwrap_err();
        assert_eq!(
            crate::operation_error::error_code(&error),
            "TRANSFER_CANCELLED"
        );
        assert_eq!(output.count, 0);
        assert!(h.begin_commit().is_err());
        h.complete(&Err(anyhow::anyhow!("network error after cancellation")));
        assert_eq!(h.snapshot().status, "cancelled");
        for (actual, expected, limit) in [(11, 10, 10), (9, 10, 10), (10, 10, 9)] {
            assert!(copy_file(
                &mut std::io::repeat(0).take(actual),
                &mut CountWriter::default(),
                expected,
                limit,
                &handle()
            )
            .is_err());
        }
    }
    #[test]
    fn partial_write_failure_retains_exact_confirmed_byte_count() {
        struct FailingWriter {
            remaining: usize,
        }
        impl Write for FailingWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.remaining == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::StorageFull,
                        "disk full",
                    ));
                }
                let n = bytes.len().min(self.remaining);
                self.remaining -= n;
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let h = handle();
        let result = copy_file(
            &mut std::io::repeat(1).take(1000),
            &mut FailingWriter { remaining: 123 },
            1000,
            1000,
            &h,
        )
        .map(|_| json!({}));
        assert!(result.is_err());
        h.complete(&result);
        assert_eq!(h.snapshot().status, "failed");
        assert_eq!(h.snapshot().transferred_bytes, 123);
    }

    #[tokio::test]
    async fn upload_does_not_enter_the_command_approval_queue() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        std::fs::write(&source, "FILE_BODY_MUST_NOT_BE_IN_APPROVAL_OR_AUDIT").unwrap();
        let state = AppState::open(dir.path().join("db")).unwrap();
        state
            .save_settings(RuntimeSettings {
                local_upload_roots: vec![dir.path().to_string_lossy().into_owned()],
                transfers: TransferSettings {
                    max_retries: 0,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        let server = state
            .save_server(
                Server {
                    name: "fixture".into(),
                    host: "127.0.0.1".into(),
                    port: 1,
                    auth: "agent".into(),
                    mcp_permissions: Some(vec!["upload_file".into(), "transfer".into()]),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let initial = start(
            state.clone(),
            Operation {
                kind: "upload_file".into(),
                server_id: server.id.clone(),
                transfer_request: Some(spec(&source)),
                ..Default::default()
            },
            "mcp",
        )
        .await
        .unwrap();
        assert!(state.approvals.list().is_empty());
        let deadline = Instant::now() + Duration::from_secs(3);
        let h = state
            .transfers
            .items
            .lock()
            .unwrap()
            .get(initial["transfer_id"].as_str().unwrap())
            .unwrap()
            .clone();
        while !terminal(&h.snapshot().status) {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(h.snapshot().status, "failed");
        let logs = serde_json::to_string(&*state.audit.lock().unwrap()).unwrap();
        assert!(!logs.contains("FILE_BODY_MUST_NOT_BE_IN_APPROVAL_OR_AUDIT"));
    }
    #[test]
    fn local_access_requires_direction_specific_roots_and_refuses_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = root.path().join("source.bin");
        std::fs::write(&file, b"private file").unwrap();
        let mut settings = RuntimeSettings::default();
        assert!(local_path(&spec(&file), true, &settings, "mcp").is_err());
        settings.local_upload_roots = vec![root.path().to_string_lossy().into_owned()];
        assert!(local_path(&spec(&file), true, &settings, "mcp").is_ok());
        assert!(local_path(
            &spec(&outside.path().join("download.bin")),
            false,
            &settings,
            "mcp"
        )
        .is_err());
        settings.local_download_roots = settings.local_upload_roots.clone();
        assert!(local_path(
            &spec(&root.path().join("download.bin")),
            false,
            &settings,
            "mcp"
        )
        .is_ok());
        assert!(local_path(&spec(&file), false, &settings, "mcp").is_err());
        assert!(local_path(
            &spec(&root.path().join("../escape")),
            false,
            &settings,
            "mcp"
        )
        .is_err());
        assert!(local_path(&spec(&file), true, &RuntimeSettings::default(), "desktop").is_ok());
        #[cfg(unix)]
        {
            let secret = outside.path().join("secret");
            std::fs::write(&secret, b"secret").unwrap();
            let link = root.path().join("escape");
            std::os::unix::fs::symlink(&secret, &link).unwrap();
            assert!(local_path(&spec(&link), true, &settings, "mcp").is_err());
            let mut download = spec(&link);
            download.overwrite = true;
            assert!(local_path(&download, false, &settings, "mcp").is_err());
        }
    }
    #[test]
    fn unlimited_files_do_not_increase_inline_text_budgets() {
        let settings = RuntimeSettings {
            upload_limit_mib: 0,
            download_limit_mib: 0,
            ..Default::default()
        };
        settings.validate().unwrap();
        assert_eq!(settings.transfer_limit("upload_file"), u64::MAX);
        assert_eq!(settings.transfer_limit("download_file"), u64::MAX);
        assert_eq!(settings.transfer_limit("write_file"), 256 * 1024 * 1024);
        assert_eq!(settings.transfer_limit("read_file"), 256 * 1024 * 1024);
        assert_eq!(settings.transfer_limit("upload"), 256 * 1024 * 1024);
    }
    #[test]
    fn audit_stores_only_transfer_parameters_and_results() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().to_path_buf()).unwrap();
        let server = Server {
            id: "s".into(),
            name: "fixture".into(),
            ..Default::default()
        };
        let op = Operation {
            kind: "download_file".into(),
            server_id: server.id.clone(),
            transfer_id: "transfer_test".into(),
            transfer_request: Some(spec(&dir.path().join("saved.bin"))),
            ..Default::default()
        };
        state.log(&server, "mcp", &op, &Ok(json!({"transfer_id":"transfer_test","status":"completed","transferred_bytes":999,"local_path":op.transfer_request.as_ref().unwrap().local_path,"remote_path":"/workspace/test.bin","content":"DO_NOT_PERSIST_FILE_CONTENT","base64":"DO_NOT_PERSIST_FILE_BYTES","chunks":["DO_NOT_PERSIST_CHUNKS"]})), 12).unwrap();
        let entry = state.audit.lock().unwrap()[0].clone();
        let data = entry.response.unwrap().data;
        assert_eq!(data["transfer_id"], "transfer_test");
        assert!(!data.to_string().contains("DO_NOT_PERSIST"));
        assert_eq!(data["transferred_bytes"], 999);
        assert!(data.get("content").is_none());
        assert!(data.get("base64").is_none());
    }
    #[tokio::test]
    async fn unversioned_read_waits_through_preparing_until_completion() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().to_path_buf()).unwrap();
        let server = state
            .save_server(
                Server {
                    name: "fixture".into(),
                    host: "localhost".into(),
                    auth: "agent".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let h = state
            .transfers
            .insert(
                &server,
                &Operation {
                    kind: "upload_file".into(),
                    transfer_request: Some(spec(&dir.path().join("source"))),
                    ..Default::default()
                },
            )
            .unwrap();
        let writer = h.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            writer.update(|s| s.status = "running".into());
            tokio::time::sleep(Duration::from_millis(30)).await;
            writer.progress(28, 28, Duration::from_millis(50));
            writer.begin_commit().unwrap();
            writer.complete(&Ok(json!({})));
        });
        let started = Instant::now();
        let result = state
            .transfers
            .operate(
                &state,
                &Operation {
                    kind: "transfer".into(),
                    server_id: server.id,
                    transfer_action: "read".into(),
                    transfer_id: h.snapshot().transfer_id,
                    wait_secs: 2,
                    ..Default::default()
                },
                "desktop",
            )
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(40));
        assert_eq!(result["status"], "completed");
        assert_eq!(result["transferred_bytes"], 28);
        assert_eq!(result["bytes_per_second"], 560);
    }

    #[tokio::test]
    async fn waiting_reads_wake_on_version_change_and_cancel_is_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().to_path_buf()).unwrap();
        let server = state
            .save_server(
                Server {
                    name: "fixture".into(),
                    host: "localhost".into(),
                    auth: "agent".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let op = Operation {
            kind: "upload_file".into(),
            server_id: server.id.clone(),
            transfer_request: Some(spec(&dir.path().join("source"))),
            ..Default::default()
        };
        let h = state.transfers.insert(&server, &op).unwrap();
        let initial = h.snapshot();
        let writer = h.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            writer.progress(5, 10, Duration::from_secs(1));
        });
        let read = Operation {
            kind: "transfer".into(),
            server_id: server.id.clone(),
            transfer_action: "read".into(),
            transfer_id: initial.transfer_id.clone(),
            version: Some(initial.version),
            wait_secs: 2,
            ..Default::default()
        };
        let result = state
            .transfers
            .operate(&state, &read, "desktop")
            .await
            .unwrap();
        assert_eq!(result["transferred_bytes"], 5);
        assert!(result["version"].as_u64().unwrap() > initial.version);
        let cancel = Operation {
            transfer_action: "cancel".into(),
            ..read
        };
        assert_eq!(
            state
                .transfers
                .operate(&state, &cancel, "desktop")
                .await
                .unwrap()["cancel_requested"],
            true
        );
        assert!(h.begin_commit().is_err());
    }
    #[test]
    fn batch_groups_aggregate_counts_and_fail_fast_cancels_siblings() {
        let manager = TransferManager::default();
        let server = Server {
            id: "s".into(),
            ..Default::default()
        };
        let operation = |remote: &str| Operation {
            kind: "upload_file".into(),
            server_id: "s".into(),
            transfer_group: Some("group".into()),
            transfer_request: Some(TransferSpec {
                remote_path: remote.into(),
                ..spec(Path::new("source"))
            }),
            ..Default::default()
        };
        let first = manager.insert(&server, &operation("/a")).unwrap();
        let second = manager.insert(&server, &operation("/b")).unwrap();
        let third = manager.insert(&server, &operation("/c")).unwrap();
        manager.register_group("group", crate::transfer_planner::BatchErrorPolicy::FailFast);
        assert_eq!(first.snapshot().batch_policy.as_deref(), Some("fail_fast"));
        assert_eq!(manager.group_summary("s", "group")["queued"], 3);
        first.complete(&Err(error("IO_TIMEOUT", "network")));
        manager.group_terminal("group", "failed");
        assert_eq!(first.snapshot().status, "failed");
        assert_eq!(second.control.load(Ordering::Acquire), 1);
        assert_eq!(third.control.load(Ordering::Acquire), 1);
        let summary = manager.group_summary("s", "group");
        assert_eq!(summary["total"], 3);
        assert_eq!(summary["failed"], 1);
        assert_eq!(manager.group_summaries("other").len(), 0);
        assert_eq!(manager.cancel_group("other", "group"), 0);
        assert!(manager.group_exists("s", "group"));
        assert!(!manager.group_exists("other", "group"));
    }
}
