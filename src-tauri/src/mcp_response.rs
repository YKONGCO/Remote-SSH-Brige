//! MCP rendering is separate from execution. Failures are one short sentence;
//! raw text results keep their metadata separate from the original body.
use crate::{model::Operation, operation_error};
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::Value;

pub fn from_outcome(op: &Operation, outcome: anyhow::Result<Value>) -> CallToolResult {
    match outcome {
        Ok(value) => success(&op.kind, value),
        Err(error) => failure(op, &error),
    }
}

pub fn success(kind: &str, mut value: Value) -> CallToolResult {
    compact_errors(&mut value);
    let body_field = match kind {
        "read_file" => "content",
        "terminal_read" => "stdout",
        _ => return CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
    };
    let body = value
        .as_object_mut()
        .and_then(|object| object.remove(body_field))
        .and_then(|body| body.as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut result = CallToolResult::success(vec![ContentBlock::text(body)]);
    result.structured_content = Some(value);
    result
}

pub fn failure(op: &Operation, error: &anyhow::Error) -> CallToolResult {
    let response = operation_error::error_response(error, op);
    let text = if op.kind == "exec" {
        command_policy_json(&response).unwrap_or_else(|| error_text(&response))
    } else {
        error_text(&response)
    };
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn command_policy_json(error: &Value) -> Option<String> {
    if error["code"] != "COMMAND_POLICY_DENIED" {
        return None;
    }
    let command = error["details"]["command"].as_str()?;
    Some(
        serde_json::json!({
            "code":"COMMAND_POLICY_DENIED",
            "details":{"command":command},
            "message":error_text(error),
        })
        .to_string(),
    )
}

/// Full diagnostics stay in audit records; MCP only needs the actionable cause.
fn error_text(error: &Value) -> String {
    let details = &error["details"];
    let mut text = if let Some(permissions) = details["missing_permissions"].as_array() {
        format!(
            "Missing MCP permission: {}",
            permissions
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        details["reason"]
            .as_str()
            .filter(|reason| !reason.trim().is_empty())
            .or_else(|| error["message"].as_str())
            .unwrap_or("Operation failed")
            .to_owned()
    };
    if let Some(native) = native_error_text(error) {
        text = native;
    }
    if let Some(path) = details["failed_path"].as_str() {
        text = format!("Cannot delete '{}'", short_line(path));
    }
    if details["outcome_unknown"] == true {
        text.push_str("; outcome unknown, inspect the target before retrying");
    } else if details["partial"] == true {
        text.push_str(&format!(
            "; removed entries: {}, inspect the target before retrying",
            details["removed_entries"]
        ));
    }
    short_line(&text)
}

pub fn short_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 240 {
        format!("{}…", line.chars().take(239).collect::<String>())
    } else if line.is_empty() {
        "Operation failed".into()
    } else {
        line
    }
}

fn native_error_text(error: &Value) -> Option<String> {
    let details = &error["details"];
    if details.get("ssh_code").is_none() && details.get("os_code").is_none() {
        return None;
    }
    let message = operation_error::native_message(error["code"].as_str()?)?;
    let primary = error["message"].as_str().unwrap_or("");
    if primary != message && !primary.starts_with('[') && !primary.contains("(os error ") {
        return None;
    }
    Some(
        match details["path"].as_str().filter(|path| !path.is_empty()) {
            Some(path) => format!("{message}: {path}"),
            None => message.into(),
        },
    )
}

/// Keep the offending field or type, without Serde's full expected-field list.
pub fn parameter_error(text: &str) -> String {
    let reason = text
        .strip_prefix("failed to deserialize parameters: ")
        .unwrap_or(text);
    let reason = reason.split(", expected one of ").next().unwrap_or(reason);
    short_line(&format!("Invalid arguments: {reason}"))
}

pub fn normalize_tool_result(mut result: CallToolResult) -> CallToolResult {
    if result.is_error == Some(true) {
        let text = result
            .content
            .iter()
            .filter_map(ContentBlock::as_text)
            .map(|text| text.text.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let policy_json = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|error| command_policy_json(&error));
        let message = if let Some(json) = policy_json {
            // Bound the message, never truncate the JSON or the original command.
            json
        } else if text.starts_with("failed to deserialize parameters: ") {
            parameter_error(&text)
        } else {
            short_line(&text)
        };
        result.content = vec![ContentBlock::text(message)];
        result.structured_content = None;
    }
    result
}

/// Status queries can succeed while the underlying transfer or session failed.
fn compact_errors(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "error" | "audit_error" | "last_error") {
                    if value.get("message").and_then(Value::as_str).is_some() {
                        *value = Value::String(error_text(value));
                    } else if let Some(text) = value.as_str() {
                        *value = Value::String(short_line(text));
                    }
                } else {
                    compact_errors(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(compact_errors),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wire(result: CallToolResult) -> Value {
        serde_json::to_value(result).unwrap()
    }

    #[test]
    fn text_body_is_unchanged_and_pagination_is_metadata_only() {
        let result = wire(success(
            "read_file",
            json!({
                "content":"first\r\n", "has_more":true, "next_offset":2,
                "truncated_by":"scan_bytes", "first_line_exceeds_limit":false
            }),
        ));
        assert_eq!(result["content"][0]["text"], "first\r\n");
        assert_eq!(result["content"].as_array().unwrap().len(), 1);
        assert_eq!(result["structuredContent"]["next_offset"], 2);
        assert!(result["structuredContent"].get("content").is_none());
    }

    #[test]
    fn terminal_keeps_cursor_without_duplicating_stdout() {
        let result = wire(success(
            "terminal_read",
            json!({"stdout":"hello", "cursor":5}),
        ));
        assert_eq!(result["content"][0]["text"], "hello");
        assert_eq!(result["structuredContent"], json!({"cursor":5}));
    }

    #[test]
    fn failed_command_and_failed_transfer_are_successful_status_queries() {
        for (kind, value) in [
            ("exec", json!({"exit_code":7,"stderr":"failure"})),
            ("transfer", json!({"status":"failed"})),
        ] {
            let result = wire(success(kind, value.clone()));
            assert_ne!(result["isError"], true);
            assert_eq!(
                serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap())
                    .unwrap(),
                value
            );
            assert!(result.get("structuredContent").is_none());
        }
    }

    #[test]
    fn tool_error_is_one_sentence_without_diagnostic_payload() {
        let result = wire(failure(
            &Operation::default(),
            &operation_error::error("NOT_FOUND", "File not found"),
        ));
        assert_eq!(result["isError"], true);
        assert!(result.get("structuredContent").is_none());
        assert_eq!(result["content"][0]["text"], "File not found");
    }

    #[test]
    fn policy_failure_only_returns_the_specific_reason() {
        let command = "find /tmp -maxdepth 1 -name 'wlx*' -exec rm -rf {} ';'";
        let op = Operation {
            kind: "exec".into(),
            command: command.into(),
            ..Default::default()
        };
        let settings = crate::model::RuntimeSettings {
            command_policy: Some(crate::model::CommandPolicy {
                execution_mode: Some("whitelist".into()),
                allow_advanced_shell: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let error = crate::security::check(&settings, &op, "mcp").unwrap_err();
        let diagnostic = operation_error::error_response(&error, &op);
        assert!(diagnostic["details"].get("command_policy").is_none());
        assert!(diagnostic["details"].get("command_protection").is_none());
        let result = wire(normalize_tool_result(failure(&op, &error)));
        assert_eq!(result["isError"], true);
        assert!(result.get("structuredContent").is_none());
        let data: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            data,
            json!({"code":"COMMAND_POLICY_DENIED", "details":{"command":command}, "message":"find delete and execute actions are blocked"})
        );
    }

    #[test]
    fn long_exec_commands_keep_valid_json_and_the_original_command() {
        let command = format!("ls {}", "a".repeat(2000));
        let op = Operation {
            kind: "exec".into(),
            command: command.clone(),
            ..Default::default()
        };
        let error = operation_error::error_with_details(
            "COMMAND_POLICY_DENIED",
            "Command rejected",
            json!({"command":command,"reason":"Command 'ls' is blocked by the denylist","command_policy":{"large":"must not escape"}}),
        );
        let result = wire(normalize_tool_result(failure(&op, &error)));
        let data: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(data["details"], json!({"command":command}));
        assert_eq!(data["message"], "Command 'ls' is blocked by the denylist");
        assert!(data.get("stage").is_none());
    }

    #[test]
    fn status_errors_are_compact_but_keep_partial_outcome_warning() {
        let result = wire(success(
            "transfer",
            json!({"status":"failed", "error": {
                "code":"PERMISSION_DENIED", "message":"Deletion failed",
                "details":{"partial":true, "removed_entries":1, "command_policy":{"large":"config"}}
            }}),
        ));
        let data: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            data["error"],
            "Deletion failed; removed entries: 1, inspect the target before retrying"
        );
        assert_eq!(data["status"], "failed");
    }

    #[test]
    fn permissions_identify_the_missing_grant_and_long_errors_are_bounded() {
        assert_eq!(
            error_text(
                &json!({"message":"Not authorized", "details":{"missing_permissions":["edit_file","read_file"]}})
            ),
            "Missing MCP permission: edit_file, read_file"
        );
        let text = error_text(&json!({"message":format!("Error\n{}", "x".repeat(400))}));
        assert_eq!(text.chars().count(), 240);
        assert!(!text.contains('\n'));
        assert!(text.ends_with('…'));
    }

    #[test]
    fn parameter_errors_keep_the_field_but_drop_expected_field_lists() {
        let result = normalize_tool_result(CallToolResult::error(vec![ContentBlock::text(
            "failed to deserialize parameters: unknown field `force`, expected one of `server_id`, `path`, `recursive`, `missing_ok`",
        )]));
        assert_eq!(
            wire(result)["content"][0]["text"],
            "Invalid arguments: unknown field `force`"
        );
        assert_eq!(
            parameter_error("failed to deserialize parameters: missing field `path`"),
            "Invalid arguments: missing field `path`"
        );
        assert_eq!(
            parameter_error("failed to deserialize parameters: invalid type: string, expected u64"),
            "Invalid arguments: invalid type: string, expected u64"
        );
    }

    #[test]
    fn native_errors_use_english_and_keep_the_affected_path() {
        let access_denied = if cfg!(windows) { 5 } else { 13 };
        let result = wire(failure(
            &Operation {
                path: "/private".into(),
                ..Default::default()
            },
            &std::io::Error::from_raw_os_error(access_denied).into(),
        ));
        assert_eq!(result["content"][0]["text"], "File access denied: /private");
        let result = wire(failure(
            &Operation::default(),
            &anyhow::Error::from(std::io::Error::from_raw_os_error(access_denied))
                .context("Cannot resolve SSH server address"),
        ));
        assert_eq!(
            result["content"][0]["text"],
            "Cannot resolve SSH server address"
        );
    }

    #[test]
    fn partial_delete_names_the_failed_path_without_dumping_details() {
        let text = error_text(&json!({"message":"Deletion stopped", "details":{
            "failed_path":"/cache/locked", "partial":true, "removed_entries":2
        }}));
        assert_eq!(
            text,
            "Cannot delete '/cache/locked'; removed entries: 2, inspect the target before retrying"
        );
    }
}
