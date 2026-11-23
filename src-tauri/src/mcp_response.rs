use crate::{model::Operation, operation_error};
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{json, Value};

pub fn from_outcome(op: &Operation, outcome: anyhow::Result<Value>) -> CallToolResult {
    match outcome {
        Ok(value) => success(op, &value),
        Err(error) => failure(op, &error),
    }
}

fn text(body: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(body)])
}

fn report(body: String, notice: String, metadata: Value) -> CallToolResult {
    let mut result = if body.is_empty() {
        text(notice)
    } else {
        let mut result = text(body);
        if !notice.is_empty() {
            result.content.push(ContentBlock::text(notice));
        }
        result
    };
    if metadata
        .as_object()
        .is_some_and(|fields| !fields.is_empty())
    {
        result.structured_content = Some(metadata);
    }
    result
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn select(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| {
                let field = value.get(*key).filter(|field| !field.is_null())?;
                Some(((*key).into(), field.clone()))
            })
            .collect(),
    )
}

fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn list(value: &Value, present: impl Fn(&Value) -> Value) -> Value {
    Value::Array(items(value).map(present).collect())
}

pub fn success(op: &Operation, value: &Value) -> CallToolResult {
    let compact = match op.kind.as_str() {
        "exec" => return command_output(value, true),
        "job" => match op.job_action.as_str() {
            "list" => json!({"jobs":list(&value["jobs"], |job| {
                let mut item = select(job, &["job_id", "status", "exit_code"]);
                item["command"] = json!(bounded(field(job, "command"), 200));
                item
            })}),
            _ => return command_output(value, false),
        },
        "read_file" => return read_file(value),
        "list_files" => {
            let mut page = json!({"entries":list(&value["entries"], |entry| {
                let mut item = select(entry, &["name"]);
                item["type"] = json!(if entry["is_symlink"] == true {
                    "symlink"
                } else if entry["is_dir"] == true {
                    "dir"
                } else {
                    "file"
                });
                if item["type"] == "file" {
                    if let Some(size) = entry["size"].as_u64() {
                        item["size"] = json!(size);
                    }
                }
                item
            })});
            if let Some(offset) = value["next_offset"].as_u64() {
                page["next_offset"] = json!(offset);
                return report(
                    page.to_string(),
                    format!("[More entries available. Use offset={offset} to continue.]"),
                    json!({}),
                );
            }
            page
        }
        "stat_file" => {
            let mut stat = select(value, &["kind", "size", "modified", "permissions"]);
            if value["is_symlink"] == true {
                stat["is_symlink"] = json!(true);
            }
            stat
        }
        "write_file" | "port_forward_close" | "terminal_close" => return text("OK"),
        "remove_path" => {
            return text(if value["missing"] == true {
                "Already absent".into()
            } else if let Some(n) = value["removed_entries"].as_u64().filter(|n| *n != 1) {
                format!("Removed {n} entries")
            } else {
                "OK".into()
            });
        }
        "edit_file" => {
            let mut edit = select(value, &["applied_edits", "diff"]);
            if value["diff_truncated"] == true {
                edit["diff_truncated"] = json!(true);
            }
            edit
        }
        "terminal_open" => select(value, &["session_id"]),
        "list_terminals" => list(value, |session| {
            select(session, &["id", "server_id", "name", "state"])
        }),
        "terminal_write" => return text("Input submitted"),
        "terminal_read" => return terminal_read(value),
        "upload_file" | "download_file" => return transfer_start(value),
        "transfer" => return transfer(op, value),
        "port_forward_open" => select(value, &["tunnel_id", "local_host", "local_port"]),
        "port_forward_list" => json!({"tunnels":list(&value["tunnels"], tunnel)}),
        "port_forward_read" => {
            let mut status = select(value, &["status", "connections"]);
            add_error(&mut status, value);
            status
        }
        "list_servers" | "server_info" => value.clone(),
        _ => return text("OK"),
    };
    text(compact.to_string())
}

fn tunnel(value: &Value) -> Value {
    let mut item = select(
        value,
        &[
            "tunnel_id",
            "local_host",
            "local_port",
            "remote_port",
            "status",
        ],
    );
    add_error(&mut item, value);
    item
}

fn read_file(value: &Value) -> CallToolResult {
    let body = field(value, "content");
    if value["has_more"] != true {
        return text(if body.is_empty() {
            "[Empty file]"
        } else {
            body
        });
    }
    let range = match (value["start_line"].as_u64(), value["end_line"].as_u64()) {
        (Some(start), Some(end)) => format!("Showing lines {start}-{end}; "),
        _ => String::new(),
    };
    let next = value["next_offset"].as_u64();
    let continuation = if let Some(next) = next {
        format!("more content available. Use offset={next} to continue.")
    } else if value["truncated_by"] == "long_line" {
        "the next line exceeds 50 KiB. Use download_file for the full file.".into()
    } else {
        "the scan budget prevents reading the next line. Use download_file for the full file."
            .into()
    };
    report(
        body.into(),
        format!("[{range}{continuation}]"),
        select(
            value,
            &["start_line", "end_line", "next_offset", "truncated_by"],
        ),
    )
}

fn streams(value: &Value) -> String {
    let out = field(value, "stdout");
    let err = field(value, "stderr");
    match (out.is_empty(), err.is_empty()) {
        (_, true) => out.into(),
        (true, false) => format!("stderr:\n{err}"),
        (false, false) => format!("stdout:\n{out}\n\nstderr:\n{err}"),
    }
}

fn stream_cursors(
    value: &Value,
    can_read: bool,
    running: bool,
    notices: &mut Vec<String>,
) -> Value {
    let mut metadata = json!({});
    let continue_reading = can_read
        && (running
            || [
                "stdout_truncated",
                "stderr_truncated",
                "stdout_has_more",
                "stderr_has_more",
            ]
            .iter()
            .any(|key| value[*key] == true));
    for (name, next, truncated) in [
        ("stdout_cursor", "next_stdout_cursor", "stdout_truncated"),
        ("stderr_cursor", "next_stderr_cursor", "stderr_truncated"),
    ] {
        if value[truncated] == true {
            notices.push(format!("Earlier {} lost", name.trim_end_matches("_cursor")));
        }
        if continue_reading {
            if let Some(cursor) = value[next].as_u64() {
                notices.push(format!("{name}={cursor}"));
                metadata[name] = json!(cursor);
            }
        }
    }
    metadata
}

fn command_output(value: &Value, foreground: bool) -> CallToolResult {
    let mut body = streams(value);
    let status = field(value, "status");
    let running = status == "running";
    let more = value["stdout_has_more"] == true || value["stderr_has_more"] == true;
    let mut notices = Vec::new();
    let mut metadata = stream_cursors(value, !foreground || running, running, &mut notices);
    if running {
        metadata["status"] = json!(status);
        let label = if foreground {
            metadata["job_id"] = value["job_id"].clone();
            format!("Running: {}. Use job.read", field(value, "job_id"))
        } else if body.is_empty() {
            "Running; no new output".into()
        } else {
            "Running".into()
        };
        notices.insert(0, label);
    } else if let Some(code) = value["exit_code"].as_i64().filter(|code| *code != 0) {
        metadata["exit_code"] = json!(code);
        body = if body.is_empty() {
            format!("Exit {code}")
        } else if field(value, "stdout").is_empty() {
            format!("Exit {code}\n{}", field(value, "stderr"))
        } else {
            format!("Exit {code}\n{body}")
        };
    } else if !matches!(status, "" | "exited") {
        metadata["status"] = json!(status);
        notices.insert(
            0,
            if status == "disconnected" {
                "Disconnected; execution outcome unknown. Inspect remote state before retrying"
                    .into()
            } else {
                status.into()
            },
        );
    }
    if let Some(error) = diagnostic(&value["error"]) {
        notices.push(error);
    }
    if more {
        notices.insert(0, "More output available".into());
    }
    if body.is_empty() && notices.is_empty() {
        body = "OK".into();
    }
    report(body, notice(notices), metadata)
}

pub fn progress(value: &Value) -> Option<String> {
    let mut notices = Vec::new();
    stream_cursors(value, true, true, &mut notices);
    let output = streams(value);
    (!output.is_empty() || !notices.is_empty()).then(|| format!("{output}\n{}", notice(notices)))
}

fn notice(parts: Vec<String>) -> String {
    if parts.is_empty() {
        String::new()
    } else {
        format!("[{}]", parts.join("; "))
    }
}

fn terminal_read(value: &Value) -> CallToolResult {
    let body = field(value, "stdout");
    let alive = value["alive"] == true;
    let more = value["has_more"] == true;
    let lost = value["truncated"] == true;
    let mut parts = Vec::new();
    let mut metadata = json!({});
    if lost {
        parts.push("Earlier terminal output lost".into());
    }
    if more {
        parts.push("More output available".into());
    }
    if alive || more || lost {
        if let Some(cursor) = value["cursor"].as_u64() {
            parts.push(format!("cursor={cursor}"));
            metadata["cursor"] = json!(cursor);
        }
    }
    let state = field(value, "state");
    if !alive || state == "closing" {
        parts.push(format!("Terminal {state}"));
        metadata["state"] = json!(state);
    } else if body.is_empty() {
        parts.push("No new output".into());
    }
    report(body.into(), notice(parts), metadata)
}

fn transfer_start(value: &Value) -> CallToolResult {
    if let Some(id) = value["transfer_id"].as_str() {
        return report(
            format!("Queued: {id}"),
            String::new(),
            json!({"transfer_id":id}),
        );
    }
    let mut counts = json!({});
    let rejected = value["rejected"].as_u64().unwrap_or(0);
    let skipped = value["skipped"].as_u64().unwrap_or(0);
    let summary = format!(
        "Queued {} files: {}",
        value["queued"],
        field(value, "group_id")
    );
    if rejected > 0 {
        counts["rejected"] = json!(rejected);
        counts["failures"] = failures(&value["tasks"]);
    }
    if skipped > 0 {
        counts["skipped"] = json!(skipped);
    }
    report(
        summary,
        if rejected + skipped == 0 {
            String::new()
        } else {
            counts.to_string()
        },
        select(value, &["group_id", "queued"]),
    )
}

fn failures(tasks: &Value) -> Value {
    Value::Array(
        items(tasks)
            .filter(|task| {
                matches!(task["status"].as_str(), None | Some("failed")) && !task["error"].is_null()
            })
            .map(|task| {
                let mut item = select(task, &["remote_path", "transfer_id"]);
                add_error(&mut item, task);
                item
            })
            .collect(),
    )
}

fn group_summary(value: &Value, include_id: bool) -> Value {
    let mut summary = select(
        value,
        &["status", "total", "completed", "transferred_bytes"],
    );
    if include_id {
        summary["group_id"] = value["group_id"].clone();
    }
    for key in [
        "failed",
        "cancelled",
        "paused",
        "queued",
        "running",
        "retrying",
    ] {
        if value[key].as_u64().is_some_and(|count| count > 0) {
            summary[key] = value[key].clone();
        }
    }
    summary
}

fn transfer(op: &Operation, value: &Value) -> CallToolResult {
    let compact = match op.transfer_action.as_str() {
        "list" => json!({
            "transfers":list(&value["transfers"], |task| select(
                task, &["transfer_id", "group_id", "direction", "remote_path", "status"],
            )),
            "groups":list(&value["groups"], |group| group_summary(group, true)),
        }),
        "read" if op.transfer_group.is_some() => {
            let mut summary = group_summary(&value["group"], false);
            let failed = failures(&value["tasks"]);
            if failed.as_array().is_some_and(|items| !items.is_empty()) {
                summary["failures"] = failed;
            }
            summary
        }
        "read" => {
            let mut status = select(
                value,
                &["status", "transferred_bytes", "total_bytes", "version"],
            );
            if let (Some(bytes), Some(total)) = (
                value["transferred_bytes"].as_u64(),
                value["total_bytes"].as_u64(),
            ) {
                if value["status"] == "completed" && bytes == total {
                    status.as_object_mut().unwrap().remove("total_bytes");
                } else if total != 0 {
                    status["percent"] =
                        json!((u128::from(bytes) * 100 / u128::from(total)).min(100) as u64);
                }
            }
            if value["phase"] == "skipped" {
                status["skipped"] = json!(true);
            }
            if value["status"] != "cancelled" {
                add_error(&mut status, value);
            }
            if value["resume_available"] == true {
                status["resume_available"] = json!(true);
            }
            status
        }
        "pause" if op.transfer_group.is_some() => json!({"pause_requested":value["paused"]}),
        "cancel" if op.transfer_group.is_some() => json!({"cancel_requested":value["cancelled"]}),
        "resume" if op.transfer_group.is_some() => {
            let failed = failures(&value["failed"]);
            let count = items(&failed).count();
            if count == 0 {
                select(value, &["resumed"])
            } else {
                json!({"resumed":value["resumed"],"failed":count,"failures":failed})
            }
        }
        "pause" if value["pause_requested"] == true => return text("Pause requested"),
        "cancel" if value["cancel_requested"] == true => return text("Cancellation requested"),
        "resume" => return text("Resume accepted"),
        _ => select(value, &["status", "pause_requested", "cancel_requested"]),
    };
    text(compact.to_string())
}

fn add_error(target: &mut Value, value: &Value) {
    if let Some(error) = diagnostic(&value["error"]) {
        target["error"] = json!(error);
    }
}

fn diagnostic(value: &Value) -> Option<String> {
    if let Some(message) = value.as_str() {
        return Some(bounded(message, 4096));
    }
    value["code"].as_str().map(|_| error_text(value))
}

pub fn failure(op: &Operation, error: &anyhow::Error) -> CallToolResult {
    let response = operation_error::error_response(error, op);
    let details = &response["details"];
    let mut content = vec![ContentBlock::text(error_text(&response))];
    if op.kind == "exec" {
        let output = details.get("result").unwrap_or(details);
        let streams = streams(output);
        if !streams.is_empty() {
            content.push(ContentBlock::text(bounded(&streams, 4096)));
        }
        if let Some(error) = diagnostic(&output["error"]) {
            content.push(ContentBlock::text(error));
        }
        let mut notices = Vec::new();
        stream_cursors(output, false, false, &mut notices);
        if !notices.is_empty() {
            content.push(ContentBlock::text(notice(notices)));
        }
    }
    CallToolResult::error(content)
}

fn error_text(error: &Value) -> String {
    let code = error["code"].as_str().unwrap_or("OPERATION_FAILED");
    let details = &error["details"];
    if code == "APPROVAL_REQUIRED" {
        return "APPROVAL_REQUIRED: Use exec_approved with the same arguments.".into();
    }
    let message = if let Some(permissions) = details["missing_permissions"].as_array() {
        format!(
            "Missing MCP permission: {}",
            permissions
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        let primary = error["message"].as_str().unwrap_or("Operation failed");
        let reason = details["reason"]
            .as_str()
            .filter(|reason| !reason.is_empty());
        let mut lines = Vec::new();
        if code == "COMMAND_POLICY_DENIED" {
            lines.push(reason.unwrap_or(primary));
        } else {
            lines.push(primary);
            if let Some(reason) = reason.filter(|reason| !primary.contains(reason)) {
                lines.push(reason);
            }
        }
        for cause in items(&details["causes"]).filter_map(Value::as_str) {
            if !lines.iter().any(|line| line.contains(cause)) {
                lines.push(cause);
            }
        }
        lines.join("\n")
    };
    let mut result = format!("{code}: {}", bounded(&message, 4096));
    if let Some(path) = details["failed_path"].as_str() {
        result.push_str(&format!("\nFailed path: {}", bounded(path, 1024)));
    }
    if let Some(removed) = details["removed_entries"].as_u64() {
        result.push_str(&format!("\nRemoved entries: {removed}"));
    }
    if details["outcome_unknown"] == true || code == "COMMIT_STATUS_UNKNOWN" {
        if !["outcome unknown", "outcome is unknown", "status is unknown"]
            .iter()
            .any(|phrase| message.contains(phrase))
        {
            result.push_str("\nOutcome unknown.");
        }
        if !message.contains("before retrying") {
            result.push_str("\nInspect remote state before retrying.");
        }
    } else if details["partial"] == true && !message.contains("before retrying") {
        result.push_str("\nInspect the target before retrying.");
    }
    result
}

pub fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let head = text.floor_char_boundary(limit * 3 / 4);
    let tail = text.ceil_char_boundary(text.len() - limit / 4);
    format!("{}\n[Text truncated]\n{}", &text[..head], &text[tail..])
}

pub fn normalize_tool_result(mut result: CallToolResult) -> CallToolResult {
    if let Some(block) = result.content.first_mut() {
        if result.is_error == Some(true) {
            if let Some(reason) = block.as_text().and_then(|block| {
                block
                    .text
                    .strip_prefix("failed to deserialize parameters: ")
            }) {
                let reason = reason.split(", expected one of ").next().unwrap_or(reason);
                *block = ContentBlock::text(format!("INVALID_ARGUMENT: {}", bounded(reason, 4096)));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present(kind: &str, value: Value) -> CallToolResult {
        success(
            &Operation {
                kind: kind.into(),
                ..Default::default()
            },
            &value,
        )
    }

    fn blocks(result: &CallToolResult) -> Vec<&str> {
        result
            .content
            .iter()
            .map(|block| block.as_text().unwrap().text.as_str())
            .collect()
    }

    #[test]
    fn exec_outputs_stdout_verbatim_and_reports_nonzero_exit_separately() {
        let result = present(
            "exec",
            json!({
                "stdout": "  ready\r\n", "stderr":"", "status":"exited", "exit_code":0,
                "connection":{"reused":true},"next_stdout_cursor":9,"next_stderr_cursor":0
            }),
        );
        assert_eq!(blocks(&result), ["  ready\r\n"]);
        assert!(result.structured_content.is_none());
        let result = present("exec", json!({"exit_code":2,"stderr":"missing\n"}));
        assert_eq!(blocks(&result), ["Exit 2\nmissing\n"]);
        assert_ne!(result.is_error, Some(true));
        assert_eq!(result.structured_content, Some(json!({"exit_code":2})));
        assert_eq!(blocks(&present("exec", json!({"exit_code":0}))), ["OK"]);
        assert_eq!(
            blocks(&present("exec", json!({"stdout":"out","stderr":"warn"}))),
            ["stdout:\nout\n\nstderr:\nwarn"]
        );
        let result = present(
            "exec",
            json!({"stdout":"tail","status":"exited","exit_code":0,
                "stdout_truncated":true,"next_stdout_cursor":900}),
        );
        assert_eq!(blocks(&result), ["tail", "[Earlier stdout lost]"]);
        assert!(result.structured_content.is_none());
    }

    #[test]
    fn approval_and_unknown_outcomes_keep_only_actionable_details() {
        let op = Operation {
            kind: "exec".into(),
            ..Default::default()
        };
        let protected = operation_error::error_with_details(
            "APPROVAL_REQUIRED",
            "Approval needed",
            json!({"command":"PRIVATE","matched_rules":["PRIVATE"]}),
        );
        let result = failure(&op, &protected);
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            blocks(&result),
            ["APPROVAL_REQUIRED: Use exec_approved with the same arguments."]
        );

        let lost = operation_error::error_with_details(
            "SSH_DISCONNECTED",
            "Final exit status is unknown",
            json!({"outcome_unknown":true, "result":{"stdout":"partial output","connection":"PRIVATE"}}),
        );
        let result = failure(&op, &lost);
        assert_eq!(blocks(&result), [
            "SSH_DISCONNECTED: Final exit status is unknown\nInspect remote state before retrying.",
            "partial output",
        ]);
    }

    #[test]
    fn diagnostics_keep_line_breaks_and_important_tail() {
        let message = format!("System error\n{}\nlast diagnostic", "é".repeat(4000));
        let message = bounded(&message, 4096);
        assert!(message.starts_with("System error\n"));
        assert!(message.contains("[Text truncated]"));
        assert!(message.ends_with("last diagnostic"));
        assert!(message.len() < 4200);
        let result = normalize_tool_result(CallToolResult::error(vec![ContentBlock::text(
            "failed to deserialize parameters: unknown field `force`, expected one of `path`",
        )]));
        assert_eq!(blocks(&result), ["INVALID_ARGUMENT: unknown field `force`"]);
    }

    #[test]
    fn read_separates_continuation_from_verbatim_content() {
        let result = present(
            "read_file",
            json!({
                "content":"first\r\n","has_more":true,"start_line":1,"end_line":1,"next_offset":2
            }),
        );
        assert_eq!(
            blocks(&result),
            [
                "first\r\n",
                "[Showing lines 1-1; more content available. Use offset=2 to continue.]"
            ]
        );
        let result = present(
            "read_file",
            json!({"has_more":true,"truncated_by":"long_line"}),
        );
        assert!(blocks(&result)[0].contains("exceeds 50 KiB"));
        assert!(!blocks(&result)[0].contains("offset="));
        let result = present("read_file", json!({"content":"","has_more":false}));
        assert_eq!(blocks(&result), ["[Empty file]"]);
    }

    #[test]
    fn job_and_terminal_keep_cursors_when_output_was_lost_or_needs_draining() {
        let job = command_output(
            &json!({
                "stdout":"output", "status":"exited", "exit_code":0, "stdout_has_more":true,
                "stdout_truncated":true, "next_stdout_cursor":400, "next_stderr_cursor":7
            }),
            false,
        );
        assert_eq!(
            blocks(&job),
            [
                "output",
                "[More output available; Earlier stdout lost; stdout_cursor=400; stderr_cursor=7]"
            ]
        );
        assert_eq!(
            job.structured_content,
            Some(json!({"stdout_cursor":400,"stderr_cursor":7}))
        );
        let terminal = present(
            "terminal_read",
            json!({
                "stdout":"new", "cursor":90, "alive":false, "truncated":true, "state":"closed"
            }),
        );
        assert_eq!(
            blocks(&terminal),
            [
                "new",
                "[Earlier terminal output lost; cursor=90; Terminal closed]"
            ]
        );
        let terminal = present(
            "terminal_read",
            json!({
                "stdout":"tail", "cursor":95, "alive":false, "state":"closed"
            }),
        );
        assert_eq!(terminal.structured_content, Some(json!({"state":"closed"})));
    }

    #[test]
    fn failed_transfer_query_reports_only_failed_files_and_real_errors() {
        let value = json!({
            "group":{"status":"failed","total":3,"completed":1,"failed":1,"cancelled":1},
            "tasks":[
                {"status":"completed","remote_path":"/done"},
                {"status":"cancelled","remote_path":"/cancelled","error":{
                    "code":"TRANSFER_CANCELLED","message":"Transfer cancelled"
                }},
                {"status":"failed","remote_path":"/failed","error":{
                    "code":"IO_ERROR","message":"Connection lost"
                }}
            ]
        });
        let op = Operation {
            kind: "transfer".into(),
            transfer_action: "read".into(),
            transfer_group: Some("g".into()),
            ..Default::default()
        };
        let result = success(&op, &value);
        assert_ne!(result.is_error, Some(true));
        let body: Value = serde_json::from_str(blocks(&result)[0]).unwrap();
        assert_eq!(
            body["failures"],
            json!([{"remote_path":"/failed","error":"IO_ERROR: Connection lost"}])
        );
        assert_eq!(body["completed"], 1);
        assert_eq!(body["cancelled"], 1);
        assert!(!blocks(&result)[0].contains("/done"));
    }
}
