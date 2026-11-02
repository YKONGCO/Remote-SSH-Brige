use serde::{Deserialize, Serialize};
pub const MCP_SERVER_NAME: &str = "remote-ssh";
pub const MCP_SERVER_TITLE: &str = "Remote SSH Brige";
pub const MAX_TRANSFER_MIB: u64 = 8_589_934_591; // Exact byte counts in JavaScript; 0 means unlimited.
pub const MAX_INLINE_TRANSFER_MIB: u64 = 256; // Text, Base64 and MCP payloads.
pub const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 120;
pub const MAX_COMMAND_RULES: usize = 4096;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeSettings {
    pub close_to_tray: bool,
    pub deletion_protection: bool,
    pub block_dangerous_commands: bool,
    pub command_policy: Option<CommandPolicy>,
    pub remote_read_only: bool,
    pub approval_mode: String,
    pub approval_timeout_secs: u64,
    pub upload_limit_mib: u64,
    pub download_limit_mib: u64,
    pub local_upload_roots: Vec<String>,
    pub local_download_roots: Vec<String>,
    pub connect_timeout_secs: u64,
    pub io_timeout_secs: u64,
    pub command_timeout_secs: u64,
    pub exec_timeout_to_job: bool,
    pub max_terminals: usize,
}
impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            deletion_protection: true,
            block_dangerous_commands: true,
            command_policy: None,
            remote_read_only: false,
            approval_mode: "off".into(),
            approval_timeout_secs: DEFAULT_APPROVAL_TIMEOUT_SECS,
            upload_limit_mib: 128,
            download_limit_mib: 128,
            local_upload_roots: vec![],
            local_download_roots: vec![],
            connect_timeout_secs: 8,
            io_timeout_secs: 15,
            command_timeout_secs: 30,
            exec_timeout_to_job: false,
            max_terminals: 32,
        }
    }
}
impl RuntimeSettings {
    pub fn effective_command_policy(&self) -> CommandPolicy {
        let mut policy = self.command_policy.clone().unwrap_or_else(|| {
            if self.block_dangerous_commands {
                CommandPolicy::default()
            } else {
                CommandPolicy {
                    diagnostic_only: false,
                    allow_pipes: true,
                    allow_redirection: true,
                    allow_compound: true,
                    allow_scripts: true,
                    allow_substitution: true,
                    allow_mcp_terminal: true,
                    ..Default::default()
                }
            }
        });
        if let Some(mode) = &policy.execution_mode {
            policy.diagnostic_only = mode == "whitelist";
        }
        if let Some(advanced) = policy.allow_advanced_shell {
            policy.allow_pipes = advanced;
            policy.allow_redirection = advanced;
            policy.allow_compound = advanced;
            policy.allow_scripts = advanced;
            policy.allow_substitution = advanced;
        }
        policy
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(policy) = &self.command_policy {
            policy.validate()?;
        }
        anyhow::ensure!(
            ["off", "mcp", "all"].contains(&self.approval_mode.as_str()),
            "Invalid approval mode"
        );
        anyhow::ensure!(
            (1..=120).contains(&self.approval_timeout_secs),
            "Approval timeout must be 1-120 seconds"
        );
        anyhow::ensure!(
            self.upload_limit_mib <= MAX_TRANSFER_MIB
                && self.download_limit_mib <= MAX_TRANSFER_MIB,
            "File size limit must be 0-{MAX_TRANSFER_MIB} MiB; 0 means unlimited"
        );
        for roots in [&self.local_upload_roots, &self.local_download_roots] {
            anyhow::ensure!(
                roots.len() <= 64,
                "Each transfer direction supports at most 64 local roots"
            );
            for root in roots {
                anyhow::ensure!(
                    root.len() <= 8192
                        && std::path::Path::new(root).is_absolute()
                        && !root.contains('\0')
                        && !std::path::Path::new(root)
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir)),
                    "Local transfer roots must be absolute paths without .."
                );
            }
        }
        anyhow::ensure!(
            (1..=120).contains(&self.connect_timeout_secs),
            "Connection timeout must be 1-120 seconds"
        );
        anyhow::ensure!(
            (1..=300).contains(&self.io_timeout_secs)
                && (1..=300).contains(&self.command_timeout_secs),
            "I/O and command timeouts must be 1-300 seconds"
        );
        anyhow::ensure!(
            (1..=64).contains(&self.max_terminals),
            "Terminal session limit must be 1-64"
        );
        Ok(())
    }
    pub fn transfer_limit(&self, kind: &str) -> u64 {
        let limit = if matches!(kind, "upload" | "upload_file" | "write_file") {
            self.upload_limit_mib
        } else {
            self.download_limit_mib
        };
        let bytes = if limit == 0 {
            u64::MAX
        } else {
            limit * 1024 * 1024
        };
        if matches!(kind, "upload_file" | "download_file") {
            bytes
        } else {
            bytes.min(MAX_INLINE_TRANSFER_MIB * 1024 * 1024)
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CommandPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
    pub allowed_commands: Vec<String>,
    pub blocked_commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_advanced_shell: Option<bool>,
    pub diagnostic_only: bool,
    pub allow_pipes: bool,
    pub allow_redirection: bool,
    pub allow_compound: bool,
    pub allow_scripts: bool,
    pub allow_substitution: bool,
    pub allow_mcp_terminal: bool,
}
impl Default for CommandPolicy {
    fn default() -> Self {
        Self {
            execution_mode: None,
            allowed_commands: default_allowed_commands(),
            blocked_commands: vec![],
            allow_advanced_shell: None,
            diagnostic_only: true,
            allow_pipes: false,
            allow_redirection: false,
            allow_compound: false,
            allow_scripts: false,
            allow_substitution: false,
            allow_mcp_terminal: false,
        }
    }
}

pub fn default_allowed_commands() -> Vec<String> {
    #[derive(Deserialize)]
    struct Preset {
        commands: Vec<String>,
    }
    let presets: Vec<Preset> = serde_json::from_str(include_str!("../../src/command-presets.json"))
        .expect("bundled command presets must be valid JSON");
    let mut commands = Vec::new();
    for preset in presets {
        for command in preset.commands {
            if !commands.contains(&command) {
                commands.push(command);
            }
        }
    }
    commands
}

impl CommandPolicy {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.execution_mode
                .as_deref()
                .is_none_or(|mode| matches!(mode, "disabled" | "whitelist" | "unrestricted")),
            "Invalid command execution mode"
        );
        for list in [&self.allowed_commands, &self.blocked_commands] {
            anyhow::ensure!(
                list.len() <= MAX_COMMAND_RULES,
                "Each command list supports at most {MAX_COMMAND_RULES} entries"
            );
            for name in list {
                anyhow::ensure!(
                    !name.is_empty()
                        && name.chars().count() <= 512
                        && name
                            .chars()
                            .all(|c| c.is_alphanumeric() || "_.+-/".contains(c))
                        && (!name.contains('/')
                            || (name.starts_with('/')
                                && name
                                    .split('/')
                                    .skip(1)
                                    .all(|part| !part.is_empty() && part != "." && part != ".."))),
                    "Command rules must be names or absolute paths without arguments, wildcards or shell syntax"
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: String,
    pub key_path: String,
    pub fingerprint: String,
    pub group: String,
    pub color: String,
    pub enabled: bool,
    pub allow_exec: bool,
    pub allow_write: bool,
    pub allow_terminal: bool,
    pub mcp_enabled: bool,
    pub mcp_permissions: Option<Vec<String>>,
    pub root: String,
}
impl Default for Server {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            host: String::new(),
            port: 22,
            username: "root".into(),
            auth: "key".into(),
            key_path: String::new(),
            fingerprint: String::new(),
            group: "默认分组".into(),
            color: "blue".into(),
            enabled: true,
            allow_exec: false,
            allow_write: false,
            allow_terminal: false,
            mcp_enabled: true,
            mcp_permissions: None,
            root: "/".into(),
        }
    }
}
pub const MCP_OPERATIONS: &[&str] = &[
    "port_forward_open",
    "port_forward_list",
    "port_forward_read",
    "port_forward_close",
    "read_file",
    "list_files",
    "stat_file",
    "write_file",
    "edit_file",
    "remove_path",
    "exec",
    "job",
    "upload_file",
    "download_file",
    "transfer",
    "list_terminals",
    "terminal_open",
    "terminal_read",
    "terminal_write",
    "terminal_resize",
    "terminal_close",
];
impl Server {
    pub fn terminal_actions(&self) -> Vec<&'static str> {
        [
            ("list", "list_terminals"),
            ("open", "terminal_open"),
            ("read", "terminal_read"),
            ("write", "terminal_write"),
            ("resize", "terminal_resize"),
            ("close", "terminal_close"),
        ]
        .into_iter()
        .filter_map(|(action, permission)| self.permits_mcp(permission).then_some(action))
        .collect()
    }
    pub fn permits_mcp(&self, kind: &str) -> bool {
        if !self.enabled || !self.mcp_enabled || !MCP_OPERATIONS.contains(&kind) {
            return false;
        }
        if let Some(permissions) = &self.mcp_permissions {
            return permissions.iter().any(|permission| permission == kind)
                && (kind != "edit_file" || permissions.iter().any(|p| p == "read_file"));
        }
        match kind {
            "exec" => self.allow_exec,
            "write_file" => self.allow_write,
            "list_terminals" | "terminal_open" | "terminal_read" | "terminal_write"
            | "terminal_resize" | "terminal_close" => self.allow_terminal,
            "read_file" | "list_files" | "stat_file" => true,
            _ => false,
        }
    }
    pub fn effective_mcp_permissions(&self) -> Vec<&'static str> {
        MCP_OPERATIONS
            .iter()
            .copied()
            .filter(|kind| self.permits_mcp(kind))
            .collect()
    }
}
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub servers: Vec<Server>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Audit {
    pub id: String,
    pub time: String,
    pub server_id: String,
    pub server_name: String,
    pub source: String,
    pub kind: String,
    pub detail: String,
    pub success: bool,
    pub duration_ms: u64,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<AuditResponse>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct AuditResponse {
    pub data: serde_json::Value,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<AuditInput>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct AuditInput {
    pub data: serde_json::Value,
    pub truncated: bool,
}
#[derive(Clone, Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct Operation {
    pub tunnel_id: String,
    pub local_port: u16,
    pub remote_port: u16,
    pub remote_host: String,
    pub kind: String,
    pub server_id: String,
    pub path: String,
    pub target: String,
    pub content: String,
    pub command: String,
    pub session_id: String,
    pub cursor: u64,
    pub cols: u32,
    pub rows: u32,
    pub timeout_secs: u64,
    pub job_action: String,
    pub job_id: String,
    pub stdout_cursor: u64,
    pub stderr_cursor: u64,
    pub wait_secs: u64,
    /// Set by native pickers or validated MCP arguments, never by raw Operation IPC.
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub transfer_request: Option<crate::transfers::TransferSpec>,
    pub transfer_id: String,
    pub transfer_action: String,
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_request: Option<FileRequest>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileRequest {
    Remove {
        recursive: bool,
        missing_ok: bool,
    },
    Read {
        offset: u64,
        limit: u64,
    },
    List {
        offset: u64,
        limit: u64,
    },
    Write {
        mode: WriteMode,
        #[serde(default)]
        create_parents: bool,
    },
    Edit {
        edits: Vec<FileEdit>,
    },
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    Create,
    #[default]
    Overwrite,
}

#[derive(Clone, Serialize, Deserialize, Debug, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileEdit {
    /// Unique text from the original file. Never include read continuation notices.
    #[schemars(length(min = 1))]
    pub old_text: String,
    /// Replacement text; empty deletes the matched block.
    pub new_text: String,
}
#[derive(Clone, Serialize)]
pub struct TerminalInfo {
    pub id: String,
    pub server_id: String,
    pub name: String,
    pub created_at: String,
    pub alive: bool,
}
#[derive(Clone, Serialize, Default)]
pub struct McpStatus {
    pub running: bool,
    pub host: String,
    pub port: u16,
    pub token: String,
    pub error: Option<String>,
}
impl McpStatus {
    pub fn client_url(&self) -> String {
        let host = match self.host.as_str() {
            "" | "0.0.0.0" => "127.0.0.1",
            "::" => "::1",
            host => host,
        };
        if host.contains(':') {
            format!("http://[{}]:{}/mcp", host, self.port)
        } else {
            format!("http://{}:{}/mcp", host, self.port)
        }
    }
}
