use crate::model::{Operation, RuntimeSettings, Server};
use anyhow::{bail, Context, Result};
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
            | "mkdir"
            | "remove"
            | "remove_path"
            | "rename"
            | "exec"
            | "terminal_open"
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

pub(crate) fn check_diagnostic_words(words: &[String]) -> Result<()> {
    let first = words.first().context("命令不能为空")?;
    let executable = if let Some(name) = first
        .strip_prefix("/usr/bin/")
        .or_else(|| first.strip_prefix("/bin/"))
    {
        anyhow::ensure!(!name.contains('/'), "命令保护：不允许自定义可执行文件路径");
        name
    } else {
        anyhow::ensure!(!first.contains('/'), "命令保护：不允许自定义可执行文件路径");
        first.as_str()
    };
    let diagnostics = [
        "pwd", "ls", "cat", "head", "tail", "stat", "file", "du", "df", "free", "uptime", "whoami",
        "id", "uname", "date", "hostname", "ps", "ss", "netstat", "wc", "readlink", "realpath",
    ];
    anyhow::ensure!(
        diagnostics.contains(&executable),
        "命令保护：拒绝危险或未经允许的命令「{executable}」；保护模式仅允许常用只读诊断指令"
    );
    // These otherwise diagnostic tools also expose mutating options.
    anyhow::ensure!(
        executable != "date"
            || words
                .iter()
                .skip(1)
                .all(|s| s == "-u" || s == "--utc" || s == "--iso-8601" || s.starts_with("+")),
        "命令保护：date 仅允许查询时间与指定显示格式"
    );
    anyhow::ensure!(
        executable != "hostname" || words.len() == 1,
        "命令保护：hostname 仅允许查询主机名"
    );
    anyhow::ensure!(
        executable != "ss"
            || !words.iter().skip(1).any(|s| s.starts_with("--kill")
                || (s.len() > 2 && "--kill".starts_with(s.as_str()))
                || (s.starts_with('-') && !s.starts_with("--") && s.contains('K'))),
        "命令保护：禁止终止网络连接"
    );
    anyhow::ensure!(
        executable != "file"
            || !words.iter().skip(1).any(|s| s.starts_with("--compile")
                || (s.len() > 2 && "--compile".starts_with(s.as_str()))
                || (s.starts_with('-') && !s.starts_with("--") && s.contains('C'))),
        "命令保护：禁止生成 magic 数据库"
    );
    Ok(())
}

pub fn check(settings: &RuntimeSettings, op: &Operation, source: &str) -> Result<()> {
    if settings.remote_read_only && (sensitive(op, source) || op.kind == "terminal_write") {
        bail!("只读保护已开启：禁止修改文件、执行命令和终端输入")
    }
    if settings.deletion_protection && op.kind == "remove" {
        bail!("桌面删除保护已开启：请在设置中关闭后再删除远程文件")
    }
    if source == "mcp"
        && !settings.effective_command_policy().allow_mcp_terminal
        && matches!(op.kind.as_str(), "terminal_open" | "terminal_write")
    {
        bail!("MCP 交互终端未允许：请在设置的「命令与 Shell 权限」中开启，或使用受校验的 exec 工具")
    }
    if op.kind == "exec" {
        check_command_with_settings(&op.command, settings)?;
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
    pub command: String,
    pub content_preview: String,
    pub content_bytes: usize,
    pub content_sha256: String,
    pub content_truncated: bool,
    pub created_at: String,
    pub expires_at: String,
}
struct Pending {
    view: ApprovalView,
    inserted: Instant,
    digest: String,
    budget_bytes: usize,
    decision: oneshot::Sender<bool>,
}
#[derive(Default)]
pub struct ApprovalQueue {
    pending: Mutex<HashMap<String, Pending>>,
    pub notify: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl ApprovalQueue {
    fn prune(items: &mut HashMap<String, Pending>) {
        items.retain(|_, p| {
            p.inserted.elapsed() < Duration::from_secs(120) && !p.decision.is_closed()
        });
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
        let request = pending.get(id).context("审批已失效或已处理")?;
        journal(&request.view)?;
        anyhow::ensure!(
            request.inserted.elapsed() < Duration::from_secs(120),
            "审批已过期"
        );
        let request = pending.remove(id).context("审批已失效或已处理")?;
        request
            .decision
            .send(approved)
            .map_err(|_| anyhow::anyhow!("请求已取消"))?;
        Ok(request.view)
    }
    pub async fn wait(&self, server: &Server, op: &Operation, source: &str) -> Result<()> {
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(server, op, source))?)
        );
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let review = if op.kind == "write_file" {
            std::borrow::Cow::Borrowed(op.content.as_str())
        } else if op.kind == "edit_file" {
            std::borrow::Cow::Owned(serde_json::to_string(&op.file_request)?)
        } else if op.kind == "terminal_write" {
            std::borrow::Cow::Owned(serde_json::to_string(&op.content)?)
        } else {
            std::borrow::Cow::Borrowed("")
        };
        let preview: String = review.chars().take(8192).collect();
        let content_bytes = if op.kind == "edit_file" {
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
            content_truncated: preview.len() < review.len(),
            content_preview: preview,
            content_bytes,
            content_sha256: format!(
                "{:x}",
                Sha256::digest(if op.kind == "edit_file" {
                    review.as_bytes()
                } else {
                    op.content.as_bytes()
                })
            ),
            created_at: chrono::Utc::now().to_rfc3339(),
            expires_at: (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc3339(),
        };
        {
            let mut pending = self.pending.lock().unwrap();
            Self::prune(&mut pending);
            anyhow::ensure!(pending.len() < 16, "待审批队列已满，请稍后重试");
            let budget = (96 * 1024 * 1024)
                .max(content_bytes)
                .max(pending.values().map(|p| p.budget_bytes).max().unwrap_or(0));
            anyhow::ensure!(
                pending.values().map(|p| p.budget_bytes).sum::<usize>() + content_bytes <= budget,
                "待审批内容超过队列大小限制"
            );
            anyhow::ensure!(
                !pending.values().any(|p| p.digest == digest),
                "相同操作已在等待审批"
            );
            pending.insert(
                id.clone(),
                Pending {
                    view,
                    inserted: Instant::now(),
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
        let decision = tokio::time::timeout(Duration::from_secs(120), rx).await;
        drop(guard);
        match decision {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => bail!("操作已被拒绝"),
            _ => bail!("审批超时或请求已取消，操作未执行"),
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
                .wait(&Server::default(), &Operation::default(), "mcp")
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
            .inserted = Instant::now() - Duration::from_secs(121);
        assert!(queue.list().is_empty());
        assert!(task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("未执行"));
    }
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.queue.pending.lock().unwrap().remove(&self.id);
    }
}
