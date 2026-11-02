use crate::mcp_args::*;
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
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::{
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
        StreamableHttpServerConfig,
    },
    RoleServer, ServerHandler,
};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;

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
                        // Notifications are best effort; a slow client must never stall SSH reads.
                        let notification = ProgressNotificationParam::new(ProgressToken(token.clone()), count as f64).with_message(chunk.to_string());
                        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), ctx.peer.notify_progress(notification)).await;
                    }
                }
            }
        };
        crate::mcp_response::from_outcome(&op, outcome)
    }
    #[tool(
        description = "List enabled MCP servers, IDs, file roots, permissions and security settings. No credentials are returned."
    )]
    async fn list_servers(&self, Parameters(_a): Parameters<EmptyArgs>) -> CallToolResult {
        let policy = self.state.settings.lock().unwrap().clone();
        let servers: Vec<_> = self.state.config.lock().unwrap().servers.iter().filter(|s| s.enabled && s.mcp_enabled).map(|s| serde_json::json!({"id":s.id,"name":s.name,"host":s.host,"root":s.root,"allow_exec":s.permits_mcp("exec"),"allow_write":s.permits_mcp("write_file"),"allow_terminal":s.permits_mcp("terminal_open"),"mcp_permissions":s.effective_mcp_permissions().into_iter().filter(|kind| *kind != "remove_path" || !policy.deletion_protection).collect::<Vec<_>>(),"terminal_actions":s.terminal_actions(),
            "file_transfer":{"upload_limit_mib":policy.upload_limit_mib,"download_limit_mib":policy.download_limit_mib,"local_upload_roots":if s.permits_mcp("upload_file") {policy.local_upload_roots.clone()} else {vec![]},"local_download_roots":if s.permits_mcp("download_file") {policy.local_download_roots.clone()} else {vec![]}},"exec_timeout_to_job":policy.exec_timeout_to_job,"security":{"deletion_protection":policy.deletion_protection,"deletion_protection_scope":"sftp_remove","command_protection":policy.block_dangerous_commands,"command_policy":policy.effective_command_policy(),"read_only":policy.remote_read_only,"approval_mode":policy.approval_mode,"approval_timeout_secs":policy.approval_timeout_secs}})).collect();
        let servers: Vec<_> = servers.into_iter().map(|mut server| {
            server["port_forwarding"] = serde_json::json!({"local_host":"127.0.0.1","remote_hosts":["127.0.0.1","::1"],"protocol":"tcp","max_tunnels":32,"max_connections_per_tunnel":16,"local_address_scope":"app_host"});
            server
        }).collect();
        self.global_result("list_servers", serde_json::json!(servers))
    }
    #[tool(
        description = "Execute a command under security.command_policy. Returns stdout, stderr and exit_code; a nonzero exit code means command failure. On timeout, exec_timeout_to_job plus job permission retains the session and returns job_id; continue with job.read and its cursors. Otherwise the channel closes, but remote descendants may still run."
    )]
    async fn exec(
        &self,
        Parameters(a): Parameters<ExecArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.call_with_progress("exec", a, Some(ctx)).await
    }
    #[tool(
        description = "Manage SSH TCP forwarding: open, list, read or close. The local listener belongs to the app host and binds to 127.0.0.1; the remote target is server loopback. Each action requires port_forward_<action>. list shows running/closing tunnels; read also finds retained closed records. closing is transitional. Configuration changes, permission revocation and shutdown close affected tunnels. No reverse forwarding or SOCKS."
    )]
    async fn port_forward(&self, Parameters(a): Parameters<PortForwardArgs>) -> CallToolResult {
        let op: Operation = a.into();
        let kind = op.kind.clone();
        self.call(&kind, op).await
    }
    #[tool(
        description = "List, read or close retained command sessions; requires job permission. read returns incremental stdout/stderr with absolute byte cursors and truncation flags. Continue with returned cursors. States: running/exited/closed/disconnected; only exited has a known exit_code. close does not guarantee remote descendants stop. Finished sessions expire after 30 minutes; restart clears sessions."
    )]
    async fn job(
        &self,
        Parameters(a): Parameters<JobArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.call_with_progress("job", a, Some(ctx)).await
    }
    #[tool(
        description = "Queue an upload from the app host to the server file root. Requires upload_file and transfer permissions plus local_upload_roots. Returns transfer_id; use transfer.read until completed, failed or cancelled. Only completed confirms destination commit. No file bytes are returned."
    )]
    async fn upload_file(&self, Parameters(a): Parameters<TransferStartArgs>) -> CallToolResult {
        self.call("upload_file", a).await
    }
    #[tool(
        description = "Queue a download from the server file root to the app host. Requires download_file and transfer permissions plus local_download_roots. Returns transfer_id; use transfer.read until completed, failed or cancelled. Only completed confirms destination commit. No file bytes are returned."
    )]
    async fn download_file(&self, Parameters(a): Parameters<TransferStartArgs>) -> CallToolResult {
        self.call("download_file", a).await
    }
    #[tool(
        description = "List, read or cancel file transfers; requires transfer permission. read without version waits for terminal status; current version waits for a change, older versions return immediately, bounded by wait_secs. Returns status, phase, byte counts, speed, version and errors. cancel_requested acknowledges a request; poll for final status. Commit cannot be cancelled. Finished records expire after 30 minutes; restart clears records."
    )]
    async fn transfer(
        &self,
        Parameters(a): Parameters<TransferArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.call_with_progress("transfer", a, Some(ctx)).await
    }
    #[tool(
        description = "Read UTF-8 text, bounded to 2000 lines or 50 KiB. content contains only the original text; structuredContent contains pagination metadata. When has_more, continue at next_offset. If first_line_exceeds_limit or truncated_by=scan_bytes, use a bounded exec command if authorized."
    )]
    async fn read_file(&self, Parameters(a): Parameters<ReadArgs>) -> CallToolResult {
        self.call("read_file", a).await
    }
    #[tool(
        description = "List directory metadata via SFTP. Returns entries, has_more and next_offset. Directories sort first, then names; scans at most 10000 entries and returns at most 50 KiB. No exec permission is required."
    )]
    async fn list_files(&self, Parameters(a): Parameters<ListArgs>) -> CallToolResult {
        self.call("list_files", a).await
    }
    #[tool(
        description = "Read resolved path metadata via SFTP, including kind, size, times, octal permissions, uid/gid and is_symlink. The resolved target must stay inside the file root. No file contents are read."
    )]
    async fn stat_file(&self, Parameters(a): Parameters<PathArgs>) -> CallToolResult {
        self.call("stat_file", a).await
    }
    #[tool(
        description = "Create or overwrite UTF-8 text. create mode refuses an existing target. create_parents creates missing parent directories inside the root; directories may remain if writing later fails. Use edit_file for targeted changes."
    )]
    async fn write_file(&self, Parameters(a): Parameters<WriteArgs>) -> CallToolResult {
        self.call("write_file", a).await
    }
    #[tool(
        description = "Apply edits atomically against the same original UTF-8 file. Each old_text must match uniquely and edits must not overlap. Exact matching precedes limited whitespace/Unicode normalization. Requires edit_file and read_file permissions. Returns a bounded diff."
    )]
    async fn edit_file(&self, Parameters(a): Parameters<EditArgs>) -> CallToolResult {
        self.call("edit_file", a).await
    }
    #[tool(
        description = "Delete an exact path via SFTP without following symlinks. Requires remove_path permission and is subject to deletion protection, read-only policy and approval. Cannot delete the allowed root or system top-level directories. Recursive preflight is bounded to 10000 entries and depth 64. On partial or unknown outcomes, inspect the target before retrying."
    )]
    async fn remove_path(&self, Parameters(a): Parameters<RemoveArgs>) -> CallToolResult {
        self.call("remove_path", a).await
    }
    #[tool(
        description = "Manage a persistent SSH PTY: list, open, read, write, resize or close. Check terminal_actions for independent grants. read returns original UTF-8 text and structuredContent with absolute byte cursor, alive, truncated and has_more; pages cover at most 64 KiB of raw bytes. Continue with returned cursor until has_more=false, including after alive=false. Sessions share working directory and interactive state with the desktop UI."
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
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        let tool_name = request.name.clone();
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
        info.description =
            Some("Remote SSH commands, file editing and persistent terminals over MCP.".into());
        info.version = env!("CARGO_PKG_VERSION").into();
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(info).with_instructions("Call list_servers first. mcp_permissions and terminal_actions define grants; legacy allow_* fields are summaries. Global security may further restrict operations. Approval waits up to security.approval_timeout_secs. File roots constrain SFTP only, not exec or terminal. Local paths refer to the computer running this app, not the MCP client. Use remove_path for deletion, upload_file/download_file for binary transfer, and exec for search or moves under command policy. Data results are JSON. Exec command-policy denials return JSON with code, details.command and a concise English message; no policy configuration is included. Other tool errors are one short English sentence. read_file and terminal.read return original text in content and metadata in structuredContent. A successful status query does not imply command or transfer success; inspect exit_code and status. Unknown or partial outcomes require inspection before retrying.")
    }
}
async fn authorize(
    State((token, state, address)): State<(String, Arc<AppState>, std::net::IpAddr)>,
    request: Request,
    next: Next,
) -> std::result::Result<Response, (StatusCode, &'static str)> {
    // No browser origins are allowed: clients connect server-to-server using a bearer token.
    if request.headers().contains_key("origin") {
        return Err((
            StatusCode::FORBIDDEN,
            "Browser-origin MCP requests are not allowed",
        ));
    }
    // Wildcard listeners accept literal IP hosts, never arbitrary DNS names.
    // This replaces the SDK's loopback-only host list for remote IP access.
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
    // Bound both Content-Length and chunked bodies using the current settings.
    // JSON escaping can expand UTF-8 text sixfold; base64 needs less space.
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
            // The middleware applies the live limit; this is the supported ceiling.
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
    // Wait for the old listener to stop before permitting a restart.
    for _ in 0..100 {
        if !state.mcp.lock().unwrap().running {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    anyhow::bail!("MCP server is waiting for connections to close; retry shortly")
}
