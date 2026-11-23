use crate::model::{CommandPolicy, RuntimeSettings, LEGACY_DIAGNOSTIC_COMMANDS};
use serde::Serialize;
use std::collections::BTreeSet;
use tree_sitter::{Node, Parser};

const MAX_SOURCE_BYTES: usize = 16384;
const MAX_SYNTAX_DEPTH: usize = 128;
const MAX_NESTING: usize = 8;
const MAX_REPORTED_COMMANDS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    RequireApproval,
    Deny,
}

#[derive(Clone, Serialize, Debug, PartialEq, Eq)]
pub struct RuleMatch {
    pub source: &'static str,
    pub command: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument: Option<String>,
}

#[derive(Clone, Serialize, Debug)]
pub struct Assessment {
    pub decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<&'static str>,
    pub matched_rules: Vec<RuleMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action: Option<&'static str>,

    pub uncertain: bool,
    pub commands: Vec<String>,
}

fn is_empty_slice<T>(value: &[T]) -> bool {
    value.is_empty()
}

impl Assessment {
    fn denial(reason: impl Into<String>) -> Self {
        Self {
            decision: Decision::Deny,
            reason: Some(reason.into()),
            risk: None,
            matched_rules: vec![],
            next_action: None,
            uncertain: false,
            commands: vec![],
        }
    }
}

#[derive(Clone, Serialize, Debug)]
pub struct BuiltinRule {
    pub command: &'static str,

    #[serde(skip_serializing_if = "is_empty_slice")]
    pub arguments: &'static [&'static str],
    pub reason: &'static str,
}

pub const BUILTIN_RULES: &[BuiltinRule] = &[
    BuiltinRule {
        command: "rm",
        arguments: &[],
        reason: "Deletes files or directories; data may be unrecoverable",
    },
    BuiltinRule {
        command: "unlink",
        arguments: &[],
        reason: "Removes a filesystem entry",
    },
    BuiltinRule {
        command: "rmdir",
        arguments: &[],
        reason: "Removes directories",
    },
    BuiltinRule {
        command: "shred",
        arguments: &[],
        reason: "Overwrites file contents irrecoverably",
    },
    BuiltinRule {
        command: "truncate",
        arguments: &[],
        reason: "Can discard file contents",
    },
    BuiltinRule {
        command: "dd",
        arguments: &[],
        reason: "Can overwrite disks, devices or files",
    },
    BuiltinRule {
        command: "mkswap",
        arguments: &[],
        reason: "Reformats a swap device",
    },
    BuiltinRule {
        command: "wipefs",
        arguments: &[],
        reason: "Erases filesystem signatures",
    },
    BuiltinRule {
        command: "fdisk",
        arguments: &[],
        reason: "Modifies disk partitions",
    },
    BuiltinRule {
        command: "sfdisk",
        arguments: &[],
        reason: "Modifies disk partitions",
    },
    BuiltinRule {
        command: "cfdisk",
        arguments: &[],
        reason: "Modifies disk partitions",
    },
    BuiltinRule {
        command: "parted",
        arguments: &[],
        reason: "Modifies disk partitions",
    },
    BuiltinRule {
        command: "gdisk",
        arguments: &[],
        reason: "Modifies disk partitions",
    },
    BuiltinRule {
        command: "sgdisk",
        arguments: &[],
        reason: "Modifies disk partitions",
    },
    BuiltinRule {
        command: "shutdown",
        arguments: &[],
        reason: "Powers off or restarts the machine",
    },
    BuiltinRule {
        command: "reboot",
        arguments: &[],
        reason: "Restarts the machine",
    },
    BuiltinRule {
        command: "halt",
        arguments: &[],
        reason: "Stops the machine",
    },
    BuiltinRule {
        command: "poweroff",
        arguments: &[],
        reason: "Powers off the machine",
    },
    BuiltinRule {
        command: "init",
        arguments: &[],
        reason: "Changes the system runlevel",
    },
    BuiltinRule {
        command: "telinit",
        arguments: &[],
        reason: "Changes the system runlevel",
    },
    BuiltinRule {
        command: "kill",
        arguments: &[],
        reason: "Terminates processes",
    },
    BuiltinRule {
        command: "killall",
        arguments: &[],
        reason: "Terminates processes by name",
    },
    BuiltinRule {
        command: "pkill",
        arguments: &[],
        reason: "Terminates processes by pattern",
    },
    BuiltinRule {
        command: "chroot",
        arguments: &[],
        reason: "Changes the filesystem root and executes a command",
    },
    BuiltinRule {
        command: "find",
        arguments: &["-delete", "-exec", "-execdir", "-ok", "-okdir"],
        reason: "find may delete files or execute commands",
    },
    BuiltinRule {
        command: "systemctl",
        arguments: &["reboot", "poweroff", "halt", "rescue", "emergency"],
        reason: "System power or rescue state changes",
    },
];

pub fn builtin_rules() -> &'static [BuiltinRule] {
    BUILTIN_RULES
}

#[derive(Default)]
struct Analysis {
    commands: Vec<String>,
    matches: Vec<RuleMatch>,
}

pub fn assess(command: &str, settings: &RuntimeSettings) -> Assessment {
    let policy = settings.effective_command_policy();
    match analyze(command, &policy) {
        Ok(analysis) => decide(analysis),
        Err(reason) => Assessment::denial(reason),
    }
}

fn analyze(source: &str, policy: &CommandPolicy) -> Result<Analysis, String> {
    policy.validate().map_err(|e| e.to_string())?;
    if policy.exec_disabled {
        return Err("Command execution is disabled".into());
    }
    let mut analysis = Analysis::default();
    parse_source(source, policy, 0, &mut analysis)?;
    let mut seen = BTreeSet::new();
    analysis
        .commands
        .retain(|command| seen.insert(command.clone()));
    analysis.commands.truncate(MAX_REPORTED_COMMANDS);
    Ok(analysis)
}

fn decide(mut analysis: Analysis) -> Assessment {
    let mut seen = BTreeSet::new();
    analysis.matches.retain(|rule| {
        seen.insert((
            rule.source,
            rule.command.clone(),
            rule.argument.clone(),
            rule.reason.clone(),
        ))
    });
    if analysis.matches.is_empty() {
        return Assessment {
            decision: Decision::Allow,
            reason: None,
            risk: None,
            matched_rules: vec![],
            next_action: None,
            uncertain: false,
            commands: analysis.commands,
        };
    }
    let risk = if analysis.matches.iter().any(|rule| rule.source == "builtin") {
        "high"
    } else if analysis.matches.iter().any(|rule| rule.source == "user") {
        "protected"
    } else {
        "unknown"
    };

    analysis.matches.sort_by_key(|rule| match rule.source {
        "builtin" => 0,
        "legacy_whitelist" => 1,
        "analysis" => 2,
        _ => 3,
    });
    let uncertain = analysis
        .matches
        .iter()
        .any(|rule| rule.source == "analysis");
    let total = analysis.matches.len();
    let first = analysis.matches[0].reason.clone();
    let reason = if total > 1 {
        format!(
            "{first} (and {} more rule match{})",
            total - 1,
            if total == 2 { "" } else { "es" }
        )
    } else {
        first
    };

    analysis.matches.truncate(16);
    Assessment {
        decision: Decision::RequireApproval,
        reason: Some(reason),
        risk: Some(risk),
        matched_rules: analysis.matches,
        next_action: Some("exec_approved"),
        uncertain,
        commands: analysis.commands,
    }
}

fn parse_source(
    source: &str,
    policy: &CommandPolicy,
    nesting: usize,
    analysis: &mut Analysis,
) -> Result<(), String> {
    if nesting > MAX_NESTING {
        return Err("Script nesting exceeds 8 levels".into());
    }
    if source.trim().is_empty() {
        return if nesting == 0 {
            Err("command must not be empty".into())
        } else {
            Ok(())
        };
    }
    if source.len() > MAX_SOURCE_BYTES
        || source
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(
            "command exceeds 16384 bytes or contains unsupported control characters".into(),
        );
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .map_err(|e| format!("Cannot initialize shell parser: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "Cannot parse shell command".to_string())?;
    if tree.root_node().has_error() {
        return Err("Invalid or incomplete shell syntax".into());
    }
    walk(tree.root_node(), source, policy, nesting, analysis)
}

fn walk(
    root: Node<'_>,
    source: &str,
    policy: &CommandPolicy,
    nesting: usize,
    analysis: &mut Analysis,
) -> Result<(), String> {
    let mut pending = vec![(root, 0usize)];
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_SYNTAX_DEPTH {
            return Err("Shell syntax nesting exceeds 128 levels".into());
        }
        match node.kind() {
            "pipeline" => require(policy.allow_pipes, "Pipes are not allowed")?,
            "file_redirect" | "heredoc_redirect" | "herestring_redirect" => {
                require(policy.allow_redirection, "Redirection is not allowed")?
            }
            "command_substitution" | "process_substitution" => require(
                policy.allow_substitution,
                "Command substitution is not allowed",
            )?,
            "list" | "subshell" | "compound_statement" => {
                require(policy.allow_compound, "Compound commands are not allowed")?
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
            | "brace_expression" => require(
                policy.allow_scripts,
                "Scripts, variables and control structures are not allowed",
            )?,
            "command" => inspect_command(node, source, policy, nesting, analysis)?,
            ";" | "&" | "&&" | "||" => {
                require(policy.allow_compound, "Compound commands are not allowed")?
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();

        if matches!(
            node.kind(),
            "program" | "command_substitution" | "process_substitution"
        ) {
            let count = children
                .iter()
                .filter(|child| child.is_named() && child.kind() != "comment")
                .count();
            require(
                count <= 1 || policy.allow_compound,
                "Compound or multiline commands are not allowed",
            )?;
        }
        pending.extend(children.into_iter().map(|child| (child, depth + 1)));
    }
    Ok(())
}

fn require(condition: bool, reason: &'static str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(reason.into())
    }
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
    nesting: usize,
    analysis: &mut Analysis,
) -> Result<(), String> {
    let Some(name) = node.child_by_field_name("name") else {
        analysis.matches.push(RuleMatch {
            source: "analysis",
            command: "<unknown>".into(),
            reason: "Cannot determine the command name; dynamic execution requires approval".into(),
            argument: None,
        });
        return Ok(());
    };
    if has_dynamic(name) {
        let raw: String = text(name, source).chars().take(200).collect();
        analysis.matches.push(RuleMatch {
            source: "analysis",
            command: raw.clone(),
            reason: "Dynamic command names cannot be statically verified".into(),
            argument: None,
        });
        analysis.commands.push(raw);
        return Ok(());
    }
    let mut words = shlex::split(text(name, source))
        .filter(|words| !words.is_empty())
        .unwrap_or_else(|| vec![text(name, source).to_string()]);
    if words.is_empty() {
        return Ok(());
    }
    let mut cursor = node.walk();
    for argument in node.children_by_field_name("argument", &mut cursor) {
        let raw = text(argument, source);
        let value = shlex::split(raw)
            .filter(|words| words.len() == 1)
            .map(|mut words| words.remove(0))
            .unwrap_or_else(|| raw.to_string());
        words.push(value);
    }
    inspect_words(&words, policy, nesting, analysis)
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

fn matches_rule(rule: &str, first: &str, name: &str) -> bool {
    if rule.contains('/') {
        rule == first
    } else {
        rule == name
    }
}

fn inspect_words(
    words: &[String],
    policy: &CommandPolicy,
    nesting: usize,
    analysis: &mut Analysis,
) -> Result<(), String> {
    require(
        nesting <= MAX_NESTING,
        "Command wrapper nesting exceeds 8 levels",
    )?;
    let Some(first) = words.first() else {
        return Ok(());
    };
    let name = basename(first);
    let args = &words[1..];
    analysis.commands.push(first.clone());

    if first.chars().any(|c| "$`*?[]{}".contains(c)) {
        uncertain(
            analysis,
            first,
            "Dynamic command names cannot be statically verified",
        );
        return Ok(());
    }

    if policy
        .protected_commands
        .iter()
        .any(|rule| matches_rule(rule, first, name))
    {
        analysis.matches.push(RuleMatch {
            source: "user",
            command: name.to_string(),
            reason: format!("'{name}' is protected by a user rule"),
            argument: None,
        });
    }

    for rule in BUILTIN_RULES {
        let matches = if rule.arguments.is_empty() {
            rule.command == name
        } else {
            rule.command == name
                && args
                    .iter()
                    .any(|arg| rule.arguments.iter().any(|required| arg == required))
        };
        if matches {
            analysis.matches.push(RuleMatch {
                source: "builtin",
                command: name.to_string(),
                reason: rule.reason.into(),
                argument: args
                    .iter()
                    .find(|arg| rule.arguments.iter().any(|required| *arg == *required))
                    .cloned(),
            });
        }
    }
    if name.starts_with("mkfs.") {
        analysis.matches.push(RuleMatch {
            source: "builtin",
            command: name.to_string(),
            reason: "Formats a filesystem; existing data is destroyed".into(),
            argument: None,
        });
    }

    if let Some(list) = &policy.legacy_whitelist {
        if !list.iter().any(|rule| matches_rule(rule, first, name)) {
            analysis.matches.push(RuleMatch {
                source: "legacy_whitelist",
                command: name.to_string(),
                reason: format!(
                    "'{name}' is outside the preserved legacy allowlist and requires approval"
                ),
                argument: None,
            });
        } else if let Some(reason) = legacy_argument_violation(name, words) {
            analysis.matches.push(RuleMatch {
                source: "legacy_whitelist",
                command: name.to_string(),
                reason: reason.into(),
                argument: None,
            });
        }
    }

    let shell = ["sh", "bash", "dash", "zsh", "ksh", "ash", "fish"].contains(&name);
    let interpreter = [
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

    if !policy.allow_scripts {
        require(
            !(interpreter || script_file || custom_path),
            "Scripts, interpreters and custom executable paths are not allowed",
        )?;
    } else if shell {
        inspect_shell(name, args, policy, nesting, analysis)?;
    } else if name == "eval" {
        let code = args.join(" ");
        if code.contains(['$', '`', '\\']) {
            uncertain(
                analysis,
                name,
                "eval receives a dynamic string that cannot be statically verified",
            );
        } else {
            parse_source(&code, policy, nesting + 1, analysis)?;
        }
    } else if name == "source" || name == "." {
        uncertain(
            analysis,
            name,
            "Sourced script contents cannot be statically verified",
        );
    } else if matches!(name, "su" | "runuser") {
        if let Some(index) = args
            .iter()
            .position(|arg| matches!(arg.as_str(), "-c" | "--command"))
        {
            let Some(code) = args.get(index + 1) else {
                uncertain(
                    analysis,
                    name,
                    format!("{name} -c has no command that can be statically verified"),
                );
                return Ok(());
            };
            parse_source(code, policy, nesting + 1, analysis)?;
        } else if name == "runuser" {
            match wrapper_inner(name, args) {
                Some(inner) if !inner.is_empty() => {
                    inspect_words(&inner, policy, nesting + 1, analysis)?
                }
                _ => uncertain(analysis, name, "runuser without a command starts a shell"),
            }
        } else {
            uncertain(
                analysis,
                name,
                "su starts a login shell that cannot be statically verified",
            );
        }
    } else if interpreter {
        if !version_only(args) {
            uncertain(
                analysis,
                name,
                format!("'{name}' executes code or a script that cannot be statically verified"),
            );
        }
    } else if script_file || custom_path {
        uncertain(
            analysis,
            name,
            "Custom executables and scripts cannot be statically verified",
        );
    }

    if is_wrapper(name) {
        if name == "sudo"
            && (args.iter().any(|arg| {
                arg == "--shell"
                    || arg == "--login"
                    || (arg.starts_with('-')
                        && !arg.starts_with("--")
                        && (arg.contains('s') || arg.contains('i')))
            }))
        {
            uncertain(
                analysis,
                name,
                "sudo shell mode starts an interactive shell and cannot be statically verified",
            );
        } else {
            let inner = wrapper_inner(name, args);
            match inner {
                Some(inner) if !inner.is_empty() => {
                    inspect_words(&inner, policy, nesting + 1, analysis)?;
                }
                Some(_) => {}
                None if matches!(name, "sudo" | "doas") => uncertain(
                    analysis,
                    name,
                    "Privilege wrapper without a command starts an interactive shell",
                ),
                None => {}
            }
        }
    }
    Ok(())
}

fn inspect_shell(
    name: &str,
    args: &[String],
    policy: &CommandPolicy,
    nesting: usize,
    analysis: &mut Analysis,
) -> Result<(), String> {
    if let Some(index) = args
        .iter()
        .position(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
    {
        let Some(code) = args.get(index + 1) else {
            uncertain(
                analysis,
                name,
                "Shell -c has no script content that can be statically verified",
            );
            return Ok(());
        };
        parse_source(code, policy, nesting + 1, analysis)?;
        return Ok(());
    }
    if args.iter().any(|arg| !arg.starts_with('-')) {
        uncertain(
            analysis,
            name,
            "Shell script contents cannot be statically verified",
        );
    } else if args.is_empty() || args.iter().any(|arg| arg == "-s" || arg == "--stdin") {
        uncertain(
            analysis,
            name,
            "An interactive shell or stdin script cannot be statically verified",
        );
    } else if version_only(args) {
    } else {
        uncertain(
            analysis,
            name,
            "Shell startup modes cannot be statically verified",
        );
    }
    Ok(())
}

fn version_only(args: &[String]) -> bool {
    !args.is_empty()
        && args
            .iter()
            .all(|arg| matches!(arg.as_str(), "--version" | "-V" | "-v" | "--help" | "-h"))
}

fn uncertain(analysis: &mut Analysis, command: &str, reason: impl Into<String>) {
    analysis.matches.push(RuleMatch {
        source: "analysis",
        command: command.to_string(),
        reason: reason.into(),
        argument: None,
    });
}

fn is_wrapper(name: &str) -> bool {
    [
        "sudo", "doas", "pkexec", "env", "command", "builtin", "nohup", "timeout", "nice",
        "stdbuf", "exec", "xargs", "busybox",
    ]
    .contains(&name)
}

fn wrapper_inner(name: &str, args: &[String]) -> Option<Vec<String>> {
    if name == "env" {
        for (index, arg) in args.iter().enumerate() {
            if let Some(code) = arg
                .strip_prefix("--split-string=")
                .or_else(|| arg.strip_prefix("-S").filter(|code| !code.is_empty()))
            {
                return Some(shlex::split(code).unwrap_or_else(|| vec![code.to_string()]));
            }
            if matches!(arg.as_str(), "-S" | "--split-string") {
                return args
                    .get(index + 1)
                    .map(|code| shlex::split(code).unwrap_or_else(|| vec![code.clone()]));
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
                    "-u", "-g", "-h", "-p", "-C", "-T", "-R", "-D", "--user", "--group", "--host",
                    "--prompt", "--chdir",
                ]
                .contains(&arg.as_str()),
                "doas" => ["-u", "-C"].contains(&arg.as_str()),
                "pkexec" => ["--user", "--argv0"].contains(&arg.as_str()),
                "runuser" => ["-u", "-g", "--user", "--group"].contains(&arg.as_str()),
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
    (index < args.len()).then(|| args[index..].to_vec())
}

fn legacy_argument_violation(name: &str, words: &[String]) -> Option<String> {
    if !LEGACY_DIAGNOSTIC_COMMANDS.contains(&name) || !trusted_executable(&words[0]) {
        return None;
    }
    let args = &words[1..];
    match name {
        "date" => {
            let allowed = args.iter().all(|arg| {
                matches!(
                    arg.as_str(),
                    "-u" | "--utc"
                        | "--universal"
                        | "-I"
                        | "-Id"
                        | "-Ih"
                        | "-Im"
                        | "-Is"
                        | "-Ins"
                        | "--iso-8601"
                        | "--iso-8601=date"
                        | "--iso-8601=hours"
                        | "--iso-8601=minutes"
                        | "--iso-8601=seconds"
                        | "--iso-8601=ns"
                        | "-R"
                        | "--rfc-email"
                ) || arg.starts_with('+')
            });
            (!allowed).then(|| "date may only query or format time".into())
        }
        "hostname" if !args.is_empty() => Some("hostname may only query the hostname".into()),
        "ss" if args.iter().any(|arg| {
            arg.starts_with("--kill")
                || (arg.len() > 2 && "--kill".starts_with(arg.as_str()))
                || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('K'))
        }) =>
        {
            Some("Terminating network connections is blocked".into())
        }
        "file"
            if args.iter().any(|arg| {
                arg.starts_with("--compile")
                    || (arg.len() > 2 && "--compile".starts_with(arg.as_str()))
                    || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('C'))
            }) =>
        {
            Some("Compiling a magic database is blocked".into())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(policy: CommandPolicy) -> RuntimeSettings {
        RuntimeSettings {
            command_policy: Some(policy),
            ..Default::default()
        }
    }

    fn classify(command: &str, policy: CommandPolicy) -> Assessment {
        assess(command, &settings(policy))
    }

    #[test]
    fn ordinary_commands_need_no_allowlist_or_approval() {
        for command in [
            "ls -la /var/log",
            "echo ok | grep ok",
            "cat /etc/os-release",
            "docker compose ps",
            "git status && git diff",
            "printf 'x' > /tmp/x",
            "bash -c 'ls -la'",
        ] {
            let result = classify(command, CommandPolicy::default());
            assert_eq!(result.decision, Decision::Allow, "{command}: {result:?}");
        }
    }

    #[test]
    fn builtin_rules_require_approval_and_cannot_be_overridden() {
        for command in [
            "rm -rf /tmp/x",
            "sudo rm -rf /",
            "/bin/rm -f file",
            "find . -name x -delete",
            "find . -exec rm {} ;",
            "systemctl reboot",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.ext4 /dev/sdb1",
            "echo $(rm -rf /tmp/x)",
            "bash -c 'rm -rf /tmp/x'",
            "su -c 'rm -rf /tmp/x'",
            "runuser -u root -- rm -rf /tmp/x",
            "pkexec --user root rm -rf /tmp/x",
            "chroot /mnt rm -rf /tmp/x",
        ] {
            let result = classify(
                command,
                CommandPolicy {
                    protected_commands: vec!["ls".into()],
                    ..Default::default()
                },
            );
            assert_eq!(
                result.decision,
                Decision::RequireApproval,
                "{command}: {result:?}"
            );
            assert!(
                result
                    .matched_rules
                    .iter()
                    .any(|rule| rule.source == "builtin"),
                "{command}: {result:?}"
            );
            assert_eq!(result.next_action, Some("exec_approved"));
        }
    }

    #[test]
    fn user_rules_add_protection_without_disabling_builtins() {
        let result = classify(
            "curl https://example.com",
            CommandPolicy {
                protected_commands: vec!["curl".into()],
                ..Default::default()
            },
        );
        assert_eq!(result.decision, Decision::RequireApproval);
        assert!(result
            .matched_rules
            .iter()
            .any(|rule| rule.source == "user"));
        let builtin = classify(
            "rm -rf /tmp/x",
            CommandPolicy {
                protected_commands: vec!["curl".into()],
                ..Default::default()
            },
        );
        assert!(builtin
            .matched_rules
            .iter()
            .any(|rule| rule.source == "builtin"));
    }

    #[test]
    fn dynamic_and_opaque_execution_is_uncertain_not_silently_allowed() {
        for command in [
            "$CMD",
            "sh -c \"$CMD\"",
            "sudo \"$CMD\"",
            "sudo -u root",
            "su -",
            "runuser -u root",
            "python script.py",
            "node -e 'console.log(1)'",
            "sed -n '1p' /etc/passwd",
            "./deploy.sh",
            "source /etc/profile",
        ] {
            let result = classify(command, CommandPolicy::default());
            assert_eq!(
                result.decision,
                Decision::RequireApproval,
                "{command}: {result:?}"
            );
            assert!(result.uncertain, "{command}: {result:?}");
        }
        assert_eq!(
            classify("python --version", CommandPolicy::default()).decision,
            Decision::Allow
        );
    }

    #[test]
    fn advanced_shell_limits_and_invalid_input_are_denied() {
        let pipes_off = CommandPolicy {
            allow_pipes: false,
            ..Default::default()
        };
        assert_eq!(classify("ls | grep x", pipes_off).decision, Decision::Deny);
        let scripts_off = CommandPolicy {
            allow_scripts: false,
            ..Default::default()
        };
        assert_eq!(
            classify("python script.py", scripts_off.clone()).decision,
            Decision::Deny
        );
        assert_eq!(classify("FOO=1 ls", scripts_off).decision, Decision::Deny);
        let compound_off = CommandPolicy {
            allow_compound: false,
            ..Default::default()
        };
        assert_eq!(classify("ls; ls", compound_off).decision, Decision::Deny);
        assert_eq!(
            classify("ls 'unterminated", CommandPolicy::default()).decision,
            Decision::Deny
        );
        assert_eq!(
            classify("", CommandPolicy::default()).decision,
            Decision::Deny
        );
        let disabled = CommandPolicy {
            exec_disabled: true,
            ..Default::default()
        };
        assert_eq!(classify("ls", disabled).decision, Decision::Deny);
    }

    #[test]
    fn migrated_whitelist_still_requires_approval_for_other_commands() {
        let policy = CommandPolicy {
            legacy_whitelist: Some(vec!["ls".into(), "date".into()]),
            ..Default::default()
        };
        assert_eq!(classify("ls -la", policy.clone()).decision, Decision::Allow);
        let other = classify("cat /etc/passwd", policy.clone());
        assert_eq!(other.decision, Decision::RequireApproval);
        assert!(other
            .matched_rules
            .iter()
            .any(|rule| rule.source == "legacy_whitelist"));
        let dangerous_date = classify("date -s 12:00", policy);
        assert_eq!(dangerous_date.decision, Decision::RequireApproval);
        assert!(dangerous_date
            .matched_rules
            .iter()
            .any(|rule| rule.source == "legacy_whitelist"));
    }

    #[test]
    fn nested_wrappers_are_analyzed() {
        let result = classify(
            "sudo -u root env A=1 timeout 5 rm -f /tmp/x",
            CommandPolicy::default(),
        );
        assert_eq!(result.decision, Decision::RequireApproval);
        assert!(result.commands.iter().any(|command| command == "rm"));
    }
}
