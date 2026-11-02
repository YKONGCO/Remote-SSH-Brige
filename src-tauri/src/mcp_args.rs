//! Public tools expose only the fields they consume. Internal desktop Operations stay separate.
use crate::model::{FileEdit, FileRequest, Operation, WriteMode};
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyArgs {}

macro_rules! args {
    ($name:ident { $($(#[$attr:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            /// Configured server ID returned by list_servers.
            #[schemars(length(min = 1, max = 128))]
            pub server_id: String,
            $($(#[$attr])* pub $field: $ty),*
        }
    };
}

args!(PathArgs {
    /// Absolute remote POSIX path within the configured file root.
    #[schemars(length(min = 1, max = 8192))]
    path: String,
});
args!(ReadArgs {
    /// Absolute remote UTF-8 file path.
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// First line to read (1-indexed; default 1).
    #[schemars(range(min = 1))]
    offset: Option<u64>,
    /// Maximum lines (default and hard ceiling 2000). Output also capped at 50 KiB.
    #[schemars(range(min = 1, max = 2000))]
    limit: Option<u64>,
});
args!(ListArgs {
    /// Absolute remote directory path within the configured file root.
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Zero-based entry offset. Entries sort directories first, then by name.
    offset: Option<u64>,
    /// Maximum entries, default 200, ceiling 1000. Response capped at 50 KiB.
    #[schemars(range(min = 1, max = 1000))]
    limit: Option<u64>,
});
args!(WriteArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Complete UTF-8 file content. Use edit_file for targeted changes.
    content: String,
    /// overwrite creates or replaces (default); create refuses existing targets.
    mode: Option<WriteMode>,
    /// Create missing parent directories inside the file root. Default false.
    create_parents: Option<bool>,
});
args!(EditArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Non-overlapping replacements, all matched against the original file, committed together.
    #[schemars(length(min = 1, max = 100))]
    edits: Vec<FileEdit>,
});
args!(ExecArgs {
    #[schemars(length(min = 1, max = 16384))]
    command: String,
    /// Seconds, 0 uses application default; otherwise 1-300.
    #[schemars(range(max = 300))]
    timeout_secs: Option<u64>,
});
args!(RemoveArgs {
    /// Exact absolute remote path inside the configured root. No wildcard expansion.
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Explicitly allow non-empty directory deletion. Default false. Symlinks are never followed.
    recursive: Option<bool>,
    /// Succeed when already absent, for idempotent cleanup. Default true.
    missing_ok: Option<bool>,
});

macro_rules! into_op {
    ($ty:ident, $a:ident, {$($field:ident: $value:expr),* $(,)?}) => {
        impl From<$ty> for Operation {
            fn from($a: $ty) -> Self {
                Self { server_id: $a.server_id, $($field: $value,)* ..Default::default() }
            }
        }
    };
}
into_op!(PathArgs, a, {path: a.path});
into_op!(ReadArgs, a, {path: a.path, file_request: Some(FileRequest::Read {offset:a.offset.unwrap_or(1),limit:a.limit.unwrap_or(2000)})});
into_op!(ListArgs, a, {path:a.path, file_request: Some(FileRequest::List {offset:a.offset.unwrap_or(0),limit:a.limit.unwrap_or(200)})});
into_op!(WriteArgs, a, {path:a.path, content:a.content, file_request:Some(FileRequest::Write {mode:a.mode.unwrap_or_default(),create_parents:a.create_parents.unwrap_or(false)})});
into_op!(EditArgs, a, {path:a.path, file_request:Some(FileRequest::Edit {edits:a.edits})});
into_op!(ExecArgs, a, {command:a.command, timeout_secs:a.timeout_secs.unwrap_or(0)});

args!(TransferStartArgs {
    /// Absolute file path on the computer running this app, within its configured MCP local roots.
    #[schemars(length(min = 1, max = 8192))]
    local_path: String,
    /// Absolute remote POSIX file path within the server file root.
    #[schemars(length(min = 1, max = 8192))]
    remote_path: String,
    /// Default false. Replace the destination only after the complete transfer succeeds.
    overwrite: Option<bool>,
});
into_op!(TransferStartArgs, a, { transfer_request: Some(crate::transfers::TransferSpec {local_path:a.local_path,remote_path:a.remote_path,overwrite:a.overwrite.unwrap_or(false),size:None,sha256:None}) });

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum TransferArgs {
    List {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
    },
    Read {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        transfer_id: String,
        version: Option<u64>,
        #[schemars(range(max = 30))]
        wait_secs: Option<u64>,
    },
    Cancel {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        transfer_id: String,
    },
}
impl From<TransferArgs> for Operation {
    fn from(a: TransferArgs) -> Self {
        let mut op = Self::default();
        match a {
            TransferArgs::List { server_id } => {
                op.server_id = server_id;
                op.transfer_action = "list".into();
            }
            TransferArgs::Read {
                server_id,
                transfer_id,
                version,
                wait_secs,
            } => {
                op.server_id = server_id;
                op.transfer_id = transfer_id;
                op.version = version;
                op.wait_secs = wait_secs.unwrap_or(0);
                op.transfer_action = "read".into();
            }
            TransferArgs::Cancel {
                server_id,
                transfer_id,
            } => {
                op.server_id = server_id;
                op.transfer_id = transfer_id;
                op.transfer_action = "cancel".into();
            }
        }
        op
    }
}
into_op!(RemoveArgs, a, {path:a.path, file_request:Some(FileRequest::Remove {recursive:a.recursive.unwrap_or(false),missing_ok:a.missing_ok.unwrap_or(true)})});

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum JobArgs {
    List {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
    },
    Read {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        job_id: String,
        /// Absolute byte offsets returned by exec/job.read; initially zero.
        stdout_cursor: Option<u64>,
        stderr_cursor: Option<u64>,
        /// Wait at most 0-30 seconds, returning early on output or completion.
        #[schemars(range(max = 30))]
        wait_secs: Option<u64>,
    },
    Close {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        job_id: String,
    },
}
impl From<JobArgs> for Operation {
    fn from(args: JobArgs) -> Self {
        let mut op = Self::default();
        match args {
            JobArgs::List { server_id } => {
                op.server_id = server_id;
                op.job_action = "list".into();
            }
            JobArgs::Read {
                server_id,
                job_id,
                stdout_cursor,
                stderr_cursor,
                wait_secs,
            } => {
                op.server_id = server_id;
                op.job_id = job_id;
                op.job_action = "read".into();
                op.stdout_cursor = stdout_cursor.unwrap_or(0);
                op.stderr_cursor = stderr_cursor.unwrap_or(0);
                op.wait_secs = wait_secs.unwrap_or(0);
            }
            JobArgs::Close { server_id, job_id } => {
                op.server_id = server_id;
                op.job_id = job_id;
                op.job_action = "close".into();
            }
        }
        op
    }
}

/// A discriminated union keeps action-specific required fields and rejects
/// accidental combinations (for example, command on a read request).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum TerminalArgs {
    List {},
    Open {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(range(min = 20, max = 500))]
        cols: Option<u32>,
        #[schemars(range(min = 5, max = 200))]
        rows: Option<u32>,
    },
    Read {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        session_id: String,
        /// Byte cursor from the previous read, initially 0.
        cursor: Option<u64>,
    },
    Write {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        session_id: String,
        /// Raw input; include newline to execute a command.
        content: String,
    },
    Resize {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        session_id: String,
        #[schemars(range(min = 20, max = 500))]
        cols: u32,
        #[schemars(range(min = 5, max = 200))]
        rows: u32,
    },
    Close {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        session_id: String,
    },
}

impl TerminalArgs {
    pub fn operation(self) -> Option<Operation> {
        let mut op = Operation::default();
        match self {
            Self::List {} => return None,
            Self::Open {
                server_id,
                cols,
                rows,
            } => {
                op.kind = "terminal_open".into();
                op.server_id = server_id;
                op.cols = cols.unwrap_or(100);
                op.rows = rows.unwrap_or(30);
            }
            Self::Read {
                server_id,
                session_id,
                cursor,
            } => {
                op.kind = "terminal_read".into();
                op.server_id = server_id;
                op.session_id = session_id;
                op.cursor = cursor.unwrap_or(0);
            }
            Self::Write {
                server_id,
                session_id,
                content,
            } => {
                op.kind = "terminal_write".into();
                op.server_id = server_id;
                op.session_id = session_id;
                op.content = content;
            }
            Self::Resize {
                server_id,
                session_id,
                cols,
                rows,
            } => {
                op.kind = "terminal_resize".into();
                op.server_id = server_id;
                op.session_id = session_id;
                op.cols = cols;
                op.rows = rows;
            }
            Self::Close {
                server_id,
                session_id,
            } => {
                op.kind = "terminal_close".into();
                op.server_id = server_id;
                op.session_id = session_id;
            }
        }
        Some(op)
    }
}
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum PortForwardArgs {
    Open {
        server_id: String,
        /// Local port on the computer running this app. 0 selects a free port.
        local_port: Option<u16>,
        /// Only 127.0.0.1 or ::1 on the SSH server. Defaults to 127.0.0.1.
        remote_host: Option<String>,
        #[schemars(range(min = 1, max = 65535))]
        remote_port: u16,
    },
    List {
        server_id: String,
    },
    Read {
        server_id: String,
        tunnel_id: String,
    },
    Close {
        server_id: String,
        tunnel_id: String,
    },
}
impl From<PortForwardArgs> for Operation {
    fn from(args: PortForwardArgs) -> Self {
        let mut op = Operation::default();
        match args {
            PortForwardArgs::Open {
                server_id,
                local_port,
                remote_host,
                remote_port,
            } => {
                op.kind = "port_forward_open".into();
                op.server_id = server_id;
                op.local_port = local_port.unwrap_or(0);
                op.remote_host = remote_host.unwrap_or_else(|| "127.0.0.1".into());
                op.remote_port = remote_port;
                op.path = format!("127.0.0.1:{}", op.local_port);
                op.target = format!("{}:{}", op.remote_host, op.remote_port);
            }
            PortForwardArgs::List { server_id } => {
                op.kind = "port_forward_list".into();
                op.server_id = server_id;
            }
            PortForwardArgs::Read {
                server_id,
                tunnel_id,
            } => {
                op.kind = "port_forward_read".into();
                op.server_id = server_id;
                op.tunnel_id = tunnel_id;
            }
            PortForwardArgs::Close {
                server_id,
                tunnel_id,
            } => {
                op.kind = "port_forward_close".into();
                op.server_id = server_id;
                op.tunnel_id = tunnel_id;
            }
        }
        op
    }
}
