use crate::{
    file_tools,
    model::{FileRequest, Operation, RuntimeSettings, Server, WriteMode},
};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ssh2::{Session, Sftp};
use std::{
    io::{BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    time::Duration,
};

pub fn handshake(server: &Server) -> Result<Session> {
    handshake_with_settings(server, &RuntimeSettings::default())
}
pub fn handshake_with_settings(server: &Server, settings: &RuntimeSettings) -> Result<Session> {
    let addresses = (server.host.as_str(), server.port)
        .to_socket_addrs()
        .context("Cannot resolve SSH server address")?;
    let mut connection = None;
    let mut failures = Vec::new();
    for addr in addresses {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(settings.connect_timeout_secs)) {
            Ok(tcp) => { connection = Some(tcp); break; }
            Err(e) => failures.push(json!({"address":addr.to_string(),"reason":e.to_string(),"os_code":e.raw_os_error()})),
        }
    }
    let tcp = connection.ok_or_else(|| {
        crate::operation_error::error_with_details(
            "SSH_CONNECT_FAILED",
            "Cannot connect to SSH server",
            json!({"host":server.host,"port":server.port,"attempts":failures}),
        )
    })?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(Duration::from_secs(settings.io_timeout_secs)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(settings.io_timeout_secs)))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.set_timeout((settings.io_timeout_secs * 1000) as u32);
    session.handshake()?;
    Ok(session)
}
pub fn fingerprint(session: &Session) -> Result<String> {
    let (key, _) = session
        .host_key()
        .context("SSH server did not provide a host key")?;
    Ok(format!(
        "SHA256:{}",
        STANDARD.encode(Sha256::digest(key)).trim_end_matches('=')
    ))
}
pub fn connect(server: &Server) -> Result<Session> {
    connect_with_settings(server, &RuntimeSettings::default())
}
pub fn connect_with_settings(server: &Server, settings: &RuntimeSettings) -> Result<Session> {
    let session = handshake_with_settings(server, settings)
        .map_err(|e| crate::operation_error::with_stage(e, "connection"))?;
    let actual = fingerprint(&session)?;
    if server.fingerprint.is_empty() {
        return Err(crate::operation_error::error_with_details(
            "SSH_HOST_UNTRUSTED",
            "SSH host is not trusted; verify its fingerprint",
            json!({"actual_fingerprint":actual}),
        ));
    }
    if actual != server.fingerprint {
        return Err(crate::operation_error::error_with_details(
            "SSH_HOST_KEY_MISMATCH",
            "SSH host fingerprint mismatch; connection aborted",
            json!({"actual_fingerprint":actual,"expected_fingerprint":server.fingerprint}),
        ));
    }
    match server.auth.as_str() {
        "password" => {
            let password = keyring::Entry::new("dev.remotedesk.ssh", &server.id)?
                .get_password()
                .context("SSH password is not configured")?;
            session.userauth_password(&server.username, &password)?;
        }
        "key" => {
            let passphrase =
                match keyring::Entry::new("dev.remotedesk.ssh", &server.id)?.get_password() {
                    Ok(p) => Some(p),
                    Err(keyring::Error::NoEntry) => None,
                    Err(e) => return Err(e.into()),
                };
            session.userauth_pubkey_file(
                &server.username,
                None,
                Path::new(&server.key_path),
                passphrase.as_deref(),
            )?;
        }
        "agent" => session.userauth_agent(&server.username)?,
        _ => bail!("Unsupported SSH authentication method"),
    }
    if !session.authenticated() {
        bail!("SSH authentication failed")
    }
    Ok(session)
}
pub(crate) fn remote_path(
    sftp: &Sftp,
    server: &Server,
    value: &str,
    create: bool,
) -> Result<PathBuf> {
    if !value.starts_with('/')
        || value.contains('\0')
        || value.contains('\\')
        || value.split('/').any(|p| p == "..")
    {
        return Err(crate::operation_error::error_with_details(
            "INVALID_ARGUMENT",
            "path must be an absolute POSIX path without NUL, .. or backslashes",
            json!({"field":"path","path":value}),
        ));
    }
    let root = sftp.realpath(Path::new(&server.root))?;
    // Resolve existing targets, including symlinks. New targets resolve their parent.
    let path = Path::new(value);
    let resolved = match sftp.realpath(path) {
        Ok(p) => p,
        Err(e) if create => {
            match sftp.lstat(path) {
                Ok(_) => return Err(e.into()), // Reject dangling symlinks rather than following them.
                Err(missing) if missing.code() == ssh2::ErrorCode::SFTP(2) => {}
                Err(other) => return Err(other.into()),
            }
            sftp.realpath(path.parent().context("Invalid path")?)
                .map_err(|e| {
                    if e.code() == ssh2::ErrorCode::SFTP(2) {
                        crate::operation_error::error(
                            "PARENT_NOT_FOUND",
                            "Parent directory does not exist; use write_file with create_parents=true or create it first",
                        )
                    } else {
                        e.into()
                    }
                })?
                .join(path.file_name().context("Invalid filename")?)
        }
        Err(e) => return Err(e.into()),
    };
    if resolved != root && !resolved.starts_with(&root) {
        return Err(crate::operation_error::error_with_details(
            "PATH_OUTSIDE_ROOT",
            "Path is outside the allowed file root",
            json!({"field":"path","path":value,"resolved_path":resolved,"allowed_root":root}),
        ));
    }
    Ok(resolved)
}
fn create_parent_directories(sftp: &Sftp, server: &Server, value: &str) -> Result<()> {
    // Validate the entire request before creating anything. Resolve the nearest
    // existing ancestor first, so symlinks outside the root cannot cause writes.
    if !value.starts_with('/')
        || value.contains('\0')
        || value.contains('\\')
        || value.split('/').any(|p| p == "..")
        || value.ends_with('/')
    {
        return Err(crate::operation_error::error_with_details(
            "INVALID_ARGUMENT",
            "path must be an absolute file path without .., NUL or backslashes",
            json!({"field":"path","path":value}),
        ));
    }
    let mut ancestor = Path::new(value)
        .parent()
        .context("INVALID_ARGUMENT: path has no parent")?;
    let mut missing = Vec::new();
    loop {
        match sftp.lstat(ancestor) {
            Ok(_) => break,
            Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => {
                missing.push(ancestor.to_path_buf());
                anyhow::ensure!(
                    missing.len() <= 128,
                    "INVALID_ARGUMENT: create_parents exceeds 128 directory levels"
                );
                ancestor = ancestor
                    .parent()
                    .context("INVALID_ARGUMENT: no existing parent directory")?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let resolved = remote_path(sftp, server, &ancestor.to_string_lossy(), false)?;
    if !sftp.stat(&resolved)?.is_dir() {
        return Err(crate::operation_error::error_with_details(
            "NOT_A_DIRECTORY",
            "A parent path is not a directory",
            json!({"path":ancestor}),
        ));
    }
    for directory in missing.into_iter().rev() {
        let checked = remote_path(sftp, server, &directory.to_string_lossy(), true)?;
        match sftp.mkdir(&checked, 0o755) {
            Ok(()) => {}
            Err(e) => {
                // A concurrent creator is acceptable only after rechecking scope
                // and type. Other errors retain the original SFTP diagnostic.
                let existing = remote_path(sftp, server, &directory.to_string_lossy(), false);
                if !existing.is_ok_and(|p| sftp.stat(&p).is_ok_and(|st| st.is_dir())) {
                    return Err(anyhow::Error::from(e).context(format!(
                        "create_parents: cannot create {}",
                        directory.display()
                    )));
                }
            }
        }
    }
    Ok(())
}
fn destructive_path(sftp: &Sftp, server: &Server, value: &str) -> Result<PathBuf> {
    let p = remote_path(sftp, server, value, false)?;
    if p == sftp.realpath(Path::new(&server.root))? {
        bail!("Cannot delete or rename the allowed file root")
    }
    // Preserve the original basename for unlink/rename so a symlink itself is affected.
    let original = Path::new(value);
    Ok(sftp
        .realpath(original.parent().context("Invalid path")?)?
        .join(original.file_name().context("Invalid filename")?))
}
pub fn operate(server: &Server, op: &Operation) -> Result<Value> {
    operate_with_settings(server, op, &RuntimeSettings::default())
}
pub fn operate_with_settings(
    server: &Server,
    op: &Operation,
    settings: &RuntimeSettings,
) -> Result<Value> {
    anyhow::ensure!(
        op.kind != "exec",
        "Use state::execute for managed SSH execution"
    );
    settings.validate()?;
    validate_payload(op, settings)?;
    let limit = settings.transfer_limit(&op.kind);
    // Reject oversized payloads before allocating decoded bytes or connecting.
    if op.kind == "upload" && op.content.len() as u64 > ((limit + 2) / 3) * 4 {
        bail!(
            "File exceeds the {} MiB upload limit",
            settings.upload_limit_mib
        )
    }
    if op.kind == "write_file" && op.content.len() as u64 > limit {
        bail!(
            "File exceeds the {} MiB write limit",
            settings.upload_limit_mib
        )
    }
    let upload_bytes = if op.kind == "upload" {
        let bytes = STANDARD.decode(&op.content)?;
        if bytes.len() as u64 > limit {
            bail!(
                "File exceeds the {} MiB upload limit",
                settings.upload_limit_mib
            )
        }
        Some(bytes)
    } else {
        None
    };
    let session = connect_with_settings(server, settings)?;
    if op.kind == "test" {
        return Ok(json!({"connected": true, "fingerprint": fingerprint(&session)?}));
    }
    let sftp = session.sftp()?;
    if let Some(FileRequest::Remove {
        recursive,
        missing_ok,
    }) = op.file_request
    {
        return crate::safe_remove::remove(
            &SftpRemoval(&sftp),
            &server.root,
            &op.path,
            recursive,
            missing_ok,
        );
    }
    if let Some(FileRequest::Read { offset, limit }) = op.file_request {
        let p = remote_path(&sftp, server, &op.path, false)?;
        let mut file = sftp.open(&p)?;
        if !file.stat()?.is_file() {
            return Err(crate::operation_error::error(
                "UNSUPPORTED_TEXT",
                "Path is not a regular file",
            ));
        }
        let scan_limit = settings.transfer_limit("read_file");
        let mut value = file_tools::read_page(
            BufReader::new(file.take(scan_limit + 1)),
            offset,
            limit,
            scan_limit,
        )?;
        value["path"] = json!(p.to_string_lossy().replace('\\', "/"));
        return Ok(value);
    }
    if let Some(FileRequest::Edit { edits }) = &op.file_request {
        let p = remote_path(&sftp, server, &op.path, false)?;
        let mut file = sftp.open(&p)?;
        if !file.stat()?.is_file() {
            return Err(crate::operation_error::error(
                "UNSUPPORTED_TEXT",
                "Path is not a regular file",
            ));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(settings.transfer_limit("read_file") + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > settings.transfer_limit("read_file") {
            return Err(crate::operation_error::error(
                "PAYLOAD_TOO_LARGE",
                "Original file exceeds download limit",
            ));
        }
        let original = std::str::from_utf8(&bytes).map_err(|_| {
            crate::operation_error::error("UNSUPPORTED_TEXT", "Cannot edit non-UTF-8 text")
        })?;
        let mut edit =
            file_tools::edit_text(original, edits, settings.transfer_limit("write_file"))?;
        atomic_write(
            &session,
            &sftp,
            &p,
            edit.content.as_bytes(),
            WriteMode::Overwrite,
        )?;
        edit.response["path"] = json!(p.to_string_lossy().replace('\\', "/"));
        return Ok(edit.response);
    }
    match op.kind.as_str() {
        "list_files" => {
            let p = remote_path(&sftp, server, &op.path, false)?;
            let mut directory = sftp.opendir(&p)?;
            let mut scanned = Vec::new();
            loop {
                match directory.readdir() {
                    Ok((name, stat)) => {
                        if name == Path::new(".") || name == Path::new("..") {
                            continue;
                        }
                        if scanned.len() >= file_tools::DIRECTORY_SCAN_LIMIT {
                            return Err(crate::operation_error::error(
                                "DIRECTORY_SCAN_LIMIT",
                                "Directory exceeds 10000 entries; choose a smaller directory",
                            ));
                        }
                        scanned.push((p.join(name), stat));
                    }
                    Err(e) if e.code() == ssh2::ErrorCode::Session(-16) => break,
                    Err(e) => return Err(e.into()),
                }
            }
            let mut entries = scanned.into_iter().map(|(p, st)| {
                json!({"name":p.file_name().unwrap_or_default().to_string_lossy(), "path": p.to_string_lossy().replace('\\', "/"),
                    "is_dir": st.is_dir(), "is_symlink": st.file_type().is_symlink(), "size":st.size.unwrap_or(0),
                    "modified":st.mtime.unwrap_or(0), "permissions": format!("{:o}", st.perm.unwrap_or(0) & 0o777)})
            }).collect::<Vec<_>>();
            entries.sort_by(|a, b| {
                b["is_dir"]
                    .as_bool()
                    .cmp(&a["is_dir"].as_bool())
                    .then(a["name"].as_str().cmp(&b["name"].as_str()))
            });
            if let Some(FileRequest::List { offset, limit }) = op.file_request {
                let path = p.to_string_lossy().replace('\\', "/");
                let envelope = serde_json::to_vec(&json!({"path":path,"entries":[],"has_more":true,"next_offset":u64::MAX,"truncated":true,"truncated_by":"requested_limit"}))?.len();
                let mut page = file_tools::list_page_with_budget(
                    entries,
                    offset,
                    limit,
                    file_tools::MAX_BYTES.saturating_sub(envelope),
                )?;
                page["path"] = json!(path);
                Ok(page)
            } else {
                Ok(json!({"path":p.to_string_lossy().replace('\\', "/"), "entries": entries}))
            }
        }
        "stat_file" => {
            let p = remote_path(&sftp, server, &op.path, false)?;
            let stat = sftp
                .stat(&p)
                .context("stat_file: reading resolved path metadata")?;
            let original = sftp.lstat(Path::new(&op.path))?;
            Ok(json!({
                "path":p.to_string_lossy().replace('\\', "/"),
                "name":p.file_name().unwrap_or_default().to_string_lossy(),
                "kind":if stat.is_dir() {"directory"} else if stat.is_file() {"file"} else {"other"},
                "is_dir":stat.is_dir(), "is_symlink":original.file_type().is_symlink(),
                "size":stat.size,"modified":stat.mtime,"accessed":stat.atime,
                "permissions":stat.perm.map(|perm| format!("{:o}",perm & 0o7777)),
                "uid":stat.uid,"gid":stat.gid,
            }))
        }
        "read_file" | "download" => {
            let p = remote_path(&sftp, server, &op.path, false)?;
            let mut bytes = Vec::new();
            sftp.open(&p)?.take(limit + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > limit {
                bail!(
                    "File exceeds the {} MiB read/download limit",
                    settings.download_limit_mib
                )
            }
            if op.kind == "download" {
                Ok(json!({"base64": STANDARD.encode(bytes)}))
            } else {
                Ok(
                    json!({"content":String::from_utf8(bytes).context("File is not UTF-8 text; use download_file")?}),
                )
            }
        }
        "write_file" | "upload" => {
            let mode = match op.file_request {
                Some(FileRequest::Write { mode, .. }) => mode,
                _ => WriteMode::Overwrite,
            };
            if mode == WriteMode::Create {
                match sftp.lstat(Path::new(&op.path)) {
                    Ok(_) => {
                        return Err(crate::operation_error::error(
                            "ALREADY_EXISTS",
                            "Create mode refuses existing targets, including symlinks",
                        ))
                    }
                    Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            if matches!(
                op.file_request,
                Some(FileRequest::Write {
                    create_parents: true,
                    ..
                })
            ) {
                create_parent_directories(&sftp, server, &op.path)?;
            }
            let p = remote_path(&sftp, server, &op.path, true)?;
            let bytes = if op.kind == "upload" {
                upload_bytes.context("Upload content was not decoded")?
            } else {
                op.content.as_bytes().to_vec()
            };
            if bytes.len() as u64 > limit {
                bail!(
                    "File exceeds the {} MiB upload/write limit",
                    settings.upload_limit_mib
                )
            }
            let created = atomic_write(&session, &sftp, &p, &bytes, mode)?;
            Ok(
                json!({"path":p.to_string_lossy().replace('\\', "/"), "created":created,"written":bytes.len(),"written_bytes":bytes.len()}),
            )
        }
        "mkdir" => {
            let p = remote_path(&sftp, server, &op.path, true)?;
            mutation_result(sftp.mkdir(&p, 0o755))?;
            Ok(json!({"created":true}))
        }
        "remove" => {
            let p = destructive_path(&sftp, server, &op.path)?;
            let st = sftp.lstat(&p)?;
            if st.is_dir() {
                let mut dir = sftp.opendir(&p)?;
                loop {
                    match dir.readdir() {
                        Ok((name, _)) if name == Path::new(".") || name == Path::new("..") => {
                            continue
                        }
                        Ok(_) => {
                            return Err(crate::operation_error::error(
                                "DIRECTORY_NOT_EMPTY",
                                "Only empty directories can be removed",
                            ))
                        }
                        Err(e) if e.code() == ssh2::ErrorCode::Session(-16) => break,
                        Err(e) => return Err(e.into()),
                    }
                }
                mutation_result(sftp.rmdir(&p))?;
            } else {
                mutation_result(sftp.unlink(&p))?;
            }
            Ok(
                json!({"path":op.path,"removed":true,"kind":if st.is_dir() {"directory"} else if st.file_type().is_symlink() {"symlink"} else {"file"}}),
            )
        }
        "rename" => {
            let p = destructive_path(&sftp, server, &op.path)?;
            let target = remote_path(&sftp, server, &op.target, true)?;
            if sftp.lstat(&target).is_ok() {
                return Err(crate::operation_error::error(
                    "ALREADY_EXISTS",
                    "Rename target already exists",
                ));
            }
            // None in ssh2 enables OVERWRITE by default. Empty flags request SFTP no-replace.
            mutation_result(sftp.rename(&p, &target, Some(ssh2::RenameFlags::empty())))?;
            Ok(json!({"path":op.path,"target":op.target,"renamed":true}))
        }
        _ => bail!("Unknown operation: {}", op.kind),
    }
}
struct SftpRemoval<'a>(&'a Sftp);
impl crate::safe_remove::FileSystem for SftpRemoval<'_> {
    fn realpath(&self, path: &str) -> Result<String> {
        Ok(self
            .0
            .realpath(Path::new(path))?
            .to_str()
            .context("Remote path is not UTF-8")?
            .replace('\\', "/"))
    }
    fn metadata(&self, path: &str) -> Result<crate::safe_remove::Metadata> {
        let st = self.0.lstat(Path::new(path))?;
        let kind = if st.file_type().is_symlink() {
            crate::safe_remove::Kind::Symlink
        } else if st.is_dir() {
            crate::safe_remove::Kind::Directory
        } else if st.is_file() {
            crate::safe_remove::Kind::File
        } else {
            return Err(crate::operation_error::error(
                "UNSUPPORTED_FILE_TYPE",
                "Safe deletion supports regular files, directories, and symlinks only",
            ));
        };
        Ok(crate::safe_remove::Metadata {
            kind,
            size: st.size,
            modified: st.mtime,
        })
    }
    fn entries(&self, path: &str, limit: usize, exhaustive: bool) -> Result<Vec<String>> {
        let mut dir = self.0.opendir(Path::new(path))?;
        let mut names = Vec::new();
        loop {
            match dir.readdir() {
                Ok((name, _)) => {
                    if name == Path::new(".") || name == Path::new("..") {
                        continue;
                    }
                    if names.len() >= limit {
                        return Err(crate::operation_error::error(
                            "REMOVE_LIMIT",
                            "Directory exceeds the deletion entry budget; no entries were deleted",
                        ));
                    }
                    names.push(
                        name.to_str()
                            .context("Remote filename is not UTF-8")?
                            .to_string(),
                    );
                    if !exhaustive && names.len() >= limit {
                        break;
                    }
                }
                Err(e) if e.code() == ssh2::ErrorCode::Session(-16) => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(names)
    }
    fn unlink(&self, path: &str) -> Result<()> {
        mutation_result(self.0.unlink(Path::new(path)))
    }
    fn rmdir(&self, path: &str) -> Result<()> {
        mutation_result(self.0.rmdir(Path::new(path)))
    }
}

fn mutation_result(result: std::result::Result<(), ssh2::Error>) -> Result<()> {
    result.map_err(|e| match e.code() {
        ssh2::ErrorCode::Session(_) | ssh2::ErrorCode::SFTP(6 | 7) => {
            crate::operation_error::error(
                "COMMIT_STATUS_UNKNOWN",
                "Connection failed during mutation; inspect the path before retrying",
            )
        }
        _ => e.into(),
    })
}

/// Use an explicit OpenSSH extension: libssh2's generic rename flags are only
/// preferences and do not guarantee an atomic overwrite on an SFTP v3 server.
fn commit_extension(session: &Session, extension: &str, from: &Path, to: &Path) -> Result<()> {
    fn packet(channel: &mut ssh2::Channel, payload: &[u8]) -> Result<()> {
        channel.write_all(&(payload.len() as u32).to_be_bytes())?;
        channel.write_all(payload)?;
        Ok(())
    }
    fn receive(channel: &mut ssh2::Channel) -> Result<Vec<u8>> {
        let mut size = [0; 4];
        channel.read_exact(&mut size)?;
        let size = u32::from_be_bytes(size) as usize;
        anyhow::ensure!((5..=65536).contains(&size), "Invalid SFTP response size");
        let mut bytes = vec![0; size];
        channel.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    fn string(payload: &mut Vec<u8>, value: &str) {
        payload.extend_from_slice(&(value.len() as u32).to_be_bytes());
        payload.extend_from_slice(value.as_bytes());
    }
    let mut channel = session.channel_session()?;
    channel.subsystem("sftp")?;
    packet(&mut channel, &[1, 0, 0, 0, 3])?; // SSH_FXP_INIT v3
    let version = receive(&mut channel)?;
    anyhow::ensure!(
        version[0] == 2 && version[1..5] == [0, 0, 0, 3],
        "Unsupported SFTP protocol version"
    );
    // A server's successful extension response confirms support; OP_UNSUPPORTED
    // fails safely. Do not fall back to a generic rename or shell command.
    let mut payload = vec![200, 0, 0, 0, 1]; // SSH_FXP_EXTENDED, request id 1
    string(&mut payload, extension);
    string(
        &mut payload,
        &from
            .to_str()
            .context("Remote path is not UTF-8")?
            .replace('\\', "/"),
    );
    string(
        &mut payload,
        &to.to_str()
            .context("Remote path is not UTF-8")?
            .replace('\\', "/"),
    );
    let response = (|| -> Result<Vec<u8>> {
        packet(&mut channel, &payload)?;
        receive(&mut channel)
    })()
    .map_err(|_| {
        crate::operation_error::error(
            "COMMIT_STATUS_UNKNOWN",
            "Connection failed during commit; read back before retrying",
        )
    })?;
    if response.len() < 9 || response[0] != 101 || response[1..5] != [0, 0, 0, 1] {
        return Err(crate::operation_error::error(
            "COMMIT_STATUS_UNKNOWN",
            "Invalid commit response; read back before retrying",
        ));
    }
    let status = u32::from_be_bytes(response[5..9].try_into().unwrap());
    if status == 0 {
        return Ok(());
    }
    if status == 8 {
        return Err(crate::operation_error::error(
            if extension.starts_with("hardlink") {
                "ATOMIC_CREATE_UNSUPPORTED"
            } else {
                "ATOMIC_WRITE_UNSUPPORTED"
            },
            "Required atomic SFTP extension is unsupported; destination was not replaced",
        ));
    }
    if status == 11 {
        return Err(crate::operation_error::error(
            "ALREADY_EXISTS",
            "Destination already exists",
        ));
    }
    Err(crate::operation_error::error(
        "COMMIT_FAILED",
        "SFTP server rejected atomic commit; destination was not replaced",
    ))
}

fn atomic_write(
    session: &Session,
    sftp: &Sftp,
    path: &Path,
    bytes: &[u8],
    mode: WriteMode,
) -> Result<bool> {
    atomic_write_using(
        session,
        sftp,
        path,
        mode,
        |file| {
            file.write_all(bytes)?;
            Ok(())
        },
        || Ok(()),
    )
}

fn atomic_write_using(
    session: &Session,
    sftp: &Sftp,
    path: &Path,
    mode: WriteMode,
    write: impl FnOnce(&mut ssh2::File) -> Result<()>,
    before_commit: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    let existing = match sftp.stat(path) {
        Ok(st) => {
            if !st.is_file() {
                return Err(crate::operation_error::error(
                    "INVALID_ARGUMENT",
                    "Destination must be a regular file",
                ));
            }
            if mode == WriteMode::Create {
                return Err(crate::operation_error::error(
                    "ALREADY_EXISTS",
                    "Destination already exists",
                ));
            }
            Some(st)
        }
        Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => None,
        Err(e) => return Err(e.into()),
    };
    let temp = path
        .parent()
        .context("Invalid remote path")?
        .join(format!(".remote-ssh-brige-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = sftp.open_mode(
            &temp,
            ssh2::OpenFlags::WRITE | ssh2::OpenFlags::CREATE | ssh2::OpenFlags::EXCLUSIVE,
            0o600,
            ssh2::OpenType::File,
        )?;
        write(&mut file)?;
        file.close()?;
        if let Some(perm) = existing.as_ref().and_then(|st| st.perm) {
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
        before_commit()?;
        commit_extension(
            session,
            if mode == WriteMode::Create {
                "hardlink@openssh.com"
            } else {
                "posix-rename@openssh.com"
            },
            &temp,
            path,
        )
    })();
    // After hardlink success, removing the temp name does not affect the committed inode.
    // Cleanup is best-effort and never turns a completed commit into a retryable failure.
    let _ = sftp.unlink(&temp);
    if let Err(error) = result {
        if mode == WriteMode::Create
            && crate::operation_error::error_code(&error) == "COMMIT_FAILED"
            && sftp.lstat(path).is_ok()
        {
            return Err(crate::operation_error::error(
                "ALREADY_EXISTS",
                "Destination was concurrently created; it was not replaced",
            ));
        }
        return Err(error);
    }
    Ok(existing.is_none())
}

pub(crate) fn transfer_file(
    state: &crate::state::AppState,
    server: &Server,
    op: &Operation,
    settings: &RuntimeSettings,
    source: &str,
    local: &Path,
    local_file: Option<std::fs::File>,
    handle: &crate::transfers::Handle,
) -> Result<Value> {
    use crate::transfers::{copy_file, unchanged, validate_commit};
    let spec = op
        .transfer_request
        .as_ref()
        .context("Missing transfer request")?;
    handle.check_cancelled()?;
    let session = connect_with_settings(server, settings)?;
    let sftp = session.sftp()?;
    let upload = op.kind == "upload_file";
    let remote = remote_path(&sftp, server, &spec.remote_path, upload)?;
    let limit = settings.transfer_limit(&op.kind);
    if upload {
        let mut input = local_file.context("Missing selected upload file")?;
        let before = input.metadata()?;
        anyhow::ensure!(
            Some(before.len()) == spec.size,
            "SOURCE_CHANGED: Local file size changed"
        );
        if !spec.overwrite {
            match sftp.lstat(&remote) {
                Ok(_) => {
                    return Err(crate::operation_error::error(
                        "ALREADY_EXISTS",
                        "Remote target already exists; overwrite is disabled",
                    ))
                }
                Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let mode = if spec.overwrite {
            WriteMode::Overwrite
        } else {
            WriteMode::Create
        };
        atomic_write_using(
            &session,
            &sftp,
            &remote,
            mode,
            |output| {
                let hash = copy_file(&mut input, output, before.len(), limit, handle)?;
                anyhow::ensure!(
                    unchanged(&before, &input.metadata()?),
                    "SOURCE_CHANGED: Local source changed during transfer"
                );
                if let Some(expected) = &spec.sha256 {
                    anyhow::ensure!(
                        *expected == hash,
                        "SOURCE_CHANGED: Upload content differs from the approved file"
                    );
                }
                Ok(())
            },
            || {
                validate_commit(state, server, op, source, local)?;
                anyhow::ensure!(
                    remote_path(&sftp, server, &spec.remote_path, true)? == remote,
                    "Remote target path changed"
                );
                handle.begin_commit()
            },
        )?;
        Ok(json!({"written_bytes":before.len()}))
    } else {
        let mut input = sftp.open(&remote)?;
        let before = input.stat()?;
        anyhow::ensure!(
            before.is_file(),
            "INVALID_ARGUMENT: remote_path must be a regular file"
        );
        let size = before.size.context("Remote file size is unavailable")?;
        if size > limit {
            return Err(crate::operation_error::error(
                "PAYLOAD_TOO_LARGE",
                "File exceeds the download limit",
            ));
        }
        let mut temp =
            tempfile::NamedTempFile::new_in(local.parent().context("Invalid local parent")?)?;
        copy_file(&mut input, temp.as_file_mut(), size, limit, handle)?;
        let after = input.stat()?;
        anyhow::ensure!(
            after.size == before.size && after.mtime == before.mtime,
            "SOURCE_CHANGED: Remote source changed during transfer"
        );
        temp.as_file().sync_all()?;
        validate_commit(state, server, op, source, local)?;
        anyhow::ensure!(
            remote_path(&sftp, server, &spec.remote_path, false)? == remote,
            "Remote source path changed"
        );
        handle.begin_commit()?;
        if spec.overwrite {
            temp.persist(local).map_err(|e| e.error)?;
        } else {
            temp.persist_noclobber(local).map_err(|e| {
                if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                    crate::operation_error::error(
                        "ALREADY_EXISTS",
                        "Local target already exists; overwrite is disabled",
                    )
                } else {
                    e.error.into()
                }
            })?;
        }
        Ok(json!({"downloaded_bytes":size}))
    }
}

pub fn validate_payload(op: &Operation, settings: &RuntimeSettings) -> Result<()> {
    file_tools::validate(op, settings)?;
    let limit = settings.transfer_limit(&op.kind);
    if op.kind == "write_file" && op.content.len() as u64 > limit {
        bail!(
            "File exceeds the {} MiB write limit",
            settings.upload_limit_mib
        )
    }
    if op.kind == "upload" {
        anyhow::ensure!(
            op.content.len() as u64 <= ((limit + 2) / 3) * 4,
            "File exceeds the {} MiB upload limit",
            settings.upload_limit_mib
        );
        anyhow::ensure!(
            STANDARD.decode(&op.content)?.len() as u64 <= limit,
            "File exceeds the {} MiB upload limit",
            settings.upload_limit_mib
        );
    }
    if op.kind == "terminal_write" {
        anyhow::ensure!(op.content.len() <= 65536, "Terminal input exceeds 64 KiB");
    }
    Ok(())
}
