use remote_ssh_brige_lib::{
    model::{Operation, Server},
    state::{execute, AppState},
};
fn workspace() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("remote-ssh-brige-test-{}", uuid::Uuid::new_v4()))
}

fn cleanup(dir: std::path::PathBuf) {
    let target = dir.canonicalize().unwrap();
    let temp = std::env::temp_dir().canonicalize().unwrap();
    assert!(target.starts_with(&temp));
    assert!(target
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("remote-ssh-brige-test-"));
    std::fs::remove_dir_all(target).unwrap();
}

fn persisted_audits(dir: &std::path::Path) -> Vec<remote_ssh_brige_lib::model::Audit> {
    remote_ssh_brige_lib::database::Database::open(dir)
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
fn retired_preview_grant_is_removed_on_load_without_granting_text_reads() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    drop(state);
    let connection = rusqlite::Connection::open(dir.join("remote-desk.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE servers SET mcp_permissions_json=?1 WHERE id=?2",
            rusqlite::params![r#"["preview_file","terminal_read"]"#, saved.id],
        )
        .unwrap();
    drop(connection);
    let state = AppState::open(dir.clone()).unwrap();
    let loaded = state.server(&saved.id).unwrap();
    assert_eq!(loaded.mcp_permissions.as_ref().unwrap(), &["terminal_read"]);
    assert!(!loaded.permits_mcp("preview_file"));
    assert!(!loaded.permits_mcp("read_file"));
    assert!(loaded.permits_mcp("terminal_read"));
    state.save_server(loaded, None).unwrap();
    drop(state);
    cleanup(dir);
}

#[test]
fn granular_permissions_persist_and_invalid_grants_are_rejected() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    let mut updated = state.server(&saved.id).unwrap();
    updated.mcp_permissions = Some(vec!["read_file".into(), "terminal_read".into()]);
    state.save_server(updated, None).unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.server(&saved.id).unwrap();
    assert!(!saved.permits_mcp("upload"));
    assert!(!saved
        .mcp_permissions
        .as_ref()
        .unwrap()
        .iter()
        .any(|op| op == "upload"));
    assert!(saved.permits_mcp("read_file"));
    assert!(!saved.permits_mcp("exec"));
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
    for grant in ["invalid", "upload", "remove", "download", "mkdir", "rename"] {
        let mut invalid = saved.clone();
        invalid.mcp_permissions = Some(vec![grant.into()]);
        assert!(state.save_server(invalid, None).is_err());
    }
    let mut saved = state.server(&saved.id).unwrap();
    saved.mcp_enabled = false;
    let saved = state.save_server(saved, None).unwrap();
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    assert!(state
        .server(&saved.id)
        .unwrap()
        .effective_mcp_permissions()
        .is_empty());
    drop(state);
    cleanup(dir);
}

#[tokio::test]
async fn granular_denials_are_enforced_before_network_and_survive_live_updates() {
    use remote_ssh_brige_lib::model::MCP_OPERATIONS;
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
        assert!(
            error.to_string().contains("not authorized"),
            "{operation}: {error}"
        );
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
    assert!(error.to_string().contains("not authorized"));
    let audits = state.audit.lock().unwrap();
    assert!(audits.iter().all(|entry| !entry.success));
    assert!(audits.iter().any(|entry| entry.kind == "terminal_write"));
    drop(audits);
    drop(state);
    cleanup(dir);
}
#[test]
fn runtime_settings_persist_and_reject_invalid_updates() {
    use remote_ssh_brige_lib::model::RuntimeSettings;
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
        approval_timeout_secs: 15,
        command_policy: Some(remote_ssh_brige_lib::model::CommandPolicy {
            allow_pipes: true,
            allow_redirection: true,
            ..Default::default()
        }),
        upload_limit_mib: 123,
        download_limit_mib: 237,
        connect_timeout_secs: 12,
        io_timeout_secs: 60,
        command_timeout_secs: 90,
        exec_timeout_to_job: true,
        max_terminals: 4,
        ..Default::default()
    };
    let mut legacy_settings = serde_json::to_value(&settings).unwrap();
    legacy_settings
        .as_object_mut()
        .unwrap()
        .remove("approval_timeout_secs");
    assert_eq!(
        serde_json::from_value::<RuntimeSettings>(legacy_settings.clone())
            .unwrap()
            .approval_timeout_secs,
        120
    );
    legacy_settings
        .as_object_mut()
        .unwrap()
        .remove("exec_timeout_to_job");
    assert!(
        !serde_json::from_value::<RuntimeSettings>(legacy_settings.clone())
            .unwrap()
            .exec_timeout_to_job
    );
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
            approval_timeout_secs: 0,
            ..settings.clone()
        },
        RuntimeSettings {
            approval_timeout_secs: 121,
            ..settings.clone()
        },
        RuntimeSettings {
            upload_limit_mib: u64::MAX,
            ..settings.clone()
        },
        RuntimeSettings {
            download_limit_mib: u64::MAX,
            ..settings.clone()
        },
        RuntimeSettings {
            upload_limit_mib: remote_ssh_brige_lib::model::MAX_TRANSFER_MIB + 1,
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
    use remote_ssh_brige_lib::model::RuntimeSettings;
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
            if source == "mcp" && kind == "upload" {
                continue;
            } // upload remains desktop-only.
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
        assert!(result.unwrap_err().to_string().contains("not authorized"));
    }
    let logs = state.audit.lock().unwrap();
    assert_eq!(logs.len(), 5);
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
    let first = remote_ssh_brige_lib::mcp::start(state.clone(), port)
        .await
        .unwrap();
    assert!(first.running);
    assert_eq!(first.host, "127.0.0.1");
    assert_eq!(first.token.len(), 64);
    assert!(remote_ssh_brige_lib::mcp::start(state.clone(), port)
        .await
        .is_err());
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    client.write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 401"));
    assert!(response.ends_with("Missing or invalid MCP bearer token"));
    drop(client);
    remote_ssh_brige_lib::mcp::stop(state.clone())
        .await
        .unwrap();
    assert!(!state.mcp.lock().unwrap().running);
    assert!(
        remote_ssh_brige_lib::mcp::start_on(state.clone(), port, "not-an-ip")
            .await
            .is_err()
    );
    assert!(!state.mcp.lock().unwrap().running);
    let second = remote_ssh_brige_lib::mcp::start_on(state.clone(), port, "0.0.0.0")
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
    remote_ssh_brige_lib::mcp::stop(state.clone())
        .await
        .unwrap();
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
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
fn audit_persists_output_without_polling_payload() {
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
    let mut edit_op = op.clone();
    edit_op.kind = "edit_file".into();
    state
        .log(
            &saved,
            "mcp",
            &edit_op,
            &Ok(json!({"diff":"private-file-body","applied_edits":1,"written_bytes":10})),
            0,
        )
        .unwrap();
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
            .data["message"],
        "connection failed"
    );
    drop(state);
    cleanup(dir);
}

#[test]
fn json_workspaces_are_rejected_without_creating_a_database_or_changing_files() {
    for filename in ["servers.json", "audit.previous.jsonl", "audit.jsonl"] {
        for existing_empty_database in [false, true] {
            let dir = workspace();
            std::fs::create_dir_all(&dir).unwrap();
            let database_path = dir.join("remote-desk.sqlite3");
            if existing_empty_database {
                std::fs::write(&database_path, []).unwrap();
            }
            let path = dir.join(filename);
            let bytes = b"retained old workspace";
            std::fs::write(&path, bytes).unwrap();
            let error = AppState::open(dir.clone()).err().unwrap();
            assert!(error.to_string().contains("JSON/JSONL"), "{error}");
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            if existing_empty_database {
                assert!(std::fs::read(&database_path).unwrap().is_empty());
            } else {
                assert!(!database_path.exists());
            }
            cleanup(dir);
        }
    }
}

#[test]
fn unsupported_database_versions_are_rejected_without_modifying_the_database() {
    for version in [0, 1, 3] {
        let dir = workspace();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("remote-desk.sqlite3");
        let database = rusqlite::Connection::open(&path).unwrap();
        database
            .execute_batch(
                "CREATE TABLE retained (value TEXT); INSERT INTO retained VALUES ('keep');",
            )
            .unwrap();
        database
            .pragma_update(None, "user_version", version)
            .unwrap();
        drop(database);
        let bytes = std::fs::read(&path).unwrap();
        assert!(AppState::open(dir.clone()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        cleanup(dir);
    }
}

#[test]
fn current_sqlite_workspace_ignores_retained_json_files() {
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    drop(state);
    for filename in ["servers.json", "audit.previous.jsonl", "audit.jsonl"] {
        std::fs::write(dir.join(filename), "unused backup").unwrap();
    }
    let state = AppState::open(dir.clone()).unwrap();
    assert_eq!(state.server(&saved.id).unwrap().host, saved.host);
    assert!(state.audit.lock().unwrap().is_empty());
    state.remove_server(&saved.id).unwrap();
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
    use remote_ssh_brige_lib::database::AuditQuery;
    let mut query = AuditQuery {
        category: "all".into(),
        limit: 50,
        ..Default::default()
    };
    let first = state.audit_page(&query).unwrap();
    assert_eq!(first.total, 503);
    assert_eq!(first.entries.len(), 50);
    assert!(first.entries.iter().all(|entry| entry.response.is_none()));
    query.upper = Some(first.upper);
    query.offset = 500;
    let last = state.audit_page(&query).unwrap();
    assert_eq!(last.entries.len(), 3);
    assert_eq!(last.entries.last().unwrap().id, oldest);
    let output = dir.join("export.json");
    state.export_audits(&query, &output).unwrap();
    let exported: Vec<remote_ssh_brige_lib::model::Audit> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(exported.len(), 503); // Export ignores the visible page's offset/limit.
    assert!(exported.iter().all(|entry| entry.response.is_some()));
    assert!(state.snapshot_with_audits(false)["audits"]
        .as_array()
        .unwrap()
        .is_empty());
    drop(state);
    cleanup(dir);
}

#[test]
fn audit_pages_filter_globally_and_stay_stable_when_records_arrive() {
    use remote_ssh_brige_lib::database::AuditQuery;
    let dir = workspace();
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.save_server(server(), None).unwrap();
    for index in 0..120 {
        state
            .log(
                &saved,
                if index % 2 == 0 { "mcp" } else { "desktop" },
                &Operation {
                    kind: if index % 3 == 0 { "exec" } else { "download" }.into(),
                    command: format!("echo Needle_{index}"),
                    path: format!("/Needle_{index}"),
                    ..Default::default()
                },
                &Ok(serde_json::json!({"exit_code": if index % 4 == 0 { 1 } else { 0 }})),
                1,
            )
            .unwrap();
    }
    let mut query = AuditQuery {
        category: "all".into(),
        limit: 50,
        ..Default::default()
    };
    let first = state.audit_page(&query).unwrap();
    assert_eq!(first.total, 120);
    state
        .log(
            &saved,
            "desktop",
            &Operation {
                kind: "security_reject".into(),
                ..Default::default()
            },
            &Ok(serde_json::json!({})),
            0,
        )
        .unwrap();
    query.upper = Some(first.upper);
    query.offset = 50;
    let second = state.audit_page(&query).unwrap();
    assert_eq!(second.total, 120);
    assert!(second
        .entries
        .iter()
        .all(|entry| !first.entries.iter().any(|previous| previous.id == entry.id)));
    query.offset = 0;
    query.upper = None;
    query.category = "error".into();
    assert_eq!(state.audit_page(&query).unwrap().total, 31);
    query.category = "exec".into();
    assert_eq!(state.audit_page(&query).unwrap().total, 40);
    query.category = "files".into();
    assert_eq!(state.audit_page(&query).unwrap().total, 80);
    query.category = "mcp".into();
    assert_eq!(state.audit_page(&query).unwrap().total, 60);
    query.category = "all".into();
    query.search = "needle_119".into();
    assert_eq!(state.audit_page(&query).unwrap().total, 1);
    query.search = "%".into();
    assert_eq!(state.audit_page(&query).unwrap().total, 0); // Search characters are literal.
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
