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
    time::{Duration, Instant},
};

pub fn handshake(server: &Server) -> Result<Session> {
    handshake_with_settings(server, &RuntimeSettings::default())
}
pub fn handshake_with_settings(server: &Server, settings: &RuntimeSettings) -> Result<Session> {
    let addresses = (server.host.as_str(), server.port)
        .to_socket_addrs()
        .context("无法解析服务器地址")?;
    let mut connection = None;
    for addr in addresses {
        if let Ok(tcp) =
            TcpStream::connect_timeout(&addr, Duration::from_secs(settings.connect_timeout_secs))
        {
            connection = Some(tcp);
            break;
        }
    }
    let tcp = connection.context("SSH 连接失败，请检查地址、端口和网络")?;
    tcp.set_read_timeout(Some(Duration::from_secs(settings.io_timeout_secs)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(settings.io_timeout_secs)))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.set_timeout((settings.io_timeout_secs * 1000) as u32);
    session.handshake()?;
    Ok(session)
}
pub fn fingerprint(session: &Session) -> Result<String> {
    let (key, _) = session.host_key().context("SSH 服务器未提供主机密钥")?;
    Ok(format!(
        "SHA256:{}",
        STANDARD.encode(Sha256::digest(key)).trim_end_matches('=')
    ))
}
pub fn connect(server: &Server) -> Result<Session> {
    connect_with_settings(server, &RuntimeSettings::default())
}
pub fn connect_with_settings(server: &Server, settings: &RuntimeSettings) -> Result<Session> {
    let session = handshake_with_settings(server, settings)?;
    let actual = fingerprint(&session)?;
    if server.fingerprint.is_empty() {
        bail!("尚未信任 SSH 主机，请在服务器菜单中核验指纹。当前指纹：{actual}")
    }
    if actual != server.fingerprint {
        bail!("SSH 主机指纹不匹配，连接已中止。当前指纹：{actual}")
    }
    match server.auth.as_str() {
        "password" => {
            let password = keyring::Entry::new("dev.remotedesk.ssh", &server.id)?
                .get_password()
                .context("请先保存服务器密码")?;
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
        _ => bail!("未知的认证方式"),
    }
    if !session.authenticated() {
        bail!("SSH 身份认证失败")
    }
    Ok(session)
}
fn remote_path(sftp: &Sftp, server: &Server, value: &str, create: bool) -> Result<PathBuf> {
    if !value.starts_with('/')
        || value.contains('\0')
        || value.contains('\\')
        || value.split('/').any(|p| p == "..")
    {
        bail!("请使用不含 .. 或反斜杠的远程绝对路径")
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
            sftp.realpath(path.parent().context("无效路径")?)
                .map_err(|e| {
                    if e.code() == ssh2::ErrorCode::SFTP(2) {
                        file_tools::error(
                            "PARENT_NOT_FOUND",
                            "Parent directory must exist; create it with mkdir first",
                        )
                    } else {
                        e.into()
                    }
                })?
                .join(path.file_name().context("无效文件名")?)
        }
        Err(e) => return Err(e.into()),
    };
    if resolved != root && !resolved.starts_with(&root) {
        bail!("路径超出此服务器授权的文件根目录")
    }
    Ok(resolved)
}
fn destructive_path(sftp: &Sftp, server: &Server, value: &str) -> Result<PathBuf> {
    let p = remote_path(sftp, server, value, false)?;
    if p == sftp.realpath(Path::new(&server.root))? {
        bail!("不能删除或重命名授权根目录")
    }
    // Preserve the original basename for unlink/rename so a symlink itself is affected.
    let original = Path::new(value);
    Ok(sftp
        .realpath(original.parent().context("无效路径")?)?
        .join(original.file_name().context("无效文件名")?))
}
pub fn operate(server: &Server, op: &Operation) -> Result<Value> {
    operate_with_settings(server, op, &RuntimeSettings::default())
}
pub fn operate_with_settings(
    server: &Server,
    op: &Operation,
    settings: &RuntimeSettings,
) -> Result<Value> {
    settings.validate()?;
    validate_payload(op, settings)?;
    let limit = settings.transfer_limit(&op.kind);
    // Reject oversized payloads before allocating decoded bytes or connecting.
    if op.kind == "upload" && op.content.len() as u64 > ((limit + 2) / 3) * 4 {
        bail!("文件超过 {} MiB 上传限制", settings.upload_limit_mib)
    }
    if op.kind == "write_file" && op.content.len() as u64 > limit {
        bail!("文件超过 {} MiB 写入限制", settings.upload_limit_mib)
    }
    let upload_bytes = if op.kind == "upload" {
        let bytes = STANDARD.decode(&op.content)?;
        if bytes.len() as u64 > limit {
            bail!("文件超过 {} MiB 上传限制", settings.upload_limit_mib)
        }
        Some(bytes)
    } else {
        None
    };
    let session = connect_with_settings(server, settings)?;
    if op.kind == "test" {
        return Ok(json!({"connected": true, "fingerprint": fingerprint(&session)?}));
    }
    if op.kind == "exec" {
        return exec(&session, op, settings);
    }
    let sftp = session.sftp()?;
    if let Some(FileRequest::Read { offset, limit }) = op.file_request {
        let p = remote_path(&sftp, server, &op.path, false)?;
        let mut file = sftp.open(&p)?;
        if !file.stat()?.is_file() {
            return Err(file_tools::error(
                "UNSUPPORTED_TEXT",
                "Path is not a regular file",
            ));
        }
        let scan_limit = settings.transfer_limit("download");
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
            return Err(file_tools::error(
                "UNSUPPORTED_TEXT",
                "Path is not a regular file",
            ));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(settings.transfer_limit("download") + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > settings.transfer_limit("download") {
            return Err(file_tools::error(
                "PAYLOAD_TOO_LARGE",
                "Original file exceeds download limit",
            ));
        }
        let original = std::str::from_utf8(&bytes)
            .map_err(|_| file_tools::error("UNSUPPORTED_TEXT", "Cannot edit non-UTF-8 text"))?;
        let mut edit = file_tools::edit_text(original, edits, settings.transfer_limit("upload"))?;
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
        "preview_file" => {
            let p = remote_path(&sftp, server, &op.path, false)?;
            let stat = sftp.stat(&p)?;
            let cap = settings.transfer_limit("download").min(2 * 1024 * 1024);
            let mut bytes = Vec::new();
            sftp.open(&p)?.take(cap + 1).read_to_end(&mut bytes)?;
            let truncated = bytes.len() as u64 > cap;
            bytes.truncate(cap as usize);
            let mut preview = preview_content(bytes, stat.size, truncated)?;
            preview["preview_limit_bytes"] = json!(cap);
            Ok(preview)
        }
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
                            return Err(file_tools::error(
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
        "read_file" | "download" => {
            let p = remote_path(&sftp, server, &op.path, false)?;
            let mut bytes = Vec::new();
            sftp.open(&p)?.take(limit + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > limit {
                bail!("文件超过 {} MiB 读取/下载限制", settings.download_limit_mib)
            }
            if op.kind == "download" {
                Ok(json!({"base64": STANDARD.encode(bytes)}))
            } else {
                Ok(
                    json!({"content":String::from_utf8(bytes).context("此文件不是 UTF-8 文本，请使用下载")?}),
                )
            }
        }
        "write_file" | "upload" => {
            let mode = match op.file_request {
                Some(FileRequest::Write { mode }) => mode,
                _ => WriteMode::Overwrite,
            };
            if mode == WriteMode::Create {
                match sftp.lstat(Path::new(&op.path)) {
                    Ok(_) => {
                        return Err(file_tools::error(
                            "ALREADY_EXISTS",
                            "Create mode refuses existing targets, including symlinks",
                        ))
                    }
                    Err(e) if e.code() == ssh2::ErrorCode::SFTP(2) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            let p = remote_path(&sftp, server, &op.path, true)?;
            let bytes = if op.kind == "upload" {
                upload_bytes.context("上传内容未解码")?
            } else {
                op.content.as_bytes().to_vec()
            };
            if bytes.len() as u64 > limit {
                bail!("文件超过 {} MiB 上传/写入限制", settings.upload_limit_mib)
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
                            return Err(file_tools::error(
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
                return Err(file_tools::error(
                    "ALREADY_EXISTS",
                    "目标已存在，重命名不会覆盖文件",
                ));
            }
            // None in ssh2 enables OVERWRITE by default. Empty flags request SFTP no-replace.
            mutation_result(sftp.rename(&p, &target, Some(ssh2::RenameFlags::empty())))?;
            Ok(json!({"path":op.path,"target":op.target,"renamed":true}))
        }
        _ => bail!("未知操作：{}", op.kind),
    }
}
fn mutation_result(result: std::result::Result<(), ssh2::Error>) -> Result<()> {
    result.map_err(|e| match e.code() {
        ssh2::ErrorCode::Session(_) | ssh2::ErrorCode::SFTP(6 | 7) => file_tools::error(
            "COMMIT_STATUS_UNKNOWN",
            "Connection failed during mutation; inspect the path before retrying",
        ),
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
        file_tools::error(
            "COMMIT_STATUS_UNKNOWN",
            "Connection failed during commit; read back before retrying",
        )
    })?;
    if response.len() < 9 || response[0] != 101 || response[1..5] != [0, 0, 0, 1] {
        return Err(file_tools::error(
            "COMMIT_STATUS_UNKNOWN",
            "Invalid commit response; read back before retrying",
        ));
    }
    let status = u32::from_be_bytes(response[5..9].try_into().unwrap());
    if status == 0 {
        return Ok(());
    }
    if status == 8 {
        return Err(file_tools::error(
            if extension.starts_with("hardlink") {
                "ATOMIC_CREATE_UNSUPPORTED"
            } else {
                "ATOMIC_WRITE_UNSUPPORTED"
            },
            "Required atomic SFTP extension is unsupported; destination was not replaced",
        ));
    }
    if status == 11 {
        return Err(file_tools::error(
            "ALREADY_EXISTS",
            "Destination already exists",
        ));
    }
    Err(file_tools::error(
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
    let existing = match sftp.stat(path) {
        Ok(st) => {
            if !st.is_file() {
                return Err(file_tools::error(
                    "INVALID_ARGUMENT",
                    "Destination must be a regular file",
                ));
            }
            if mode == WriteMode::Create {
                return Err(file_tools::error(
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
        .join(format!(".remote-desk-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = sftp.open_mode(
            &temp,
            ssh2::OpenFlags::WRITE | ssh2::OpenFlags::CREATE | ssh2::OpenFlags::EXCLUSIVE,
            0o600,
            ssh2::OpenType::File,
        )?;
        file.write_all(bytes)?;
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
            && file_tools::error_code(&error) == "COMMIT_FAILED"
            && sftp.lstat(path).is_ok()
        {
            return Err(file_tools::error(
                "ALREADY_EXISTS",
                "Destination was concurrently created; it was not replaced",
            ));
        }
        return Err(error);
    }
    Ok(existing.is_none())
}

pub fn validate_payload(op: &Operation, settings: &RuntimeSettings) -> Result<()> {
    file_tools::validate(op, settings)?;
    let limit = settings.transfer_limit(&op.kind);
    if op.kind == "write_file" && op.content.len() as u64 > limit {
        bail!("文件超过 {} MiB 写入限制", settings.upload_limit_mib)
    }
    if op.kind == "upload" {
        anyhow::ensure!(
            op.content.len() as u64 <= ((limit + 2) / 3) * 4,
            "文件超过 {} MiB 上传限制",
            settings.upload_limit_mib
        );
        anyhow::ensure!(
            STANDARD.decode(&op.content)?.len() as u64 <= limit,
            "文件超过 {} MiB 上传限制",
            settings.upload_limit_mib
        );
    }
    if op.kind == "terminal_write" {
        anyhow::ensure!(op.content.len() <= 65536, "单次终端输入不得超过 64 KiB");
    }
    Ok(())
}
fn exec(session: &Session, op: &Operation, settings: &RuntimeSettings) -> Result<Value> {
    if op.command.trim().is_empty() {
        bail!("命令不能为空")
    }
    let mut channel = session.channel_session()?;
    channel.exec(&op.command)?;
    session.set_blocking(false);
    let started = Instant::now();
    let timeout = Duration::from_secs(if op.timeout_secs == 0 {
        settings.command_timeout_secs
    } else {
        op.timeout_secs.clamp(1, 300)
    });
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        if started.elapsed() > timeout {
            let _ = channel.close();
            bail!(
                "命令执行超时（{} 秒）；SSH 通道已关闭，远程派生进程可能仍在运行",
                timeout.as_secs()
            )
        }
        for (mut stream, dest) in [(channel.stream(0), &mut out), (channel.stderr(), &mut err)] {
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let remaining = (1024 * 1024_usize).saturating_sub(dest.len());
                        dest.extend_from_slice(&buffer[..n.min(remaining)]);
                        if n > remaining {
                            truncated = true;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                }
            }
        }
        if channel.eof() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    session.set_blocking(true);
    channel.wait_close()?;
    Ok(
        json!({"stdout":String::from_utf8_lossy(&out), "stderr":String::from_utf8_lossy(&err), "exit_code":channel.exit_status()?, "truncated":truncated}),
    )
}

pub fn preview_content(bytes: Vec<u8>, size: Option<u64>, truncated: bool) -> Result<Value> {
    let size = size.unwrap_or(bytes.len() as u64);
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    };
    if let Some(mime) = mime {
        anyhow::ensure!(!truncated, "图片超过预览上限，请下载查看");
        Ok(
            json!({"kind":"image","mime":mime,"base64":STANDARD.encode(&bytes),"bytes":size,"truncated":false}),
        )
    } else if bytes.contains(&0) {
        Ok(json!({"kind":"unsupported","bytes":size,"truncated":truncated}))
    } else {
        let content = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) if truncated && error.utf8_error().error_len().is_none() => {
                let valid = error.utf8_error().valid_up_to();
                String::from_utf8(error.into_bytes()[..valid].to_vec())?
            }
            Err(_) => return Ok(json!({"kind":"unsupported","bytes":size,"truncated":truncated})),
        };
        Ok(json!({"kind":"text","content":content,"bytes":size,"truncated":truncated}))
    }
}
