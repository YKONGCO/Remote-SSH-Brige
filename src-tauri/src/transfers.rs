//! Transfer content only through fixed-size file streams. No content enters IPC or audit.
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

// A fixed 1 MiB window lets libssh2 pipeline SFTP requests on higher-latency links.
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
}

pub struct Handle {
    snapshot: Mutex<Snapshot>,
    notify: tokio::sync::Notify,
    cancel: CancellationToken,
    // 0 transfers, 1 cancellation accepted, 2 committing, 3 terminal.
    control: AtomicU8,
    confirmed_bytes: AtomicU64,
    destination_key: String,
    created: Instant,
    finished: Mutex<Option<Instant>>,
    state: Mutex<Weak<AppState>>,
    pub(crate) operation: Mutex<Operation>,
    origin: Mutex<String>,
    pub(crate) checkpoint: Mutex<crate::transfer_io::Checkpoint>,
    worker_active: std::sync::atomic::AtomicBool,
    server_identity: Mutex<Option<Server>>,
    journal_lock: Mutex<()>,
    lifecycle: tokio::sync::Mutex<()>,
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
        self.update(|s| {
            s.phase = "committing".into();
        });
        Ok(())
    }
    fn request_cancel(&self) -> bool {
        match self
            .control
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                self.cancel.cancel();
                self.update(|s| s.phase = "cancelling".into());
                true
            }
            Err(1) => true,
            _ => false,
        }
    }
    fn progress(&self, count: u64, total: u64, elapsed: Duration) {
        self.confirmed_bytes.store(count, Ordering::Release);
        self.update(|s| {
            s.transferred_bytes = count;
            s.total_bytes = Some(total);
            s.bytes_per_second = (count as f64 / elapsed.as_secs_f64().max(0.001)) as u64;
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
            // Preserve the measured streaming average after completion.
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
                s.error = Some(crate::operation_error::error_response(
                    if cancelled { &cancellation } else { e },
                    &Operation {
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
                    },
                ));
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

include!("transfer_tasks.rs");

#[derive(Default)]
pub struct TransferManager {
    items: Mutex<HashMap<String, Arc<Handle>>>,
    running: Mutex<HashSet<String>>,
    state: Mutex<Weak<AppState>>,
}
impl TransferManager {
    pub fn prune(&self) {
        // Failed/paused tasks keep their checkpoints until explicit cleanup.
        // Only successful records without retained partial data expire.
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
    fn insert(&self, server: &Server, op: &Operation) -> Result<Arc<Handle>> {
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
        });
        items.insert(id, h.clone());
        Ok(h)
    }
    pub fn cancel_all(&self) {
        for h in self.items.lock().unwrap().values() {
            h.request_cancel();
        }
    }
    pub async fn operate(
        &self,
        state: &Arc<AppState>,
        op: &Operation,
        source: &str,
    ) -> Result<Value> {
        let server = state.server(&op.server_id)?;
        authorize(&server, op, source)?;
        anyhow::ensure!(
            matches!(
                op.transfer_action.as_str(),
                "list" | "read" | "cancel" | "pause" | "resume" | "retry" | "cleanup" | "configure"
            ) && op.wait_secs <= 30,
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
            return Ok(json!({"transfers":items}));
        }
        anyhow::ensure!(
            !op.transfer_id.is_empty() && op.transfer_id.len() <= 128,
            "INVALID_ARGUMENT: transfer_id is required"
        );
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
            // Without a version, wait for completion (or timeout). With a
            // version, return the next change for incremental progress polling.
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

/// Resolve symlinks before checking local roots; downloads authorize the parent
/// and refuse symlink destinations. Native picker selections bypass MCP roots.
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

/// Stream exactly one file, with a fixed allocation and checked u64 offsets.
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

/// Revalidate authorization and local path immediately before publishing a file.
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
    async fn upload_approval_contains_metadata_and_rejection_never_connects() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        std::fs::write(&source, "FILE_BODY_MUST_NOT_BE_IN_APPROVAL_OR_AUDIT").unwrap();
        let state = AppState::open(dir.path().join("db")).unwrap();
        state
            .save_settings(RuntimeSettings {
                approval_mode: "mcp".into(),
                approval_timeout_secs: 7,
                local_upload_roots: vec![dir.path().to_string_lossy().into_owned()],
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
        let deadline = Instant::now() + Duration::from_secs(3);
        let view = loop {
            if let Some(view) = state.approvals.list().first().cloned() {
                break view;
            }
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(view.content_preview.contains("source.txt"));
        let created = chrono::DateTime::parse_from_rfc3339(&view.created_at).unwrap();
        let expires = chrono::DateTime::parse_from_rfc3339(&view.expires_at).unwrap();
        assert!((7000..=7100).contains(&(expires - created).num_milliseconds()));
        assert!(view.content_preview.contains("sha256"));
        assert!(!view
            .content_preview
            .contains("FILE_BODY_MUST_NOT_BE_IN_APPROVAL_OR_AUDIT"));
        state.resolve_approval(&view.id, false).unwrap();
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
}
