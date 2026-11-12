//! Bounded recursive planning; every child uses normal transfer authorization.
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
pub struct BatchItem {
    pub local_path: String,
    pub remote_path: String,
}
#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upload,
    Download,
}
#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchArgs {
    pub server_id: String,
    pub direction: Direction,
    #[schemars(length(min = 1, max = 100))]
    pub items: Vec<BatchItem>,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub skip_existing: bool,
    pub request_id: Option<String>,
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
/// Turn paths from a native file drop into upload items under `remote_dir`.
pub fn upload_items(remote_dir: &str, paths: Vec<String>) -> Result<Vec<BatchItem>> {
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
            Ok(BatchItem {
                local_path,
                remote_path,
            })
        })
        .collect()
}
pub async fn start(state: Arc<AppState>, args: BatchArgs, source: &'static str) -> Result<Value> {
    anyhow::ensure!(
        !args.items.is_empty() && args.items.len() <= 100,
        "INVALID_ARGUMENT: Batch requires 1-100 items"
    );
    anyhow::ensure!(
        args.request_id
            .as_ref()
            .is_none_or(|s| !s.is_empty() && s.len() <= 80),
        "INVALID_ARGUMENT: Batch request_id must be 1-80 bytes"
    );
    let server = state.server(&args.server_id)?;
    anyhow::ensure!(server.enabled, "SERVER_DISABLED: Server is disabled");
    let upload = matches!(args.direction, Direction::Upload);
    let kind = if upload {
        "upload_file"
    } else {
        "download_file"
    };
    if source == "mcp" {
        crate::file_tools::require_mcp_permissions(&server, &[kind, "transfer"])?;
    }
    let settings = state.settings.lock().unwrap().clone();
    let planning = args.clone();
    let planning_server = server.clone();
    let planning_state = state.clone();
    let specs = tokio::task::spawn_blocking(move || -> Result<Vec<TransferSpec>> {
        let check = Operation {
            kind: kind.into(),
            server_id: planning_server.id.clone(),
            ..Default::default()
        };
        crate::security::check(&settings, &check, source)?;
        let session = if upload {
            None
        } else {
            Some(crate::ssh::connect_with_settings(
                &planning_server,
                &settings,
            )?)
        };
        let sftp = session.as_ref().map(|s| s.sftp()).transpose()?;
        let mut pending: Vec<_> = planning
            .items
            .iter()
            .map(|i| (PathBuf::from(&i.local_path), i.remote_path.clone(), 0))
            .collect();
        let mut specs = Vec::new();
        while let Some((local, remote, depth)) = pending.pop() {
            anyhow::ensure!(
                depth <= 64 && specs.len() + pending.len() < 1000,
                "TRANSFER_LIMIT: Recursive transfer exceeds 1000 entries or 64 levels"
            );
            let mut spec = TransferSpec {
                local_path: local.to_str().context("Local path is not UTF-8")?.into(),
                remote_path: remote.clone(),
                overwrite: planning.overwrite,
                size: None,
                sha256: None,
                create_parents: true,
                skip_existing: planning.skip_existing,
                directory: false,
            };
            // Validate POSIX syntax before any remote lookup.
            anyhow::ensure!(
                remote.starts_with('/')
                    && !remote.contains(['\\', '\0'])
                    && !remote.split('/').any(|p| p == ".."),
                "INVALID_ARGUMENT: Invalid remote path"
            );
            if upload {
                let metadata = std::fs::symlink_metadata(&local)?;
                anyhow::ensure!(
                    !metadata.file_type().is_symlink(),
                    "INVALID_ARGUMENT: Recursive upload does not follow symlinks"
                );
                spec.directory = metadata.is_dir();
                transfers::local_path(&spec, true, &settings, source)?;
                if spec.directory {
                    anyhow::ensure!(
                        planning.recursive,
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
                let resolved = crate::ssh::remote_path(sftp, &planning_server, &remote, false)?;
                let metadata = sftp.lstat(Path::new(&remote))?;
                anyhow::ensure!(
                    (metadata.perm.unwrap_or(0) & 0o170000) != 0o120000,
                    "INVALID_ARGUMENT: Recursive download does not follow symlinks"
                );
                spec.directory = metadata.is_dir();
                transfers::local_path(&spec, false, &settings, source)?;
                if spec.directory {
                    anyhow::ensure!(
                        planning.recursive,
                        "INVALID_ARGUMENT: Directory transfer requires recursive=true"
                    );
                    let mut dir = sftp.opendir(&resolved)?;
                    loop {
                        match dir.readdir() {
                            Ok((child, _)) => {
                                let name = child
                                    .file_name()
                                    .and_then(|n| n.to_str())
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
                            Err(e) if e.code() == ssh2::ErrorCode::Session(-16) => break,
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
            }
            specs.push(spec);
        }
        let _ = planning_state;
        // Directory ancestors are submitted before their descendants.
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
        Ok(specs)
    })
    .await??;
    let group = args
        .request_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut results = Vec::new();
    for (index, spec) in specs.into_iter().enumerate() {
        let op = Operation {
            kind: kind.into(),
            server_id: server.id.clone(),
            transfer_group: Some(group.clone()),
            request_id: args.request_id.as_ref().map(|id| format!("{id}:{index}")),
            transfer_request: Some(spec),
            ..Default::default()
        };
        let diagnostic = op.clone();
        match transfers::start(state.clone(), op, source).await {
            Ok(task) => {
                results.push(json!({"transfer_id":task["transfer_id"],"status":task["status"]}))
            }
            Err(e) => results.push(
                json!({"remote_path":diagnostic.transfer_request.as_ref().map(|s|&s.remote_path),"error":crate::operation_error::error_response(&e,&diagnostic)}),
            ),
        }
    }
    let rejected = results.iter().filter(|r| r.get("error").is_some()).count();
    Ok(
        json!({"group_id":group,"queued":results.len()-rejected,"rejected":rejected,"tasks":results}),
    )
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
