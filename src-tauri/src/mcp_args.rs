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
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Entry index (0-indexed; default 0). Directory changes can invalidate pagination.
    offset: Option<u64>,
    /// Maximum entries, 1-1000 (default 200).
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
});
args!(EditArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Non-overlapping replacements, all matched against the original file, committed together.
    #[schemars(length(min = 1, max = 100))]
    edits: Vec<FileEdit>,
});
args!(ContentArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Base64-encoded file bytes.
    content: String,
});
args!(RenameArgs {
    #[schemars(length(min = 1, max = 8192))]
    path: String,
    /// Absolute destination path; must not already exist.
    #[schemars(length(min = 1, max = 8192))]
    target: String,
});
args!(ExecArgs {
    #[schemars(length(min = 1, max = 16384))]
    command: String,
    /// Seconds, 0 uses application default; otherwise 1-300.
    #[schemars(range(max = 300))]
    timeout_secs: Option<u64>,
});
args!(OpenArgs {
    #[schemars(range(min = 20, max = 500))]
    cols: Option<u32>,
    #[schemars(range(min = 5, max = 200))]
    rows: Option<u32>,
});
args!(SessionArgs {
    #[schemars(length(min = 1, max = 128))]
    session_id: String,
});
args!(TerminalReadArgs {
    #[schemars(length(min = 1, max = 128))]
    session_id: String,
    /// Byte cursor returned by the previous read (default 0).
    cursor: Option<u64>,
});
args!(TerminalWriteArgs {
    #[schemars(length(min = 1, max = 128))]
    session_id: String,
    /// Raw terminal input. Include newline to execute a command.
    content: String,
});
args!(ResizeArgs {
    #[schemars(length(min = 1, max = 128))]
    session_id: String,
    #[schemars(range(min = 20, max = 500))]
    cols: u32,
    #[schemars(range(min = 5, max = 200))]
    rows: u32,
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
into_op!(ListArgs, a, {path: a.path, file_request: Some(FileRequest::List {offset:a.offset.unwrap_or(0),limit:a.limit.unwrap_or(200)})});
into_op!(WriteArgs, a, {path:a.path, content:a.content, file_request:Some(FileRequest::Write {mode:a.mode.unwrap_or_default()})});
into_op!(EditArgs, a, {path:a.path, file_request:Some(FileRequest::Edit {edits:a.edits})});
into_op!(ContentArgs, a, {path:a.path, content:a.content});
into_op!(RenameArgs, a, {path:a.path, target:a.target});
into_op!(ExecArgs, a, {command:a.command, timeout_secs:a.timeout_secs.unwrap_or(0)});
into_op!(OpenArgs, a, {cols:a.cols.unwrap_or(100), rows:a.rows.unwrap_or(30)});
into_op!(SessionArgs, a, {session_id:a.session_id});
into_op!(TerminalReadArgs, a, {session_id:a.session_id, cursor:a.cursor.unwrap_or(0)});
into_op!(TerminalWriteArgs, a, {session_id:a.session_id, content:a.content});
into_op!(ResizeArgs, a, {session_id:a.session_id, cols:a.cols, rows:a.rows});
