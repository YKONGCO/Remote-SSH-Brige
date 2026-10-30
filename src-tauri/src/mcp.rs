use crate::mcp_args::*;
use crate::{
    model::*,
    state::{execute, AppState},
};
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
    tool, tool_handler, tool_router,
    transport::{
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
        StreamableHttpServerConfig,
    },
    ServerHandler,
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
        let mut op = args.into();
        op.kind = kind.into();
        match execute(self.state.clone(), op, "mcp").await {
            Ok(mut v) if kind == "read_file" => {
                let mut text = v["content"].as_str().unwrap_or("").to_string();
                if let Some(object) = v.as_object_mut() {
                    object.remove("content");
                }
                if v["first_line_exceeds_limit"] == true {
                    text.push_str(
                        "[Starting line exceeds 50 KiB. Use download to inspect this file.]",
                    );
                } else if v["has_more"] == true {
                    text.push_str(&format!(
                        "\n\n[More content. Use offset={} to continue.{}]",
                        v["next_offset"],
                        if v["truncated_by"] == "scan_bytes" {
                            " Download scan budget reached; further reads may require download."
                        } else {
                            ""
                        }
                    ));
                }
                let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
                result.structured_content = Some(v);
                result
            }
            Ok(v) => CallToolResult::success(vec![ContentBlock::text(v.to_string())]),
            Err(e) => {
                let code = crate::file_tools::error_code(&e);
                CallToolResult::error(vec![ContentBlock::text(
                    serde_json::json!({"code":code,"message":format!("{e:#}")}).to_string(),
                )])
            }
        }
    }
    #[tool(
        description = "List enabled SSH servers with IDs, file root and MCP permissions. Credentials are never exposed."
    )]
    async fn list_servers(&self, Parameters(_a): Parameters<EmptyArgs>) -> CallToolResult {
        let policy = self.state.settings.lock().unwrap().clone();
        let servers: Vec<_> = self.state.config.lock().unwrap().servers.iter().filter(|s| s.enabled && s.mcp_enabled).map(|s| serde_json::json!({"id":s.id,"name":s.name,"host":s.host,"root":s.root,"allow_exec":s.permits_mcp("exec"),"allow_write":s.permits_mcp("write_file"),"allow_terminal":s.permits_mcp("terminal_open"),"mcp_permissions":s.effective_mcp_permissions(),
            "security":{"deletion_protection":policy.deletion_protection,"command_protection":policy.block_dangerous_commands,"command_policy":policy.effective_command_policy(),"read_only":policy.remote_read_only,"approval_mode":policy.approval_mode}})).collect();
        CallToolResult::success(vec![ContentBlock::text(
            serde_json::json!(servers).to_string(),
        )])
    }
    #[tool(
        description = "Execute a command with exec permission. Check list_servers security.command_policy for independent diagnostic-only, pipe, redirection, compound, script and substitution permissions. Known dangerous shell commands are blocked when command_protection is enabled. External scripts and interpreter code cannot be fully inspected. Approval, when enabled, waits up to 120 seconds for the desktop user. File root is not a shell sandbox."
    )]
    async fn exec(&self, Parameters(a): Parameters<ExecArgs>) -> CallToolResult {
        self.call("exec", a).await
    }
    #[tool(
        description = "List files within root. Paginate by 0-indexed entry offset (default 0), limit (default 200, max 1000), and 50 KiB output budget. Includes hidden entries. Requires list_files permission."
    )]
    async fn list_files(&self, Parameters(a): Parameters<ListArgs>) -> CallToolResult {
        self.call("list_files", a).await
    }
    #[tool(
        description = "Read UTF-8 text with 1-indexed offset and optional limit. Returns original text capped at 2000 lines or 50 KiB, with continuation metadata. Continue with next_offset. Requires read_file permission. Use preview_file for images and download for complete bytes."
    )]
    async fn read_file(&self, Parameters(a): Parameters<ReadArgs>) -> CallToolResult {
        self.call("read_file", a).await
    }
    #[tool(
        description = "Preview a remote file within the configured root, up to 2 MiB or the download limit. Requires preview_file permission. Returns UTF-8 text, raster image base64, or unsupported metadata. Large text is truncated with an explicit flag. Does not execute HTML or SVG."
    )]
    async fn preview_file(&self, Parameters(a): Parameters<PathArgs>) -> CallToolResult {
        self.call("preview_file", a).await
    }
    #[tool(
        description = "Create or completely overwrite UTF-8 text. mode defaults to overwrite; create refuses existing targets. Parent must exist. Use edit_file for targeted changes. Requires write_file permission."
    )]
    async fn write_file(&self, Parameters(a): Parameters<WriteArgs>) -> CallToolResult {
        self.call("write_file", a).await
    }
    #[tool(
        description = "Edit UTF-8 text with edits[].old_text/new_text. Each old_text must uniquely match the same original file; edits must not overlap. Exact matching first, then limited whitespace/Unicode normalization. All changes commit together. Requires edit_file and read_file permissions. Returns bounded diff. Never include read continuation notices in old_text."
    )]
    async fn edit_file(&self, Parameters(a): Parameters<EditArgs>) -> CallToolResult {
        self.call("edit_file", a).await
    }
    #[tool(
        description = "Create a directory at path. Requires mkdir permission. Parent must exist."
    )]
    async fn mkdir(&self, Parameters(a): Parameters<PathArgs>) -> CallToolResult {
        self.call("mkdir", a).await
    }
    #[tool(
        description = "Delete a file or EMPTY directory at path. Requires remove permission. Does not recursively delete."
    )]
    async fn remove(&self, Parameters(a): Parameters<PathArgs>) -> CallToolResult {
        self.call("remove", a).await
    }
    #[tool(
        description = "Rename path to target within the allowed root, without overwriting. Requires rename permission."
    )]
    async fn rename(&self, Parameters(a): Parameters<RenameArgs>) -> CallToolResult {
        self.call("rename", a).await
    }
    #[tool(
        description = "Download file at path as base64. Requires download permission. Size is capped by the custom download limit (default 128 MiB, up to 256 MiB)."
    )]
    async fn download(&self, Parameters(a): Parameters<PathArgs>) -> CallToolResult {
        self.call("download", a).await
    }
    #[tool(
        description = "Upload base64-encoded content to path. Size is capped by the custom upload limit (default 128 MiB, up to 256 MiB), requires upload permission."
    )]
    async fn upload(&self, Parameters(a): Parameters<ContentArgs>) -> CallToolResult {
        self.call("upload", a).await
    }
    #[tool(
        description = "Open a persistent SSH PTY shell (cols, rows). Returns session_id. Requires terminal_open permission. Session lasts until closed, disconnected or this app exits."
    )]
    async fn terminal_open(&self, Parameters(a): Parameters<OpenArgs>) -> CallToolResult {
        self.call("terminal_open", a).await
    }
    #[tool(
        description = "Read persistent terminal output by session_id and byte cursor (initially 0). Returns base64 bytes and next cursor. Requires terminal_read permission."
    )]
    async fn terminal_read(&self, Parameters(a): Parameters<TerminalReadArgs>) -> CallToolResult {
        self.call("terminal_read", a).await
    }
    #[tool(
        description = "Write content to persistent terminal session_id. Include newline to execute a command. Requires terminal_write permission."
    )]
    async fn terminal_write(&self, Parameters(a): Parameters<TerminalWriteArgs>) -> CallToolResult {
        self.call("terminal_write", a).await
    }
    #[tool(
        description = "Resize a persistent terminal session_id using cols and rows. Requires terminal_resize permission."
    )]
    async fn terminal_resize(&self, Parameters(a): Parameters<ResizeArgs>) -> CallToolResult {
        self.call("terminal_resize", a).await
    }
    #[tool(
        description = "Close and remove persistent terminal session_id. Requires terminal_close permission."
    )]
    async fn terminal_close(&self, Parameters(a): Parameters<SessionArgs>) -> CallToolResult {
        self.call("terminal_close", a).await
    }
    #[tool(
        description = "List persistent sessions on MCP-enabled servers with list_terminals permission."
    )]
    async fn list_terminals(&self, Parameters(_a): Parameters<EmptyArgs>) -> CallToolResult {
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
        CallToolResult::success(vec![ContentBlock::text(
            serde_json::json!(items).to_string(),
        )])
    }
}
#[tool_handler(router = self.tool_router)]
impl ServerHandler for RemoteMcp {
    fn get_info(&self) -> ServerConfig {
        let mut info = Implementation::from_build_env();
        info.version = env!("CARGO_PKG_VERSION").into();
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(info).with_instructions("SSH MCP Bridge gives AI agents permission-controlled remote server access over MCP and SSH. Call list_servers first. Each server's mcp_permissions lists the allowed tool names; all other operations are denied. Legacy allow_* fields are summaries only. Global security settings may further restrict granted operations. Use read_file for bounded text reads and edit_file for targeted edits. Continue reads using structuredContent.next_offset. File roots apply only to SFTP, not exec or terminal. Persistent sessions share state with the desktop UI.")
    }
}
async fn authorize(
    State((token, state, address)): State<(String, Arc<AppState>, std::net::IpAddr)>,
    request: Request,
    next: Next,
) -> std::result::Result<Response, StatusCode> {
    // No browser origins are allowed: clients connect server-to-server using a bearer token.
    if request.headers().contains_key("origin") {
        return Err(StatusCode::FORBIDDEN);
    }
    // Wildcard listeners accept literal IP hosts, never arbitrary DNS names.
    // This replaces the SDK's loopback-only host list for remote IP access.
    if address.is_unspecified() {
        let authority = request
            .headers()
            .get("host")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']');
        if host != "localhost" && host.parse::<std::net::IpAddr>().is_err() {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    let auth = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if !bool::from(auth.as_bytes().ct_eq(token.as_bytes())) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    // Bound both Content-Length and chunked bodies using the current settings.
    // JSON escaping can expand UTF-8 text sixfold; base64 needs less space.
    let limit = state.settings.lock().unwrap().upload_limit_mib as usize * 1024 * 1024 * 6 + 65536;
    if let Some(length) = request.headers().get("content-length") {
        let length = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;
        if length > limit as u64 {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
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
        anyhow::bail!("MCP 服务已经运行")
    }
    anyhow::ensure!(port > 0, "端口必须在 1–65535 之间");
    let address: std::net::IpAddr = host
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("监听地址必须是有效的 IPv4 或 IPv6 地址"))?;
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
            .with_max_request_body_bytes(MAX_TRANSFER_MIB as usize * 1024 * 1024 * 6 + 65536)
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
    // Wait for the old listener to stop before permitting a restart.
    for _ in 0..100 {
        if !state.mcp.lock().unwrap().running {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    anyhow::bail!("服务正在等待连接结束，请稍后重试")
}
