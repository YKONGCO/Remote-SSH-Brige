//! Static shell policy checks. This is deliberately not an execution sandbox:
//! external scripts, interpreter code and remote executable contents are opaque.
use crate::model::{CommandPolicy, RuntimeSettings};
use anyhow::{ensure, Context, Result};
use tree_sitter::{Node, Parser};

pub fn check(command: &str, settings: &RuntimeSettings) -> Result<()> {
    inspect(
        command,
        &settings.effective_command_policy(),
        settings.block_dangerous_commands,
        0,
    )
}

fn inspect(
    source: &str,
    policy: &CommandPolicy,
    block_dangerous: bool,
    nesting: usize,
) -> Result<()> {
    ensure!(nesting <= 8, "命令保护：嵌套脚本过深");
    ensure!(!source.trim().is_empty(), "命令不能为空");
    ensure!(
        source.len() <= 16384
            && !source
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
        "命令保护：命令过长或包含不支持的控制字符"
    );
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_bash::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("命令保护：无法解析 Shell 命令")?;
    ensure!(
        !tree.root_node().has_error(),
        "命令保护：Shell 语法不完整或无法安全解析"
    );
    let mut pending = vec![(tree.root_node(), 0)];
    while let Some((node, depth)) = pending.pop() {
        ensure!(depth <= 128, "命令保护：Shell 语法嵌套过深");
        match node.kind() {
            "pipeline" => ensure!(policy.allow_pipes, "命令保护：未允许管道"),
            "file_redirect" | "heredoc_redirect" | "herestring_redirect" => {
                ensure!(policy.allow_redirection, "命令保护：未允许重定向")
            }
            "command_substitution" | "process_substitution" => {
                ensure!(policy.allow_substitution, "命令保护：未允许命令替换")
            }
            "list" | "subshell" | "compound_statement" => {
                ensure!(policy.allow_compound, "命令保护：未允许组合命令")
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
                    policy.allow_scripts && !policy.diagnostic_only,
                    "命令保护：未允许脚本、变量或控制结构"
                );
            }
            "command" => inspect_command(node, source, policy, block_dangerous, nesting)?,
            ";" | "&" | "&&" | "||" => ensure!(policy.allow_compound, "命令保护：未允许组合命令"),
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
                "命令保护：未允许组合命令或多行命令"
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
        .context("命令保护：无法确定命令名")?;
    if has_dynamic(name) {
        ensure!(
            policy.allow_scripts && !policy.diagnostic_only && !block_dangerous,
            "命令保护：无法静态确定命令名"
        );
    }
    let mut words = shlex::split(text(name, source)).context("命令保护：无法解析命令名")?;
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
    if policy.diagnostic_only {
        crate::security::check_diagnostic_words(&words)?;
    }
    inspect_words(&words, policy, block_dangerous, nesting)
}

fn basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn inspect_words(
    words: &[String],
    policy: &CommandPolicy,
    block_dangerous: bool,
    nesting: usize,
) -> Result<()> {
    ensure!(nesting <= 8, "命令保护：包装命令嵌套过深");
    let Some(first) = words.first() else {
        return Ok(());
    };
    let name = basename(first);
    let args = &words[1..];
    ensure!(
        !(block_dangerous || policy.diagnostic_only)
            || !first.chars().any(|c| "$`*?[]{}".contains(c)),
        "命令保护：无法静态确定命令名"
    );
    if block_dangerous {
        let dangerous = [
            "rm", "unlink", "rmdir", "shred", "truncate", "dd", "mkfs", "mkswap", "wipefs",
            "fdisk", "sfdisk", "cfdisk", "parted", "gdisk", "sgdisk", "shutdown", "reboot", "halt",
            "poweroff", "init", "telinit", "kill", "killall", "pkill",
        ];
        ensure!(
            !dangerous.contains(&name) && !name.starts_with("mkfs."),
            "命令保护：已拦截高危命令「{name}」"
        );
        ensure!(
            name != "find"
                || !args.iter().any(|arg| matches!(
                    arg.as_str(),
                    "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
                )),
            "命令保护：已拦截 find 删除或执行动作"
        );
        ensure!(
            name != "systemctl"
                || !args.iter().any(|arg| matches!(
                    arg.as_str(),
                    "reboot" | "poweroff" | "halt" | "rescue" | "emergency"
                )),
            "命令保护：已拦截系统电源操作"
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
    let custom_path = first.contains('/')
        && !["/bin/", "/usr/bin/", "/sbin/", "/usr/sbin/"]
            .iter()
            .any(|prefix| {
                first
                    .strip_prefix(prefix)
                    .is_some_and(|rest| !rest.contains('/'))
            });
    ensure!(
        policy.allow_scripts || !(interpreter || script_file || custom_path),
        "命令保护：未允许脚本、解释器或自定义程序路径"
    );
    if shell {
        if let Some(index) = args
            .iter()
            .position(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
        {
            let code = args
                .get(index + 1)
                .context("命令保护：Shell -c 缺少脚本内容")?;
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
                ensure!(policy.allow_scripts, "命令保护：未允许 sudo Shell 模式");
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
