use crate::model::{FileEdit, FileRequest, Operation, WriteMode};
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerInfoArgs {
    pub server_id: String,
    #[serde(default)]
    pub topic: ServerInfoTopic,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServerInfoTopic {
    #[default]
    Permissions,
    Transfers,
    Commands,
}

macro_rules! args {
    ($name:ident { $($(#[$attr:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {

            #[schemars(length(min = 1, max = 128))]
            pub server_id: String,
            $($(#[$attr])* pub $field: $ty),*
        }
    };
}

args!(PathArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,
});
args!(ReadArgs {

    #[schemars(length(min = 1, max = 8192))]
    path: String,

    #[schemars(range(min = 1))]
    offset: Option<u64>,

    #[schemars(range(min = 1, max = 2000))]
    limit: Option<u64>,
});
args!(ListArgs {

    #[schemars(length(min = 1, max = 8192))]
    path: String,

    offset: Option<u64>,

    #[schemars(range(min = 1, max = 1000))]
    limit: Option<u64>,
});
args!(WriteArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,

    content: String,

    mode: Option<WriteMode>,

    create_parents: Option<bool>,
});
args!(EditArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,

    #[schemars(length(min = 1, max = 100))]
    edits: Vec<FileEdit>,
});
args!(ExecArgs {
    #[schemars(length(min = 1, max = 16384))]
    command: String,

    #[schemars(range(max = 300))]
    timeout_secs: Option<u64>,
});
args!(RemoveArgs {

    #[schemars(length(min = 1, max = 8192))]
    path: String,

    recursive: Option<bool>,

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
        transfer_id: Option<String>,
        #[schemars(length(min = 1, max = 128))]
        group_id: Option<String>,
        version: Option<u64>,
        #[schemars(range(max = 30))]
        wait_secs: Option<u64>,
    },
    Cancel {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        transfer_id: Option<String>,
        #[schemars(length(min = 1, max = 128))]
        group_id: Option<String>,
    },
    Pause {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        transfer_id: Option<String>,
        #[schemars(length(min = 1, max = 128))]
        group_id: Option<String>,
    },
    Resume {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        transfer_id: Option<String>,
        #[schemars(length(min = 1, max = 128))]
        group_id: Option<String>,
    },
}
impl From<TransferArgs> for Operation {
    fn from(a: TransferArgs) -> Self {
        let mut op = Self::default();
        let (server, id, group, action) = match a {
            TransferArgs::List { server_id } => (server_id, String::new(), None, "list"),
            TransferArgs::Read {
                server_id,
                transfer_id,
                group_id,
                version,
                wait_secs,
            } => {
                op.version = version;
                op.wait_secs = wait_secs.unwrap_or(0);
                (server_id, transfer_id.unwrap_or_default(), group_id, "read")
            }
            TransferArgs::Cancel {
                server_id,
                transfer_id,
                group_id,
            } => (
                server_id,
                transfer_id.unwrap_or_default(),
                group_id,
                "cancel",
            ),
            TransferArgs::Pause {
                server_id,
                transfer_id,
                group_id,
            } => (
                server_id,
                transfer_id.unwrap_or_default(),
                group_id,
                "pause",
            ),
            TransferArgs::Resume {
                server_id,
                transfer_id,
                group_id,
            } => (
                server_id,
                transfer_id.unwrap_or_default(),
                group_id,
                "resume",
            ),
        };
        op.server_id = server;
        op.transfer_id = id;
        op.transfer_group = group;
        op.transfer_action = action.into();
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

        stdout_cursor: Option<u64>,
        stderr_cursor: Option<u64>,

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

        cursor: Option<u64>,

        #[schemars(range(max = 30))]
        wait_secs: Option<u64>,
    },
    Write {
        #[schemars(length(min = 1, max = 128))]
        server_id: String,
        #[schemars(length(min = 1, max = 128))]
        session_id: String,

        content: String,
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
                wait_secs,
            } => {
                op.kind = "terminal_read".into();
                op.server_id = server_id;
                op.session_id = session_id;
                op.cursor = cursor.unwrap_or(0);
                op.wait_secs = wait_secs.unwrap_or(0);
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

        local_port: Option<u16>,

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
