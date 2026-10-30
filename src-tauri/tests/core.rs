use remote_desk_lib::{
    model::{Operation, Server},
    state::{execute, AppState},
};
fn workspace() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("remote-desk-test-{}", uuid::Uuid::new_v4()))
}

fn cleanup(dir: std::path::PathBuf) {
    let target = dir.canonicalize().unwrap();
    let temp = std::env::temp_dir().canonicalize().unwrap();
    assert!(target.starts_with(&temp));
    assert!(target
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("remote-desk-test-"));
    std::fs::remove_dir_all(target).unwrap();
}

fn persisted_audits(dir: &std::path::Path) -> Vec<remote_desk_lib::model::Audit> {
    remote_desk_lib::database::Database::open(dir)
        .unwrap()
        .recent_audits(10000)
        .unwrap()
}

fn server() -> Server {
    Server {
        name: "Integration host".into(),
        host: "127.0.0.1".into(),
        auth: "agent".into(),
        ..Default::default()
    }
}

#[test]
fn legacy_database_migrates_and_granular_permissions_persist() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let legacy = state
        .save_server(
            Server {
                allow_write: true,
                ..server()
            },
            None,
        )
        .unwrap();
    drop(state);
    let database = rusqlite::Connection::open(dir.join("remote-desk.sqlite3")).unwrap();
    database.execute_batch("ALTER TABLE servers DROP COLUMN mcp_enabled; ALTER TABLE servers DROP COLUMN mcp_permissions_json; PRAGMA user_version=1;").unwrap();
    drop(database);
    let state = AppState::open(dir.clone()).unwrap();
    let mut migrated = state.server(&legacy.id).unwrap();
    assert!(migrated.mcp_enabled);
    assert!(migrated.mcp_permissions.is_none());
    assert!(migrated.permits_mcp("remove"));
    assert!(migrated.permits_mcp("download"));
    assert!(!migrated.permits_mcp("exec"));
    migrated.mcp_permissions = Some(vec!["upload".into(), "terminal_read".into()]);
    state.save_server(migrated, None).unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    let mut saved = state.server(&legacy.id).unwrap();
    assert!(saved.permits_mcp("upload"));
    assert!(saved.permits_mcp("terminal_read"));
    for denied in [
        "write_file",
        "remove",
        "download",
        "terminal_write",
        "terminal_open",
    ] {
        assert!(!saved.permits_mcp(denied));
    }
    saved.mcp_permissions = Some(vec!["invalid".into()]);
    assert!(state.save_server(saved, None).is_err());
    let mut saved = state.server(&legacy.id).unwrap();
    saved.mcp_enabled = false;
    state.save_server(saved, None).unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    assert!(state
        .server(&legacy.id)
        .unwrap()
        .effective_mcp_permissions()
        .is_empty());
    drop(state);
    cleanup(dir);
}

#[tokio::test]
async fn granular_denials_are_enforced_before_network_and_survive_live_updates() {
    use remote_desk_lib::model::MCP_OPERATIONS;
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let mut saved = state
        .save_server(
            Server {
                allow_write: true,
                allow_exec: true,
                allow_terminal: true,
                mcp_permissions: Some(vec![]),
                ..server()
            },
            None,
        )
        .unwrap();
    for operation in MCP_OPERATIONS
        .iter()
        .filter(|kind| **kind != "list_terminals")
    {
        let error = execute(
            state.clone(),
            Operation {
                server_id: saved.id.clone(),
                kind: (*operation).into(),
                ..Default::default()
            },
            "mcp",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("未授权"), "{operation}: {error}");
    }
    saved.mcp_permissions = Some(vec!["read_file".into()]);
    state.save_server(saved.clone(), None).unwrap();
    saved.mcp_enabled = false;
    state.save_server(saved.clone(), None).unwrap();
    let error = execute(
        state.clone(),
        Operation {
            server_id: saved.id,
            kind: "read_file".into(),
            ..Default::default()
        },
        "mcp",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("未授权"));
    let audits = state.audit.lock().unwrap();
    assert!(audits.iter().all(|entry| !entry.success));
    assert!(audits.iter().any(|entry| entry.kind == "upload"));
    drop(audits);
    drop(state);
    cleanup(dir);
}
#[test]
fn runtime_settings_persist_and_reject_invalid_updates() {
    use remote_desk_lib::model::RuntimeSettings;
    let dir = workspace();
    let maximum = RuntimeSettings {
        upload_limit_mib: 256,
        download_limit_mib: 256,
        ..Default::default()
    };
    assert!(maximum.validate().is_ok());
    assert_eq!(maximum.transfer_limit("upload"), 256 * 1024 * 1024);
    let state = AppState::open(dir.clone()).unwrap();
    assert_eq!(*state.settings.lock().unwrap(), RuntimeSettings::default());
    let settings = RuntimeSettings {
        close_to_tray: false,
        command_policy: Some(remote_desk_lib::model::CommandPolicy {
            allow_pipes: true,
            allow_redirection: true,
            ..Default::default()
        }),
        upload_limit_mib: 123,
        download_limit_mib: 237,
        connect_timeout_secs: 12,
        io_timeout_secs: 60,
        command_timeout_secs: 90,
        max_terminals: 4,
        ..Default::default()
    };
    let mut legacy_settings = serde_json::to_value(&settings).unwrap();
    legacy_settings
        .as_object_mut()
        .unwrap()
        .remove("close_to_tray");
    assert!(
        serde_json::from_value::<RuntimeSettings>(legacy_settings)
            .unwrap()
            .close_to_tray
    );
    state.save_settings(settings.clone()).unwrap();
    for invalid in [
        RuntimeSettings {
            upload_limit_mib: 0,
            ..settings.clone()
        },
        RuntimeSettings {
            download_limit_mib: 257,
            ..settings.clone()
        },
        RuntimeSettings {
            upload_limit_mib: 257,
            ..settings.clone()
        },
        RuntimeSettings {
            connect_timeout_secs: 121,
            ..settings.clone()
        },
        RuntimeSettings {
            io_timeout_secs: 0,
            ..settings.clone()
        },
        RuntimeSettings {
            command_timeout_secs: 301,
            ..settings.clone()
        },
        RuntimeSettings {
            max_terminals: 0,
            ..settings.clone()
        },
    ] {
        assert!(state.save_settings(invalid).is_err());
    }
    assert_eq!(*state.settings.lock().unwrap(), settings);
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    assert_eq!(*state.settings.lock().unwrap(), settings);
    drop(state);
    cleanup(dir);
}

#[tokio::test]
async fn transfer_limits_apply_to_desktop_and_mcp_before_network_access() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use remote_desk_lib::model::RuntimeSettings;
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state
        .save_server(
            Server {
                allow_write: true,
                ..server()
            },
            None,
        )
        .unwrap();
    state
        .save_settings(RuntimeSettings {
            upload_limit_mib: 1,
            ..Default::default()
        })
        .unwrap();
    for source in ["desktop", "mcp"] {
        for kind in ["upload", "write_file"] {
            let bytes = vec![b'x'; 1024 * 1024 + 1];
            let content = if kind == "upload" {
                STANDARD.encode(bytes)
            } else {
                String::from_utf8(bytes).unwrap()
            };
            let error = execute(
                state.clone(),
                Operation {
                    kind: kind.into(),
                    server_id: saved.id.clone(),
                    content,
                    path: "/oversize".into(),
                    ..Default::default()
                },
                source,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("1 MiB"), "{error}");
        }
    }
    drop(state);
    cleanup(dir);
}
#[test]
fn config_survives_restart_and_host_change_invalidates_trust() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    let mut trusted = saved.clone();
    trusted.fingerprint = "SHA256:known-host".into();
    state.save_server(trusted, None).unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    assert_eq!(
        state.server(&saved.id).unwrap().fingerprint,
        "SHA256:known-host"
    );
    let mut changed = state.server(&saved.id).unwrap();
    changed.port = 2222;
    assert!(state
        .save_server(changed, None)
        .unwrap()
        .fingerprint
        .is_empty());
    drop(state);
    cleanup(dir);
}
#[test]
fn corrupted_configuration_is_never_silently_replaced() {
    let dir = workspace();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("servers.json"), "invalid json").unwrap();
    assert!(AppState::open(dir.clone()).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.join("servers.json")).unwrap(),
        "invalid json"
    );
    cleanup(dir);
}
#[tokio::test]
async fn mcp_permissions_are_denied_before_connecting_and_audited() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    for kind in [
        "exec",
        "write_file",
        "terminal_open",
        "terminal_read",
        "remove",
    ] {
        let result = execute(
            state.clone(),
            Operation {
                server_id: saved.id.clone(),
                kind: kind.into(),
                ..Default::default()
            },
            "mcp",
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("未授权"));
    }
    let logs = state.audit.lock().unwrap();
    assert_eq!(logs.len(), 4);
    assert!(logs.iter().all(|a| !a.success));
    drop(logs);
    drop(state);
    cleanup(dir);
}
#[tokio::test]
async fn disabled_host_is_blocked_and_sensitive_content_is_not_logged() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let mut s = server();
    s.enabled = false;
    let saved = state.save_server(s, None).unwrap();
    let result = execute(
        state.clone(),
        Operation {
            server_id: saved.id,
            kind: "write_file".into(),
            path: "/secret".into(),
            content: "sensitive-file-content".into(),
            ..Default::default()
        },
        "mcp",
    )
    .await;
    assert!(result.is_err());
    let logs = serde_json::to_string(&persisted_audits(&dir)).unwrap();
    assert!(!logs.contains("sensitive-file-content"));
    assert!(logs.contains("/secret"));
    drop(state);
    cleanup(dir);
}

#[tokio::test]
async fn mcp_start_stop_and_restart_releases_listener() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let first = remote_desk_lib::mcp::start(state.clone(), port)
        .await
        .unwrap();
    assert!(first.running);
    assert_eq!(first.host, "127.0.0.1");
    assert_eq!(first.token.len(), 64);
    assert!(remote_desk_lib::mcp::start(state.clone(), port)
        .await
        .is_err());
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    client.write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 401"));
    drop(client);
    remote_desk_lib::mcp::stop(state.clone()).await.unwrap();
    assert!(!state.mcp.lock().unwrap().running);
    assert!(
        remote_desk_lib::mcp::start_on(state.clone(), port, "not-an-ip")
            .await
            .is_err()
    );
    assert!(!state.mcp.lock().unwrap().running);
    let second = remote_desk_lib::mcp::start_on(state.clone(), port, "0.0.0.0")
        .await
        .unwrap();
    assert_eq!(second.host, "0.0.0.0");
    assert_ne!(first.token, second.token);
    for (credential, request_host, expected) in [
        ("invalid", "192.168.1.10", "HTTP/1.1 401"),
        (second.token.as_str(), "untrusted.example", "HTTP/1.1 403"),
        (second.token.as_str(), "192.168.1.10", "HTTP/1.1 200"),
    ] {
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
        let request = format!("POST /mcp HTTP/1.1\r\nHost: {request_host}:{port}\r\nAuthorization: Bearer {credential}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        client.write_all(request.as_bytes()).await.unwrap();
        let mut response = vec![0; 4096];
        let count = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            String::from_utf8_lossy(&response[..count]).starts_with(expected),
            "{}",
            String::from_utf8_lossy(&response[..count])
        );
    }
    // Session workers release their shared database after cancellation is observed.
    remote_desk_lib::mcp::stop(state.clone()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while std::sync::Arc::strong_count(&state) > 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(state);
    cleanup(dir);
}

#[cfg(target_os = "windows")]
#[test]
fn password_is_stored_in_system_keyring_and_removed_with_host() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let mut s = server();
    s.auth = "password".into();
    let secret = format!("fixture-{}", uuid::Uuid::new_v4());
    let saved = state.save_server(s, Some(secret.clone())).unwrap();
    let entry = keyring::Entry::new("dev.remotedesk.ssh", &saved.id).unwrap();
    assert_eq!(entry.get_password().unwrap(), secret);
    assert!(!state.snapshot().to_string().contains(&secret));
    assert!(!dir.join("servers.json").exists());
    state.remove_server(&saved.id).unwrap();
    assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
    drop(state);
    cleanup(dir);
}

#[test]
fn audit_persists_output_without_polling_payload_and_loads_legacy_records() {
    use serde_json::json;
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    let op = Operation {
        kind: "exec".into(),
        command: "fixture command".into(),
        server_id: saved.id.clone(),
        ..Default::default()
    };
    state
        .log(
            &saved,
            "mcp",
            &op,
            &Ok(json!({"stdout":"返回结果 ✓","stderr":"warning","exit_code":7})),
            12,
        )
        .unwrap();
    let snapshot = state.snapshot();
    assert!(snapshot["audits"][0].get("response").is_none());
    let entry = state.audit.lock().unwrap()[0].clone();
    assert!(!entry.success);
    let result = entry.response.as_ref().unwrap();
    assert_eq!(result.data["stdout"], "返回结果 ✓");
    assert_eq!(result.data["stderr"], "warning");
    assert_eq!(result.data["exit_code"], 7);
    assert!(!result.truncated);
    assert_eq!(
        persisted_audits(&dir)[0].response.as_ref().unwrap().data["stdout"],
        "返回结果 ✓"
    );
    assert_eq!(
        &std::fs::read(dir.join("remote-desk.sqlite3")).unwrap()[..16],
        b"SQLite format 3\0"
    );
    assert!(!dir.join("audit.jsonl").exists());
    drop(state);
    let reopened = AppState::open(dir.clone()).unwrap();
    let audits = reopened.audit.lock().unwrap();
    assert_eq!(audits.len(), 1);
    assert!(audits[0].response.is_some());
    drop(audits);
    drop(reopened);
    cleanup(dir);
}

#[test]
fn audit_caps_multibyte_output_and_omits_file_and_input_payloads() {
    use serde_json::json;
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    let mut op = Operation {
        kind: "exec".into(),
        server_id: saved.id.clone(),
        ..Default::default()
    };
    state
        .log(
            &saved,
            "mcp",
            &op,
            &Ok(json!({"stdout":"输出🙂".repeat(10000),"stderr":"","exit_code":0})),
            0,
        )
        .unwrap();
    let entry = state.audit.lock().unwrap()[0].clone();
    let response = entry.response.unwrap();
    assert!(response.truncated);
    assert!(response.data["stdout"].as_str().unwrap().len() <= 16384);
    op.kind = "read_file".into();
    state
        .log(
            &saved,
            "mcp",
            &op,
            &Ok(json!({"content":"private-file-body"})),
            0,
        )
        .unwrap();
    op.kind = "write_file".into();
    op.content = "private-file-body".into();
    state
        .log(&saved, "mcp", &op, &Ok(json!({"written":17})), 0)
        .unwrap();
    op.kind = "download".into();
    state
        .log(&saved, "mcp", &op, &Ok(json!({"base64":"cHJpdmF0ZQ=="})), 0)
        .unwrap();
    op.kind = "terminal_write".into();
    op.content = "secret keystrokes".into();
    state
        .log(&saved, "mcp", &op, &Ok(json!({"sent":true})), 0)
        .unwrap();
    let log = serde_json::to_string(&persisted_audits(&dir)).unwrap();
    assert!(!log.contains("private-file-body"));
    assert!(!log.contains("cHJpdmF0ZQ=="));
    assert!(!log.contains("secret keystrokes"));
    op.kind = "exec".into();
    state
        .log(
            &saved,
            "mcp",
            &op,
            &Err(anyhow::anyhow!("connection failed")),
            0,
        )
        .unwrap();
    assert_eq!(
        state
            .audit
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .response
            .as_ref()
            .unwrap()
            .data["error"],
        "connection failed"
    );
    drop(state);
    cleanup(dir);
}

#[test]
fn legacy_import_is_atomic_idempotent_and_preserves_identity_and_files() {
    use remote_desk_lib::model::{Audit, Config};
    let dir = workspace();
    std::fs::create_dir_all(&dir).unwrap();
    let mut saved = server();
    saved.id = "existing-credential-id".into();
    saved.fingerprint = "SHA256:trusted".into();
    let config = serde_json::to_vec(&Config {
        servers: vec![saved.clone()],
    })
    .unwrap();
    std::fs::write(dir.join("servers.json"), &config).unwrap();
    let entry = Audit {
        id: "legacy-audit".into(),
        time: chrono::Utc::now().to_rfc3339(),
        server_id: saved.id.clone(),
        server_name: saved.name.clone(),
        source: "mcp".into(),
        kind: "exec".into(),
        detail: "pwd".into(),
        success: true,
        duration_ms: 1,
        summary: "exit: 0".into(),
        response: None,
    };
    let line = serde_json::to_string(&entry).unwrap();
    let mut archived = entry.clone();
    archived.id = "archive-only".into();
    let archived_line = serde_json::to_string(&archived).unwrap();
    std::fs::write(
        dir.join("audit.previous.jsonl"),
        format!("{archived_line}\n{line}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("audit.jsonl"), format!("{line}\nmalformed")).unwrap();
    assert!(AppState::open(dir.clone()).is_err());
    let connection = rusqlite::Connection::open(dir.join("remote-desk.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='servers'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    drop(connection);
    std::fs::write(dir.join("audit.jsonl"), format!("{line}\n")).unwrap();
    let state = AppState::open(dir.clone()).unwrap();
    assert_eq!(
        state.server(&saved.id).unwrap().fingerprint,
        "SHA256:trusted"
    );
    assert_eq!(state.audit.lock().unwrap().len(), 2);
    assert!(state.audit_details("archive-only").is_ok());
    assert_eq!(std::fs::read(dir.join("servers.json")).unwrap(), config);
    state
        .log(
            &saved,
            "mcp",
            &Operation {
                kind: "exec".into(),
                ..Default::default()
            },
            &Ok(serde_json::json!({"stdout":"new result","stderr":"","exit_code":0})),
            0,
        )
        .unwrap();
    // Deleting imported rows must not resurrect them from the retained JSON backup.
    state.remove_server(&saved.id).unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    assert!(state.config.lock().unwrap().servers.is_empty());
    assert_eq!(state.audit.lock().unwrap().len(), 3);
    assert!(state
        .audit_details("legacy-audit")
        .unwrap()
        .response
        .is_none());
    // Corrupt old files after import are ignored; SQLite remains the source of persistence.
    std::fs::write(dir.join("servers.json"), "changed backup").unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    assert!(state.config.lock().unwrap().servers.is_empty());
    drop(state);
    cleanup(dir);
}

#[test]
fn sqlite_retains_audit_history_beyond_recent_list_and_after_server_removal() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    let op = Operation {
        kind: "test".into(),
        ..Default::default()
    };
    state
        .log(
            &saved,
            "desktop",
            &op,
            &Ok(serde_json::json!({"connected":true})),
            0,
        )
        .unwrap();
    let oldest = state.audit.lock().unwrap()[0].id.clone();
    for _ in 0..502 {
        state
            .log(
                &saved,
                "desktop",
                &op,
                &Ok(serde_json::json!({"connected":true})),
                0,
            )
            .unwrap();
    }
    assert_eq!(state.audit.lock().unwrap().len(), 500);
    state.remove_server(&saved.id).unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    assert!(state.config.lock().unwrap().servers.is_empty());
    assert_eq!(state.audit.lock().unwrap().len(), 500);
    assert!(state.audit_details(&oldest).unwrap().success);
    let all = persisted_audits(&dir);
    assert_eq!(all.len(), 503);
    drop(state);
    cleanup(dir);
}

#[test]
fn database_failure_does_not_update_visible_configuration_or_audit_cache() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    let connection = rusqlite::Connection::open(dir.join("remote-desk.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_server_update BEFORE UPDATE ON servers BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;
        CREATE TRIGGER reject_audit_insert BEFORE INSERT ON audit_entries BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;").unwrap();
    let mut changed = saved.clone();
    changed.name = "must not appear".into();
    assert!(state.save_server(changed, None).is_err());
    assert_eq!(state.server(&saved.id).unwrap().name, saved.name);
    assert!(state
        .log(
            &saved,
            "desktop",
            &Operation {
                kind: "test".into(),
                ..Default::default()
            },
            &Ok(serde_json::json!({"connected":true})),
            0
        )
        .is_err());
    assert!(state.audit.lock().unwrap().is_empty());
    drop(connection);
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    assert_eq!(state.server(&saved.id).unwrap().name, saved.name);
    assert!(state.audit.lock().unwrap().is_empty());
    drop(state);
    cleanup(dir);
}
