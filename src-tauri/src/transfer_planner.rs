use crate::{
    model::Operation,
    state::AppState,
    transfers::{self, TransferSpec},
};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferItem {
    #[schemars(length(min = 1, max = 8192))]
    pub local_path: String,
    #[schemars(length(min = 1, max = 8192))]
    pub remote_path: String,
}

#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferRequest {
    #[schemars(length(min = 1, max = 128))]
    pub server_id: String,
    #[schemars(length(min = 1, max = 100))]
    pub items: Vec<TransferItem>,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub skip_existing: bool,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BatchErrorPolicy {
    #[default]
    #[serde(rename = "continue_on_error")]
    Continue,
    FailFast,
}

#[derive(Clone, Copy, Debug)]
pub enum Direction {
    Upload,
    Download,
}

impl Direction {
    fn kind(self) -> &'static str {
        match self {
            Self::Upload => "upload_file",
            Self::Download => "download_file",
        }
    }
    fn is_upload(self) -> bool {
        matches!(self, Self::Upload)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StartOptions {
    pub on_error: BatchErrorPolicy,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            on_error: BatchErrorPolicy::Continue,
        }
    }
}

struct Plan {
    specs: Vec<TransferSpec>,
    grouped: bool,
}

fn filename(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']),
        "INVALID_ARGUMENT: Unsafe directory entry name"
    );
    #[cfg(windows)]
    anyhow::ensure!(
        !name.contains(['<', '>', ':', '"', '|', '?', '*']) && !name.ends_with(['.', ' ']),
        "INVALID_ARGUMENT: Directory entry cannot be represented on Windows"
    );
    Ok(())
}

pub fn upload_items(remote_dir: &str, paths: Vec<String>) -> Result<Vec<TransferItem>> {
    anyhow::ensure!(
        remote_dir.starts_with('/')
            && !remote_dir.contains(['\\', '\0'])
            && !remote_dir.split('/').any(|part| part == ".."),
        "INVALID_ARGUMENT: Invalid remote directory"
    );
    anyhow::ensure!(
        !paths.is_empty(),
        "INVALID_ARGUMENT: No local files were dropped"
    );
    let base = remote_dir.trim_end_matches('/');
    paths
        .into_iter()
        .map(|local_path| {
            let name = Path::new(&local_path)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty() && *name != "." && *name != "..")
                .context("INVALID_ARGUMENT: Invalid local file name")?;
            anyhow::ensure!(
                !name.contains(['/', '\\', '\0']),
                "INVALID_ARGUMENT: Invalid local file name"
            );
            let remote_path = format!("{base}/{name}");
            Ok(TransferItem {
                local_path,
                remote_path,
            })
        })
        .collect()
}

fn plan(
    state: &Arc<AppState>,
    server: &crate::model::Server,
    direction: Direction,
    request: &TransferRequest,
    settings: &crate::model::RuntimeSettings,
    source: &str,
) -> Result<Plan> {
    anyhow::ensure!(
        (1..=100).contains(&request.items.len()),
        "INVALID_ARGUMENT: Transfer requires 1-100 items"
    );
    anyhow::ensure!(
        !(request.overwrite && request.skip_existing),
        "INVALID_ARGUMENT: overwrite and skip_existing cannot both be enabled"
    );
    let upload = direction.is_upload();
    let create_parents = request.recursive || request.items.len() > 1;
    let mut lease = if upload {
        None
    } else {
        Some(state.sftp_sessions.acquire(server, settings)?)
    };
    let result = (|| -> Result<Plan> {
        let sftp = match lease.as_mut() {
            Some(lease) => Some(lease.session().sftp()?),
            None => None,
        };
        let mut pending: Vec<_> = request
            .items
            .iter()
            .map(|item| (PathBuf::from(&item.local_path), item.remote_path.clone(), 0))
            .collect();
        let mut specs = Vec::new();
        let mut directory_item = false;
        while let Some((local, remote, depth)) = pending.pop() {
            anyhow::ensure!(
                depth <= 64 && specs.len() + pending.len() < 1000,
                "TRANSFER_LIMIT: Recursive transfer exceeds 1000 entries or 64 levels"
            );
            let mut spec = TransferSpec {
                local_path: local.to_str().context("Local path is not UTF-8")?.into(),
                remote_path: remote.clone(),
                overwrite: request.overwrite,
                size: None,
                sha256: None,
                create_parents,
                skip_existing: request.skip_existing,
                directory: false,
            };
            anyhow::ensure!(
                remote.starts_with('/')
                    && !remote.contains(['\\', '\0'])
                    && !remote.split('/').any(|part| part == ".."),
                "INVALID_ARGUMENT: Invalid remote path"
            );
            if upload {
                let metadata = std::fs::symlink_metadata(&local)?;
                anyhow::ensure!(
                    !metadata.file_type().is_symlink(),
                    "INVALID_ARGUMENT: Recursive upload does not follow symlinks"
                );
                spec.directory = metadata.is_dir();
                transfers::local_path(&spec, true, settings, source)?;
                if spec.directory {
                    directory_item = true;
                    anyhow::ensure!(
                        request.recursive,
                        "INVALID_ARGUMENT: Directory transfer requires recursive=true"
                    );
                    for child in std::fs::read_dir(&local)? {
                        let child = child?;
                        let name = child.file_name();
                        let name = name.to_str().context("Filename is not UTF-8")?;
                        filename(name)?;
                        pending.push((
                            child.path(),
                            format!("{}/{name}", remote.trim_end_matches('/')),
                            depth + 1,
                        ));
                        anyhow::ensure!(
                            specs.len() + pending.len() < 1000,
                            "TRANSFER_LIMIT: Recursive transfer exceeds 1000 entries"
                        );
                    }
                }
            } else {
                let sftp = sftp.as_ref().unwrap();
                let resolved = crate::ssh::remote_path(sftp, server, &remote, false)?;
                let metadata = sftp.lstat(Path::new(&remote))?;
                anyhow::ensure!(
                    (metadata.perm.unwrap_or(0) & 0o170000) != 0o120000,
                    "INVALID_ARGUMENT: Recursive download does not follow symlinks"
                );
                spec.directory = metadata.is_dir();
                transfers::local_path(&spec, false, settings, source)?;
                if spec.directory {
                    directory_item = true;
                    anyhow::ensure!(
                        request.recursive,
                        "INVALID_ARGUMENT: Directory transfer requires recursive=true"
                    );
                    let mut dir = sftp.opendir(&resolved)?;
                    loop {
                        match dir.readdir() {
                            Ok((child, _)) => {
                                let name = child
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .context("Filename is not UTF-8")?;
                                if matches!(name, "." | "..") {
                                    continue;
                                }
                                filename(name)?;
                                pending.push((
                                    local.join(name),
                                    format!("{}/{name}", remote.trim_end_matches('/')),
                                    depth + 1,
                                ));
                                anyhow::ensure!(
                                    specs.len() + pending.len() < 1000,
                                    "TRANSFER_LIMIT: Recursive transfer exceeds 1000 entries"
                                );
                            }
                            Err(error) if error.code() == ssh2::ErrorCode::Session(-16) => break,
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
            }
            specs.push(spec);
        }
        specs.sort_by(|a, b| {
            (
                a.remote_path.split('/').count(),
                &a.remote_path,
                &a.local_path,
            )
                .cmp(&(
                    b.remote_path.split('/').count(),
                    &b.remote_path,
                    &b.local_path,
                ))
        });
        let grouped = directory_item || specs.len() > 1 || request.items.len() > 1;
        Ok(Plan { specs, grouped })
    })();
    if let Some(lease) = lease {
        let healthy = result
            .as_ref()
            .err()
            .is_none_or(|error| !crate::transfer_engine::session_unhealthy(error));
        lease.release(healthy);
    }
    result
}

pub async fn start(
    state: Arc<AppState>,
    direction: Direction,
    request: TransferRequest,
    options: StartOptions,
    source: &'static str,
) -> Result<Value> {
    let kind = direction.kind();
    let op = Operation {
        kind: kind.into(),
        server_id: request.server_id.clone(),
        ..Default::default()
    };
    let result = start_inner(state.clone(), direction, request, options, source).await;
    if result.is_err() && source == "mcp" {
        if let Ok(server) = state.server(&op.server_id) {
            state.log(&server, source, &op, &result, 0)?;
        }
    }
    result
}

async fn start_inner(
    state: Arc<AppState>,
    direction: Direction,
    request: TransferRequest,
    options: StartOptions,
    source: &'static str,
) -> Result<Value> {
    let server = state.server(&request.server_id)?;
    anyhow::ensure!(server.enabled, "SERVER_DISABLED: Server is disabled");
    let kind = direction.kind();
    if source == "mcp" {
        crate::file_tools::require_mcp_permissions(&server, &[kind, "transfer"])?;
    }
    let settings = state.settings.lock().unwrap().clone();
    let check = Operation {
        kind: kind.into(),
        server_id: server.id.clone(),
        ..Default::default()
    };
    crate::security::check(&settings, &check, source)?;
    let planning = request.clone();
    let planning_server = server.clone();
    let planning_state = state.clone();
    let planning_settings = settings.clone();
    let specs = tokio::task::spawn_blocking(move || {
        plan(
            &planning_state,
            &planning_server,
            direction,
            &planning,
            &planning_settings,
            source,
        )
    })
    .await??;
    anyhow::ensure!(
        !specs.specs.is_empty(),
        "INVALID_ARGUMENT: Transfer produced no files"
    );
    if !specs.grouped {
        let spec = specs.specs.into_iter().next().unwrap();
        let op = Operation {
            kind: kind.into(),
            server_id: server.id.clone(),
            transfer_request: Some(spec),
            ..Default::default()
        };
        return transfers::start(state, op, source).await;
    }
    let group = uuid::Uuid::new_v4().to_string();
    state.transfers.register_group(&group, options.on_error);
    let mut results = Vec::new();
    let mut stopped = false;
    for spec in specs.specs {
        let remote_path = spec.remote_path.clone();
        if stopped {
            results.push(json!({
                "remote_path":remote_path,
                "status":"skipped",
                "error":{"code":"BATCH_ABORTED","message":"Batch stopped after an earlier failure","retryable":false},
            }));
            continue;
        }
        let op = Operation {
            kind: kind.into(),
            server_id: server.id.clone(),
            transfer_group: Some(group.clone()),
            transfer_request: Some(spec),
            ..Default::default()
        };
        match transfers::start(state.clone(), op.clone(), source).await {
            Ok(task) => {
                results.push(json!({"transfer_id":task["transfer_id"],"status":task["status"],"remote_path":task["remote_path"]}))
            }
            Err(error) => {
                results.push(json!({"remote_path":remote_path,"error":crate::operation_error::error_response(&error,&op)}));
                if options.on_error == BatchErrorPolicy::FailFast {
                    stopped = true;
                }
            }
        }
    }
    let skipped = results.iter().filter(|r| r["status"] == "skipped").count();
    let rejected = results
        .iter()
        .filter(|r| r.get("error").is_some() && r["status"] != "skipped")
        .count();
    Ok(json!({
        "group_id":group,
        "queued":results.len()-rejected-skipped,
        "rejected":rejected,
        "skipped":skipped,
        "tasks":results,
        "group":state.transfers.group_summary(&server.id,&group),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_uploads_join_the_current_remote_directory() {
        let items = upload_items(
            "/workspace/docs/",
            vec!["/tmp/report.txt".into(), "/tmp/archive".into()],
        )
        .unwrap();
        assert_eq!(items[0].local_path, "/tmp/report.txt");
        assert_eq!(items[0].remote_path, "/workspace/docs/report.txt");
        assert_eq!(items[1].remote_path, "/workspace/docs/archive");
    }

    #[test]
    fn dropped_uploads_accept_the_remote_root() {
        let items = upload_items("/", vec!["/tmp/report.txt".into()]).unwrap();
        assert_eq!(items[0].remote_path, "/report.txt");
    }

    #[test]
    fn dropped_uploads_reject_unsafe_inputs() {
        for remote in [
            "",
            "workspace",
            "/workspace/../etc",
            "/work\\space",
            "/work\0space",
        ] {
            assert!(
                upload_items(remote, vec!["/tmp/report.txt".into()]).is_err(),
                "{remote:?}"
            );
        }
        assert!(upload_items("/workspace", Vec::new()).is_err());
        assert!(upload_items("/workspace", vec!["/".into()]).is_err());
    }
}
