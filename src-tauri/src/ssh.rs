use crate::model::{Operation, RuntimeSettings, Server};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ssh2::{Session, Sftp};
use std::{
    io::{Read, Write},
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
            sftp.realpath(path.parent().context("无效路径")?)?
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
            let mut entries = sftp.readdir(&p)?.into_iter().filter(|(p, _)| p.file_name().map(|n| n != "." && n != "..").unwrap_or(false)).map(|(p, st)| {
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
            Ok(json!({"path":p.to_string_lossy().replace('\\', "/"), "entries": entries}))
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
            let p = remote_path(&sftp, server, &op.path, true)?;
            let bytes = if op.kind == "upload" {
                upload_bytes.context("上传内容未解码")?
            } else {
                op.content.as_bytes().to_vec()
            };
            if bytes.len() as u64 > limit {
                bail!("文件超过 {} MiB 上传/写入限制", settings.upload_limit_mib)
            }
            let existing = sftp.stat(&p).ok();
            let temp = p
                .parent()
                .context("无效路径")?
                .join(format!(".remote-desk-{}", uuid::Uuid::new_v4()));
            let result = (|| -> Result<()> {
                let mut f = sftp.open_mode(
                    &temp,
                    ssh2::OpenFlags::WRITE | ssh2::OpenFlags::CREATE | ssh2::OpenFlags::EXCLUSIVE,
                    0o600,
                    ssh2::OpenType::File,
                )?;
                f.write_all(&bytes)?;
                f.close()?;
                if let Some(st) = existing {
                    if let Some(perm) = st.perm {
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
                }
                sftp.rename(
                    &temp,
                    &p,
                    Some(ssh2::RenameFlags::OVERWRITE | ssh2::RenameFlags::ATOMIC),
                )?;
                Ok(())
            })();
            if result.is_err() {
                let _ = sftp.unlink(&temp);
            }
            result?;
            Ok(json!({"written": bytes.len()}))
        }
        "mkdir" => {
            let p = remote_path(&sftp, server, &op.path, true)?;
            sftp.mkdir(&p, 0o755)?;
            Ok(json!({"created":true}))
        }
        "remove" => {
            let p = destructive_path(&sftp, server, &op.path)?;
            let st = sftp.lstat(&p)?;
            if st.is_dir() {
                sftp.rmdir(&p)?;
            } else {
                sftp.unlink(&p)?;
            }
            Ok(json!({"removed":true}))
        }
        "rename" => {
            let p = destructive_path(&sftp, server, &op.path)?;
            let target = remote_path(&sftp, server, &op.target, true)?;
            if sftp.lstat(&target).is_ok() {
                bail!("目标已存在，重命名不会覆盖文件")
            }
            sftp.rename(&p, &target, None)?;
            Ok(json!({"renamed":true}))
        }
        _ => bail!("未知操作：{}", op.kind),
    }
}
pub fn validate_payload(op: &Operation, settings: &RuntimeSettings) -> Result<()> {
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
