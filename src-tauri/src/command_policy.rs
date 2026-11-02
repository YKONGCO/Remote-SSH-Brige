//! Static shell policy checks. This is deliberately not an execution sandbox:
//! external scripts, interpreter code and remote executable contents are opaque.
use crate::model::{CommandPolicy, RuntimeSettings};
use anyhow::{ensure, Context, Result};
use tree_sitter::{Node, Parser};

pub fn check(command: &str, settings: &RuntimeSettings) -> Result<()> {
    let policy = settings.effective_command_policy();
    policy.validate()?;
    ensure!(
        policy.execution_mode.as_deref() != Some("disabled"),
        "Command execution is disabled"
    );
    inspect(command, &policy, settings.block_dangerous_commands, 0)
}

fn inspect(
    source: &str,
    policy: &CommandPolicy,
    block_dangerous: bool,
    nesting: usize,
) -> Result<()> {
    ensure!(nesting <= 8, "Script nesting exceeds 8 levels");
    ensure!(!source.trim().is_empty(), "command must not be empty");
    ensure!(
        source.len() <= 16384
            && !source
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
        "command exceeds 16384 bytes or contains unsupported control characters"
    );
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_bash::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("Cannot parse shell command")?;
    ensure!(
        !tree.root_node().has_error(),
        "Invalid or incomplete shell syntax"
    );
    let mut pending = vec![(tree.root_node(), 0)];
    while let Some((node, depth)) = pending.pop() {
        ensure!(depth <= 128, "Shell syntax nesting exceeds 128 levels");
        match node.kind() {
            "pipeline" => ensure!(policy.allow_pipes, "Pipes are not allowed"),
            "file_redirect" | "heredoc_redirect" | "herestring_redirect" => {
                ensure!(policy.allow_redirection, "Redirection is not allowed")
            }
            "command_substitution" | "process_substitution" => {
                ensure!(
                    policy.allow_substitution,
                    "Command substitution is not allowed"
                )
            }
            "list" | "subshell" | "compound_statement" => {
                ensure!(policy.allow_compound, "Compound commands are not allowed")
            }
            "function_definition"
            | "if_statement"
            | "for_statement"
            | "c_style_for_statement"
            | "while_statement"
            | "case_statement"
            | "variable_assignment"
            | "variable_assignments"
            | "declaration_command"
            | "unset_command"
            | "test_command"
            | "expansion"
            | "simple_expansion"
            | "arithmetic_expansion"
            | "brace_expression" => {
                ensure!(
                    policy.allow_scripts
                        && (!policy.diagnostic_only || policy.execution_mode.is_some()),
                    "Scripts, variables and control structures are not allowed"
                );
            }
            "command" => inspect_command(node, source, policy, block_dangerous, nesting)?,
            ";" | "&" | "&&" | "||" => {
                ensure!(policy.allow_compound, "Compound commands are not allowed")
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();
        // Newline-separated statements have no separator node in the syntax tree.
        if matches!(
            node.kind(),
            "program" | "command_substitution" | "process_substitution"
        ) {
            let count = children
                .iter()
                .filter(|child| child.is_named() && child.kind() != "comment")
                .count();
            ensure!(
                count <= 1 || policy.allow_compound,
                "Compound or multiline commands are not allowed"
            );
        }
        pending.extend(children.into_iter().map(|child| (child, depth + 1)));
    }
    Ok(())
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn has_dynamic(node: Node<'_>) -> bool {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        if matches!(
            node.kind(),
            "expansion"
                | "simple_expansion"
                | "command_substitution"
                | "process_substitution"
                | "ansi_c_string"
                | "translated_string"
                | "brace_expression"
        ) {
            return true;
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    false
}

fn inspect_command(
    node: Node<'_>,
    source: &str,
    policy: &CommandPolicy,
    block_dangerous: bool,
    nesting: usize,
) -> Result<()> {
    let name = node
        .child_by_field_name("name")
        .context("Cannot determine command name")?;
    if has_dynamic(name) {
        ensure!(
            policy.allow_scripts
                && !policy.diagnostic_only
                && !block_dangerous
                && policy.execution_mode.as_deref() != Some("whitelist")
                && policy.blocked_commands.is_empty(),
            "Dynamic command names are not allowed by the current policy"
        );
    }
    let mut words = shlex::split(text(name, source)).context("Cannot parse command name")?;
    let mut cursor = node.walk();
    for argument in node.children_by_field_name("argument", &mut cursor) {
        // Keep expansions as one argument; nested executable nodes are checked separately.
        let raw = text(argument, source);
        let value = shlex::split(raw)
            .filter(|words| words.len() == 1)
            .map(|mut words| words.remove(0))
            .unwrap_or_else(|| raw.to_string());
        words.push(value);
    }
    if policy.diagnostic_only && policy.execution_mode.is_none() {
        crate::security::check_diagnostic_words(&words)?;
    }
    inspect_words(&words, policy, block_dangerous, nesting)
}

fn basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

pub(crate) fn trusted_executable(name: &str) -> bool {
    !name.contains('/')
        || ["/bin/", "/usr/bin/", "/sbin/", "/usr/sbin/"]
            .iter()
            .any(|prefix| {
                name.strip_prefix(prefix)
                    .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
            })
}

fn inspect_words(
    words: &[String],
    policy: &CommandPolicy,
    block_dangerous: bool,
    nesting: usize,
) -> Result<()> {
    ensure!(nesting <= 8, "Command wrapper nesting exceeds 8 levels");
    let Some(first) = words.first() else {
        return Ok(());
    };
    let name = basename(first);
    let args = &words[1..];
    ensure!(
        !policy
            .blocked_commands
            .iter()
            .any(|rule| if rule.contains('/') {
                rule == first
            } else {
                rule == name
            }),
        "Command '{name}' is blocked by the denylist"
    );
    if policy.execution_mode.as_deref() == Some("whitelist") {
        let explicitly_allowed = policy.allowed_commands.iter().any(|rule| rule == first);
        ensure!(
            explicitly_allowed || trusted_executable(first),
            "Executable path is outside trusted directories: {first}"
        );
        ensure!(
            policy
                .allowed_commands
                .iter()
                .any(|rule| rule == first || (!rule.contains('/') && rule == name)),
            "Command '{name}' is not on the allowlist"
        );
        // Preserve parameter restrictions for built-in diagnostic commands.
        if crate::security::DIAGNOSTIC_COMMANDS.contains(&name) {
            let mut diagnostic = words.to_vec();
            diagnostic[0] = name.to_owned();
            crate::security::check_diagnostic_words(&diagnostic)?;
        }
    }
    ensure!(
        !(block_dangerous
            || policy.diagnostic_only
            || policy.execution_mode.as_deref() == Some("whitelist")
            || !policy.blocked_commands.is_empty())
            || !first.chars().any(|c| "$`*?[]{}".contains(c)),
        "Dynamic command names are not allowed by the current policy"
    );
    if block_dangerous {
        let dangerous = [
            "rm", "unlink", "rmdir", "shred", "truncate", "dd", "mkfs", "mkswap", "wipefs",
            "fdisk", "sfdisk", "cfdisk", "parted", "gdisk", "sgdisk", "shutdown", "reboot", "halt",
            "poweroff", "init", "telinit", "kill", "killall", "pkill",
        ];
        ensure!(
            !dangerous.contains(&name) && !name.starts_with("mkfs."),
            "Dangerous command '{name}' is blocked"
        );
        ensure!(
            name != "find"
                || !args.iter().any(|arg| matches!(
                    arg.as_str(),
                    "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
                )),
            "find delete and execute actions are blocked"
        );
        ensure!(
            name != "systemctl"
                || !args.iter().any(|arg| matches!(
                    arg.as_str(),
                    "reboot" | "poweroff" | "halt" | "rescue" | "emergency"
                )),
            "System power operations are blocked"
        );
    }
    let shell = ["sh", "bash", "dash", "zsh", "ksh", "ash", "fish"].contains(&name);
    let interpreter = shell
        || [
            "eval",
            "source",
            ".",
            "perl",
            "ruby",
            "node",
            "nodejs",
            "php",
            "lua",
            "awk",
            "gawk",
            "mawk",
            "sed",
            "pwsh",
            "powershell",
        ]
        .contains(&name)
        || name.starts_with("python");
    let script_file = [
        ".sh", ".bash", ".py", ".pl", ".rb", ".js", ".mjs", ".php", ".lua", ".ps1",
    ]
    .iter()
    .any(|extension| name.ends_with(extension));
    let custom_path = !trusted_executable(first);
    ensure!(
        policy.allow_scripts || !(interpreter || script_file || custom_path),
        "Scripts, interpreters and custom executable paths are not allowed"
    );
    if shell {
        if let Some(index) = args
            .iter()
            .position(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
        {
            let code = args
                .get(index + 1)
                .context("Shell -c requires script content")?;
            inspect(code, policy, block_dangerous, nesting + 1)?;
        }
    } else if name == "eval" {
        inspect(&args.join(" "), policy, block_dangerous, nesting + 1)?;
    }
    // Resolve common command wrappers so they cannot hide a directly destructive command.
    if [
        "sudo", "doas", "env", "command", "builtin", "nohup", "timeout", "nice", "stdbuf", "exec",
        "xargs", "busybox",
    ]
    .contains(&name)
    {
        if name == "env" {
            for arg in args {
                if let Some(code) = arg
                    .strip_prefix("--split-string=")
                    .or_else(|| arg.strip_prefix("-S").filter(|code| !code.is_empty()))
                {
                    inspect(code, policy, block_dangerous, nesting + 1)?;
                }
            }
            if let Some(index) = args
                .iter()
                .position(|arg| arg == "-S" || arg == "--split-string")
            {
                if let Some(code) = args.get(index + 1) {
                    inspect(code, policy, block_dangerous, nesting + 1)?;
                }
            }
        }
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if arg == "--" {
                index += 1;
                break;
            }
            if arg.starts_with('-') {
                let takes_value = match name {
                    "sudo" => [
                        "-u", "-g", "-h", "-p", "-C", "-T", "-R", "-D", "--user", "--group",
                        "--host", "--prompt", "--chdir",
                    ]
                    .contains(&arg.as_str()),
                    "doas" => ["-u", "-C"].contains(&arg.as_str()),
                    "env" => ["-u", "-C", "-S", "--unset", "--chdir", "--split-string"]
                        .contains(&arg.as_str()),
                    "xargs" => [
                        "-n",
                        "-P",
                        "-I",
                        "-L",
                        "-a",
                        "-E",
                        "-d",
                        "-s",
                        "--max-args",
                        "--max-procs",
                        "--replace",
                        "--max-lines",
                        "--arg-file",
                        "--delimiter",
                    ]
                    .contains(&arg.as_str()),
                    "nice" => ["-n", "--adjustment"].contains(&arg.as_str()),
                    "stdbuf" => {
                        ["-i", "-o", "-e", "--input", "--output", "--error"].contains(&arg.as_str())
                    }
                    "timeout" => ["-k", "-s", "--kill-after", "--signal"].contains(&arg.as_str()),
                    "exec" => arg == "-a",
                    _ => false,
                };
                index += if takes_value { 2 } else { 1 };
            } else if name == "env" && arg.contains('=') {
                index += 1;
            } else {
                break;
            }
        }
        if name == "timeout" {
            index += 1;
        }
        if index < args.len() {
            if name == "sudo"
                && args.iter().any(|arg| {
                    arg == "--shell"
                        || arg == "--login"
                        || (arg.starts_with('-')
                            && !arg.starts_with("--")
                            && (arg.contains('s') || arg.contains('i')))
                })
            {
                ensure!(policy.allow_scripts, "sudo shell mode is not allowed");
                inspect(
                    &args[index..].join(" "),
                    policy,
                    block_dangerous,
                    nesting + 1,
                )?;
            }
            inspect_words(&args[index..], policy, block_dangerous, nesting + 1)?;
        }
    }
    Ok(())
}
