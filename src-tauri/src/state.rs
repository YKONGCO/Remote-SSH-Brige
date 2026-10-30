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
    database: Mutex<Database>,
    pub config: Mutex<Config>,
    pub settings: Mutex<RuntimeSettings>,
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
        Ok(Arc::new(Self {
            dir,
            database: Mutex::new(database),
            config: Mutex::new(config),
            settings: Mutex::new(settings),
            approvals: crate::security::ApprovalQueue::default(),
            audit: Mutex::new(audit),
            terminals: Mutex::new(HashMap::new()),
            terminal_open_lock: Mutex::new(()),
            file_mutation_locks: Mutex::new(HashMap::new()),
            mcp: Mutex::new(McpStatus::default()),
            cancel: Mutex::new(None),
            service_lock: tokio::sync::Mutex::new(()),
        }))
    }
    pub fn server(&self, id: &str) -> Result<Server> {
        self.config
            .lock()
            .unwrap()
            .servers
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .context("服务器不存在")
    }
    pub fn save_settings(&self, settings: RuntimeSettings) -> Result<RuntimeSettings> {
        settings.validate()?;
        let mut current = self.settings.lock().unwrap();
        self.database.lock().unwrap().save_settings(&settings)?;
        *current = settings.clone();
        Ok(settings)
    }
    pub fn snapshot(&self) -> Value {
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
                "recursive":view.recursive,"missing_ok":view.missing_ok,
                "requested_at":view.created_at,"decided_at":chrono::Utc::now().to_rfc3339(),"expires_at":view.expires_at,
                "content_bytes":view.content_bytes,"content_sha256":view.content_sha256
            }})), 0)
        }).map(|_| ())
    }
    pub fn save_server(&self, mut server: Server, secret: Option<String>) -> Result<Server> {
        if let Some(permissions) = &mut server.mcp_permissions {
            permissions.retain(|kind| !RETIRED_MCP_OPERATIONS.contains(&kind.as_str()));
            anyhow::ensure!(
                permissions
                    .iter()
                    .all(|kind| MCP_OPERATIONS.contains(&kind.as_str())),
                "无效的 MCP 操作权限"
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
            bail!("请填写名称、地址、用户名和有效端口")
        }
        if !["password", "key", "agent"].contains(&server.auth.as_str()) {
            bail!("不支持的认证方式")
        }
        if server.auth == "key" && server.key_path.is_empty() {
            bail!("请填写本地私钥绝对路径")
        }
        if !server.root.starts_with('/') || server.root.split('/').any(|p| p == "..") {
            bail!("文件根目录必须是绝对路径且不含 ..")
        }
        if server.id.is_empty() {
            server.id = uuid::Uuid::new_v4().to_string();
        }
        let mut config = self.config.lock().unwrap();
        if let Some(old) = config.servers.iter().find(|s| s.id == server.id) {
            if old.host != server.host || old.port != server.port {
                server.fingerprint.clear();
            }
        } else if !server.fingerprint.is_empty() {
            bail!("新增服务器请先留空指纹，再核验主机")
        }
        if let Some(secret) = secret.filter(|s| !s.is_empty()) {
            keyring::Entry::new("dev.remotedesk.ssh", &server.id)?
                .set_password(&secret)
                .context("保存系统凭据失败")?;
        }
        let mut next = config.clone();
        if let Some(s) = next.servers.iter_mut().find(|s| s.id == server.id) {
            *s = server.clone();
        } else {
            next.servers.push(server.clone());
        }
        self.database.lock().unwrap().save_server(&server)?;
        *config = next;
        Ok(server)
    }
    pub fn remove_server(&self, id: &str) -> Result<()> {
        if self
            .terminals
            .lock()
            .unwrap()
            .values()
            .any(|t| t.info().server_id == id && t.info().alive)
        {
            bail!("请先关闭此服务器的终端会话")
        }
        let mut config = self.config.lock().unwrap();
        let mut next = config.clone();
        next.servers.retain(|s| s.id != id);
        self.database.lock().unwrap().remove_server(id)?;
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
        let detail = if op.kind.starts_with("terminal") {
            op.session_id.clone()
        } else if op.kind == "exec" || (op.kind.starts_with("security_") && !op.command.is_empty())
        {
            op.command.chars().take(1000).collect()
        } else {
            op.path.clone()
        };
        let summary = match result {
            Ok(_) if op.kind == "security_approve" => "审批已批准".into(),
            Ok(_) if op.kind == "security_reject" => "审批已拒绝，操作未执行".into(),
            Ok(v) if op.kind == "exec" => format!("exit: {}", v["exit_code"]),
            Ok(_) => "完成".into(),
            Err(e) => e.to_string().chars().take(1000).collect(),
        };
        let entry = Audit {
            id: uuid::Uuid::new_v4().to_string(),
            time: chrono::Utc::now().to_rfc3339(),
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            source: source.into(),
            kind: op.kind.clone(),
            detail,
            success: op.kind != "security_reject"
                && result.is_ok()
                && result
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get("exit_code"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    == 0,
            duration_ms: elapsed,
            summary,
            response: Some(audit_response(op, result)),
        };
        let mut audits = self.audit.lock().unwrap();
        self.database
            .lock()
            .unwrap()
            .insert_audit(&entry)
            .context("操作记录写入 SQLite 失败")?;
        audits.push(entry);
        if audits.len() > 500 {
            audits.remove(0);
        }
        Ok(())
    }
}

pub async fn execute(state: Arc<AppState>, op: Operation, source: &'static str) -> Result<Value> {
    let start = std::time::Instant::now();
    let server = state.server(&op.server_id)?;
    let approval = async {
        let settings = validate_operation(&state, &server, &op, source)?;
        let needed = crate::security::needs_approval(&settings, &op, source);
        if needed {
            state.approvals.wait(&server, &op, source).await?;
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
                bail!("服务器配置已变更，审批失效，请重新发起操作")
            }
            let settings = validate_operation(&state, &live, &op, source)?;
            if !approved && crate::security::needs_approval(&settings, &op, source) {
                bail!("审批策略已变更，请重新发起操作")
            }
            if op.kind.starts_with("terminal_") {
                crate::terminal::operate(&state, &live, &op)
            } else {
                crate::ssh::operate_with_settings(&live, &op, &settings)
            }
        });
        if op.kind != "terminal_read" && op.kind != "terminal_resize" {
            state
                .log(
                    &server,
                    source,
                    &op,
                    &result,
                    start.elapsed().as_millis() as u64,
                )
                .context("操作已处理，但审计日志写入失败；请检查数据目录")?;
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
    if !server.enabled && !(source == "desktop" && op.kind == "terminal_close") {
        bail!("服务器已停用")
    }
    if source == "mcp" && !server.permits_mcp(&op.kind) {
        return Err(crate::file_tools::error(
            "PERMISSION_DENIED",
            "此服务器未授权 MCP 操作",
        ));
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
                | "preview_file"
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
        "操作参数过长"
    );
    let settings = state.settings.lock().unwrap().clone();
    crate::ssh::validate_payload(op, &settings)?;
    crate::security::check(&settings, op, source)?;
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
        Err(error) => {
            let details = error
                .downcast_ref::<crate::file_tools::FileError>()
                .and_then(|e| e.details.clone());
            json!({"error": clip_text(&format!("{error:#}"), 16384, &mut truncated),"details":details})
        }
        Ok(value) if op.kind == "exec" => {
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
            json!({"stdout": stdout, "stderr": stderr, "exit_code":value["exit_code"]})
        }
        Ok(value) if op.kind == "preview_file" => {
            json!({"bytes": value["bytes"], "kind": value["kind"], "truncated":value["truncated"], "message":"预览完成，文件正文不写入操作记录"})
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
    AuditResponse { data, truncated }
}
