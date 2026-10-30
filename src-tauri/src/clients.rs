use crate::model::{McpStatus, MCP_SERVER_NAME};
use anyhow::{bail, Context, Result};
use jsonc_parser::cst::{CstInputValue, CstRootNode};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const KEY: &str = MCP_SERVER_NAME;
const LEGACY_KEY: &str = "remote-desk";
const MAX_CONFIG: u64 = 2 * 1024 * 1024;
const TRANSPORT_KEYS: &[&str] = &[
    "command",
    "args",
    "env",
    "cwd",
    "type",
    "url",
    "httpUrl",
    "headers",
    "http_headers",
    "env_http_headers",
    "bearer_token_env_var",
    "http_headers_helper",
    "headersHelper",
    "oauth",
    "authProviderType",
];
#[derive(Clone, Serialize)]
pub struct ClientTarget {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}
#[derive(Serialize)]
pub struct ClientConnection {
    #[serde(flatten)]
    pub target: ClientTarget,
    pub status: String,
    pub error: Option<String>,
}
pub fn targets() -> Result<Vec<ClientTarget>> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .context("找不到用户目录")?;
    let mut result = targets_at(Path::new(&home));
    if let Some(dir) = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        result[1].path = dir.join("config.toml");
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        result[3].path = opencode_path(&dir);
    }
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        result[5].path = dir.join("mcp.json");
    }
    Ok(result)
}
fn opencode_path(config: &Path) -> PathBuf {
    let dir = config.join("opencode");
    if dir.join("opencode.jsonc").exists() {
        dir.join("opencode.jsonc")
    } else {
        dir.join("opencode.json")
    }
}
pub fn targets_at(home: &Path) -> Vec<ClientTarget> {
    [
        ("claude", "Claude Code", home.join(".claude.json")),
        ("codex", "Codex", home.join(".codex/config.toml")),
        ("gemini", "Gemini CLI", home.join(".gemini/settings.json")),
        ("opencode", "OpenCode", opencode_path(&home.join(".config"))),
        ("cursor", "Cursor", home.join(".cursor/mcp.json")),
        ("pi", "Pi", home.join(".pi/agent/mcp.json")),
    ]
    .into_iter()
    .map(|(id, name, path)| ClientTarget {
        id: id.into(),
        name: name.into(),
        path,
    })
    .collect()
}
fn entry(id: &str, old: Option<&Value>, status: &McpStatus) -> Result<Value> {
    let mut result = match old {
        Some(Value::Object(v)) => v.clone(),
        None => Default::default(),
        _ => bail!("已有 {KEY} 配置不是对象"),
    };
    for key in TRANSPORT_KEYS {
        result.remove(*key);
    }
    let url = status.client_url();
    let headers = json!({"Authorization": format!("Bearer {}", status.token)});
    let transport = match id {
        "codex" => json!({"url": url, "http_headers": headers, "enabled": true}),
        "gemini" => json!({"httpUrl": url, "headers": headers}),
        "opencode" => {
            json!({"type":"remote", "url":url, "headers":headers, "enabled":true, "oauth":false})
        }
        "claude" => json!({"type":"http", "url":url, "headers":headers}),
        "cursor" => json!({"url":url, "headers":headers}),
        "pi" => json!({"type":"http", "url":url, "headers":headers, "enabled":true}),
        _ => bail!("不支持的客户端"),
    };
    result.extend(transport.as_object().unwrap().clone());
    Ok(Value::Object(result))
}
fn input(value: Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(v) => CstInputValue::Bool(v),
        Value::Number(v) => CstInputValue::Number(v.to_string()),
        Value::String(v) => CstInputValue::String(v),
        Value::Array(v) => CstInputValue::Array(v.into_iter().map(input).collect()),
        Value::Object(v) => {
            CstInputValue::Object(v.into_iter().map(|(k, v)| (k, input(v))).collect())
        }
    }
}
// Only this application's entry is changed. Comments and unrelated settings survive.
pub fn update(id: &str, source: &str, status: &McpStatus) -> Result<String> {
    if id == "codex" {
        let mut doc: toml_edit::DocumentMut =
            source.parse().context("TOML 配置格式无效，原文件未修改")?;
        if doc.get("mcp_servers").is_none() {
            doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let servers = doc["mcp_servers"]
            .as_table_like_mut()
            .context("mcp_servers 必须是表")?;
        for key in [KEY, LEGACY_KEY] {
            if let Some(current) = servers.get(key) {
                anyhow::ensure!(current.is_table_like(), "已有 {key} 配置不是表");
            }
        }
        if let Some(legacy) = servers.remove(LEGACY_KEY) {
            if let Some(current) = servers.get_mut(KEY) {
                if let (Some(old_table), Some(new_table)) =
                    (legacy.as_table(), current.as_table_mut())
                {
                    let old_prefix = old_table
                        .decor()
                        .prefix()
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let old_suffix = old_table
                        .decor()
                        .suffix()
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .trim();
                    let new_prefix = new_table
                        .decor()
                        .prefix()
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let header_comment = if old_suffix.starts_with('#') {
                        format!("{old_suffix}\n")
                    } else {
                        String::new()
                    };
                    let prefix = format!("{old_prefix}{header_comment}{new_prefix}");
                    new_table.decor_mut().set_prefix(prefix);
                }
                let table = current.as_table_like_mut().unwrap();
                for (key, value) in legacy.as_table_like().unwrap().iter() {
                    if table.get(key).is_none() {
                        table.insert(key, value.clone());
                    }
                }
            } else {
                servers.insert(KEY, legacy);
            }
        }
        if servers.get(KEY).is_none() {
            servers.insert(KEY, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let table = servers
            .get_mut(KEY)
            .unwrap()
            .as_table_like_mut()
            .context("remote-ssh 必须是表")?;
        for key in TRANSPORT_KEYS {
            table.remove(key);
        }
        table.remove("enabled");
        table.insert("url", toml_edit::value(status.client_url()));
        table.insert("enabled", toml_edit::value(true));
        let mut headers = toml_edit::InlineTable::new();
        headers.insert(
            "Authorization",
            toml_edit::Value::from(format!("Bearer {}", status.token)),
        );
        table.insert("http_headers", toml_edit::value(headers));
        return Ok(doc.to_string());
    }
    let root = CstRootNode::parse(source, &Default::default())
        .context("JSON 配置格式无效，原文件未修改")?;
    let object = root
        .object_value_or_create()
        .context("配置根节点必须是对象")?;
    let map_key = if id == "opencode" {
        "mcp"
    } else {
        "mcpServers"
    };
    let servers = object
        .object_value_or_create(map_key)
        .context("MCP 配置必须是对象")?;
    let current = servers.object_value(KEY).and_then(|v| v.to_serde_value());
    let legacy = servers
        .object_value(LEGACY_KEY)
        .and_then(|v| v.to_serde_value());
    for (key, value) in [(KEY, &current), (LEGACY_KEY, &legacy)] {
        if servers.get(key).is_some() && value.is_none() {
            bail!("已有 {key} 配置不是对象")
        }
    }
    let mut merged = legacy
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(current) = current.as_ref().and_then(Value::as_object) {
        merged.extend(current.clone());
    }
    let next = entry(id, Some(&Value::Object(merged)), status)?;
    if legacy.is_none() && current.as_ref() == Some(&next) {
        return Ok(source.into());
    }
    if let Some(prop) = servers.get(KEY) {
        prop.set_value(input(next));
    } else {
        servers.append(KEY, input(next));
    }
    if let Some(prop) = servers.get(LEGACY_KEY) {
        prop.remove();
    }
    Ok(root.to_string())
}
fn read(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(info) => {
            anyhow::ensure!(
                info.is_file() && !info.file_type().is_symlink(),
                "配置路径须为普通文件"
            );
            anyhow::ensure!(info.len() <= MAX_CONFIG, "配置文件超过 2 MiB，未自动修改");
            Ok(Some(fs::read_to_string(path).context("配置文件无法读取")?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("无法读取客户端配置"),
    }
}
pub fn list(targets: &[ClientTarget], status: &McpStatus) -> Vec<ClientConnection> {
    targets
        .iter()
        .map(|target| {
            let result = (|| -> Result<&str> {
                let Some(source) = read(&target.path)? else {
                    return Ok("missing");
                };
                let next = update(&target.id, &source, status)?;
                // Compare parsed documents; TOML formatting can change even for equal values.
                let value = if target.id == "codex" {
                    let doc: toml_edit::DocumentMut = source.parse()?;
                    let next: toml_edit::DocumentMut = next.parse()?;
                    doc.to_string() == next.to_string()
                } else {
                    source == next
                };
                if status.running && value {
                    Ok("configured")
                } else if source.contains(KEY) || source.contains(LEGACY_KEY) {
                    Ok("outdated")
                } else {
                    Ok("available")
                }
            })();
            match result {
                Ok(state) => ClientConnection {
                    target: target.clone(),
                    status: state.into(),
                    error: None,
                },
                Err(e) => ClientConnection {
                    target: target.clone(),
                    status: "error".into(),
                    error: Some(e.to_string()),
                },
            }
        })
        .collect()
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub fn install(target: &ClientTarget, status: &McpStatus) -> Result<()> {
    anyhow::ensure!(
        status.running && !status.token.is_empty(),
        "请先启动 MCP 服务"
    );
    fs::create_dir_all(target.path.parent().context("无效配置路径")?)?;
    let lock = target.path.with_extension(format!(
        "{}.remote-desk.lock",
        target
            .path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
    ));
    private_write(&lock, b"").context("配置正在被修改，请稍后重试")?;
    let _lock = Cleanup(lock);
    let old = read(&target.path)?;
    let next = update(&target.id, old.as_deref().unwrap_or(""), status)?;
    if old.as_deref() == Some(&next) {
        return Ok(());
    }
    let temp = target
        .path
        .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let _temp = Cleanup(temp.clone());
    private_write(&temp, next.as_bytes())?;
    if let Some(ref source) = old {
        let backup = target.path.with_extension(format!(
            "{}.remote-desk-{}.bak",
            target
                .path
                .extension()
                .unwrap_or_default()
                .to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        private_write(&backup, source.as_bytes()).context("备份失败，原配置未修改")?;
    }
    anyhow::ensure!(read(&target.path)? == old, "配置已被其他程序更改，请重试");
    fs::rename(&temp, &target.path).context("无法替换配置文件，原配置已保留")?;
    Ok(())
}
