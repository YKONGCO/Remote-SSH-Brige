use remote_desk_lib::{
    clients::{self, ClientTarget},
    model::McpStatus,
};
use std::{fs, path::PathBuf};
fn status(token: &str) -> McpStatus {
    McpStatus {
        running: true,
        host: "127.0.0.1".into(),
        port: 18920,
        token: token.into(),
        error: None,
    }
}

#[test]
fn client_urls_follow_listen_address() {
    for (host, expected) in [
        ("192.168.1.10", "http://192.168.1.10:18920/mcp"),
        ("0.0.0.0", "http://127.0.0.1:18920/mcp"),
        ("::", "http://[::1]:18920/mcp"),
        ("2001:db8::1", "http://[2001:db8::1]:18920/mcp"),
    ] {
        let mut service = status("test");
        service.host = host.into();
        assert_eq!(service.client_url(), expected);
        let config = clients::update("cursor", "{}", &service).unwrap();
        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["mcpServers"]["remote-ssh"]["url"], expected);
    }
}
fn workspace() -> PathBuf {
    let p = std::env::temp_dir().join(format!("remote-ssh-clients-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&p).unwrap();
    p
}
fn cleanup(path: PathBuf) {
    let path = path.canonicalize().unwrap();
    assert!(path.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("remote-ssh-clients-"));
    fs::remove_dir_all(path).unwrap();
}
#[test]
fn jsonc_merge_preserves_comments_other_clients_and_entry_options() {
    let source = "{\n// keep this comment\n\"theme\":\"dark\",\"mcpServers\":{\"other\":{\"command\":\"keep\"},\"remote-ssh\":{\"command\":\"old\",\"args\":[\"x\"],\"timeout\":123}},\n}";
    for id in ["claude", "cursor", "gemini", "pi"] {
        let output = clients::update(id, source, &status("first")).unwrap();
        assert!(output.contains("// keep this comment"));
        let value = jsonc_parser::cst::CstRootNode::parse(&output, &Default::default())
            .unwrap()
            .to_serde_value()
            .unwrap();
        assert_eq!(value["theme"], "dark");
        assert_eq!(value["mcpServers"]["other"]["command"], "keep");
        let entry = &value["mcpServers"]["remote-ssh"];
        assert_eq!(entry["timeout"], 123);
        assert!(entry.get("command").is_none());
        assert_eq!(entry["headers"]["Authorization"], "Bearer first");
        assert_eq!(
            clients::update(id, &output, &status("first")).unwrap(),
            output
        );
    }
    let output = clients::update(
        "opencode",
        "{\"mcp\":{\"other\":{\"type\":\"local\"}}}",
        &status("first"),
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["mcp"]["remote-ssh"]["type"], "remote");
    assert_eq!(value["mcp"]["remote-ssh"]["oauth"], false);
    assert_eq!(value["mcp"]["other"]["type"], "local");
}
#[test]
fn toml_merge_preserves_unrelated_tables_and_supports_inline_tables() {
    for source in [
        "# preserve\nmodel = 'keep'\n[mcp_servers.other]\ncommand='untouched'\n[mcp_servers.remote-ssh]\ncommand='old'\nstartup_timeout_sec=60\n",
        "# preserve\nmodel = 'keep'\nmcp_servers = { other = {command='untouched'}, remote-ssh = {command='old', startup_timeout_sec=60} }\n",
    ] {
        let output = clients::update("codex", source, &status("first")).unwrap();
        assert!(output.contains("# preserve"));
        let doc: toml_edit::DocumentMut = output.parse().unwrap();
        assert_eq!(doc["model"].as_str(), Some("keep"));
        assert_eq!(doc["mcp_servers"]["other"]["command"].as_str(), Some("untouched"));
        assert_eq!(doc["mcp_servers"]["remote-ssh"]["startup_timeout_sec"].as_integer(), Some(60));
        assert_eq!(doc["mcp_servers"]["remote-ssh"]["http_headers"]["Authorization"].as_str(), Some("Bearer first"));
        assert!(doc["mcp_servers"]["remote-ssh"].get("command").is_none());
        assert_eq!(clients::update("codex", &output, &status("first")).unwrap(), output);
    }
}
#[test]
fn installs_backup_and_detects_token_rotation_for_all_clients() {
    let dir = workspace();
    let targets = clients::targets_at(&dir);
    assert_eq!(targets.len(), 6);
    assert_eq!(targets[5].id, "pi");
    assert!(targets[5].path.ends_with(".pi/agent/mcp.json"));
    assert!(clients::list(&targets, &status("first"))
        .iter()
        .all(|c| c.status == "missing"));
    for target in &targets {
        clients::install(target, &status("first")).unwrap();
    }
    assert!(clients::list(&targets, &status("first"))
        .iter()
        .all(|c| c.status == "configured"));
    assert!(clients::list(&targets, &status("second"))
        .iter()
        .all(|c| c.status == "outdated"));
    for target in &targets {
        let old = fs::read_to_string(&target.path).unwrap();
        clients::install(target, &status("second")).unwrap();
        let backups = fs::read_dir(target.path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "bak"))
            .collect::<Vec<_>>();
        assert!(backups
            .iter()
            .any(|e| fs::read_to_string(e.path()).unwrap() == old));
    }
    assert!(clients::list(&targets, &status("second"))
        .iter()
        .all(|c| c.status == "configured"));
    cleanup(dir);
}
#[test]
fn malformed_files_and_invalid_maps_are_never_overwritten() {
    let dir = workspace();
    let target = ClientTarget {
        id: "cursor".into(),
        name: "Cursor".into(),
        path: dir.join("mcp.json"),
    };
    for source in [
        "{invalid",
        "[]",
        "{\"mcpServers\":[]}",
        "{\"mcpServers\":{\"remote-ssh\":false}}",
    ] {
        fs::write(&target.path, source).unwrap();
        assert!(clients::install(&target, &status("first")).is_err());
        assert_eq!(fs::read_to_string(&target.path).unwrap(), source);
        assert_eq!(
            clients::list(&[target.clone()], &status("first"))[0].status,
            "error"
        );
    }
    assert!(clients::install(&target, &McpStatus::default()).is_err());
    cleanup(dir);
}

#[test]
fn other_service_names_are_preserved_without_merging_their_options() {
    for target in clients::targets_at(std::path::Path::new("fixture")) {
        let map = if target.id == "opencode" {
            "mcp"
        } else {
            "mcpServers"
        };
        let source = if target.id == "codex" {
            "[mcp_servers.remote-desk]\ntimeout=123\n".into()
        } else {
            serde_json::json!({map:{"remote-desk":{"timeout":123}}}).to_string()
        };
        let output = clients::update(&target.id, &source, &status("current")).unwrap();
        if target.id == "codex" {
            let before: toml_edit::DocumentMut = source.parse().unwrap();
            let after: toml_edit::DocumentMut = output.parse().unwrap();
            assert_eq!(
                before["mcp_servers"]["remote-desk"].to_string(),
                after["mcp_servers"]["remote-desk"].to_string()
            );
            assert_eq!(
                after["mcp_servers"]["remote-ssh"]["http_headers"]["Authorization"].as_str(),
                Some("Bearer current")
            );
            assert!(after["mcp_servers"]["remote-ssh"].get("timeout").is_none());
        } else {
            let before: serde_json::Value = serde_json::from_str(&source).unwrap();
            let after: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(before[map]["remote-desk"], after[map]["remote-desk"]);
            assert_eq!(
                after[map]["remote-ssh"]["headers"]["Authorization"],
                "Bearer current"
            );
            assert!(after[map]["remote-ssh"].get("timeout").is_none());
        }
        assert_eq!(
            clients::update(&target.id, &output, &status("current")).unwrap(),
            output
        );
    }
}
