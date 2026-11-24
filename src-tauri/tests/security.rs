use remote_ssh_brige_lib::{
    command_policy::{assess, Decision},
    model::{migrate_settings_value, CommandPolicy, Operation, RuntimeSettings, Server},
    security::{self, ApprovalQueue},
    state::{self, AppState},
};
use serde_json::json;
use std::sync::Arc;

fn settings_with(policy: CommandPolicy) -> RuntimeSettings {
    RuntimeSettings {
        command_policy: Some(policy),
        ..Default::default()
    }
}

fn decision(command: &str, policy: CommandPolicy) -> Decision {
    assess(command, &settings_with(policy)).decision
}

fn error_code(error: &anyhow::Error) -> &'static str {
    remote_ssh_brige_lib::file_tools::error_code(error)
}

#[test]
fn ordinary_commands_run_without_an_allowlist() {
    let policy = CommandPolicy::default();
    for command in [
        "ls -la /tmp",
        "pwd",
        "cat /etc/os-release | head",
        "git status && git diff",
        "mkdir -p /tmp/work",
        "docker compose ps",
        "python --version",
        "python3 -u script.py",
        "node -e 'console.log(1)'",
        "./deploy.sh",
        "source ./profile.sh",
        "bash -c 'ls -la'",
        "echo $(date +%s)",
    ] {
        assert_eq!(
            decision(command, policy.clone()),
            Decision::Allow,
            "{command}"
        );
    }
}

#[test]
fn builtin_protection_requires_approval_and_user_rules_only_add_protection() {
    let policy = CommandPolicy {
        protected_commands: vec!["curl".into()],
        ..Default::default()
    };
    for command in [
        "rm -rf /tmp/x",
        "sudo rm -rf /",
        "/bin/rm -f file",
        "find . -name x -delete",
        "find . -exec rm {} ;",
        "systemctl reboot",
        "dd if=/dev/zero of=/dev/sda",
        "mkfs.ext4 /dev/sdb1",
        "bash -c 'rm -rf /tmp/x'",
        "echo $(rm -rf /tmp/x)",
        "curl https://example.com",
    ] {
        assert_eq!(
            decision(command, policy.clone()),
            Decision::RequireApproval,
            "{command}"
        );
    }

    let assessment = assess("rm -rf /tmp/x", &settings_with(policy));
    assert!(assessment
        .matched_rules
        .iter()
        .any(|rule| rule.source == "builtin"));
    assert_eq!(assessment.next_action, Some("exec_approved"));
    assert!(assessment.risk == Some("high"));
}

#[test]
fn dynamic_execution_and_possible_rule_matches_require_approval() {
    let policy = CommandPolicy {
        protected_commands: vec!["docker rmi".into()],
        ..Default::default()
    };
    for command in [
        "$CMD",
        "sh -c \"$CMD\"",
        "docker \"$ACTION\"",
        "systemctl \"$ACTION\"",
    ] {
        let assessment = assess(command, &settings_with(policy.clone()));
        assert_eq!(assessment.decision, Decision::RequireApproval, "{command}");
        assert!(assessment.uncertain, "{command}");
    }
}

#[test]
fn base_policy_denials_cannot_be_overridden_by_approval() {
    let pipes_off = CommandPolicy {
        allow_pipes: false,
        ..Default::default()
    };
    assert_eq!(decision("ls | grep x", pipes_off), Decision::Deny);
    let scripts_off = CommandPolicy {
        allow_scripts: false,
        ..Default::default()
    };
    assert_eq!(decision("python script.py", scripts_off), Decision::Deny);
    assert_eq!(
        decision("ls 'unterminated", CommandPolicy::default()),
        Decision::Deny
    );
    assert_eq!(
        decision(
            "ls",
            CommandPolicy {
                exec_disabled: true,
                ..Default::default()
            }
        ),
        Decision::Deny
    );
}

#[test]
fn old_settings_migrate_without_silently_loosening_command_limits() {
    let migrated = migrate_settings_value(json!({
        "block_dangerous_commands": true,
        "approval_mode": "mcp",
        "command_policy": {
            "execution_mode": "whitelist",
            "allowed_commands": ["ls", "date"],
            "blocked_commands": ["curl"],
            "allow_advanced_shell": true
        }
    }))
    .unwrap();
    let settings: RuntimeSettings = serde_json::from_value(migrated).unwrap();
    assert_eq!(
        settings.settings_version,
        remote_ssh_brige_lib::model::SETTINGS_VERSION
    );
    let policy = settings.effective_command_policy();
    assert_eq!(
        policy.legacy_whitelist,
        Some(vec!["ls".to_string(), "date".to_string()])
    );
    assert_eq!(policy.protected_commands, vec!["curl".to_string()]);
    assert!(policy.allow_pipes);

    assert_eq!(assess("ls -la", &settings).decision, Decision::Allow);
    assert_eq!(
        assess("cat /etc/passwd", &settings).decision,
        Decision::RequireApproval
    );
    assert_eq!(
        assess("date -s tomorrow", &settings).decision,
        Decision::RequireApproval
    );

    assert_eq!(
        assess("rm -rf /tmp/x", &settings).decision,
        Decision::RequireApproval
    );

    let default_old = migrate_settings_value(json!({"block_dangerous_commands": true})).unwrap();
    let settings: RuntimeSettings = serde_json::from_value(default_old).unwrap();
    let policy = settings.effective_command_policy();
    assert!(policy
        .legacy_whitelist
        .as_ref()
        .unwrap()
        .contains(&"ls".to_string()));
    assert!(!policy.allow_pipes);
    assert_eq!(assess("ls -la", &settings).decision, Decision::Allow);
    assert_eq!(
        assess("curl https://example.com", &settings).decision,
        Decision::RequireApproval
    );

    let relaxed = migrate_settings_value(json!({"block_dangerous_commands": false})).unwrap();
    let settings: RuntimeSettings = serde_json::from_value(relaxed).unwrap();
    let policy = settings.effective_command_policy();
    assert!(policy.legacy_whitelist.is_none());
    assert!(policy.allow_pipes);
    assert!(!settings.advanced_mcp_tools.terminal);
    assert!(settings.advanced_mcp_tools.port_forward);

    let legacy_terminal = migrate_settings_value(json!({
        "settings_version": 2,
        "command_policy": {
            "protected_commands": [],
            "allow_mcp_terminal": true
        }
    }))
    .unwrap();
    let settings: RuntimeSettings = serde_json::from_value(legacy_terminal).unwrap();
    assert!(!settings.advanced_mcp_tools.terminal);
    assert!(!serde_json::to_value(settings.effective_command_policy())
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("allow_mcp_terminal"));
}

#[test]
fn advanced_mcp_tools_default_to_port_forward_only() {
    let tools = remote_ssh_brige_lib::model::AdvancedMcpTools::default();
    assert!(!tools.enabled("terminal"));
    assert!(tools.enabled("port_forward"));
    assert!(tools.enabled("exec"));
    let custom = remote_ssh_brige_lib::model::AdvancedMcpTools {
        terminal: true,
        port_forward: false,
    };
    assert!(custom.enabled("terminal"));
    assert!(!custom.enabled("port_forward"));
}

#[test]
fn approval_mode_defaults_on_and_survives_old_settings() {
    assert!(RuntimeSettings::default().approval_mode);
    let migrated = migrate_settings_value(json!({ "settings_version": 3 })).unwrap();
    let settings: RuntimeSettings = serde_json::from_value(migrated).unwrap();
    assert!(settings.approval_mode);
    let migrated = migrate_settings_value(json!({
        "settings_version": 3,
        "approval_mode": false
    }))
    .unwrap();
    let settings: RuntimeSettings = serde_json::from_value(migrated).unwrap();
    assert!(!settings.approval_mode);
}

#[test]
fn invalid_migrated_rule_lists_are_rejected() {
    let mut policy = CommandPolicy::default();
    policy.protected_commands.push("bad:name".into());
    assert!(policy.validate().is_err());
    let mut policy = CommandPolicy::default();
    policy.legacy_whitelist = Some(vec!["../escape".into()]);
    assert!(policy.validate().is_err());
}

#[test]
fn literal_combinations_are_valid_only_in_protected_rules() {
    let policy = CommandPolicy {
        protected_commands: vec!["docker\trmi".into(), "/opt/deploy --target=prod".into()],
        ..Default::default()
    };
    assert!(policy.validate().is_ok());
    for invalid in ["docker && rmi", "docker *", "docker\nrmi", "docker $ACTION"] {
        assert!(CommandPolicy {
            protected_commands: vec![invalid.into()],
            ..policy.clone()
        }
        .validate()
        .is_err());
    }
    assert!(CommandPolicy {
        legacy_whitelist: Some(vec!["docker rmi".into()]),
        ..policy
    }
    .validate()
    .is_err());
}

#[test]
fn base_checks_keep_read_only_deletion_and_terminal_limits() {
    let mut settings = RuntimeSettings::default();
    let remove = Operation {
        kind: "remove".into(),
        ..Default::default()
    };
    assert_eq!(
        error_code(&security::check(&settings, &remove, "mcp").unwrap_err()),
        "DELETION_PROTECTED"
    );
    settings.deletion_protection = false;
    assert!(security::check(&settings, &remove, "mcp").is_ok());
    let terminal = Operation {
        kind: "terminal_write".into(),
        content: "rm -rf /".into(),
        ..Default::default()
    };
    assert!(security::check(&settings, &terminal, "mcp").is_ok());
    assert!(security::check(&settings, &terminal, "desktop").is_ok());
    settings.remote_read_only = true;
    assert_eq!(
        error_code(&security::check(&settings, &terminal, "desktop").unwrap_err()),
        "READ_ONLY_POLICY"
    );
    for kind in [
        "list_files",
        "read_file",
        "download",
        "terminal_read",
        "terminal_close",
    ] {
        assert!(security::check(
            &settings,
            &Operation {
                kind: kind.into(),
                ..Default::default()
            },
            "desktop"
        )
        .is_ok());
    }
    let mut disabled = RuntimeSettings::default();
    disabled.command_policy = Some(CommandPolicy {
        exec_disabled: true,
        ..Default::default()
    });
    let exec = Operation {
        kind: "exec".into(),
        command: "ls".into(),
        ..Default::default()
    };
    assert_eq!(
        error_code(&security::check(&disabled, &exec, "mcp").unwrap_err()),
        "EXEC_DISABLED"
    );
}

fn builtin_assessment() -> remote_ssh_brige_lib::command_policy::Assessment {
    assess("rm -rf /tmp/x", &RuntimeSettings::default())
}

async fn next_pending(queue: &ApprovalQueue) -> security::ApprovalView {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(item) = queue.list().into_iter().next() {
                break item;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn approval_is_single_use_cancellable_and_fails_closed_when_journaling_fails() {
    let queue = Arc::new(ApprovalQueue::default());
    let worker = queue.clone();
    let task = tokio::spawn(async move {
        worker
            .request(
                &Server::default(),
                &Operation {
                    kind: "exec".into(),
                    command: "rm -rf /tmp/x".into(),
                    ..Default::default()
                },
                &builtin_assessment(),
                "mcp",
                30,
            )
            .await
    });
    let pending = next_pending(&queue).await;
    assert_eq!(pending.command, "rm -rf /tmp/x");
    assert_eq!(pending.kind, "exec");
    assert!(!pending.matched_rules.is_empty());
    assert_eq!(pending.command_sha256.len(), 64);
    assert!(queue
        .decide(&pending.id, true, |_| anyhow::bail!("audit unavailable"))
        .is_err());
    assert!(!task.is_finished());
    queue.decide(&pending.id, false, |_| Ok(())).unwrap();
    assert_eq!(
        error_code(&task.await.unwrap().unwrap_err()),
        "APPROVAL_REJECTED"
    );
    assert!(queue.decide(&pending.id, true, |_| Ok(())).is_err());
    assert!(queue.list().is_empty());
}

#[tokio::test]
async fn approval_timeout_uses_the_configured_deadline_and_removes_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path().to_path_buf()).unwrap();
    state
        .save_settings(RuntimeSettings {
            approval_timeout_secs: 1,
            ..Default::default()
        })
        .unwrap();
    let server = state
        .save_server(
            Server {
                name: "timeout".into(),
                host: "127.0.0.1".into(),
                port: 1,
                auth: "agent".into(),
                mcp_permissions: Some(vec!["exec".into()]),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let task = tokio::spawn(state::execute(
        state.clone(),
        Operation {
            kind: "exec".into(),
            command: "rm -rf /tmp/x".into(),
            server_id: server.id,
            approval_request: true,
            ..Default::default()
        },
        "mcp",
    ));
    let pending = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        next_pending(&state.approvals),
    )
    .await
    .unwrap();
    let created = chrono::DateTime::parse_from_rfc3339(&pending.created_at).unwrap();
    let expires = chrono::DateTime::parse_from_rfc3339(&pending.expires_at).unwrap();
    assert!((expires - created).num_milliseconds() <= 1100);
    let error = tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error_code(&error), "APPROVAL_TIMEOUT");
    let details = remote_ssh_brige_lib::file_tools::error_response(&error, &Operation::default());
    assert_eq!(details["details"]["timeout_secs"], 1);
    assert!(state.approvals.list().is_empty());
}

#[tokio::test]
async fn disabled_approval_mode_denies_protected_commands_without_queueing() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path().to_path_buf()).unwrap();
    state
        .save_settings(RuntimeSettings {
            approval_mode: false,
            ..Default::default()
        })
        .unwrap();
    let server = state
        .save_server(
            Server {
                name: "no-approval".into(),
                host: "127.0.0.1".into(),
                port: 1,
                auth: "agent".into(),
                mcp_permissions: Some(vec!["exec".into()]),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    for approval_request in [false, true] {
        let error = state::execute(
            state.clone(),
            Operation {
                kind: "exec".into(),
                command: "rm -rf /tmp/x".into(),
                server_id: server.id.clone(),
                approval_request,
                ..Default::default()
            },
            "mcp",
        )
        .await
        .unwrap_err();
        assert_eq!(error_code(&error), "APPROVAL_DISABLED");
    }
    assert!(state.approvals.list().is_empty());
}

#[tokio::test]
async fn plain_exec_reports_approval_required_without_connecting() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path().to_path_buf()).unwrap();
    let server = state
        .save_server(
            Server {
                name: "protected".into(),
                host: "127.0.0.1".into(),
                port: 1,
                auth: "agent".into(),
                mcp_permissions: Some(vec!["exec".into()]),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let error = state::execute(
        state.clone(),
        Operation {
            kind: "exec".into(),
            command: "rm -rf /tmp/x".into(),
            server_id: server.id,
            ..Default::default()
        },
        "mcp",
    )
    .await
    .unwrap_err();
    assert_eq!(error_code(&error), "APPROVAL_REQUIRED");
    let response = remote_ssh_brige_lib::file_tools::error_response(&error, &Operation::default());
    assert_eq!(response["details"]["next_tool"], "exec_approved");
    assert!(response["details"]["matched_rules"]
        .as_array()
        .is_some_and(|rules| !rules.is_empty()));
    assert!(state.approvals.list().is_empty());
}

#[tokio::test]
async fn approved_command_runs_once_and_rejection_never_connects() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path().to_path_buf()).unwrap();
    let server = state
        .save_server(
            Server {
                name: "Review host".into(),
                host: "127.0.0.1".into(),
                port: 1,
                auth: "agent".into(),
                mcp_permissions: Some(vec!["exec".into(), "job".into()]),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let op = Operation {
        kind: "exec".into(),
        command: "rm -rf /tmp/review".into(),
        server_id: server.id.clone(),
        approval_request: true,
        ..Default::default()
    };
    let task = tokio::spawn(state::execute(state.clone(), op.clone(), "mcp"));
    let pending = next_pending(&state.approvals).await;
    assert_eq!(pending.command, "rm -rf /tmp/review");
    assert!(pending
        .matched_rules
        .iter()
        .any(|rule| rule.source == "builtin"));
    state.resolve_approval(&pending.id, false).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error_code(&error), "APPROVAL_REJECTED");
    assert!(state.approvals.list().is_empty());

    let task = tokio::spawn(state::execute(state.clone(), op.clone(), "mcp"));
    let pending = next_pending(&state.approvals).await;
    state.resolve_approval(&pending.id, true).unwrap();

    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error_code(&error), "SSH_CONNECT_FAILED");
    assert!(state.approvals.list().is_empty());

    let decision = state
        .audit
        .lock()
        .unwrap()
        .iter()
        .find(|entry| entry.kind == "security_approve")
        .cloned()
        .unwrap();
    let response = state.audit_details(&decision.id).unwrap().response.unwrap();
    let metadata = &response.data["approval"];
    assert_eq!(metadata["request_id"], pending.id);
    assert_eq!(metadata["command"], "rm -rf /tmp/review");
    assert_eq!(metadata["status"], "approved");
    assert!(metadata["matched_rules"]
        .as_array()
        .is_some_and(|rules| !rules.is_empty()));
    assert_eq!(metadata["command_sha256"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn changed_server_configuration_invalidates_an_approved_request() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path().to_path_buf()).unwrap();
    let server = state
        .save_server(
            Server {
                name: "changed".into(),
                host: "127.0.0.1".into(),
                port: 1,
                auth: "agent".into(),
                mcp_permissions: Some(vec!["exec".into()]),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let task = tokio::spawn(state::execute(
        state.clone(),
        Operation {
            kind: "exec".into(),
            command: "rm -rf /tmp/x".into(),
            server_id: server.id.clone(),
            approval_request: true,
            ..Default::default()
        },
        "mcp",
    ));
    let pending = next_pending(&state.approvals).await;
    let mut changed = server;
    changed.name = "renamed".into();
    state.save_server(changed, None).unwrap();
    state.resolve_approval(&pending.id, true).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error_code(&error), "SOURCE_CHANGED");
    assert!(state.approvals.list().is_empty());
}
