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
args!(WriteArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Complete UTF-8 file content. Use edit_file for targeted changes.
    content: String,
    /// overwrite creates or replaces (default); create refuses existing targets.
    mode: Option<WriteMode>,
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
into_op!(WriteArgs, a, {path:a.path, content:a.content, file_request:Some(FileRequest::Write {mode:a.mode.unwrap_or_default()})});
into_op!(EditArgs, a, {path:a.path, file_request:Some(FileRequest::Edit {edits:a.edits})});
into_op!(ExecArgs, a, {command:a.command, timeout_secs:a.timeout_secs.unwrap_or(0)});

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
