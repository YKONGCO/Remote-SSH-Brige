use crate::mcp_args::*;
use crate::transfer_planner::{Direction, StartOptions, TransferRequest};
use crate::{model::*, state::AppState};
use anyhow::Result;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    Router,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::{Peer, RequestContext},
    tool, tool_handler, tool_router,
    transport::{
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
        StreamableHttpServerConfig,
    },
    RoleServer, ServerHandler,
};
use std::sync::{Arc, Mutex};
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct McpSessionRegistry {
    peers: Mutex<Vec<Peer<RoleServer>>>,
}
impl McpSessionRegistry {
    fn register(&self, peer: Peer<RoleServer>) {
        let mut peers = self.peers.lock().unwrap();
        if peers.len() >= 256 {
            peers.remove(0);
        }
        peers.push(peer);
    }
    pub fn notify_tools_changed(&self) {
        let peers = self.peers.lock().unwrap().clone();
        let notify = async move {
            for peer in peers {
                let _ = peer.notify_tool_list_changed().await;
            }
        };
        #[cfg(feature = "desktop")]
        {
            tauri::async_runtime::spawn(notify);
        }
        #[cfg(not(feature = "desktop"))]
        {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(notify);
            }
        }
    }
}

#[derive(Clone)]
pub struct RemoteMcp {
    state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
}
#[tool_router]
impl RemoteMcp {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
    async fn call(&self, kind: &str, args: impl Into<Operation>) -> CallToolResult {
        self.call_with_progress(kind, args, None).await
    }
    async fn call_with_progress(
        &self,
        kind: &str,
        args: impl Into<Operation>,
        ctx: Option<RequestContext<RoleServer>>,
    ) -> CallToolResult {
        let mut op = args.into();
        op.kind = kind.into();
        self.call_operation(op, ctx).await
    }
    async fn call_operation(
        &self,
        op: Operation,
        ctx: Option<RequestContext<RoleServer>>,
    ) -> CallToolResult {
        let token = ctx
            .as_ref()
            .and_then(|ctx| ctx.meta.get("progressToken"))
            .and_then(|v| serde_json::from_value::<NumberOrString>(v.clone()).ok());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let execution = crate::state::execute_with_progress(
            self.state.clone(),
            op.clone(),
            "mcp",
            token.as_ref().map(|_| tx),
        );
        tokio::pin!(execution);
        let mut count = 0;
        let outcome = loop {
            tokio::select! {
                biased;
                result = &mut execution => break result,
                Some(chunk) = rx.recv(), if token.is_some() => {
                    count += 1;
                    if let (Some(ctx), Some(token)) = (&ctx, &token) {

                        let notification = ProgressNotificationParam::new(ProgressToken(token.clone()), count as f64).with_message(chunk.to_string());
                        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), ctx.peer.notify_progress(notification)).await;
                    }
                }
            }
        };
        crate::mcp_response::from_outcome(&op, outcome)
    }
    #[tool(description = "List available servers: id, name, host and file root.")]
    async fn list_servers(&self, Parameters(_a): Parameters<EmptyArgs>) -> CallToolResult {
        let servers: Vec<_> = self
            .state
            .config
            .lock()
            .unwrap()
            .servers
            .iter()
            .filter(|s| s.enabled && s.mcp_enabled)
            .map(|s| serde_json::json!({"id":s.id,"name":s.name,"host":s.host,"root":s.root}))
            .collect();
        self.global_result("list_servers", serde_json::json!(servers))
    }
    #[tool(
        description = "Inspect a server's permissions, transfer limits or command availability on demand."
    )]
    async fn server_info(&self, Parameters(a): Parameters<ServerInfoArgs>) -> CallToolResult {
        let result = (|| -> anyhow::Result<serde_json::Value> {
            let server = self.state.server(&a.server_id)?;
            anyhow::ensure!(
                server.enabled && server.mcp_enabled,
                "Server is unavailable to MCP"
            );
            let policy = self.state.settings.lock().unwrap().clone();
            let command_policy = policy.effective_command_policy();
            let exec_available = server.permits_mcp("exec")
                && !command_policy.exec_disabled
                && !policy.remote_read_only;
            let approval_available = exec_available && policy.approval_mode;
            let capabilities: Vec<_> = server
                .effective_mcp_permissions()
                .into_iter()
                .filter(|kind| *kind != "remove_path" || !policy.deletion_protection)
                .filter(|kind| {
                    let kind = *kind;
                    let terminal = kind.starts_with("terminal_") || kind == "list_terminals";
                    let port_forward = kind.starts_with("port_forward_");
                    (!terminal && !port_forward)
                        || policy.advanced_mcp_tools.enabled(if terminal {
                            "terminal"
                        } else {
                            "port_forward"
                        })
                })
                .filter(|kind| {
                    if *kind == "exec" && !exec_available {
                        return false;
                    }
                    !policy.remote_read_only
                        || !matches!(
                            *kind,
                            "write_file"
                                | "edit_file"
                                | "remove_path"
                                | "upload_file"
                                | "exec"
                                | "terminal_open"
                                | "terminal_write"
                                | "port_forward_open"
                        )
                })
                .collect();
            Ok(match a.topic {
                ServerInfoTopic::Permissions => {
                    serde_json::json!({
                        "server_id":server.id,
                        "server_name":server.name,
                        "read_only":policy.remote_read_only,
                        "deletion_protected":policy.deletion_protection,
                        "capabilities":capabilities,
                        "approval":{
                            "tool":"exec_approved",
                            "available":approval_available,
                            "timeout_secs":policy.approval_timeout_secs,
                        },
                        "advanced_tools":{
                            "terminal":policy.advanced_mcp_tools.terminal,
                            "port_forward":policy.advanced_mcp_tools.port_forward,
                        },
                    })
                }
                ServerInfoTopic::Transfers => {
                    let upload =
                        server.permits_mcp("upload_file") && server.permits_mcp("transfer");
                    let download =
                        server.permits_mcp("download_file") && server.permits_mcp("transfer");
                    serde_json::json!({
                        "upload":{
                            "allowed":upload,
                            "limit_mib":policy.upload_limit_mib,
                            "local_roots":if upload {policy.local_upload_roots} else {vec![]},
                        },
                        "download":{
                            "allowed":download,
                            "limit_mib":policy.download_limit_mib,
                            "local_roots":if download {policy.local_download_roots} else {vec![]},
                        },
                        "limits":{
                            "max_items":100,
                            "max_entries":1000,
                            "max_depth":64,
                        },
                    })
                }
                ServerInfoTopic::Commands => {
                    serde_json::json!({
                        "exec":{"available":exec_available},
                        "exec_approved":{
                            "available":approval_available,
                            "timeout_secs":policy.approval_timeout_secs,
                        },
                        "approval_required":policy.approval_mode,
                        "timeout_to_job":policy.exec_timeout_to_job,
                    })
                }
            })
        })();
        crate::mcp_response::from_outcome(
            &Operation {
                kind: "server_info".into(),
                server_id: a.server_id,
                ..Default::default()
            },
            result,
        )
    }
    #[tool(
        description = "Execute a normal command. Protected commands return APPROVAL_REQUIRED with the matching rules when approval mode is enabled; retry them with exec_approved using the same arguments. Check exit_code; on timeout a retained session returns job_id for job.read."
    )]
    async fn exec(
        &self,
        Parameters(a): Parameters<ExecArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.call_with_progress("exec", a, Some(ctx)).await
    }
    #[tool(
        description = "Execute a protected command after the desktop user approves this exact request once. Same arguments as exec; approval is bound to the server, command and timeout and is never reusable."
    )]
    async fn exec_approved(
        &self,
        Parameters(a): Parameters<ExecArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let mut op: Operation = a.into();
        op.kind = "exec".into();
        op.approval_request = true;
        self.call_operation(op, Some(ctx)).await
    }
    #[tool(
        description = "Open, list, read or close SSH TCP forwarding from app-host loopback to server loopback."
    )]
    async fn port_forward(&self, Parameters(a): Parameters<PortForwardArgs>) -> CallToolResult {
        let op: Operation = a.into();
        let kind = op.kind.clone();
        self.call(&kind, op).await
    }
    #[tool(
        description = "List, read or close retained commands. Continue reads using returned byte cursors; check status and exit_code."
    )]
    async fn job(
        &self,
        Parameters(a): Parameters<JobArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.call_with_progress("job", a, Some(ctx)).await
    }
    #[tool(
        description = "Upload one or more files or recursive directories from the app host. Use items for every request; a single file returns transfer_id, a batch or directory returns group_id."
    )]
    async fn upload_file(&self, Parameters(a): Parameters<TransferRequest>) -> CallToolResult {
        let op = Operation {
            kind: "upload_file".into(),
            server_id: a.server_id.clone(),
            ..Default::default()
        };
        crate::mcp_response::from_outcome(
            &op,
            crate::transfer_planner::start(
                self.state.clone(),
                Direction::Upload,
                a,
                StartOptions::default(),
                "mcp",
            )
            .await,
        )
    }
    #[tool(
        description = "Download one or more files or recursive directories to the app host. Use items for every request; a single file returns transfer_id, a batch or directory returns group_id."
    )]
    async fn download_file(&self, Parameters(a): Parameters<TransferRequest>) -> CallToolResult {
        let op = Operation {
            kind: "download_file".into(),
            server_id: a.server_id.clone(),
            ..Default::default()
        };
        crate::mcp_response::from_outcome(
            &op,
            crate::transfer_planner::start(
                self.state.clone(),
                Direction::Download,
                a,
                StartOptions::default(),
                "mcp",
            )
            .await,
        )
    }
    #[tool(
        description = "Manage transfers by transfer_id or group_id: list/read/pause/resume/cancel. Read supports version and wait_secs; resume also retries failed tasks."
    )]
    async fn transfer(
        &self,
        Parameters(a): Parameters<TransferArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.call_with_progress("transfer", a, Some(ctx)).await
    }
    #[tool(
        description = "Read UTF-8 text, at most 2000 lines/50 KiB. Continue at next_offset; metadata is in structuredContent."
    )]
    async fn read_file(&self, Parameters(a): Parameters<ReadArgs>) -> CallToolResult {
        self.call("read_file", a).await
    }
    #[tool(
        description = "List directory metadata with pagination. Continue at next_offset when has_more."
    )]
    async fn list_files(&self, Parameters(a): Parameters<ListArgs>) -> CallToolResult {
        self.call("list_files", a).await
    }
    #[tool(description = "Read resolved path metadata via SFTP within the server file root.")]
    async fn stat_file(&self, Parameters(a): Parameters<PathArgs>) -> CallToolResult {
        self.call("stat_file", a).await
    }
    #[tool(
        description = "Write UTF-8 text. create refuses existing targets; create_parents defaults false."
    )]
    async fn write_file(&self, Parameters(a): Parameters<WriteArgs>) -> CallToolResult {
        self.call("write_file", a).await
    }
    #[tool(
        description = "Apply non-overlapping replacements atomically. Each old_text must match uniquely. Returns a bounded diff."
    )]
    async fn edit_file(&self, Parameters(a): Parameters<EditArgs>) -> CallToolResult {
        self.call("edit_file", a).await
    }
    #[tool(
        description = "Delete an exact path without following symlinks. Inspect partial or unknown outcomes before retrying."
    )]
    async fn remove_path(&self, Parameters(a): Parameters<RemoveArgs>) -> CallToolResult {
        self.call("remove_path", a).await
    }
    #[tool(
        description = "Manage SSH PTYs: list/open/read/write/close. Read waits for output when wait_secs is set and returns text with a byte cursor."
    )]
    async fn terminal(&self, Parameters(a): Parameters<TerminalArgs>) -> CallToolResult {
        match a.operation() {
            Some(op) => {
                let kind = op.kind.clone();
                self.call(&kind, op).await
            }
            None => self.list_terminal_sessions(),
        }
    }
    fn tool_registered(&self, name: &str) -> bool {
        let settings = self.state.settings.lock().unwrap();
        match name {
            "terminal" | "port_forward" => settings.advanced_mcp_tools.enabled(name),
            "exec_approved" => settings.approval_mode,
            _ => true,
        }
    }
    fn list_terminal_sessions(&self) -> CallToolResult {
        let allowed: Vec<_> = self
            .state
            .config
            .lock()
            .unwrap()
            .servers
            .iter()
            .filter(|s| s.permits_mcp("list_terminals"))
            .map(|s| s.id.clone())
            .collect();
        let items: Vec<_> = self
            .state
            .terminals
            .lock()
            .unwrap()
            .values()
            .map(|t| t.info())
            .filter(|t| allowed.contains(&t.server_id))
            .collect();
        self.global_result("list_terminals", serde_json::json!(items))
    }
    fn global_result(&self, kind: &str, data: serde_json::Value) -> CallToolResult {
        let server = Server {
            name: "MCP 服务".into(),
            ..Default::default()
        };
        let op = Operation {
            kind: kind.into(),
            ..Default::default()
        };
        match self.state.log(&server, "mcp", &op, &Ok(data.clone()), 0) {
            Ok(()) => crate::mcp_response::success(kind, data),
            Err(error) => crate::mcp_response::failure(
                &op,
                &crate::operation_error::with_stage(
                    error.context("Audit log write failed"),
                    "audit",
                ),
            ),
        }
    }
}
#[tool_handler(router = self.tool_router)]
impl ServerHandler for RemoteMcp {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, rmcp::ErrorData> {
        self.state.mcp_sessions.register(context.peer.clone());
        let tools = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|tool| self.tool_registered(&tool.name))
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_registered(name)
            .then(|| self.tool_router.get(name).cloned())
            .flatten()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        self.state.mcp_sessions.register(context.peer.clone());
        let tool_name = request.name.clone();
        if !self.tool_registered(&tool_name) {
            return Err(rmcp::ErrorData::invalid_params("tool not found", None));
        }
        let call = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router
            .call(call)
            .await
            .map(|response| match response {
                CallToolResponse::Complete(result) => {
                    crate::mcp_response::normalize_tool_result(result).into()
                }
                response => response,
            })
            .map_err(|mut error| {
                let message = if error.message == "tool not found" {
                    format!("Unknown tool: {tool_name}")
                } else {
                    error.message.to_string()
                };
                error.message = crate::mcp_response::short_line(&message).into();
                error.data = None;
                error
            })
    }

    fn get_info(&self) -> ServerConfig {
        let mut info = Implementation::from_build_env();
        info.name = MCP_SERVER_NAME.into();
        info.title = Some(MCP_SERVER_TITLE.into());
        info.description = Some(
            "Remote SSH commands, protected-command approval, file editing and persistent terminals over MCP."
                .into(),
        );
        info.version = env!("CARGO_PKG_VERSION").into();
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(info)
        .with_instructions("Use list_servers for IDs. Local paths belong to the app host. Ordinary commands run directly; a protected or statically unresolved command returns APPROVAL_REQUIRED and must be retried with exec_approved using identical arguments when approval mode is enabled, then approved by the desktop user once. Permissions, read-only mode and policy denials cannot be bypassed by approval. Upload and download accept an items array for single files, batches and recursive directories. Check exit_code/status, and inspect unknown outcomes before retrying.")
    }
}
async fn authorize(
    State((token, state, address)): State<(String, Arc<AppState>, std::net::IpAddr)>,
    request: Request,
    next: Next,
) -> std::result::Result<Response, (StatusCode, &'static str)> {
    if request.headers().contains_key("origin") {
        return Err((
            StatusCode::FORBIDDEN,
            "Browser-origin MCP requests are not allowed",
        ));
    }

    if address.is_unspecified() {
        let authority = request
            .headers()
            .get("host")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
            .ok_or((StatusCode::BAD_REQUEST, "Missing or invalid Host header"))?;
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']');
        if host != "localhost" && host.parse::<std::net::IpAddr>().is_err() {
            return Err((
                StatusCode::FORBIDDEN,
                "Host must be localhost or a literal IP address",
            ));
        }
    }
    let auth = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if !bool::from(auth.as_bytes().ct_eq(token.as_bytes())) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Missing or invalid MCP bearer token",
        ));
    }

    let limit = state.settings.lock().unwrap().transfer_limit("write_file") as usize * 6 + 65536;
    if let Some(length) = request.headers().get("content-length") {
        let length = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or((StatusCode::BAD_REQUEST, "Invalid Content-Length header"))?;
        if length > limit as u64 {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body exceeds the configured size limit",
            ));
        }
    }
    let (parts, body) = request.into_parts();
    let request = Request::from_parts(
        parts,
        axum::body::Body::new(http_body_util::Limited::new(body, limit)),
    );
    Ok(next.run(request).await)
}
pub async fn start(state: Arc<AppState>, port: u16) -> Result<McpStatus> {
    start_on(state, port, "127.0.0.1").await
}

pub async fn start_on(state: Arc<AppState>, port: u16, host: &str) -> Result<McpStatus> {
    let _guard = state.service_lock.lock().await;
    if state.mcp.lock().unwrap().running {
        anyhow::bail!("MCP server is already running")
    }
    anyhow::ensure!(port > 0, "Port must be 1-65535");
    let address: std::net::IpAddr = host
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("Listen address must be a valid IPv4 or IPv6 address"))?;
    let listener = tokio::net::TcpListener::bind((address, port)).await?;

    let mut settings = state.settings.lock().unwrap().clone();
    settings.mcp_host = address.to_string();
    settings.mcp_port = port;
    state.save_settings(settings)?;
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let cancel = CancellationToken::new();
    let shared = state.clone();
    let config = StreamableHttpServerConfig::default();
    let config = if address.is_unspecified() {
        config.disable_allowed_hosts()
    } else {
        config.with_allowed_hosts([
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
            address.to_string(),
        ])
    };
    let sessions = Arc::new(LocalSessionManager::default());
    let service = StreamableHttpService::new(
        move || Ok(RemoteMcp::new(shared.clone())),
        sessions.clone(),
        config
            .with_max_request_body_bytes(MAX_INLINE_TRANSFER_MIB as usize * 1024 * 1024 * 6 + 65536)
            .with_cancellation_token(cancel.child_token()),
    );
    let app = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            (token.clone(), state.clone(), address),
            authorize,
        ));
    let status = McpStatus {
        running: true,
        host: address.to_string(),
        port,
        token,
        error: None,
    };
    *state.mcp.lock().unwrap() = status.clone();
    *state.cancel.lock().unwrap() = Some(cancel.clone());
    let shared = state.clone();
    tokio::spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await;
        let handles: Vec<_> = sessions
            .sessions
            .write()
            .await
            .drain()
            .map(|(_, handle)| handle)
            .collect();
        for handle in handles {
            let _ = handle.close().await;
        }
        let mut status = shared.mcp.lock().unwrap();
        status.running = false;
        status.token.clear();
        if let Err(e) = result {
            status.error = Some(e.to_string());
        }
    });
    Ok(status)
}
pub async fn stop(state: Arc<AppState>) -> Result<()> {
    let _guard = state.service_lock.lock().await;
    if let Some(cancel) = state.cancel.lock().unwrap().take() {
        cancel.cancel();
    }
    state.tunnels.stop_mcp();
    crate::terminal::stop_mcp(&state.terminals);

    for _ in 0..100 {
        if !state.mcp.lock().unwrap().running {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    anyhow::bail!("MCP server is waiting for connections to close; retry shortly")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_and_advanced_tools_follow_live_settings() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().to_path_buf()).unwrap();
        let mcp = RemoteMcp::new(state.clone());
        assert!(mcp.tool_registered("exec_approved"));
        assert!(!mcp.tool_registered("terminal"));
        assert!(mcp.tool_registered("port_forward"));
        state
            .save_settings(RuntimeSettings {
                approval_mode: false,
                advanced_mcp_tools: AdvancedMcpTools {
                    terminal: true,
                    port_forward: false,
                },
                ..Default::default()
            })
            .unwrap();
        assert!(!mcp.tool_registered("exec_approved"));
        assert!(mcp.tool_registered("terminal"));
        assert!(!mcp.tool_registered("port_forward"));
        assert!(mcp.tool_registered("exec"));
    }
}
