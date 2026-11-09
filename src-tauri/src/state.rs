use crate::{database::Database, model::*, terminal::TerminalHandle};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub struct AppState {
    pub dir: PathBuf,
    pub(crate) database: Mutex<Database>,
    pub config: Mutex<Config>,
    pub settings: Mutex<RuntimeSettings>,
    pub jobs: crate::jobs::JobManager,
    pub tunnels: crate::tunnels::TunnelManager,
    pub transfers: crate::transfers::TransferManager,
    pub approvals: crate::security::ApprovalQueue,
    pub audit: Mutex<Vec<Audit>>,
    pub terminals: Mutex<HashMap<String, Arc<TerminalHandle>>>,
    pub terminal_open_lock: Mutex<()>,
    pub file_mutation_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub mcp: Mutex<McpStatus>,
    pub cancel: Mutex<Option<CancellationToken>>,
    pub service_lock: tokio::sync::Mutex<()>,
}
impl AppState {
    pub fn open(dir: PathBuf) -> Result<Arc<Self>> {
        let database = Database::open(&dir)?;
        let config = Config {
            servers: database.servers()?,
        };
        let audit = database.recent_audits(500)?;
        let settings = database.settings()?;
        // Listeners are process-local. Retain their metadata after a restart.
        for mut info in database.tunnel_history()? {
            if info.status != "closed" {
                info.status = "closed".into();
                info.connections = 0;
                info.error = Some("应用已重启，端口转发已关闭".into());
                database.save_tunnel(&info)?;
            }
        }
        let state = Arc::new(Self {
            dir,
            database: Mutex::new(database),
            config: Mutex::new(config),
            settings: Mutex::new(settings),
            jobs: crate::jobs::JobManager::default(),
            tunnels: crate::tunnels::TunnelManager::default(),
            transfers: crate::transfers::TransferManager::default(),
            approvals: crate::security::ApprovalQueue::default(),
            audit: Mutex::new(audit),
            terminals: Mutex::new(HashMap::new()),
            terminal_open_lock: Mutex::new(()),
            file_mutation_locks: Mutex::new(HashMap::new()),
            mcp: Mutex::new(McpStatus::default()),
            cancel: Mutex::new(None),
            service_lock: tokio::sync::Mutex::new(()),
        });
        state.transfers.restore(&state)?;
        let weak = Arc::downgrade(&state);
        std::thread::Builder::new()
            .name("job-cleanup".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                let Some(state) = weak.upgrade() else { break };
                state.jobs.prune_expired();
                state.transfers.prune();
            })?;
        Ok(state)
    }
    pub fn server(&self, id: &str) -> Result<Server> {
        self.config
            .lock()
            .unwrap()
            .servers
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| {
                crate::operation_error::error_with_details(
                    "SERVER_NOT_FOUND",
                    "Server not found; use a server_id from list_servers",
                    json!({"field":"server_id","server_id":id}),
                )
            })
    }
    pub fn save_settings(&self, settings: RuntimeSettings) -> Result<RuntimeSettings> {
        settings.validate()?;
        let mut current = self.settings.lock().unwrap();
        self.database.lock().unwrap().save_settings(&settings)?;
        *current = settings.clone();
        if settings.remote_read_only {
            self.tunnels.stop_all();
        }
        Ok(settings)
    }
    pub fn snapshot(&self) -> Value {
        self.snapshot_with_audits(true)
    }
    pub fn snapshot_with_audits(&self, include_audits: bool) -> Value {
        let terminals: Vec<_> = self
            .terminals
            .lock()
            .unwrap()
            .values()
            .map(|h| h.info())
            .collect();
        let audits: Vec<_> = self
            .audit
            .lock()
            .unwrap()
            .iter()
            .filter(|_| include_audits)
            .map(|entry| {
                let mut summary = entry.clone();
                summary.response = None;
                summary
            })
            .collect();
        json!({"servers": self.config.lock().unwrap().servers, "audits": audits,
            "terminals": terminals, "mcp": self.mcp.lock().unwrap().clone(), "data_dir": self.dir.to_string_lossy(), "database_path": self.dir.join(crate::database::DATABASE_FILE).to_string_lossy()})
    }
    pub fn audit_details(&self, id: &str) -> Result<Audit> {
        self.database.lock().unwrap().audit(id)
    }
    pub fn delete_audits(&self, range: &crate::database::AuditDeleteRange) -> Result<usize> {
        // Match log()'s lock order and replace the cache only after a successful commit.
        let mut audits = self.audit.lock().unwrap();
        let (deleted, recent) = self.database.lock().unwrap().delete_audits(range)?;
        *audits = recent;
        Ok(deleted)
    }
    pub fn audit_page(
        &self,
        query: &crate::database::AuditQuery,
    ) -> Result<crate::database::AuditPage> {
        self.database.lock().unwrap().audit_page(query)
    }
    pub fn export_audits(
        &self,
        query: &crate::database::AuditQuery,
        path: &std::path::Path,
    ) -> Result<()> {
        // A separate WAL reader lets new operations keep writing while a large
        // history is exported.
        Database::open_reader(&self.dir)?.export_audits(query, std::fs::File::create(path)?)
    }
    pub fn resolve_approval(&self, id: &str, approved: bool) -> Result<()> {
        self.approvals.decide(id, approved, |view| {
            let server = Server { id: view.server_id.clone(), name: view.server_name.clone(), ..Default::default() };
            let op = Operation {
                kind: if approved { "security_approve".into() } else { "security_reject".into() },
                server_id: view.server_id.clone(), path: view.path.clone(), command: view.command.clone(), ..Default::default()
            };
            self.log(&server, &view.source, &op, &Ok(json!({"approval":{
                "request_id":view.id,"status":if approved {"approved"} else {"rejected"},
                "approved":approved,"kind":view.kind,"request_source":view.source,"decision_by":"desktop",
                "server_name":view.server_name,"host":view.host,"username":view.username,
                "path":view.path,"target":view.target,"command":view.command,
                "recursive":view.recursive,"missing_ok":view.missing_ok,"create_parents":view.create_parents,
                "requested_at":view.created_at,"decided_at":chrono::Utc::now().to_rfc3339(),"expires_at":view.expires_at,
                "content_bytes":view.content_bytes,"content_sha256":view.content_sha256
            }})), 0)
        }).map(|_| ())
    }
    pub fn save_server(&self, server: Server, secret: Option<String>) -> Result<Server> {
        self.save_server_with_login_path(server, secret, None)
    }

    pub fn save_server_with_login_path(
        &self,
        mut server: Server,
        secret: Option<String>,
        login_path_update: Option<LoginPathUpdate>,
    ) -> Result<Server> {
        if let Some(path) = login_path_update
            .as_ref()
            .and_then(|update| update.value.as_ref())
        {
            anyhow::ensure!(
                !path.contains('\0') && path.len() <= 65536,
                "Invalid PATH: must not contain NUL or exceed 65536 bytes"
            );
        }
        if let Some(permissions) = &mut server.mcp_permissions {
            anyhow::ensure!(
                permissions
                    .iter()
                    .all(|kind| MCP_OPERATIONS.contains(&kind.as_str())),
                "Unknown MCP permission"
            );
            permissions.sort();
            permissions.dedup();
            server.allow_exec = permissions.iter().any(|kind| kind == "exec");
            server.allow_write = permissions
                .iter()
                .any(|kind| matches!(kind.as_str(), "write_file" | "edit_file" | "remove_path"));
            server.allow_terminal = permissions
                .iter()
                .any(|kind| kind.starts_with("terminal_") || kind == "list_terminals");
        }
        server.name = server.name.trim().to_string();
        server.host = server.host.trim().to_string();
        if server.name.is_empty()
            || server.host.is_empty()
            || server.username.trim().is_empty()
            || server.port == 0
        {
            bail!("Server name, host, username and a valid port are required")
        }
        if !["password", "key", "agent"].contains(&server.auth.as_str()) {
            bail!("Unsupported authentication method")
        }
        if server.auth == "key" && server.key_path.is_empty() {
            bail!("Private key path must be absolute")
        }
        if !server.root.starts_with('/') || server.root.split('/').any(|p| p == "..") {
            bail!("File root must be absolute and must not contain ..")
        }
        if server.id.is_empty() {
            server.id = uuid::Uuid::new_v4().to_string();
        }
        let mut config = self.config.lock().unwrap();
        let identity_changed = config
            .servers
            .iter()
            .find(|s| s.id == server.id)
            .is_some_and(|old| {
                old.host != server.host
                    || old.port != server.port
                    || old.username != server.username
            });
        if let Some(old) = config.servers.iter().find(|s| s.id == server.id) {
            server.login_path = old.login_path.clone();
            server.login_path_synced_at = old.login_path_synced_at.clone();
            if old.host != server.host || old.port != server.port || old.username != server.username
            {
                server.login_path = None;
                server.login_path_synced_at = None;
            }
            if old.host != server.host || old.port != server.port {
                server.fingerprint.clear();
            }
        } else {
            server.login_path = None;
            server.login_path_synced_at = None;
            if !server.fingerprint.is_empty() {
                bail!("Leave the fingerprint empty for a new server, then verify the host")
            }
        }
        if let Some(update) = login_path_update.filter(|_| !identity_changed) {
            server.login_path = update.value;
            server.login_path_synced_at = None;
        }
        if let Some(secret) = secret.filter(|s| !s.is_empty()) {
            keyring::Entry::new("dev.remotedesk.ssh", &server.id)?
                .set_password(&secret)
                .context("Cannot save system credentials")?;
        }
        let mut next = config.clone();
        if let Some(s) = next.servers.iter_mut().find(|s| s.id == server.id) {
            *s = server.clone();
        } else {
            next.servers.push(server.clone());
        }
        self.database.lock().unwrap().save_server(&server)?;
        if config
            .servers
            .iter()
            .find(|s| s.id == server.id)
            .is_some_and(|old| serde_json::to_value(old).ok() != serde_json::to_value(&server).ok())
        {
            self.tunnels.stop_server(&server.id);
        }
        *config = next;
        Ok(server)
    }
    pub fn sync_login_path(&self, id: &str) -> Result<Server> {
        let server = self.server(id)?;
        anyhow::ensure!(server.enabled, "Server is disabled");
        let settings = self.settings.lock().unwrap().clone();
        let path = crate::ssh::sync_login_path(&server, &settings)?;
        self.store_login_path(&server, path)
    }

    pub fn store_login_path(&self, expected: &Server, path: String) -> Result<Server> {
        anyhow::ensure!(
            !path.contains('\0') && path.len() <= 65536,
            "Invalid login PATH"
        );
        let mut config = self.config.lock().unwrap();
        let live = config
            .servers
            .iter_mut()
            .find(|s| s.id == expected.id)
            .context("Server was removed during PATH synchronization")?;
        anyhow::ensure!(
            serde_json::to_value(&*live)? == serde_json::to_value(expected)?,
            "Server configuration changed during PATH synchronization; retry"
        );
        let mut updated = live.clone();
        updated.login_path = Some(path);
        updated.login_path_synced_at = Some(chrono::Utc::now().to_rfc3339());
        self.database.lock().unwrap().save_server(&updated)?;
        *live = updated.clone();
        Ok(updated)
    }

    pub fn remove_server(&self, id: &str) -> Result<()> {
        if self
            .terminals
            .lock()
            .unwrap()
            .values()
            .any(|t| t.info().server_id == id && t.info().alive)
        {
            bail!("Close this server's terminal sessions first")
        }
        let mut config = self.config.lock().unwrap();
        let mut next = config.clone();
        next.servers.retain(|s| s.id != id);
        self.database.lock().unwrap().remove_server(id)?;
        self.tunnels.stop_server(id);
        *config = next;
        match keyring::Entry::new("dev.remotedesk.ssh", id)?.delete_credential() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn log(
        &self,
        server: &Server,
        source: &str,
        op: &Operation,
        result: &Result<Value>,
        elapsed: u64,
    ) -> Result<()> {
        let detail = if op.kind.starts_with("port_forward_") {
            if op.kind == "port_forward_open" {
                format!(
                    "127.0.0.1:{} → {}:{}",
                    op.local_port, op.remote_host, op.remote_port
                )
            } else {
                op.tunnel_id.clone()
            }
        } else if op.kind.starts_with("terminal") {
            op.session_id.clone()
        } else if op.kind == "exec" || (op.kind.starts_with("security_") && !op.command.is_empty())
        {
            op.command.chars().take(1000).collect()
        } else {
            if op.kind == "job" {
                op.job_id.clone()
            } else {
                op.path.clone()
            }
        };
        let summary = match result {
            Ok(_) if op.kind == "security_approve" => "审批已批准".into(),
            Ok(_) if op.kind == "security_reject" => "审批已拒绝，操作未执行".into(),
            Ok(v) if op.kind == "exec" && v.get("job_id").is_some() => {
                format!("已转入 Job: {}", v["job_id"])
            }
            Ok(v) if op.kind == "job" => format!("Job {}: {}", op.job_id, v["status"]),
            Ok(v) if op.kind == "transfer" && op.transfer_action != "list" => {
                format!("Transfer {}: {}", op.transfer_id, v["status"])
            }
            Ok(v) if matches!(op.kind.as_str(), "upload_file" | "download_file") => {
                format!("{}: {}", op.transfer_id, v["status"])
            }
            Ok(v) if op.kind == "exec" => format!("exit: {}", v["exit_code"]),
            Ok(_) => "完成".into(),
            Err(e) => crate::operation_error::error_message(e)
                .chars()
                .take(1000)
                .collect(),
        };
        let entry = Audit {
            id: uuid::Uuid::new_v4().to_string(),
            time: chrono::Utc::now().to_rfc3339(),
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            source: source.into(),
            kind: op.kind.clone(),
            detail,
            success: result
                .as_ref()
                .ok()
                .is_some_and(|v| op.kind == "exec" && v.get("job_id").is_some())
                || (!result.as_ref().ok().is_some_and(|v| {
                    match v["status"].as_str() {
                        // A closed tunnel is the expected outcome of a successful close.
                        Some("closed") => op.kind != "port_forward_close",
                        Some("disconnected" | "failed" | "cancelled") => true,
                        _ => false,
                    }
                }) && op.kind != "security_reject"
                    && result.is_ok()
                    && result
                        .as_ref()
                        .ok()
                        .and_then(|v| v.get("exit_code"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0)
                        == 0),
            duration_ms: elapsed,
            summary,
            response: Some(audit_response(op, result)),
        };
        let mut audits = self.audit.lock().unwrap();
        self.database
            .lock()
            .unwrap()
            .insert_audit(&entry)
            .context("Cannot write audit record to SQLite")?;
        audits.push(entry);
        if audits.len() > 500 {
            audits.remove(0);
        }
        Ok(())
    }
}

pub async fn execute(state: Arc<AppState>, op: Operation, source: &'static str) -> Result<Value> {
    execute_with_progress(state, op, source, None).await
}

pub async fn execute_with_progress(
    state: Arc<AppState>,
    mut op: Operation,
    source: &'static str,
    progress: crate::jobs::Progress,
) -> Result<Value> {
    if op.kind == "port_forward_open" {
        op.path = format!("127.0.0.1:{}", op.local_port);
        op.target = format!("{}:{}", op.remote_host, op.remote_port);
    }
    if matches!(op.kind.as_str(), "upload_file" | "download_file") {
        return crate::transfers::start(state, op, source).await;
    }
    if op.kind == "transfer" {
        let start = std::time::Instant::now();
        let result = state.transfers.operate(&state, &op, source).await;
        if source == "mcp" {
            state.log(
                &state.server(&op.server_id)?,
                source,
                &op,
                &result,
                start.elapsed().as_millis() as u64,
            )?;
        }
        return result;
    }
    let start = std::time::Instant::now();
    let server = state.server(&op.server_id)?;
    let approval = async {
        let settings = validate_operation(&state, &server, &op, source)?;
        let needed = crate::security::needs_approval(&settings, &op, source);
        if needed {
            state
                .approvals
                .wait_with_timeout(&server, &op, source, settings.approval_timeout_secs)
                .await
                .map_err(|e| crate::operation_error::with_stage(e, "approval"))?;
        }
        Ok::<bool, anyhow::Error>(needed)
    }
    .await;
    tokio::task::spawn_blocking(move || {
        let result = approval.and_then(|approved| {
            // Serialize directory moves and child writes as well as edits, across MCP and desktop.
            // Approval waits happen before acquiring the lock. A blocking SFTP call retains it
            // until settled even if its async caller disconnects.
            let file_lock = if crate::file_tools::mutates(&op.kind) {
                Some(
                    state
                        .file_mutation_locks
                        .lock()
                        .unwrap()
                        .entry(op.server_id.clone())
                        .or_default()
                        .clone(),
                )
            } else {
                None
            };
            let _guard = file_lock.as_ref().map(|lock| lock.lock().unwrap());
            let live = state.server(&op.server_id)?;
            if approved && serde_json::to_value(&live)? != serde_json::to_value(&server)? {
                bail!("Server configuration changed; approval is invalid")
            }
            let settings = validate_operation(&state, &live, &op, source)?;
            if !approved && crate::security::needs_approval(&settings, &op, source) {
                bail!("Approval policy changed; submit the operation again")
            }
            if op.kind.starts_with("port_forward_") {
                crate::tunnels::operate(&state, &live, &op, source)
            } else if op.kind == "exec" {
                crate::jobs::execute(&state, &live, &op, &settings, source, &progress)
            } else if op.kind == "job" {
                state.jobs.operate(&op, &progress)
            } else if op.kind.starts_with("terminal_") {
                crate::terminal::operate(&state, &live, &op)
            } else {
                let mut attempts = 0;
                loop {
                    let result = crate::ssh::operate_with_settings(&live, &op, &settings);
                    let safe = matches!(op.kind.as_str(), "list_files" | "stat_file" | "read_file");
                    if attempts >= 2
                        || !safe
                        || !result
                            .as_ref()
                            .err()
                            .is_some_and(crate::operation_error::retryable)
                    {
                        break result;
                    }
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(250 * (1 << attempts)));
                    let current = state.server(&op.server_id)?;
                    anyhow::ensure!(
                        serde_json::to_value(&current)? == serde_json::to_value(&live)?,
                        "Server configuration changed during retry"
                    );
                    validate_operation(&state, &current, &op, source)?;
                }
            }
        });
        if (source == "mcp"
            || (op.kind != "terminal_read"
                && op.kind != "terminal_resize"
                && !(op.kind == "job" && op.job_action != "close" && result.is_ok())))
            && !result
                .as_ref()
                .ok()
                .is_some_and(|v| v.get("job_id").is_some() && op.kind == "exec")
        {
            state
                .log(
                    &server,
                    source,
                    &op,
                    &result,
                    start.elapsed().as_millis() as u64,
                )
                .context("Operation processed, but audit write failed; check the data directory")?;
        }
        result
    })
    .await?
}

fn validate_operation(
    state: &AppState,
    server: &Server,
    op: &Operation,
    source: &str,
) -> Result<RuntimeSettings> {
    if !server.enabled
        && !(source == "desktop"
            && matches!(
                op.kind.as_str(),
                "terminal_close" | "port_forward_close" | "port_forward_read" | "port_forward_list"
            ))
    {
        return Err(crate::operation_error::error(
            "SERVER_DISABLED",
            "Server is disabled",
        ));
    }
    if source == "mcp" {
        let mut required = vec![op.kind.as_str()];
        if op.kind == "edit_file" {
            required.push("read_file");
        }
        crate::file_tools::require_mcp_permissions(server, &required)
            .map_err(|e| crate::operation_error::with_stage(e, "authorization"))?;
    }
    if op.kind.starts_with("port_forward_") {
        crate::tunnels::validate(op)?;
    }
    if source == "mcp" {
        anyhow::ensure!(
            !op.server_id.is_empty() && op.server_id.len() <= 128,
            "INVALID_ARGUMENT: server_id is required"
        );
        if matches!(
            op.kind.as_str(),
            "read_file"
                | "write_file"
                | "edit_file"
                | "list_files"
                | "stat_file"
                | "download"
                | "upload"
                | "remove"
                | "remove_path"
                | "rename"
                | "mkdir"
        ) {
            anyhow::ensure!(!op.path.is_empty(), "INVALID_ARGUMENT: path is required");
        }
        if op.kind == "rename" {
            anyhow::ensure!(
                !op.target.is_empty(),
                "INVALID_ARGUMENT: target is required"
            );
        }
        if op.kind == "exec" {
            anyhow::ensure!(
                !op.command.trim().is_empty() && op.timeout_secs <= 300,
                "INVALID_ARGUMENT: command is required; timeout_secs must be 0-300"
            );
        }
        if matches!(op.kind.as_str(), "terminal_open" | "terminal_resize") {
            anyhow::ensure!(
                (20..=500).contains(&op.cols) && (5..=200).contains(&op.rows),
                "INVALID_ARGUMENT: cols must be 20-500; rows must be 5-200"
            );
        }
        if op.kind.starts_with("terminal_") && op.kind != "terminal_open" {
            anyhow::ensure!(
                !op.session_id.is_empty(),
                "INVALID_ARGUMENT: session_id is required"
            );
        }
    }
    anyhow::ensure!(
        op.path.len() <= 8192
            && op.target.len() <= 8192
            && op.command.len() <= 16384
            && op.session_id.len() <= 128,
        "Operation arguments exceed size limits"
    );
    if op.kind == "job" {
        anyhow::ensure!(
            matches!(op.job_action.as_str(), "list" | "read" | "close") && op.wait_secs <= 30,
            "INVALID_ARGUMENT: job action must be list/read/close; wait_secs must be 0-30"
        );
        anyhow::ensure!(
            op.job_action == "list" || (!op.job_id.is_empty() && op.job_id.len() <= 128),
            "INVALID_ARGUMENT: job_id is required (max 128 bytes)"
        );
    }
    let settings = state.settings.lock().unwrap().clone();
    crate::ssh::validate_payload(op, &settings)?;
    crate::security::check(&settings, op, source)
        .map_err(|e| crate::operation_error::with_stage(e, "policy"))?;
    Ok(settings)
}

fn clip_text(text: &str, max_bytes: usize, truncated: &mut bool) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < text.len() {
        *truncated = true;
    }
    text[..end].to_string()
}

/// Return bounded previews without persisting uploaded data, file bodies or terminal keystrokes.
fn audit_response(op: &Operation, result: &Result<Value>) -> AuditResponse {
    let mut truncated = false;
    let data = match result {
        Err(e) if matches!(op.kind.as_str(), "upload_file" | "download_file") => {
            let spec = op.transfer_request.as_ref();
            json!({"transfer_id":op.transfer_id,"server_id":op.server_id,"direction":if op.kind == "upload_file" {"upload"} else {"download"},"local_path":spec.map(|s| &s.local_path),"remote_path":spec.map(|s| &s.remote_path),"overwrite":spec.map(|s| s.overwrite),"status":"failed","transferred_bytes":0,"error":crate::operation_error::error_response(e, op)})
        }
        Ok(value) if matches!(op.kind.as_str(), "upload_file" | "download_file") => {
            json!({"transfer_id":value["transfer_id"],"direction":value["direction"],"server_id":op.server_id,"local_path":value["local_path"],"remote_path":value["remote_path"],"overwrite":value["overwrite"],"status":value["status"],"total_bytes":value["total_bytes"],"transferred_bytes":value["transferred_bytes"],"created_at":value["created_at"],"finished_at":value["finished_at"],"elapsed_ms":value["elapsed_ms"],"retry_count":value["retry_count"],"resumed_bytes":value["resumed_bytes"],"sha256":value["sha256"],"group_id":value["group_id"],"phase":value["phase"],"error":value["error"]})
        }
        Err(error) => {
            let mut response = crate::operation_error::error_response(error, op);
            response["message"] = json!(clip_text(
                response["message"].as_str().unwrap_or(""),
                16384,
                &mut truncated
            ));
            response
        }
        Ok(value) if op.kind == "terminal_read" => {
            let stdout = clip_text(
                value["stdout"].as_str().unwrap_or(""),
                16384,
                &mut truncated,
            );
            truncated |= value["truncated"].as_bool().unwrap_or(false);
            json!({"stdout":stdout,"cursor":value["cursor"],"alive":value["alive"],"truncated":value["truncated"],"has_more":value["has_more"]})
        }
        Ok(value) if op.kind == "exec" || (op.kind == "job" && op.job_action != "list") => {
            let stdout = clip_text(
                value["stdout"].as_str().unwrap_or(""),
                16384,
                &mut truncated,
            );
            let stderr = clip_text(
                value["stderr"].as_str().unwrap_or(""),
                16384,
                &mut truncated,
            );
            truncated |= value["truncated"].as_bool().unwrap_or(false);
            truncated |= value["stdout_has_more"].as_bool().unwrap_or(false)
                || value["stderr_has_more"].as_bool().unwrap_or(false);
            json!({"stdout": stdout, "stderr": stderr, "exit_code":value["exit_code"],"status":value["status"],"job_id":value["job_id"],"action":op.job_action,"error":value["error"],"next_stdout_cursor":value["next_stdout_cursor"],"next_stderr_cursor":value["next_stderr_cursor"],"stdout_truncated":value["stdout_truncated"],"stderr_truncated":value["stderr_truncated"]})
        }
        Ok(value) if op.kind == "read_file" => {
            json!({"bytes": value["content"].as_str().unwrap_or("").len(), "message":"读取完成，文件正文不写入操作记录"})
        }
        Ok(value) if op.kind == "edit_file" => {
            json!({"path":op.path,"applied_edits":value["applied_edits"],"written_bytes":value["written_bytes"],"diff_truncated":value["diff_truncated"],"message":"编辑完成，正文及 diff 不写入操作记录"})
        }
        Ok(value) if op.kind == "download" => {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(value["base64"].as_str().unwrap_or(""))
                .map(|b| b.len())
                .unwrap_or(0);
            json!({"bytes":bytes,"message":"下载完成，文件正文不写入操作记录"})
        }
        Ok(value) => value.clone(),
    };
    let encoded = serde_json::to_string(&data).unwrap_or_default();
    let data = if encoded.len() > 65536 {
        json!({"preview": clip_text(&encoded, 65536, &mut truncated)})
    } else {
        data
    };
    AuditResponse {
        data,
        truncated,
        input: Some(crate::audit_input::capture(op)),
    }
}
