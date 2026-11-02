//! Root-scoped SFTP deletion. Preflight first; never recurse through symlinks.
use crate::operation_error::{error, error_code, error_with_details};
use anyhow::Result;
use serde_json::{json, Value};

pub const MAX_ENTRIES: usize = 10000;
pub const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    File,
    Directory,
    Symlink,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    pub kind: Kind,
    pub size: Option<u64>,
    pub modified: Option<u64>,
}

pub trait FileSystem {
    fn realpath(&self, path: &str) -> Result<String>;
    /// lstat, not stat: the final symlink is not followed.
    fn metadata(&self, path: &str) -> Result<Metadata>;
    /// Return exact single-component names; fail rather than exceeding the bound.
    fn entries(&self, path: &str, limit: usize, exhaustive: bool) -> Result<Vec<String>>;
    fn unlink(&self, path: &str) -> Result<()>;
    fn rmdir(&self, path: &str) -> Result<()>;
}

fn absolute(path: &str) -> Result<String> {
    if !path.starts_with('/')
        || path.len() > 8192
        || path.contains(['\0', '\\'])
        || path.split('/').any(|part| part == ".." || part == ".")
    {
        return Err(error(
            "INVALID_ARGUMENT",
            "Use an absolute POSIX path without dot segments, NUL or backslashes",
        ));
    }
    let normalized = format!(
        "/{}",
        path.split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    );
    Ok(normalized)
}
fn within(root: &str, path: &str) -> bool {
    root == "/"
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}
fn protected(root: &str, path: &str) -> Result<()> {
    if !within(root, path) {
        return Err(error(
            "PATH_OUTSIDE_ROOT",
            "Deletion path is outside the configured root",
        ));
    }
    if path == root
        || [
            "/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/media", "/mnt",
            "/opt", "/proc", "/root", "/run", "/sbin", "/srv", "/sys", "/tmp", "/usr", "/var",
        ]
        .contains(&path)
    {
        return Err(error(
            "PROTECTED_PATH",
            "Cannot delete the configured root or a system top-level directory",
        ));
    }
    Ok(())
}
fn parent(path: &str) -> &str {
    let index = path.rfind('/').unwrap();
    if index == 0 {
        "/"
    } else {
        &path[..index]
    }
}
fn missing(error: &anyhow::Error) -> bool {
    error_code(error) == "NOT_FOUND"
}

/// Resolve the parent only, preserving the final link itself even if dangling or external.
fn target(fs: &impl FileSystem, root: &str, path: &str) -> Result<String> {
    let canonical_parent = absolute(&fs.realpath(parent(path))?)?;
    let leaf = path.rsplit('/').next().unwrap();
    let resolved = format!("{}/{leaf}", canonical_parent.trim_end_matches('/'));
    protected(root, &resolved)?;
    Ok(resolved)
}

struct Entry {
    path: String,
    metadata: Metadata,
}
struct Planner<'a, F: FileSystem> {
    fs: &'a F,
    root: &'a str,
    recursive: bool,
    count: usize,
    entries: Vec<Entry>,
}
impl<F: FileSystem> Planner<'_, F> {
    fn visit(&mut self, path: String, metadata: Metadata, depth: usize) -> Result<()> {
        let path = absolute(&path)?;
        if depth > MAX_DEPTH || self.count >= MAX_ENTRIES {
            return Err(error(
                "REMOVE_LIMIT",
                "Deletion exceeds the 10000-entry or depth-64 limit; no entries were deleted",
            ));
        }
        self.count += 1;
        protected(self.root, &path)?;
        if metadata.kind == Kind::Directory {
            if absolute(&self.fs.realpath(&path)?)? != path {
                return Err(error(
                    "PATH_CHANGED",
                    "Directory changed or became a symlink; nothing was deleted",
                ));
            }
            let limit = if self.recursive {
                MAX_ENTRIES - self.count
            } else {
                1
            };
            let children = self.fs.entries(&path, limit, self.recursive)?;
            if !self.recursive && !children.is_empty() {
                return Err(error(
                    "DIRECTORY_NOT_EMPTY",
                    "Directory is not empty; set recursive=true explicitly",
                ));
            }
            for name in children {
                if name.is_empty()
                    || name == "."
                    || name == ".."
                    || name.contains(['/', '\\', '\0'])
                {
                    return Err(error(
                        "INVALID_DIRECTORY_ENTRY",
                        "Server returned an invalid directory entry",
                    ));
                }
                let child = format!("{path}/{name}");
                let info = self.fs.metadata(&child)?;
                self.visit(child, info, depth + 1)?;
            }
        }
        self.entries.push(Entry { path, metadata });
        Ok(())
    }
}

pub fn remove(
    fs: &impl FileSystem,
    root: &str,
    path: &str,
    recursive: bool,
    missing_ok: bool,
) -> Result<Value> {
    let configured_root = absolute(root)?;
    let root = absolute(&fs.realpath(root)?)?;
    let path = absolute(path)?;
    protected(
        if within(&configured_root, &path) {
            &configured_root
        } else {
            &root
        },
        &path,
    )?;
    let located = target(fs, &root, &path)
        .and_then(|path| fs.metadata(&path).map(|metadata| (path, metadata)));
    let (resolved, metadata) = match located {
        Ok(found) => found,
        Err(e) if missing_ok && missing(&e) => {
            return Ok(
                json!({"path":path,"removed":false,"missing":true,"removed_entries":0,"recursive":recursive}),
            )
        }
        Err(e) => return Err(e),
    };
    let kind = match metadata.kind {
        Kind::File => "file",
        Kind::Directory => "directory",
        Kind::Symlink => "symlink",
    };
    let mut plan = Planner {
        fs,
        root: &root,
        recursive,
        count: 0,
        entries: Vec::new(),
    };
    plan.visit(resolved, metadata, 0)?;
    let mut removed = 0usize;
    for entry in plan.entries {
        let result = (|| -> Result<bool> {
            // Parent aliases cannot redirect a planned deletion into a different tree.
            if absolute(&fs.realpath(parent(&entry.path))?)? != parent(&entry.path) {
                return Err(error(
                    "PATH_CHANGED",
                    "Parent directory changed during deletion",
                ));
            }
            protected(&root, &entry.path)?;
            let now = match fs.metadata(&entry.path) {
                Ok(now) => now,
                Err(e) if missing_ok && missing(&e) => return Ok(false),
                Err(e) => return Err(e),
            };
            if now.kind != entry.metadata.kind
                || (now.kind != Kind::Directory && now != entry.metadata)
            {
                return Err(error(
                    "PATH_CHANGED",
                    "File changed after preflight; deletion stopped",
                ));
            }
            if now.kind == Kind::Directory {
                if absolute(&fs.realpath(&entry.path)?)? != entry.path {
                    return Err(error("PATH_CHANGED", "Directory changed after preflight"));
                }
                fs.rmdir(&entry.path)?;
            } else {
                fs.unlink(&entry.path)?;
            }
            Ok(true)
        })();
        match result {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(e) if missing_ok && missing(&e) => {}
            Err(e) => {
                let code = error_code(&e);
                return Err(error_with_details(
                    code,
                    "Deletion stopped; inspect the failed path before retrying",
                    json!({"failed_path":entry.path,"removed_entries":removed,"partial":removed>0 || code=="COMMIT_STATUS_UNKNOWN","outcome_unknown":code=="COMMIT_STATUS_UNKNOWN"}),
                ));
            }
        }
    }
    Ok(
        json!({"path":path,"removed":removed>0,"missing":false,"removed_entries":removed,"kind":kind,"recursive":recursive}),
    )
}
