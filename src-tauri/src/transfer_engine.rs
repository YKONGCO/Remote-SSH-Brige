use crate::{
    model::{Operation, RuntimeSettings, Server},
    operation_error::{error, error_code},
    state::AppState,
    transfer_checkpoint::{block_hash, local_identity, source_identity, verify_prefix},
    transfer_verifier::{digest_reader, hash_local},
    transfers::{Handle, CHUNK_BYTES},
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};

pub(crate) use crate::transfer_checkpoint::Checkpoint;

const MIN_WINDOW: usize = 64 * 1024;
const MAX_WINDOW: usize = 1024 * 1024;
const WINDOW_STEP: usize = 128 * 1024;
const WINDOW_STREAK: u32 = 8;

pub(crate) struct AdaptiveWindow {
    bytes: usize,
    streak: u32,
}

impl Default for AdaptiveWindow {
    fn default() -> Self {
        Self {
            bytes: 256 * 1024,
            streak: 0,
        }
    }
}

impl AdaptiveWindow {
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn success(&mut self) {
        self.streak = self.streak.saturating_add(1);
        if self.streak >= WINDOW_STREAK {
            self.streak = 0;
            self.bytes = (self.bytes + WINDOW_STEP).min(MAX_WINDOW);
        }
    }

    pub(crate) fn failure(&mut self, transport: bool) {
        self.streak = 0;
        if transport {
            self.bytes = (self.bytes / 2).max(MIN_WINDOW);
        }
    }
}

fn transport_io(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionAborted
    )
}

pub(crate) fn session_unhealthy(error: &anyhow::Error) -> bool {
    matches!(
        error_code(error),
        "SSH_CONNECT_FAILED"
            | "SSH_AUTHENTICATION_FAILED"
            | "SSH_HOST_KEY_MISMATCH"
            | "SSH_HOST_UNTRUSTED"
            | "SSH_SESSION_BUSY"
            | "SSH_TIMEOUT"
            | "CONNECTION_LOST"
            | "CONNECTION_REFUSED"
            | "SSH_ERROR"
            | "COMMIT_STATUS_UNKNOWN"
    )
}

fn local_temp(path: &Path) -> Result<File> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
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
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file(),
                "INVALID_ARGUMENT: Remote temporary path is not a regular file"
            );
            Ok(sftp.open_mode(
                path,
                ssh2::OpenFlags::READ | ssh2::OpenFlags::WRITE,
                0o600,
                ssh2::OpenType::File,
            )?)
        }
        Err(error) if error.code() == ssh2::ErrorCode::SFTP(2) => Ok(sftp.open_mode(
            path,
            ssh2::OpenFlags::READ
                | ssh2::OpenFlags::WRITE
                | ssh2::OpenFlags::CREATE
                | ssh2::OpenFlags::EXCLUSIVE,
            0o600,
            ssh2::OpenType::File,
        )?),
        Err(error) => Err(error.into()),
    }
}

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
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.inner
            .read(buffer)
            .map_err(|error| io_error(self.session, error))
    }
}

impl Write for RemoteFile<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.inner
            .write(buffer)
            .map_err(|error| io_error(self.session, error))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner
            .flush()
            .map_err(|error| io_error(self.session, error))
    }
}

impl Seek for RemoteFile<'_> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner
            .seek(position)
            .map_err(|error| io_error(self.session, error))
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
    h.phase("transferring");
    h.report(offset, total, start, initial);
    while offset < total {
        h.check_cancelled()?;
        let count = (total - offset).min(CHUNK_BYTES as u64) as usize;
        let mut read = 0;
        let mut last = Instant::now();
        while read < count {
            h.check_cancelled()?;
            if last.elapsed() >= Duration::from_secs(settings.transfers.idle_timeout_secs) {
                h.window.lock().unwrap().failure(true);
                return Err(error("IO_TIMEOUT", "Transfer made no progress"));
            }
            let window = h.window.lock().unwrap().bytes();
            let end = count.min(read + window);
            match source.read(&mut bytes[read..end]) {
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
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    h.window.lock().unwrap().failure(transport_io(&error));
                    return Err(error.into());
                }
            }
        }
        let mut written = 0;
        let mut last = Instant::now();
        while written < count {
            h.check_cancelled()?;
            if last.elapsed() >= Duration::from_secs(settings.transfers.idle_timeout_secs) {
                h.window.lock().unwrap().failure(true);
                return Err(error("IO_TIMEOUT", "Transfer made no progress"));
            }
            let window = h.window.lock().unwrap().bytes();
            let end = count.min(written + window);
            match destination.write(&bytes[written..end]) {
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
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    h.window.lock().unwrap().failure(transport_io(&error));
                    return Err(error.into());
                }
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
        hash.update(&bytes[..count]);
        offset += count as u64;
        {
            let mut checkpoint = h.checkpoint.lock().unwrap();
            checkpoint.hashes.push(block_hash(&bytes[..count]));
            checkpoint.verified_bytes = offset;
            checkpoint.temp_size = Some(offset);
        }
        h.window.lock().unwrap().success();
        h.report(offset, total, start, initial);
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
    connection.io_timeout_secs = settings.transfers.io_timeout_secs;
    let mut lease = state
        .sftp_sessions
        .acquire(server, &connection)
        .map_err(|error| crate::operation_error::with_stage(error, "connection"))?;
    h.session_reused(lease.reused());
    let outcome = transfer_with_session(
        state,
        server,
        op,
        settings,
        source,
        local,
        h,
        lease.session(),
    );
    let healthy = outcome
        .as_ref()
        .err()
        .is_none_or(|error| !session_unhealthy(error));
    lease.release(healthy);
    outcome
}

fn transfer_with_session(
    state: &AppState,
    server: &Server,
    op: &Operation,
    settings: &RuntimeSettings,
    source: &str,
    local: &Path,
    h: &Handle,
    session: &mut ssh2::Session,
) -> Result<Value> {
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
                Ok(metadata) => {
                    anyhow::ensure!(
                        metadata.is_dir(),
                        "ALREADY_EXISTS: Target is not a directory"
                    )
                }
                Err(error) if error.code() == ssh2::ErrorCode::SFTP(2) => {
                    if let Err(error) = sftp.mkdir(&remote, 0o755) {
                        if !sftp.lstat(&remote).is_ok_and(|metadata| metadata.is_dir()) {
                            return Err(error.into());
                        }
                    }
                }
                Err(error) => return Err(error.into()),
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
        h.verified(0);
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
        let mut checkpoint = h.checkpoint.lock().unwrap();
        if !checkpoint.temp.is_empty() {
            anyhow::ensure!(
                Path::new(&checkpoint.temp) == temp,
                "SOURCE_CHANGED: Temporary path changed"
            );
        }
        checkpoint.temp = if upload {
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
            remote_open(&sftp, &remote, session).and_then(|mut file| digest_reader(&mut file, h))
        } else {
            File::open(local)
                .map_err(anyhow::Error::from)
                .and_then(|mut file| digest_reader(&mut file, h))
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
                h.verified(0);
                return Ok(json!({"reconciled":true}));
            }
            Err(error) if error_code(&error) == "NOT_FOUND" => {
                h.checkpoint.lock().unwrap().commit_pending = false;
                h.persist()?;
            }
            Err(error) => return Err(error),
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
            if digest_reader(&mut remote_open(&sftp, &remote, session)?, h)? == digest {
                h.resumed(total);
                h.verified(total);
                h.digest(digest);
                return Ok(json!({"skipped":true}));
            }
        }
        let existing = match sftp.stat(&remote) {
            Ok(metadata) => {
                anyhow::ensure!(
                    metadata.is_file(),
                    "INVALID_ARGUMENT: Destination must be a regular file"
                );
                anyhow::ensure!(
                    spec.overwrite,
                    "ALREADY_EXISTS: Destination exists and overwrite is disabled"
                );
                Some(metadata)
            }
            Err(error) if error.code() == ssh2::ErrorCode::SFTP(2) => None,
            Err(error) => return Err(error.into()),
        };
        let temp_size = sftp.stat(&temp).ok().and_then(|metadata| metadata.size);
        let mut output = RemoteFile {
            inner: remote_temp(&sftp, &temp)?,
            session,
        };
        let (offset, hash) =
            verify_prefix(&mut input, &mut output, total, h, true, true, temp_size)?;
        output.close()?;
        let mut output = RemoteFile {
            inner: sftp.open_mode(&temp, ssh2::OpenFlags::WRITE, 0o600, ssh2::OpenType::File)?,
            session,
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
        let actual = digest_reader(&mut remote_open(&sftp, &temp, session)?, h)?;
        if actual != digest {
            h.reset_checkpoint();
            return Err(error(
                "CHECKSUM_MISMATCH",
                "Remote temporary file checksum differs",
            ));
        }
        h.verified(total);
        if let Some(perm) = existing.and_then(|metadata| metadata.perm) {
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
                Ok(guard) => break guard,
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
            let mut checkpoint = h.checkpoint.lock().unwrap();
            checkpoint.digest = Some(digest.clone());
            checkpoint.commit_pending = true;
        }
        h.persist()?;
        if let Err(failure) = crate::ssh::commit_extension(
            session,
            if spec.overwrite {
                "posix-rename@openssh.com"
            } else {
                "hardlink@openssh.com"
            },
            &temp,
            &remote,
        ) {
            if error_code(&failure) != "COMMIT_STATUS_UNKNOWN" {
                h.checkpoint.lock().unwrap().commit_pending = false;
                h.persist()?;
            }
            if !spec.overwrite
                && error_code(&failure) == "COMMIT_FAILED"
                && sftp.lstat(&remote).is_ok()
            {
                return Err(error(
                    "ALREADY_EXISTS",
                    "Destination was concurrently created; it was not replaced",
                ));
            }
            return Err(failure);
        }
        let _ = sftp.unlink(&temp);
        h.digest(digest);
        Ok(json!({"written_bytes":total}))
    } else {
        let mut input = remote_open(&sftp, &remote, session)?;
        let before = input.stat()?;
        anyhow::ensure!(
            before.is_file(),
            "INVALID_ARGUMENT: Remote source must be a regular file"
        );
        let total = before.size.context("Remote file size unavailable")?;
        source_identity(h, json!({"size":total,"modified":before.mtime}))?;
        if spec.skip_existing && local.is_file() {
            let digest = digest_reader(&mut remote_open(&sftp, &remote, session)?, h)?;
            if hash_local(local, h)? == digest {
                h.resumed(total);
                h.verified(total);
                h.digest(digest);
                return Ok(json!({"skipped":true}));
            }
            anyhow::ensure!(
                spec.overwrite,
                "ALREADY_EXISTS: Local target content differs and overwrite is disabled"
            );
        }
        let mut output = local_temp(&temp)?;
        let temp_size = output.metadata().ok().map(|metadata| metadata.len());
        let (offset, hash) =
            verify_prefix(&mut input, &mut output, total, h, false, true, temp_size)?;
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
        if offset > 0 {
            match &spec.sha256 {
                Some(expected) => anyhow::ensure!(
                    *expected == digest,
                    "SOURCE_CHANGED: Remote content changed since checkpoint"
                ),
                None => anyhow::ensure!(
                    digest_reader(&mut remote_open(&sftp, &remote, session)?, h)? == digest,
                    "SOURCE_CHANGED: Remote content changed since checkpoint"
                ),
            }
        }
        h.verified(total);
        crate::transfers::validate_commit(state, server, op, source, local)?;
        anyhow::ensure!(
            crate::ssh::remote_path(&sftp, server, &spec.remote_path, false)? == remote,
            "Remote source path changed"
        );
        h.begin_commit()?;
        {
            let mut checkpoint = h.checkpoint.lock().unwrap();
            checkpoint.digest = Some(digest.clone());
            checkpoint.commit_pending = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfers::testing_handle;
    use std::io::Cursor;

    #[test]
    fn adaptive_window_grows_after_stable_io_and_shrinks_on_transport_errors() {
        let mut window = AdaptiveWindow::default();
        assert_eq!(window.bytes(), 256 * 1024);
        for _ in 0..WINDOW_STREAK {
            window.success();
        }
        assert_eq!(window.bytes(), 384 * 1024);
        for _ in 0..1000 {
            window.success();
        }
        assert_eq!(window.bytes(), MAX_WINDOW);
        window.failure(true);
        assert_eq!(window.bytes(), MAX_WINDOW / 2);
        for _ in 0..10 {
            window.failure(true);
        }
        assert_eq!(window.bytes(), MIN_WINDOW);
    }

    #[test]
    fn resume_preserves_verified_prefix_and_hashes_complete_content() {
        let h = testing_handle();
        let bytes = vec![42; CHUNK_BYTES + 17];
        h.checkpoint.lock().unwrap().hashes = vec![block_hash(&bytes[..CHUNK_BYTES])];
        let mut input = Cursor::new(bytes.clone());
        let mut output = Cursor::new(bytes[..CHUNK_BYTES].to_vec());
        let (offset, hash) = verify_prefix(
            &mut input,
            &mut output,
            bytes.len() as u64,
            &h,
            true,
            false,
            None,
        )
        .unwrap();
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

pub(crate) async fn cleanup(
    state: &std::sync::Arc<AppState>,
    h: &Handle,
    source: &str,
) -> Result<()> {
    let checkpoint = h.checkpoint.lock().unwrap().clone();
    if checkpoint.temp.is_empty() {
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
        let remove = Operation {
            kind: "remove_path".into(),
            server_id: op.server_id,
            path: checkpoint.temp,
            file_request: Some(crate::model::FileRequest::Remove {
                recursive: false,
                missing_ok: true,
            }),
            ..Default::default()
        };
        Box::pin(crate::state::execute(
            state.clone(),
            remove,
            if source == "mcp" { "mcp" } else { "desktop" },
        ))
        .await?;
    } else {
        let settings = state.settings.lock().unwrap().clone();
        let temp_spec = crate::transfers::TransferSpec {
            local_path: checkpoint.temp.clone(),
            overwrite: true,
            ..spec.clone()
        };
        let path = crate::transfers::local_path(&temp_spec, false, &settings, source)?;
        anyhow::ensure!(
            path.file_name().and_then(|name| name.to_str())
                == Some(format!(".remote-ssh-brige-{}.part", op.transfer_id).as_str()),
            "Invalid temporary file name"
        );
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
