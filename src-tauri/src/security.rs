use crate::command_policy::{Assessment, RuleMatch};
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
    if op.kind == "exec" {
        anyhow::ensure!(
            !op.command.trim().is_empty(),
            "INVALID_ARGUMENT: command must not be empty"
        );
        anyhow::ensure!(
            op.command.len() <= 16384,
            "INVALID_ARGUMENT: command exceeds 16384 bytes"
        );
        if settings.effective_command_policy().exec_disabled {
            return Err(crate::operation_error::error(
                "EXEC_DISABLED",
                "Command execution is disabled",
            ));
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug, Serialize)]
pub struct ApprovalView {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    pub host: String,
    pub username: String,
    pub source: String,
    pub kind: String,
    pub command: String,
    pub reason: String,
    pub risk: String,
    pub matched_rules: Vec<RuleMatch>,
    pub command_bytes: usize,
    pub command_sha256: String,
    pub server_sha256: String,
    pub timeout_secs: u64,
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

    pub fn cancel_all(&self) {
        let mut pending = self.pending.lock().unwrap();
        for (_, request) in pending.drain() {
            let _ = request.decision.send(ApprovalDecision::Rejected);
        }
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

    pub async fn request(
        &self,
        server: &Server,
        op: &Operation,
        assessment: &Assessment,
        source: &str,
        timeout_secs: u64,
    ) -> Result<ApprovalView> {
        anyhow::ensure!(
            (1..=120).contains(&timeout_secs),
            "INVALID_ARGUMENT: approval_timeout_secs must be 1-120"
        );
        let timeout = Duration::from_secs(timeout_secs);
        let server_sha256 = sha256_hex(&serde_json::to_vec(server)?);
        let digest = sha256_hex(
            format!(
                "{}\0{}\0{}\0{}\0{}",
                server.id, server_sha256, op.command, op.timeout_secs, source
            )
            .as_bytes(),
        );
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let view = ApprovalView {
            id: id.clone(),
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            host: format!("{}:{}", server.host, server.port),
            username: server.username.clone(),
            source: source.into(),
            kind: "exec".into(),
            command: op.command.clone(),
            reason: assessment.reason.clone().unwrap_or_default(),
            risk: assessment.risk.unwrap_or("unknown").into(),
            matched_rules: assessment.matched_rules.clone(),
            command_bytes: op.command.len(),
            command_sha256: sha256_hex(op.command.as_bytes()),
            server_sha256,
            timeout_secs: op.timeout_secs,
            created_at: chrono::Utc::now().to_rfc3339(),
            expires_at: (chrono::Utc::now() + chrono::Duration::seconds(timeout_secs as i64))
                .to_rfc3339(),
        };
        {
            let mut pending = self.pending.lock().unwrap();
            Self::prune(&mut pending);
            anyhow::ensure!(pending.len() < 16, "Approval queue is full; retry later");
            anyhow::ensure!(
                !pending.values().any(|p| p.digest == digest),
                "This command is already awaiting approval"
            );
            pending.insert(
                id.clone(),
                Pending {
                    view: view.clone(),
                    inserted: Instant::now(),
                    timeout,
                    digest,
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
            Ok(Ok(ApprovalDecision::Approved)) => Ok(view),
            Ok(Ok(ApprovalDecision::Rejected)) => Err(crate::operation_error::error(
                "APPROVAL_REJECTED",
                "Approval rejected; command was not executed",
            )),
            Ok(Ok(ApprovalDecision::Expired)) | Err(_) => {
                Err(crate::operation_error::error_with_details(
                    "APPROVAL_TIMEOUT",
                    "Approval timed out; command was not executed",
                    serde_json::json!({"timeout_secs":timeout_secs,"auto_rejected":true}),
                ))
            }
            Ok(Err(_)) => Err(crate::operation_error::error(
                "APPROVAL_CANCELLED",
                "Approval cancelled; command was not executed",
            )),
        }
    }
}

struct PendingGuard<'a> {
    queue: &'a ApprovalQueue,
    id: String,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.queue.pending.lock().unwrap().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assessment() -> Assessment {
        crate::command_policy::assess("rm -rf /tmp/x", &RuntimeSettings::default())
    }

    #[tokio::test]
    async fn expired_requests_are_removed_without_execution() {
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
                    &assessment(),
                    "mcp",
                    5,
                )
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

    #[tokio::test]
    async fn approval_binds_the_original_command_and_is_consumed_once() {
        let queue = Arc::new(ApprovalQueue::default());
        let worker = queue.clone();
        let task = tokio::spawn(async move {
            worker
                .request(
                    &Server {
                        id: "s1".into(),
                        name: "prod".into(),
                        ..Default::default()
                    },
                    &Operation {
                        kind: "exec".into(),
                        command: "rm -rf /tmp/x".into(),
                        ..Default::default()
                    },
                    &assessment(),
                    "mcp",
                    30,
                )
                .await
        });
        while queue.list().is_empty() {
            tokio::task::yield_now().await;
        }
        let view = queue.list().remove(0);
        assert_eq!(view.command, "rm -rf /tmp/x");
        assert_eq!(view.kind, "exec");
        assert!(!view.matched_rules.is_empty());
        queue.decide(&view.id, true, |_| Ok(())).unwrap();
        assert_eq!(task.await.unwrap().unwrap().command, "rm -rf /tmp/x");
        assert!(queue.decide(&view.id, true, |_| Ok(())).is_err());
        assert!(queue.list().is_empty());
    }
}
