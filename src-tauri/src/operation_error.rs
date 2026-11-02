//! Shared error classification and public responses for MCP, transfers and audit.
use crate::model::Operation;
use serde_json::{json, Value};
use std::fmt;

#[derive(Debug)]
pub struct OperationError {
    pub code: &'static str,
    message: &'static str,
    pub details: Option<Value>,
}
impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        // Desktop IPC still renders errors as strings. Preserve the actionable
        // reason there while structured responses read message and details directly.
        if let Some(reason) = self
            .details
            .as_ref()
            .and_then(|details| details["reason"].as_str())
        {
            if !reason.is_empty() && reason != self.message {
                write!(f, "; {reason}")?;
            }
        }
        Ok(())
    }
}
impl std::error::Error for OperationError {}
pub fn error(code: &'static str, message: &'static str) -> anyhow::Error {
    OperationError {
        code,
        message,
        details: None,
    }
    .into()
}
pub fn error_with_details(
    code: &'static str,
    message: &'static str,
    details: Value,
) -> anyhow::Error {
    OperationError {
        code,
        message,
        details: Some(details),
    }
    .into()
}

pub fn error_code(error: &anyhow::Error) -> &'static str {
    if let Some(error) = error.downcast_ref::<OperationError>() {
        return error.code;
    }
    if let Some(error) = error.downcast_ref::<ssh2::Error>() {
        return match error.code() {
            ssh2::ErrorCode::SFTP(2) => "NOT_FOUND",
            ssh2::ErrorCode::SFTP(3) => "PERMISSION_DENIED",
            ssh2::ErrorCode::SFTP(11) => "ALREADY_EXISTS",
            ssh2::ErrorCode::SFTP(4) => "SFTP_FAILURE",
            ssh2::ErrorCode::SFTP(6 | 7) => "CONNECTION_LOST",
            ssh2::ErrorCode::SFTP(8) => "UNSUPPORTED_OPERATION",
            ssh2::ErrorCode::SFTP(10) => "NOT_FOUND",
            ssh2::ErrorCode::SFTP(12) => "WRITE_PROTECTED",
            ssh2::ErrorCode::SFTP(14) => "NO_SPACE",
            ssh2::ErrorCode::SFTP(15) => "QUOTA_EXCEEDED",
            ssh2::ErrorCode::SFTP(18) => "DIRECTORY_NOT_EMPTY",
            ssh2::ErrorCode::SFTP(19) => "NOT_A_DIRECTORY",
            ssh2::ErrorCode::Session(-18) => "SSH_AUTHENTICATION_FAILED",
            ssh2::ErrorCode::Session(-9 | -30) => "SSH_TIMEOUT",
            _ => "SSH_ERROR",
        };
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        return match error.kind() {
            std::io::ErrorKind::NotFound => "NOT_FOUND",
            std::io::ErrorKind::PermissionDenied => "PERMISSION_DENIED",
            std::io::ErrorKind::AlreadyExists => "ALREADY_EXISTS",
            std::io::ErrorKind::TimedOut => "IO_TIMEOUT",
            std::io::ErrorKind::ConnectionRefused => "CONNECTION_REFUSED",
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe => {
                "CONNECTION_LOST"
            }
            _ => "IO_ERROR",
        };
    }
    for code in [
        "INVALID_ARGUMENT",
        "SOURCE_CHANGED",
        "ALREADY_EXISTS",
        "PAYLOAD_TOO_LARGE",
        "TRANSFER_LIMIT",
        "NOT_FOUND",
        "PERMISSION_DENIED",
        "SERVER_DISABLED",
    ] {
        if error
            .chain()
            .any(|cause| cause.to_string().starts_with(&format!("{code}:")))
        {
            return code;
        }
    }
    "OPERATION_FAILED"
}

#[derive(Debug)]
pub struct ErrorStage(pub &'static str, String);
impl fmt::Display for ErrorStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.1)
    }
}
impl std::error::Error for ErrorStage {}
pub fn with_stage(error: anyhow::Error, stage: &'static str) -> anyhow::Error {
    let message = error.to_string();
    error.context(ErrorStage(stage, message))
}

/// Shared by MCP results, asynchronous transfer failures and audit previews.
pub fn error_response(error: &anyhow::Error, op: &Operation) -> Value {
    let code = error_code(error);
    let stage = error
        .downcast_ref::<ErrorStage>()
        .map(|s| s.0)
        .unwrap_or(match code {
            "INVALID_ARGUMENT" | "PAYLOAD_TOO_LARGE" => "validation",
            "LOCAL_PATH_DENIED" | "PATH_OUTSIDE_ROOT" | "SERVER_DISABLED" | "SERVER_NOT_FOUND" => {
                "authorization"
            }
            "READ_ONLY_POLICY"
            | "DELETION_PROTECTED"
            | "TERMINAL_POLICY_DENIED"
            | "COMMAND_POLICY_DENIED" => "policy",
            "APPROVAL_TIMEOUT" | "APPROVAL_REJECTED" | "APPROVAL_CANCELLED" => "approval",
            _ => "execution",
        });
    let mut details = error
        .downcast_ref::<OperationError>()
        .and_then(|e| e.details.clone())
        .unwrap_or_else(|| json!({}));
    if !details.is_object() {
        details = json!({"data":details});
    }
    if let Some(object) = details.as_object_mut() {
        object.insert("operation".into(), json!(op.kind));
        object.insert("server_id".into(), json!(op.server_id));
        if !op.path.is_empty() {
            object.insert("path".into(), json!(op.path));
        }
        if !op.session_id.is_empty() {
            object.insert("session_id".into(), json!(op.session_id));
        }
        if !op.transfer_id.is_empty() {
            object.insert("transfer_id".into(), json!(op.transfer_id));
        }
        if !op.tunnel_id.is_empty() {
            object.insert("tunnel_id".into(), json!(op.tunnel_id));
        }
        if let Some(e) = error.downcast_ref::<ssh2::Error>() {
            object.insert("ssh_code".into(), json!(format!("{:?}", e.code())));
        }
        if let Some(e) = error.downcast_ref::<std::io::Error>() {
            object.insert("os_code".into(), json!(e.raw_os_error()));
        }
    }
    let message = error_message(error);
    let causes = error_causes(error, code, &message, &details);
    if !causes.is_empty() {
        details["causes"] = json!(causes);
    }
    json!({"code":code,"stage":stage,"message":message,"details":details})
}

/// A business error owns its public message; context belongs in details.
pub fn error_message(error: &anyhow::Error) -> String {
    if let Some(known) = error.downcast_ref::<OperationError>() {
        return known.message.into();
    }
    let code = error_code(error);
    let cause = error.chain().find(|cause| !cause.is::<ErrorStage>());
    if let Some(cause) = cause {
        if cause.is::<std::io::Error>() || cause.is::<ssh2::Error>() {
            if let Some(message) = native_message(code) {
                return message.into();
            }
        }
        return without_code(&cause.to_string(), code);
    }
    "Operation failed".into()
}

/// Native OS diagnostics can be localized; public error templates stay English.
pub(crate) fn native_message(code: &str) -> Option<&'static str> {
    Some(match code {
        "NOT_FOUND" => "Path not found",
        "PERMISSION_DENIED" => "File access denied",
        "ALREADY_EXISTS" => "Target already exists",
        "CONNECTION_REFUSED" => "Connection refused",
        "CONNECTION_LOST" => "Connection lost",
        "SSH_AUTHENTICATION_FAILED" => "SSH authentication failed",
        "SSH_TIMEOUT" | "IO_TIMEOUT" => "Connection or I/O timed out",
        "SFTP_FAILURE" => "SFTP operation failed",
        "UNSUPPORTED_OPERATION" => "SFTP operation is not supported",
        "WRITE_PROTECTED" => "Target is write-protected",
        "NO_SPACE" => "Remote disk is full",
        "QUOTA_EXCEEDED" => "Remote disk quota exceeded",
        "DIRECTORY_NOT_EMPTY" => "Directory is not empty",
        "NOT_A_DIRECTORY" => "Path is not a directory",
        "IO_ERROR" => "File I/O failed",
        _ => return None,
    })
}

fn without_code(message: &str, code: &str) -> String {
    message
        .strip_prefix(code)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(message)
        .trim()
        .to_owned()
}

fn error_causes(error: &anyhow::Error, code: &str, message: &str, details: &Value) -> Vec<String> {
    let mut causes = Vec::new();
    for cause in error
        .chain()
        .filter(|cause| !cause.is::<ErrorStage>() && !cause.is::<OperationError>())
    {
        let text = without_code(&cause.to_string(), code);
        if !text.is_empty()
            && text != message
            && details["reason"].as_str() != Some(text.as_str())
            && !causes.contains(&text)
        {
            causes.push(text);
        }
    }
    causes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_errors_do_not_repeat_codes_reasons_or_stage_context() {
        let business = error_with_details(
            "COMMAND_POLICY_DENIED",
            "命令未执行",
            json!({"reason":"禁止重定向"}),
        );
        let display = business.to_string();
        assert!(display.contains("禁止重定向"));
        assert_eq!(display.matches("禁止重定向").count(), 1);
        let error = with_stage(business.context("禁止重定向"), "policy");
        let value = error_response(&error, &Operation::default());
        assert_eq!(value["message"], "命令未执行");
        assert_eq!(value["stage"], "policy");
        assert_eq!(value["details"]["reason"], "禁止重定向");
        assert!(value["details"].get("causes").is_none());
    }

    #[test]
    fn unknown_error_preserves_unique_diagnostic_causes() {
        let error = anyhow::anyhow!("disk full")
            .context("cannot write file")
            .context("cannot write file");
        let value = error_response(&with_stage(error, "execution"), &Operation::default());
        assert_eq!(value["message"], "cannot write file");
        assert_eq!(value["details"]["causes"], json!(["disk full"]));
    }

    #[test]
    fn legacy_code_prefix_is_removed_without_splitting_paths() {
        let error = anyhow::anyhow!("INVALID_ARGUMENT: invalid path: C:/tmp/file");
        let value = error_response(&error, &Operation::default());
        assert_eq!(value["code"], "INVALID_ARGUMENT");
        assert_eq!(value["message"], "invalid path: C:/tmp/file");
        assert!(value["details"].get("causes").is_none());
    }

    #[test]
    fn native_errors_keep_code_and_diagnostics_separate() {
        let error = anyhow::Error::from(ssh2::Error::from_errno(ssh2::ErrorCode::SFTP(3)))
            .context("cannot open remote file");
        let value = error_response(&error, &Operation::default());
        assert_eq!(value["code"], "PERMISSION_DENIED");
        assert_eq!(value["message"], "cannot open remote file");
        assert_eq!(value["details"]["ssh_code"], "SFTP(3)");
        assert_eq!(value["details"]["causes"].as_array().unwrap().len(), 1);
    }
}
