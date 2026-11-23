use serde::{Deserialize, Serialize};
pub const MCP_SERVER_NAME: &str = "remote-ssh";
pub const MCP_SERVER_TITLE: &str = "Remote SSH Brige";
pub const MAX_TRANSFER_MIB: u64 = 8_589_934_591;
pub const MAX_INLINE_TRANSFER_MIB: u64 = 256;
pub const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 120;
pub const MAX_COMMAND_RULES: usize = 4096;

pub const SETTINGS_VERSION: u32 = 4;

pub const LEGACY_DIAGNOSTIC_COMMANDS: &[&str] = &[
    "pwd", "ls", "cat", "head", "tail", "stat", "file", "du", "df", "free", "uptime", "whoami",
    "id", "uname", "date", "hostname", "ps", "ss", "netstat", "wc", "readlink", "realpath", "true",
    "false", ":", "yes", "set",
];

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeSettings {
    pub settings_version: u32,
    pub close_to_tray: bool,
    pub mcp_auto_start: bool,
    pub mcp_host: String,
    pub mcp_port: u16,
    pub deletion_protection: bool,
    pub command_policy: Option<CommandPolicy>,
    pub remote_read_only: bool,
    pub approval_mode: bool,
    pub approval_timeout_secs: u64,
    pub upload_limit_mib: u64,
    pub download_limit_mib: u64,
    pub local_upload_roots: Vec<String>,
    pub local_download_roots: Vec<String>,
    pub connect_timeout_secs: u64,
    pub io_timeout_secs: u64,
    pub handshake_timeout_secs: u64,
    pub command_timeout_secs: u64,
    pub exec_timeout_to_job: bool,
    pub max_terminals: usize,
    pub advanced_mcp_tools: AdvancedMcpTools,
    pub transfers: crate::transfers::TransferSettings,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AdvancedMcpTools {
    pub terminal: bool,
    pub port_forward: bool,
}
impl Default for AdvancedMcpTools {
    fn default() -> Self {
        Self {
            terminal: false,
            port_forward: true,
        }
    }
}
impl AdvancedMcpTools {
    pub fn enabled(&self, tool: &str) -> bool {
        match tool {
            "terminal" => self.terminal,
            "port_forward" => self.port_forward,
            _ => true,
        }
    }
}
impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            settings_version: SETTINGS_VERSION,
            close_to_tray: true,
            mcp_auto_start: true,
            mcp_host: "127.0.0.1".into(),
            mcp_port: 18920,
            deletion_protection: true,
            command_policy: None,
            remote_read_only: false,
            approval_mode: true,
            approval_timeout_secs: DEFAULT_APPROVAL_TIMEOUT_SECS,
            upload_limit_mib: 0,
            download_limit_mib: 0,
            local_upload_roots: vec![],
            local_download_roots: vec![],
            connect_timeout_secs: 8,
            io_timeout_secs: 15,
            handshake_timeout_secs: 15,
            command_timeout_secs: 30,
            exec_timeout_to_job: false,
            max_terminals: 32,
            advanced_mcp_tools: AdvancedMcpTools::default(),
            transfers: crate::transfers::TransferSettings::default(),
        }
    }
}
impl RuntimeSettings {
    pub fn effective_command_policy(&self) -> CommandPolicy {
        self.command_policy.clone().unwrap_or_default()
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.settings_version <= SETTINGS_VERSION,
            "Runtime settings were written by a newer app version"
        );
        anyhow::ensure!(self.mcp_port > 0, "MCP port must be 1-65535");
        anyhow::ensure!(
            self.mcp_host.parse::<std::net::IpAddr>().is_ok(),
            "MCP listen address must be a valid IPv4 or IPv6 address"
        );
        self.transfers.validate()?;
        if let Some(policy) = &self.command_policy {
            policy.validate()?;
        }
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
            (1..=120).contains(&self.handshake_timeout_secs),
            "Handshake timeout must be 1-120 seconds"
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

fn default_true() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CommandPolicy {
    pub protected_commands: Vec<String>,
    #[serde(default = "default_true")]
    pub allow_pipes: bool,
    #[serde(default = "default_true")]
    pub allow_redirection: bool,
    #[serde(default = "default_true")]
    pub allow_compound: bool,
    #[serde(default = "default_true")]
    pub allow_scripts: bool,
    #[serde(default = "default_true")]
    pub allow_substitution: bool,

    pub exec_disabled: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_whitelist: Option<Vec<String>>,
}
impl Default for CommandPolicy {
    fn default() -> Self {
        Self {
            protected_commands: vec![],
            allow_pipes: true,
            allow_redirection: true,
            allow_compound: true,
            allow_scripts: true,
            allow_substitution: true,
            exec_disabled: false,
            legacy_whitelist: None,
        }
    }
}

impl CommandPolicy {
    pub fn validate(&self) -> anyhow::Result<()> {
        for list in [&self.protected_commands]
            .into_iter()
            .chain(self.legacy_whitelist.as_ref())
        {
            anyhow::ensure!(
                list.len() <= MAX_COMMAND_RULES,
                "Each command list supports at most {MAX_COMMAND_RULES} entries"
            );
        }
        for (index, rule) in self.protected_commands.iter().enumerate() {
            let mut words = protected_rule_words(rule);
            anyhow::ensure!(
                rule.chars().count() <= 512
                    && words.next().is_some_and(valid_command_name)
                    && words.all(|arg| arg.chars().all(|c| c.is_alphanumeric() || "_.+-/:=@%".contains(c))),
                "Protected rule {} must contain a command name or absolute path followed by optional literal arguments",
                index + 1
            );
        }
        if let Some(whitelist) = &self.legacy_whitelist {
            for name in whitelist {
                anyhow::ensure!(
                    name.chars().count() <= 512 && valid_command_name(name),
                    "Legacy allowlist entries must be command names or absolute paths"
                );
            }
        }
        Ok(())
    }
}

pub(crate) fn protected_rule_words(rule: &str) -> impl Iterator<Item = &str> {
    rule.split([' ', '\t']).filter(|word| !word.is_empty())
}

fn valid_command_name(name: &str) -> bool {
    !name.is_empty()
        && (name == ":"
            || name
                .chars()
                .all(|c| c.is_alphanumeric() || "_.+-/".contains(c)))
        && (!name.contains('/')
            || (name.starts_with('/')
                && name
                    .split('/')
                    .skip(1)
                    .all(|part| !part.is_empty() && part != "." && part != "..")))
}

pub fn migrate_settings_value(mut value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Runtime settings must be a JSON object"))?;
    let version = object
        .get("settings_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    anyhow::ensure!(
        version <= SETTINGS_VERSION as u64,
        "Runtime settings were written by a newer app version"
    );
    if version >= SETTINGS_VERSION as u64 {
        return Ok(value);
    }
    let block_dangerous = object
        .remove("block_dangerous_commands")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    if object
        .get("approval_mode")
        .is_some_and(|value| !value.is_boolean())
    {
        object.remove("approval_mode");
    }
    let old_policy = object.remove("command_policy");
    object.insert(
        "command_policy".into(),
        migrate_command_policy(old_policy, block_dangerous),
    );
    if let Some(policy) = object
        .get_mut("command_policy")
        .and_then(serde_json::Value::as_object_mut)
    {
        policy.remove("allow_mcp_terminal");
    }
    object
        .entry("advanced_mcp_tools")
        .or_insert_with(|| serde_json::to_value(AdvancedMcpTools::default()).unwrap());
    object
        .entry("approval_mode")
        .or_insert(serde_json::json!(true));
    object.insert(
        "settings_version".into(),
        serde_json::json!(SETTINGS_VERSION),
    );
    Ok(value)
}

fn migrate_command_policy(
    old: Option<serde_json::Value>,
    block_dangerous: bool,
) -> serde_json::Value {
    let Some(serde_json::Value::Object(mut old)) = old else {
        return serde_json::to_value(CommandPolicy {
            allow_pipes: !block_dangerous,
            allow_redirection: !block_dangerous,
            allow_compound: !block_dangerous,
            allow_scripts: !block_dangerous,
            allow_substitution: !block_dangerous,
            legacy_whitelist: block_dangerous.then(|| {
                LEGACY_DIAGNOSTIC_COMMANDS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            }),
            ..Default::default()
        })
        .expect("CommandPolicy serializes");
    };
    if old.contains_key("protected_commands") {
        return serde_json::Value::Object(old);
    }
    let execution_mode = old
        .remove("execution_mode")
        .and_then(|value| value.as_str().map(str::to_owned));
    let allowed_commands = old
        .remove("allowed_commands")
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
        .unwrap_or_default();
    let blocked_commands = old
        .remove("blocked_commands")
        .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
        .unwrap_or_default();
    let diagnostic_only = old
        .remove("diagnostic_only")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let unified = old
        .remove("allow_advanced_shell")
        .and_then(|value| value.as_bool());
    let advanced = |name: &str| {
        unified.unwrap_or_else(|| {
            old.get(name)
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        })
    };
    let legacy_whitelist = match execution_mode.as_deref() {
        Some("whitelist") => Some(allowed_commands),
        Some("disabled" | "unrestricted") => None,
        _ if diagnostic_only => Some(
            LEGACY_DIAGNOSTIC_COMMANDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ),
        _ => None,
    };
    serde_json::to_value(CommandPolicy {
        protected_commands: blocked_commands,
        allow_pipes: advanced("allow_pipes"),
        allow_redirection: advanced("allow_redirection"),
        allow_compound: advanced("allow_compound"),
        allow_scripts: advanced("allow_scripts"),
        allow_substitution: advanced("allow_substitution"),
        exec_disabled: execution_mode.as_deref() == Some("disabled"),
        legacy_whitelist,
    })
    .expect("CommandPolicy serializes")
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
    pub login_path: Option<String>,
    pub login_path_synced_at: Option<String>,
}

#[derive(Clone, Deserialize, Debug)]
pub struct LoginPathUpdate {
    pub value: Option<String>,
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
            login_path: None,
            login_path_synced_at: None,
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
    "terminal_close",
];
impl Server {
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
            | "terminal_close" => self.allow_terminal,
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

    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub transfer_request: Option<crate::transfers::TransferSpec>,
    pub transfer_id: String,
    pub transfer_action: String,
    pub priority: Option<i32>,
    pub bandwidth_kib: Option<u64>,
    pub request_id: Option<String>,

    #[serde(default, skip_serializing_if = "is_false")]
    pub approval_request: bool,
    pub transfer_group: Option<String>,
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

    Preview,
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
    #[schemars(length(min = 1))]
    pub old_text: String,

    pub new_text: String,
}
#[derive(Clone, Serialize)]
pub struct TerminalInfo {
    pub id: String,
    pub server_id: String,
    pub name: String,
    pub created_at: String,
    pub alive: bool,
    #[serde(default)]
    pub state: String,
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
