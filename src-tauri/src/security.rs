use crate::model::{Operation, RuntimeSettings, Server};
use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

pub fn sensitive(op: &Operation, source: &str) -> bool {
    matches!(
        op.kind.as_str(),
        "write_file"
            | "edit_file"
            | "upload"
            | "upload_file"
            | "download_file"
            | "mkdir"
            | "remove"
            | "remove_path"
            | "rename"
            | "exec"
            | "terminal_open"
            | "port_forward_open"
    ) || (source == "mcp" && op.kind == "terminal_write")
}

pub fn needs_approval(settings: &RuntimeSettings, op: &Operation, source: &str) -> bool {
    sensitive(op, source)
        && (settings.approval_mode == "all" || (settings.approval_mode == "mcp" && source == "mcp"))
}

pub fn check_command(command: &str) -> Result<()> {
    check_command_with_settings(command, &RuntimeSettings::default())
}

pub fn check_command_with_settings(command: &str, settings: &RuntimeSettings) -> Result<()> {
    crate::command_policy::check(command, settings)
}

pub(crate) const DIAGNOSTIC_COMMANDS: &[&str] = &[
    "pwd", "ls", "cat", "head", "tail", "stat", "file", "du", "df", "free", "uptime", "whoami",
    "id", "uname", "date", "hostname", "ps", "ss", "netstat", "wc", "readlink", "realpath", "true",
    "false", ":", "yes", "set",
];

pub(crate) fn check_diagnostic_words(words: &[String]) -> Result<()> {
    let first = words.first().context("command must not be empty")?;
    anyhow::ensure!(
        crate::command_policy::trusted_executable(first),
        "Executable path is outside trusted directories: {first}"
    );
    let executable = first.rsplit('/').next().unwrap_or(first);
    anyhow::ensure!(
        DIAGNOSTIC_COMMANDS.contains(&executable),
        "Command '{executable}' is not allowed in diagnostic-only mode"
    );
    // These otherwise diagnostic tools also expose mutating options.
    anyhow::ensure!(
        executable != "date"
            || words
                .iter()
                .skip(1)
                .all(|s| s == "-u" || s == "--utc" || s == "--iso-8601" || s.starts_with("+")),
        "date may only query or format time"
    );
    anyhow::ensure!(
        executable != "hostname" || words.len() == 1,
        "hostname may only query the hostname"
    );
    anyhow::ensure!(
        executable != "ss"
            || !words.iter().skip(1).any(|s| s.starts_with("--kill")
                || (s.len() > 2 && "--kill".starts_with(s.as_str()))
                || (s.starts_with('-') && !s.starts_with("--") && s.contains('K'))),
        "Terminating network connections is blocked"
    );
    anyhow::ensure!(
        executable != "file"
            || !words.iter().skip(1).any(|s| s.starts_with("--compile")
                || (s.len() > 2 && "--compile".starts_with(s.as_str()))
                || (s.starts_with('-') && !s.starts_with("--") && s.contains('C'))),
        "Compiling a magic database is blocked"
    );
    Ok(())
}

pub fn check(settings: &RuntimeSettings, op: &Operation, source: &str) -> Result<()> {
    if settings.remote_read_only
        && ((sensitive(op, source) && op.kind != "download_file") || op.kind == "terminal_write")
    {
        return Err(crate::operation_error::error(
            "READ_ONLY_POLICY",
            "Read-only mode blocks this operation",
        ));
    }
    if settings.deletion_protection && matches!(op.kind.as_str(), "remove" | "remove_path") {
        return Err(crate::operation_error::error(
            "DELETION_PROTECTED",
            "Deletion protection is enabled",
        ));
    }
    if source == "mcp"
        && !settings.effective_command_policy().allow_mcp_terminal
        && matches!(op.kind.as_str(), "terminal_open" | "terminal_write")
    {
        return Err(crate::operation_error::error(
            "TERMINAL_POLICY_DENIED",
            "MCP interactive terminals are disabled",
        ));
    }
    if op.kind == "exec" {
        check_command_with_settings(&op.command, settings).map_err(|e| {
            crate::operation_error::error_with_details(
                "COMMAND_POLICY_DENIED",
                "Command rejected by shell policy",
                serde_json::json!({
                    "command": op.command,
                    "reason": crate::operation_error::error_message(&e),
                }),
            )
        })?;
    }
    Ok(())
}

#[derive(Clone, Serialize)]
pub struct ApprovalView {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    pub host: String,
    pub username: String,
    pub source: String,
    pub kind: String,
    pub path: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_parents: Option<bool>,
    pub command: String,
    pub content_preview: String,
    pub content_bytes: usize,
    pub content_sha256: String,
    pub content_truncated: bool,
    pub created_at: String,
    pub expires_at: String,
}
enum ApprovalDecision {
    Approved,
    Rejected,
    Expired,
}
struct Pending {
    view: ApprovalView,
    inserted: Instant,
    timeout: Duration,
    digest: String,
    budget_bytes: usize,
    decision: oneshot::Sender<ApprovalDecision>,
}
#[derive(Default)]
pub struct ApprovalQueue {
    pending: Mutex<HashMap<String, Pending>>,
    pub notify: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl ApprovalQueue {
    fn prune(items: &mut HashMap<String, Pending>) {
        let expired: Vec<_> = items
            .iter()
            .filter(|(_, p)| p.inserted.elapsed() >= p.timeout)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(p) = items.remove(&id) {
                let _ = p.decision.send(ApprovalDecision::Expired);
            }
        }
        items.retain(|_, p| !p.decision.is_closed());
    }
    pub fn list(&self) -> Vec<ApprovalView> {
        let mut pending = self.pending.lock().unwrap();
        Self::prune(&mut pending);
        let mut result: Vec<_> = pending.values().map(|p| p.view.clone()).collect();
        result.sort_by(|a, b| a.expires_at.cmp(&b.expires_at));
        result
    }
    pub fn decide(
        &self,
        id: &str,
        approved: bool,
        journal: impl FnOnce(&ApprovalView) -> Result<()>,
    ) -> Result<ApprovalView> {
        let mut pending = self.pending.lock().unwrap();
        Self::prune(&mut pending);
        let request = pending
            .get(id)
            .context("Approval request has expired or was already handled")?;
        journal(&request.view)?;
        anyhow::ensure!(
            request.inserted.elapsed() < request.timeout,
            "Approval request has expired"
        );
        let request = pending
            .remove(id)
            .context("Approval request has expired or was already handled")?;
        request
            .decision
            .send(if approved {
                ApprovalDecision::Approved
            } else {
                ApprovalDecision::Rejected
            })
            .map_err(|_| anyhow::anyhow!("Request was cancelled"))?;
        Ok(request.view)
    }
    pub async fn wait(&self, server: &Server, op: &Operation, source: &str) -> Result<()> {
        self.wait_with_timeout(
            server,
            op,
            source,
            crate::model::DEFAULT_APPROVAL_TIMEOUT_SECS,
        )
        .await
    }
    pub async fn wait_with_timeout(
        &self,
        server: &Server,
        op: &Operation,
        source: &str,
        timeout_secs: u64,
    ) -> Result<()> {
        anyhow::ensure!(
            (1..=120).contains(&timeout_secs),
            "INVALID_ARGUMENT: approval_timeout_secs must be 1-120"
        );
        let timeout = Duration::from_secs(timeout_secs);
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(server, op, source))?)
        );
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let review = if matches!(op.kind.as_str(), "upload_file" | "download_file") {
            std::borrow::Cow::Owned(serde_json::to_string(&op.transfer_request)?)
        } else if op.kind == "write_file" {
            std::borrow::Cow::Borrowed(op.content.as_str())
        } else if op.kind == "edit_file" {
            std::borrow::Cow::Owned(serde_json::to_string(&op.file_request)?)
        } else if op.kind == "terminal_write" {
            std::borrow::Cow::Owned(serde_json::to_string(&op.content)?)
        } else {
            std::borrow::Cow::Borrowed("")
        };
        let preview: String = review.chars().take(8192).collect();
        let content_bytes = if op.kind == "edit_file" || op.transfer_request.is_some() {
            review.len()
        } else {
            op.content.len()
        };
        let view = ApprovalView {
            id: id.clone(),
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            host: format!("{}:{}", server.host, server.port),
            username: server.username.clone(),
            source: source.into(),
            kind: op.kind.clone(),
            path: op.path.clone(),
            target: op.target.clone(),
            recursive: match op.file_request {
                Some(crate::model::FileRequest::Remove { recursive, .. }) => Some(recursive),
                _ => None,
            },
            missing_ok: match op.file_request {
                Some(crate::model::FileRequest::Remove { missing_ok, .. }) => Some(missing_ok),
                _ => None,
            },
            command: op.command.clone(),
            create_parents: match op.file_request {
                Some(crate::model::FileRequest::Write { create_parents, .. }) => {
                    Some(create_parents)
                }
                _ => None,
            },
            content_truncated: preview.len() < review.len(),
            content_preview: preview,
            content_bytes,
            content_sha256: format!(
                "{:x}",
                Sha256::digest(if op.kind == "edit_file" || op.transfer_request.is_some() {
                    review.as_bytes()
                } else {
                    op.content.as_bytes()
                })
            ),
            created_at: chrono::Utc::now().to_rfc3339(),
            expires_at: (chrono::Utc::now() + chrono::Duration::seconds(timeout_secs as i64))
                .to_rfc3339(),
        };
        {
            let mut pending = self.pending.lock().unwrap();
            Self::prune(&mut pending);
            anyhow::ensure!(pending.len() < 16, "Approval queue is full; retry later");
            let budget = (96 * 1024 * 1024)
                .max(content_bytes)
                .max(pending.values().map(|p| p.budget_bytes).max().unwrap_or(0));
            anyhow::ensure!(
                pending.values().map(|p| p.budget_bytes).sum::<usize>() + content_bytes <= budget,
                "Pending approval data exceeds the queue limit"
            );
            anyhow::ensure!(
                !pending.values().any(|p| p.digest == digest),
                "This operation is already awaiting approval"
            );
            pending.insert(
                id.clone(),
                Pending {
                    view,
                    inserted: Instant::now(),
                    timeout,
                    digest,
                    budget_bytes: content_bytes,
                    decision: tx,
                },
            );
        }
        if let Some(notify) = self.notify.lock().unwrap().as_ref() {
            notify();
        }
        let guard = PendingGuard { queue: self, id };
        let decision = tokio::time::timeout(timeout, rx).await;
        drop(guard);
        match decision {
            Ok(Ok(ApprovalDecision::Approved)) => Ok(()),
            Ok(Ok(ApprovalDecision::Rejected)) => Err(crate::operation_error::error(
                "APPROVAL_REJECTED",
                "Approval rejected; operation was not executed",
            )),
            Ok(Ok(ApprovalDecision::Expired)) | Err(_) => {
                Err(crate::operation_error::error_with_details(
                    "APPROVAL_TIMEOUT",
                    "Approval timed out; operation was not executed",
                    serde_json::json!({"timeout_secs":timeout_secs,"auto_rejected":true}),
                ))
            }
            Ok(Err(_)) => Err(crate::operation_error::error(
                "APPROVAL_CANCELLED",
                "Approval cancelled; operation was not executed",
            )),
        }
    }
}
struct PendingGuard<'a> {
    queue: &'a ApprovalQueue,
    id: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn expired_requests_are_removed_without_execution() {
        let queue = Arc::new(ApprovalQueue::default());
        let worker = queue.clone();
        let task = tokio::spawn(async move {
            worker
                .wait_with_timeout(&Server::default(), &Operation::default(), "mcp", 5)
                .await
        });
        while queue.list().is_empty() {
            tokio::task::yield_now().await;
        }
        queue
            .pending
            .lock()
            .unwrap()
            .values_mut()
            .next()
            .unwrap()
            .inserted = Instant::now() - Duration::from_secs(6);
        assert!(queue.list().is_empty());
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(
            crate::operation_error::error_code(&error),
            "APPROVAL_TIMEOUT"
        );
        assert!(error.to_string().contains("not executed"));
        assert!(queue.decide("missing", true, |_| Ok(())).is_err());
    }
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.queue.pending.lock().unwrap().remove(&self.id);
    }
}
