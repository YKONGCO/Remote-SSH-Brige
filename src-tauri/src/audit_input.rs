use crate::model::{AuditInput, FileRequest, Operation};
use serde_json::json;

/// Record consumed parameters, including defaults, without file bodies or credentials.
pub fn capture(op: &Operation) -> AuditInput {
    let mut truncated = false;
    let mut text = |value: &str| {
        let mut end = value.len().min(16384);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        truncated |= end < value.len();
        json!(&value[..end])
    };
    let mut args = json!({"server_id": text(&op.server_id)});
    let mut tool = op.kind.as_str();
    match op.kind.as_str() {
        kind if kind.starts_with("port_forward_") => {
            tool = "port_forward";
            args["action"] = json!(kind.trim_start_matches("port_forward_"));
            if kind == "port_forward_open" {
                args["local_port"] = json!(op.local_port);
                args["remote_host"] = text(&op.remote_host);
                args["remote_port"] = json!(op.remote_port);
            } else if kind != "port_forward_list" {
                args["tunnel_id"] = text(&op.tunnel_id);
            }
        }
        "list_servers" => {
            args = json!({});
        }
        "list_terminals" => {
            tool = "terminal";
            args = json!({"action":"list"});
        }
        "exec" => {
            args["command"] = text(&op.command);
            args["timeout_secs"] = json!(op.timeout_secs);
        }
        "job" => {
            args["action"] = text(&op.job_action);
            if op.job_action != "list" {
                args["job_id"] = text(&op.job_id);
            }
            if op.job_action == "read" {
                args["stdout_cursor"] = json!(op.stdout_cursor);
                args["stderr_cursor"] = json!(op.stderr_cursor);
                args["wait_secs"] = json!(op.wait_secs);
            }
        }
        "upload_file" | "download_file" => {
            if let Some(spec) = &op.transfer_request {
                args["local_path"] = text(&spec.local_path);
                args["remote_path"] = text(&spec.remote_path);
                args["overwrite"] = json!(spec.overwrite);
            }
        }
        "transfer" => {
            args["action"] = text(&op.transfer_action);
            if op.transfer_action != "list" {
                args["transfer_id"] = text(&op.transfer_id);
            }
            if op.transfer_action == "read" {
                args["version"] = json!(op.version);
                args["wait_secs"] = json!(op.wait_secs);
            }
        }
        kind if kind.starts_with("terminal_") => {
            tool = "terminal";
            args["action"] = json!(kind.trim_start_matches("terminal_"));
            if kind != "terminal_open" {
                args["session_id"] = text(&op.session_id);
            }
            if matches!(kind, "terminal_open" | "terminal_resize") {
                args["cols"] = json!(op.cols);
                args["rows"] = json!(op.rows);
            }
            if kind == "terminal_read" {
                args["cursor"] = json!(op.cursor);
            }
            if kind == "terminal_write" {
                args["content_bytes"] = json!(op.content.len());
                args["content_omitted"] = json!(true);
            }
        }
        _ => {
            args["path"] = text(&op.path);
            if !op.target.is_empty() {
                args["target"] = text(&op.target);
            }
            if let Some(request) = &op.file_request {
                match request {
                    FileRequest::Preview => {
                        args["preview"] = json!(true);
                        args["preview_limit_bytes"] = json!(crate::file_tools::PREVIEW_BYTES);
                    }
                    FileRequest::Read { offset, limit } | FileRequest::List { offset, limit } => {
                        args["offset"] = json!(offset);
                        args["limit"] = json!(limit);
                    }
                    FileRequest::Remove {
                        recursive,
                        missing_ok,
                    } => {
                        args["recursive"] = json!(recursive);
                        args["missing_ok"] = json!(missing_ok);
                    }
                    FileRequest::Write {
                        mode,
                        create_parents,
                    } => {
                        args["mode"] = json!(mode);
                        args["create_parents"] = json!(create_parents);
                    }
                    FileRequest::Edit { edits } => {
                        args["edit_count"] = json!(edits.len());
                        args["old_text_bytes"] =
                            json!(edits.iter().map(|edit| edit.old_text.len()).sum::<usize>());
                        args["new_text_bytes"] =
                            json!(edits.iter().map(|edit| edit.new_text.len()).sum::<usize>());
                        args["content_omitted"] = json!(true);
                    }
                }
            }
            if matches!(op.kind.as_str(), "write_file" | "upload") {
                args["content_bytes"] = json!(op.content.len());
                args["content_omitted"] = json!(true);
            }
        }
    }
    let mut data = json!({"tool":tool,"arguments":args});
    let encoded = data.to_string();
    if encoded.len() > 65536 {
        let mut end = 65536;
        while !encoded.is_char_boundary(end) {
            end -= 1;
        }
        data = json!({"preview": &encoded[..end]});
        truncated = true;
    }
    AuditInput { data, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn mcp_queries_and_failed_reads_record_inputs_and_outputs() {
        use crate::{
            model::Server,
            state::{self, AppState},
        };
        let dir = tempfile::tempdir().unwrap();
        let app = AppState::open(dir.path().to_path_buf()).unwrap();
        app.config.lock().unwrap().servers.push(Server {
            id: "s".into(),
            mcp_permissions: Some(vec!["job".into(), "transfer".into()]),
            ..Default::default()
        });
        for kind in ["job", "transfer"] {
            let op = Operation {
                kind: kind.into(),
                server_id: "s".into(),
                job_action: "list".into(),
                transfer_action: "list".into(),
                ..Default::default()
            };
            state::execute(app.clone(), op, "mcp").await.unwrap();
        }
        let op = Operation {
            kind: "job".into(),
            server_id: "s".into(),
            job_action: "read".into(),
            job_id: "job_missing".into(),
            stdout_cursor: 123,
            ..Default::default()
        };
        assert!(state::execute(app.clone(), op, "mcp").await.is_err());
        let audits = app.audit.lock().unwrap();
        assert_eq!(audits.len(), 3);
        assert!(audits[0].response.as_ref().unwrap().data["jobs"].is_array());
        assert!(audits[1].response.as_ref().unwrap().data["transfers"].is_array());
        let response = audits[2].response.as_ref().unwrap();
        assert_eq!(
            response.input.as_ref().unwrap().data["arguments"]["stdout_cursor"],
            123
        );
        assert_eq!(response.data["code"], "JOB_NOT_FOUND");
        assert!(response.data["message"].is_string());
        assert!(response.data.get("error").is_none());
    }
    #[test]
    fn request_and_response_round_trip_sqlite_and_export() {
        use crate::{model::Server, state::AppState};
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().to_path_buf()).unwrap();
        let op = Operation {
            kind: "exec".into(),
            server_id: "s".into(),
            command: format!("echo {}", "x".repeat(1200)),
            timeout_secs: 30,
            ..Default::default()
        };
        let server = Server {
            id: "s".into(),
            ..Default::default()
        };
        state
            .log(
                &server,
                "mcp",
                &op,
                &Ok(json!({"stdout":"test output","stderr":"","exit_code":0})),
                10,
            )
            .unwrap();
        let id = state.audit.lock().unwrap()[0].id.clone();
        let entry = state.audit_details(&id).unwrap();
        let response = entry.response.unwrap();
        assert_eq!(
            response.input.unwrap().data["arguments"]["command"],
            op.command
        );
        assert_eq!(response.data["stdout"], "test output");
        let db = crate::database::Database::open_reader(dir.path()).unwrap();
        let page = db.audit_page(&Default::default()).unwrap();
        assert!(page.entries[0].response.is_none());
        let mut output = Vec::new();
        db.export_audits(&Default::default(), &mut output).unwrap();
        let entries: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            entries[0]["response"]["input"]["data"]["arguments"]["timeout_secs"],
            30
        );
        let legacy: crate::model::AuditResponse =
            serde_json::from_value(json!({"data":{"stdout":"old"},"truncated":false})).unwrap();
        assert!(legacy.input.is_none());
    }
    #[test]
    fn captures_long_commands_and_marks_utf8_safe_truncation() {
        let mut op = Operation {
            kind: "exec".into(),
            command: "echo hello".into(),
            timeout_secs: 30,
            ..Default::default()
        };
        let input = capture(&op);
        assert_eq!(input.data["arguments"]["command"], "echo hello");
        assert_eq!(input.data["arguments"]["timeout_secs"], 30);
        assert!(!input.truncated);
        op.command = "中".repeat(10000);
        let input = capture(&op);
        assert!(input.truncated);
        assert!(input.data["arguments"]["command"].as_str().unwrap().len() <= 16384);
    }
    #[test]
    fn file_parameters_are_recorded_without_file_bodies() {
        let op = Operation {
            kind: "write_file".into(),
            content: "DO_NOT_STORE".into(),
            path: "/tmp/test".into(),
            file_request: Some(FileRequest::Write {
                mode: Default::default(),
                create_parents: false,
            }),
            ..Default::default()
        };
        let input = capture(&op);
        assert_eq!(input.data["arguments"]["mode"], "overwrite");
        assert_eq!(input.data["arguments"]["content_bytes"], 12);
        assert!(!input.data.to_string().contains("DO_NOT_STORE"));
    }
}
