//! Resumable SFTP streams. Checkpoints only refer to task-owned temporary files.
use crate::{
    model::{Operation, RuntimeSettings, Server},
    operation_error::{error, error_code},
    state::AppState,
    transfers::{Handle, CHUNK_BYTES},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Checkpoint {
    source: Option<Value>,
    pub(crate) temp: String,
    hashes: Vec<String>,
    pub(crate) digest: Option<String>,
    pub(crate) commit_pending: bool,
}

fn digest_reader(reader: &mut impl Read, h: &Handle) -> Result<String> {
    let mut hash = Sha256::new();
    let mut bytes = vec![0; CHUNK_BYTES];
    loop {
        h.check_cancelled()?;
        let n = reader.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub(crate) fn hash_local(path: &Path, h: &Handle) -> Result<String> {
    digest_reader(&mut File::open(path)?, h)
}

fn local_identity(file: &File) -> Result<Value> {
    let m = file.metadata()?;
    Ok(
        json!({"size":m.len(),"modified":m.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos().to_string()}),
    )
}
fn source_identity(h: &Handle, identity: Value) -> Result<()> {
    {
        let mut cp = h.checkpoint.lock().unwrap();
        if let Some(before) = &cp.source {
            anyhow::ensure!(
                *before == identity,
                "SOURCE_CHANGED: Source file changed; create a new transfer"
            );
        } else {
            cp.source = Some(identity);
        }
    }
    h.persist()
}
fn local_temp(path: &Path) -> Result<File> {
    if let Ok(m) = std::fs::symlink_metadata(path) {
        anyhow::ensure!(
            m.is_file() && !m.file_type().is_symlink(),
            "INVALID_ARGUMENT: Temporary path is not a regular file"
        );
        Ok(OpenOptions::new().read(true).write(true).open(path)?)
    } else {
        Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?)
    }
}
fn remote_temp(sftp: &ssh2::Sftp, path: &Path) -> Result<ssh2::File> {
    match sftp.lstat(path) {
        Ok(m) => {
            anyhow::ensure!(
                m.is_file(),
                "INVALID_ARGUMENT: Remote temporary path is not a regular file"
            );
            Ok(sftp.open_mode(
                path,
                ssh2::OpenFlags::READ | ssh2::OpenFlags::WRITE,
                0o600,
                ssh2::OpenType::File,
            )?)
        }
        Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => Ok(sftp.open_mode(
            path,
            ssh2::OpenFlags::READ
                | ssh2::OpenFlags::WRITE
                | ssh2::OpenFlags::CREATE
                | ssh2::OpenFlags::EXCLUSIVE,
            0o600,
            ssh2::OpenType::File,
        )?),
        Err(e) => Err(e.into()),
    }
}
fn block_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// ssh2's std::io conversion discards the numeric SSH error. Preserve transport
// classification at the boundary instead of guessing from English messages.
struct RemoteFile<'a> {
    inner: ssh2::File,
    session: &'a ssh2::Session,
}
impl std::ops::Deref for RemoteFile<'_> {
    type Target = ssh2::File;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for RemoteFile<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
fn io_error(session: &ssh2::Session, original: std::io::Error) -> std::io::Error {
    if let Some(native) = ssh2::Error::last_session_error(session) {
        if matches!(native.code(), ssh2::ErrorCode::Session(-9 | -30)) {
            return std::io::Error::new(std::io::ErrorKind::TimedOut, native);
        }
        if matches!(
            native.code(),
            ssh2::ErrorCode::Session(-7 | -13 | -26 | -43)
        ) {
            return std::io::Error::new(std::io::ErrorKind::ConnectionReset, native);
        }
        return std::io::Error::new(original.kind(), native);
    }
    original
}
impl Read for RemoteFile<'_> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(b).map_err(|e| io_error(self.session, e))
    }
}
impl Write for RemoteFile<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.inner.write(b).map_err(|e| io_error(self.session, e))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().map_err(|e| io_error(self.session, e))
    }
}
impl Seek for RemoteFile<'_> {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(p).map_err(|e| io_error(self.session, e))
    }
}
fn remote_open<'a>(
    sftp: &ssh2::Sftp,
    path: &Path,
    session: &'a ssh2::Session,
) -> Result<RemoteFile<'a>> {
    Ok(RemoteFile {
        inner: sftp.open(path)?,
        session,
    })
}

/// Verify retained blocks before seeking a newly opened stream. Unconfirmed tails
/// are overwritten, never treated as acknowledged progress.
fn verify_prefix(
    source: &mut (impl Read + Seek),
    destination: &mut (impl Read + Seek),
    total: u64,
    h: &Handle,
    upload: bool,
) -> Result<(u64, Sha256)> {
    let hashes = h.checkpoint.lock().unwrap().hashes.clone();
    let mut bytes = vec![0; CHUNK_BYTES];
    let mut other = vec![0; CHUNK_BYTES];
    let mut offset = 0;
    let mut whole = Sha256::new();
    let mut valid = 0;
    for expected in &hashes {
        h.check_cancelled()?;
        let n = (total - offset).min(CHUNK_BYTES as u64) as usize;
        if n == 0 {
            break;
        }
        if upload {
            source.read_exact(&mut other[..n])?;
            anyhow::ensure!(
                block_hash(&other[..n]) == *expected,
                "SOURCE_CHANGED: Upload source content changed"
            );
        }
        match destination.read_exact(&mut bytes[..n]) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        if block_hash(&bytes[..n]) != *expected {
            break;
        }
        whole.update(&bytes[..n]);
        offset += n as u64;
        valid += 1;
    }
    h.checkpoint.lock().unwrap().hashes.truncate(valid);
    // These handles have no in-flight writes; seeking them is safe.
    source.seek(SeekFrom::Start(offset))?;
    h.resumed(offset);
    h.persist()?;
    Ok((offset, whole))
}

fn stream(
    source: &mut impl Read,
    destination: &mut impl Write,
    total: u64,
    initial: u64,
    mut hash: Sha256,
    settings: &RuntimeSettings,
    h: &Handle,
) -> Result<String> {
    anyhow::ensure!(
        total
            <= settings.transfer_limit(if h.operation.lock().unwrap().kind == "upload_file" {
                "upload_file"
            } else {
                "download_file"
            }),
        "PAYLOAD_TOO_LARGE: File exceeds transfer limit"
    );
    let mut bytes = vec![0; CHUNK_BYTES];
    let mut offset = initial;
    let start = Instant::now();
    let mut progress = Instant::now();
    let window = if h.operation.lock().unwrap().kind.is_empty() {
        CHUNK_BYTES
    } else if h.retry_count() > 0 {
        64 * 1024
    } else {
        CHUNK_BYTES
    };
    h.phase("transferring");
    h.report(offset, total, start, initial);
    while offset < total {
        h.check_cancelled()?;
        let n = (total - offset).min(CHUNK_BYTES as u64) as usize;
        let mut read = 0;
        let mut last = Instant::now();
        while read < n {
            h.check_cancelled()?;
            anyhow::ensure!(
                last.elapsed() < Duration::from_secs(settings.transfers.idle_timeout_secs),
                "IO_TIMEOUT: Transfer made no progress"
            );
            match source.read(&mut bytes[read..n.min(read + window)]) {
                Ok(0) => {
                    return Err(error(
                        "SOURCE_CHANGED",
                        "Source file was truncated during transfer",
                    ))
                }
                Ok(amount) => {
                    read += amount;
                    last = Instant::now();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        let mut written = 0;
        let mut last = Instant::now();
        while written < n {
            h.check_cancelled()?;
            anyhow::ensure!(
                last.elapsed() < Duration::from_secs(settings.transfers.idle_timeout_secs),
                "IO_TIMEOUT: Transfer made no progress"
            );
            match destination.write(&bytes[written..n.min(written + window)]) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "Transfer stream closed",
                    )
                    .into())
                }
                Ok(amount) => {
                    written += amount;
                    last = Instant::now();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
            if progress.elapsed() >= Duration::from_millis(200) {
                h.report(offset + written as u64, total, start, initial);
                progress = Instant::now();
            }
            let rate = h.bandwidth(&settings.transfers);
            if rate > 0 {
                let due = Duration::from_secs_f64(
                    (offset + written as u64 - initial) as f64 / (rate as f64 * 1024.0),
                );
                while start.elapsed() < due {
                    h.check_cancelled()?;
                    std::thread::sleep(
                        (due - start.elapsed().min(due)).min(Duration::from_millis(50)),
                    );
                }
            }
        }
        hash.update(&bytes[..n]);
        offset += n as u64;
        h.checkpoint
            .lock()
            .unwrap()
            .hashes
            .push(block_hash(&bytes[..n]));
        h.report(offset, total, start, initial);
        // Bound disk writes without losing the verified in-memory checkpoint.
        if offset % (4 * CHUNK_BYTES as u64) == 0 || offset == total {
            destination.flush()?;
            h.persist()?;
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn transfer(
    state: &AppState,
    server: &Server,
    op: &Operation,
    settings: &RuntimeSettings,
    source: &str,
    local: &Path,
    h: &Handle,
) -> Result<Value> {
    h.check_cancelled()?;
    let mut connection = settings.clone();
    connection.io_timeout_secs = settings
        .transfers
        .io_timeout_secs
        .min(settings.transfers.idle_timeout_secs);
    let session = crate::ssh::connect_with_settings(server, &connection)?;
    let sftp = session.sftp()?;
    let upload = op.kind == "upload_file";
    let spec = op
        .transfer_request
        .as_ref()
        .context("Missing transfer spec")?;
    if upload && spec.create_parents {
        crate::ssh::create_parent_directories(&sftp, server, &spec.remote_path)?;
    }
    let remote = crate::ssh::remote_path(&sftp, server, &spec.remote_path, upload)?;
    if spec.directory {
        crate::transfers::validate_commit(state, server, op, source, local)?;
        if upload {
            match sftp.lstat(&remote) {
                Ok(st) => anyhow::ensure!(st.is_dir(), "ALREADY_EXISTS: Target is not a directory"),
                Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => {
                    if let Err(e) = sftp.mkdir(&remote, 0o755) {
                        if !sftp.lstat(&remote).is_ok_and(|st| st.is_dir()) {
                            return Err(e.into());
                        }
                    }
                }
                Err(e) => return Err(e.into()),
            }
        } else {
            anyhow::ensure!(
                sftp.stat(&remote)?.is_dir(),
                "SOURCE_CHANGED: Remote source is not a directory"
            );
            std::fs::create_dir_all(local)?;
            crate::transfers::validate_commit(state, server, op, source, local)?;
        }
        h.resumed(0);
        return Ok(json!({"directory":true}));
    }
    if !upload && spec.create_parents {
        std::fs::create_dir_all(local.parent().context("Invalid local parent")?)?;
        crate::transfers::validate_commit(state, server, op, source, local)?;
    }
    let id = &op.transfer_id;
    let temp = if upload {
        remote
            .parent()
            .context("Invalid remote parent")?
            .join(format!(".remote-ssh-brige-{id}.part"))
    } else {
        local
            .parent()
            .context("Invalid local parent")?
            .join(format!(".remote-ssh-brige-{id}.part"))
    };
    {
        let mut cp = h.checkpoint.lock().unwrap();
        if !cp.temp.is_empty() {
            anyhow::ensure!(
                Path::new(&cp.temp) == temp,
                "SOURCE_CHANGED: Temporary path changed"
            );
        }
        cp.temp = if upload {
            temp.to_string_lossy().replace('\\', "/")
        } else {
            temp.to_string_lossy().into_owned()
        };
    }
    h.persist()?;
    let pending = h.checkpoint.lock().unwrap().clone();
    if pending.commit_pending {
        h.phase("reconciling");
        let expected = pending.digest.context("Missing commit checksum")?;
        let result = if upload {
            remote_open(&sftp, &remote, &session).and_then(|mut f| digest_reader(&mut f, h))
        } else {
            File::open(local)
                .map_err(anyhow::Error::from)
                .and_then(|mut f| digest_reader(&mut f, h))
        };
        match result {
            Ok(actual) if actual == expected => {
                let current = state.server(&server.id)?;
                anyhow::ensure!(
                    serde_json::to_value(&current)? == serde_json::to_value(server)?,
                    "Server configuration changed during reconciliation"
                );
                let settings = state.settings.lock().unwrap().clone();
                let mut path_check = spec.clone();
                path_check.overwrite = true;
                crate::security::check(&settings, op, source)?;
                if source == "mcp" {
                    crate::file_tools::require_mcp_permissions(
                        &current,
                        &[op.kind.as_str(), "transfer"],
                    )?;
                }
                anyhow::ensure!(
                    crate::transfers::local_path(&path_check, upload, &settings, source)? == local,
                    "Local path changed during reconciliation"
                );
                if upload {
                    let _ = sftp.unlink(&temp);
                } else {
                    let _ = std::fs::remove_file(&temp);
                }
                h.digest(actual);
                return Ok(json!({"reconciled":true}));
            }
            Err(e) if error_code(&e) == "NOT_FOUND" => {
                h.checkpoint.lock().unwrap().commit_pending = false;
                h.persist()?;
            }
            Err(e) => return Err(e),
            _ => {
                return Err(error(
                    "COMMIT_STATUS_UNKNOWN",
                    "Destination differs from the pending commit; inspect it before retrying",
                ))
            }
        }
    }
    h.phase("verifying");
    if upload {
        let mut input = File::open(local)?;
        let identity = local_identity(&input)?;
        let total = input.metadata()?.len();
        anyhow::ensure!(
            Some(total) == spec.size,
            "SOURCE_CHANGED: Upload source size changed"
        );
        source_identity(h, identity.clone())?;
        if spec.skip_existing && sftp.stat(&remote).is_ok() {
            let digest = hash_local(local, h)?;
            if digest_reader(&mut remote_open(&sftp, &remote, &session)?, h)? == digest {
                h.resumed(total);
                h.digest(digest);
                return Ok(json!({"skipped":true}));
            }
        }
        let existing = match sftp.stat(&remote) {
            Ok(st) => {
                anyhow::ensure!(
                    st.is_file(),
                    "INVALID_ARGUMENT: Destination must be a regular file"
                );
                anyhow::ensure!(
                    spec.overwrite,
                    "ALREADY_EXISTS: Destination exists and overwrite is disabled"
                );
                Some(st)
            }
            Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => None,
            Err(e) => return Err(e.into()),
        };
        let mut output = RemoteFile {
            inner: remote_temp(&sftp, &temp)?,
            session: &session,
        };
        let (offset, hash) = verify_prefix(&mut input, &mut output, total, h, true)?;
        output.close()?;
        let mut output = RemoteFile {
            inner: sftp.open_mode(&temp, ssh2::OpenFlags::WRITE, 0o600, ssh2::OpenType::File)?,
            session: &session,
        };
        output.seek(SeekFrom::Start(offset))?;
        output.setstat(ssh2::FileStat {
            size: Some(offset),
            uid: None,
            gid: None,
            perm: None,
            atime: None,
            mtime: None,
        })?;
        let digest = stream(&mut input, &mut output, total, offset, hash, settings, h)?;
        output.close()?;
        anyhow::ensure!(
            local_identity(&input)? == identity,
            "SOURCE_CHANGED: Upload source changed during transfer"
        );
        if let Some(expected) = &spec.sha256 {
            anyhow::ensure!(
                *expected == digest,
                "SOURCE_CHANGED: Content differs from approved source"
            );
        }
        h.phase("verifying");
        let actual = digest_reader(&mut remote_open(&sftp, &temp, &session)?, h)?;
        anyhow::ensure!(
            actual == digest,
            "CHECKSUM_MISMATCH: Remote temporary file checksum differs"
        );
        if let Some(perm) = existing.and_then(|st| st.perm) {
            sftp.setstat(
                &temp,
                ssh2::FileStat {
                    size: None,
                    uid: None,
                    gid: None,
                    perm: Some(perm & 0o777),
                    atime: None,
                    mtime: None,
                },
            )?;
        }
        let lock = state
            .file_mutation_locks
            .lock()
            .unwrap()
            .entry(server.id.clone())
            .or_default()
            .clone();
        let _guard = loop {
            h.check_cancelled()?;
            match lock.try_lock() {
                Ok(g) => break g,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => anyhow::bail!("File lock poisoned"),
            }
        };
        crate::transfers::validate_commit(state, server, op, source, local)?;
        anyhow::ensure!(
            crate::ssh::remote_path(&sftp, server, &spec.remote_path, true)? == remote,
            "Remote path changed"
        );
        h.begin_commit()?;
        {
            let mut cp = h.checkpoint.lock().unwrap();
            cp.digest = Some(digest.clone());
            cp.commit_pending = true;
        }
        h.persist()?;
        if let Err(e) = crate::ssh::commit_extension(
            &session,
            if spec.overwrite {
                "posix-rename@openssh.com"
            } else {
                "hardlink@openssh.com"
            },
            &temp,
            &remote,
        ) {
            if error_code(&e) != "COMMIT_STATUS_UNKNOWN" {
                h.checkpoint.lock().unwrap().commit_pending = false;
                h.persist()?;
            }
            if !spec.overwrite && error_code(&e) == "COMMIT_FAILED" && sftp.lstat(&remote).is_ok() {
                return Err(error(
                    "ALREADY_EXISTS",
                    "Destination was concurrently created; it was not replaced",
                ));
            }
            return Err(e);
        }
        let _ = sftp.unlink(&temp);
        h.digest(digest);
        Ok(json!({"written_bytes":total}))
    } else {
        let mut input = remote_open(&sftp, &remote, &session)?;
        let before = input.stat()?;
        anyhow::ensure!(
            before.is_file(),
            "INVALID_ARGUMENT: Remote source must be a regular file"
        );
        let total = before.size.context("Remote file size unavailable")?;
        source_identity(h, json!({"size":total,"modified":before.mtime}))?;
        if spec.skip_existing && local.is_file() {
            let digest = digest_reader(&mut remote_open(&sftp, &remote, &session)?, h)?;
            if hash_local(local, h)? == digest {
                h.resumed(total);
                h.digest(digest);
                return Ok(json!({"skipped":true}));
            }
            anyhow::ensure!(
                spec.overwrite,
                "ALREADY_EXISTS: Local target content differs and overwrite is disabled"
            );
        }
        let mut output = local_temp(&temp)?;
        let (offset, hash) = verify_prefix(&mut input, &mut output, total, h, false)?;
        output.seek(SeekFrom::Start(offset))?;
        output.set_len(offset)?;
        let digest = stream(&mut input, &mut output, total, offset, hash, settings, h)?;
        output.sync_all()?;
        let after = input.stat()?;
        anyhow::ensure!(
            before.size == after.size && before.mtime == after.mtime,
            "SOURCE_CHANGED: Remote source changed"
        );
        h.phase("verifying");
        // Reopen to avoid seeking an SFTP handle with outstanding packets.
        if offset > 0 {
            anyhow::ensure!(
                digest_reader(&mut remote_open(&sftp, &remote, &session)?, h)? == digest,
                "SOURCE_CHANGED: Remote content changed since checkpoint"
            );
        }
        anyhow::ensure!(
            hash_local(&temp, h)? == digest,
            "CHECKSUM_MISMATCH: Download checksum differs"
        );
        crate::transfers::validate_commit(state, server, op, source, local)?;
        anyhow::ensure!(
            crate::ssh::remote_path(&sftp, server, &spec.remote_path, false)? == remote,
            "Remote source path changed"
        );
        h.begin_commit()?;
        {
            let mut cp = h.checkpoint.lock().unwrap();
            cp.digest = Some(digest.clone());
            cp.commit_pending = true;
        }
        h.persist()?;
        drop(output);
        if spec.overwrite {
            std::fs::rename(&temp, local)?;
        } else {
            std::fs::hard_link(&temp, local)?;
            let _ = std::fs::remove_file(&temp);
        }
        h.digest(digest);
        Ok(json!({"downloaded_bytes":total}))
    }
}

pub(crate) async fn cleanup(
    state: &std::sync::Arc<AppState>,
    h: &Handle,
    source: &str,
) -> Result<()> {
    let cp = h.checkpoint.lock().unwrap().clone();
    if cp.temp.is_empty() {
        return Ok(());
    }
    let op = h.operation.lock().unwrap().clone();
    let server = state.server(&op.server_id)?;
    h.validate_server(&server)?;
    let spec = op
        .transfer_request
        .as_ref()
        .context("Missing transfer spec")?;
    if op.kind == "upload_file" {
        // Remote cleanup uses the same deletion protection and approvals as other removals.
        let remove = Operation {
            kind: "remove_path".into(),
            server_id: op.server_id,
            path: cp.temp,
            file_request: Some(crate::model::FileRequest::Remove {
                recursive: false,
                missing_ok: true,
            }),
            ..Default::default()
        };
        // execute() requires a static source identifier.
        Box::pin(crate::state::execute(
            state.clone(),
            remove,
            if source == "mcp" { "mcp" } else { "desktop" },
        ))
        .await?;
    } else {
        let settings = state.settings.lock().unwrap().clone();
        let temp_spec = crate::transfers::TransferSpec {
            local_path: cp.temp.clone(),
            overwrite: true,
            ..spec.clone()
        };
        let path = crate::transfers::local_path(&temp_spec, false, &settings, source)?;
        anyhow::ensure!(
            path.file_name().and_then(|n| n.to_str())
                == Some(format!(".remote-ssh-brige-{}.part", op.transfer_id).as_str()),
            "Invalid temporary file name"
        );
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let _ = server;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn corrupted_checkpoint_tail_rolls_back_to_last_verified_block() {
        let h = crate::transfers::testing_handle();
        let data = vec![0x53; CHUNK_BYTES * 2 + 17];
        h.checkpoint.lock().unwrap().hashes = vec![
            block_hash(&data[..CHUNK_BYTES]),
            block_hash(&data[CHUNK_BYTES..CHUNK_BYTES * 2]),
        ];
        let mut retained = data.clone();
        retained[CHUNK_BYTES + 2] = 0;
        let (offset, _) = verify_prefix(
            &mut Cursor::new(data.clone()),
            &mut Cursor::new(retained),
            data.len() as u64,
            &h,
            true,
        )
        .unwrap();
        assert_eq!(offset, CHUNK_BYTES as u64);
        assert_eq!(h.checkpoint.lock().unwrap().hashes.len(), 1);
    }
    #[test]
    fn source_content_change_is_rejected_even_with_matching_metadata() {
        let h = crate::transfers::testing_handle();
        h.checkpoint.lock().unwrap().hashes = vec![block_hash(b"old")];
        let e = verify_prefix(
            &mut Cursor::new(b"new"),
            &mut Cursor::new(b"old"),
            3,
            &h,
            true,
        )
        .unwrap_err();
        assert_eq!(error_code(&e), "SOURCE_CHANGED");
    }
    #[test]
    fn resume_preserves_verified_prefix_and_hashes_complete_content() {
        let h = crate::transfers::testing_handle();
        let bytes = vec![42; CHUNK_BYTES + 17];
        h.checkpoint.lock().unwrap().hashes = vec![block_hash(&bytes[..CHUNK_BYTES])];
        let mut input = Cursor::new(bytes.clone());
        let mut output = Cursor::new(bytes[..CHUNK_BYTES].to_vec());
        let (offset, hash) =
            verify_prefix(&mut input, &mut output, bytes.len() as u64, &h, true).unwrap();
        let digest = stream(
            &mut input,
            &mut output,
            bytes.len() as u64,
            offset,
            hash,
            &RuntimeSettings::default(),
            &h,
        )
        .unwrap();
        assert_eq!(output.into_inner(), bytes);
        assert_eq!(digest, block_hash(&bytes));
    }
    #[test]
    fn sparse_source_sizes_above_four_gib_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large");
        let file = File::create(path).unwrap();
        file.set_len(5 * 1024 * 1024 * 1024 + 17).unwrap();
        assert_eq!(
            local_identity(&file).unwrap()["size"],
            5u64 * 1024 * 1024 * 1024 + 17
        );
    }
    #[test]
    fn permanent_failures_are_never_retried() {
        let full: anyhow::Error = std::io::Error::from(std::io::ErrorKind::StorageFull).into();
        assert_eq!(error_code(&full), "NO_SPACE");
        assert!(!crate::operation_error::retryable(&full));
        for code in [
            "SOURCE_CHANGED",
            "NO_SPACE",
            "PERMISSION_DENIED",
            "COMMIT_STATUS_UNKNOWN",
            "SSH_HOST_KEY_CHANGED",
            "TRANSFER_CANCELLED",
        ] {
            assert!(!crate::operation_error::retryable(&error(
                code,
                "permanent"
            )));
        }
        assert!(crate::operation_error::retryable(&error(
            "IO_TIMEOUT",
            "temporary"
        )));
    }
}
