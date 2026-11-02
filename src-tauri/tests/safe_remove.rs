use anyhow::Result;
use remote_ssh_brige_lib::{
    file_tools::{self, error, error_code},
    model::{FileRequest, Operation, RuntimeSettings, Server},
    safe_remove::{self, FileSystem, Kind, Metadata, MAX_DEPTH, MAX_ENTRIES},
    security,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

#[derive(Clone)]
struct Node {
    metadata: Metadata,
    link: Option<String>,
}
#[derive(Default)]
struct Fs {
    nodes: RefCell<BTreeMap<String, Node>>,
    removed: RefCell<Vec<String>>,
    fail: RefCell<Option<(String, &'static str, bool)>>,
    swap: RefCell<Option<(usize, String, String)>>,
    realpath_calls: Cell<usize>,
}
impl Fs {
    fn new() -> Self {
        let fs = Self::default();
        for p in ["/", "/workspace", "/outside"] {
            fs.add(p, Kind::Directory);
        }
        fs
    }
    fn add(&self, p: &str, kind: Kind) {
        self.nodes.borrow_mut().insert(
            p.into(),
            Node {
                metadata: Metadata {
                    kind,
                    size: Some(0),
                    modified: Some(1),
                },
                link: None,
            },
        );
    }
    fn link(&self, p: &str, target: &str) {
        self.add(p, Kind::Symlink);
        self.nodes.borrow_mut().get_mut(p).unwrap().link = Some(target.into());
    }
    fn resolve(&self, p: &str, depth: usize) -> Result<String> {
        if depth > 16 {
            return Err(error("OPERATION_FAILED", "link cycle"));
        }
        let parts: Vec<_> = p.split('/').filter(|p| !p.is_empty()).collect();
        let mut current = String::new();
        for (i, part) in parts.iter().enumerate() {
            current.push('/');
            current.push_str(part);
            let node = self
                .nodes
                .borrow()
                .get(&current)
                .cloned()
                .ok_or_else(|| error("NOT_FOUND", "missing"))?;
            if let Some(link) = node.link {
                let tail = parts[i + 1..].join("/");
                return self.resolve(
                    &format!(
                        "{link}{}",
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!("/{tail}")
                        }
                    ),
                    depth + 1,
                );
            }
        }
        Ok(if current.is_empty() {
            "/".into()
        } else {
            current
        })
    }
    fn delete(&self, p: &str) -> Result<()> {
        let failure = self.fail.borrow().clone().filter(|(path, _, _)| path == p);
        if failure.as_ref().is_none_or(|(_, _, after)| *after) {
            self.nodes
                .borrow_mut()
                .remove(p)
                .ok_or_else(|| error("NOT_FOUND", "missing"))?;
            self.removed.borrow_mut().push(p.into());
        }
        if let Some((_, code, _)) = failure {
            return Err(error(code, "injected deletion failure"));
        }
        Ok(())
    }
}
impl FileSystem for Fs {
    fn realpath(&self, p: &str) -> Result<String> {
        let call = self.realpath_calls.get() + 1;
        self.realpath_calls.set(call);
        let change = self.swap.borrow().clone().filter(|(at, _, _)| *at == call);
        if let Some((_, path, target)) = change {
            self.link(&path, &target);
        }
        self.resolve(p, 0)
    }
    fn metadata(&self, p: &str) -> Result<Metadata> {
        self.nodes
            .borrow()
            .get(p)
            .map(|n| n.metadata.clone())
            .ok_or_else(|| error("NOT_FOUND", "missing"))
    }
    fn entries(&self, p: &str, limit: usize, exhaustive: bool) -> Result<Vec<String>> {
        let prefix = format!("{p}/");
        let names: Vec<_> = self
            .nodes
            .borrow()
            .keys()
            .filter_map(|name| name.strip_prefix(&prefix))
            .filter(|name| !name.contains('/'))
            .map(str::to_string)
            .collect();
        if exhaustive && names.len() > limit {
            return Err(error("REMOVE_LIMIT", "entry limit"));
        }
        Ok(names.into_iter().take(limit).collect())
    }
    fn unlink(&self, p: &str) -> Result<()> {
        self.delete(p)
    }
    fn rmdir(&self, p: &str) -> Result<()> {
        if self
            .nodes
            .borrow()
            .keys()
            .any(|name| name.starts_with(&format!("{p}/")))
        {
            return Err(error("DIRECTORY_NOT_EMPTY", "not empty"));
        }
        self.delete(p)
    }
}
#[test]
fn exact_file_cleanup_is_idempotent_and_missing_can_be_strict() {
    let fs = Fs::new();
    fs.add("/tmp", Kind::Directory);
    fs.add("/tmp/mcp_test_remote_ssh.txt", Kind::File);
    assert_eq!(
        safe_remove::remove(&fs, "/", "/tmp/mcp_test_remote_ssh.txt", false, true).unwrap()
            ["removed"],
        true
    );
    fs.add("/workspace/mcp_test_remote_ssh.txt", Kind::File);
    let result = safe_remove::remove(
        &fs,
        "/workspace",
        "/workspace/mcp_test_remote_ssh.txt",
        false,
        true,
    )
    .unwrap();
    assert_eq!(result["removed_entries"], 1);
    assert_eq!(result["removed"], true);
    let again = safe_remove::remove(
        &fs,
        "/workspace",
        "/workspace/mcp_test_remote_ssh.txt",
        false,
        true,
    )
    .unwrap();
    assert_eq!(again["missing"], true);
    assert_eq!(again["removed_entries"], 0);
    assert_eq!(
        error_code(
            &safe_remove::remove(
                &fs,
                "/workspace",
                "/workspace/mcp_test_remote_ssh.txt",
                false,
                false
            )
            .unwrap_err()
        ),
        "NOT_FOUND"
    );
    assert_eq!(
        safe_remove::remove(&fs, "/workspace", "/workspace/missing/child", false, true).unwrap()
            ["missing"],
        true
    );
}
#[test]
fn non_empty_directory_requires_recursive_and_preflight_does_not_delete() {
    let fs = Fs::new();
    fs.add("/workspace/tree", Kind::Directory);
    fs.add("/workspace/tree/a", Kind::File);
    fs.add("/workspace/tree/sub", Kind::Directory);
    fs.add("/workspace/tree/sub/b", Kind::File);
    assert_eq!(
        error_code(
            &safe_remove::remove(&fs, "/workspace", "/workspace/tree", false, true).unwrap_err()
        ),
        "DIRECTORY_NOT_EMPTY"
    );
    assert!(fs.removed.borrow().is_empty());
    assert_eq!(
        safe_remove::remove(&fs, "/workspace", "/workspace/tree", true, true).unwrap()
            ["removed_entries"],
        4
    );
    assert_eq!(fs.removed.borrow().last().unwrap(), "/workspace/tree");
}
#[test]
fn symlinks_are_unlinked_without_following_external_or_dangling_targets() {
    let fs = Fs::new();
    fs.add("/outside/secret", Kind::File);
    fs.link("/workspace/external", "/outside/secret");
    fs.link("/workspace/dangling", "/outside/missing");
    for p in ["/workspace/external", "/workspace/dangling"] {
        assert_eq!(
            safe_remove::remove(&fs, "/workspace", p, true, true).unwrap()["kind"],
            "symlink"
        );
    }
    assert!(fs.nodes.borrow().contains_key("/outside/secret"));
    fs.add("/workspace/tree", Kind::Directory);
    fs.link("/workspace/tree/link", "/outside");
    assert_eq!(
        safe_remove::remove(&fs, "/workspace", "/workspace/tree", true, true).unwrap()
            ["removed_entries"],
        2
    );
    assert!(fs.nodes.borrow().contains_key("/outside/secret"));
}
#[test]
fn protects_roots_system_directories_and_path_boundaries() {
    let fs = Fs::new();
    for p in [
        "/workspace",
        "/workspace/",
        "//workspace//",
        "/workspace-other/file",
        "/outside/secret",
        "relative",
        "/workspace/../outside",
        "/workspace/./file",
    ] {
        assert!(
            safe_remove::remove(&fs, "/workspace", p, true, true).is_err(),
            "{p}"
        );
    }
    for p in ["/", "/etc", "/tmp", "/var", "/home", "/usr"] {
        assert_eq!(
            error_code(&safe_remove::remove(&fs, "/", p, true, true).unwrap_err()),
            "PROTECTED_PATH"
        );
    }
    assert!(fs.removed.borrow().is_empty());
    fs.link("/workspace/escape", "/outside");
    fs.add("/outside/file", Kind::File);
    assert_eq!(
        error_code(
            &safe_remove::remove(&fs, "/workspace", "/workspace/escape/file", false, true)
                .unwrap_err()
        ),
        "PATH_OUTSIDE_ROOT"
    );
}
#[test]
fn configured_root_alias_is_supported_but_cannot_itself_be_deleted() {
    let fs = Fs::new();
    fs.link("/alias", "/workspace");
    fs.add("/workspace/file", Kind::File);
    assert!(
        safe_remove::remove(&fs, "/alias", "/alias/file", false, true).unwrap()["removed"]
            .as_bool()
            .unwrap()
    );
    for p in ["/alias", "/workspace"] {
        assert_eq!(
            error_code(&safe_remove::remove(&fs, "/alias", p, true, true).unwrap_err()),
            "PROTECTED_PATH"
        );
    }
}
#[test]
fn recursion_limits_fail_before_any_mutation() {
    let fs = Fs::new();
    fs.add("/workspace/tree", Kind::Directory);
    for n in 0..MAX_ENTRIES {
        fs.add(&format!("/workspace/tree/{n}"), Kind::File);
    }
    assert_eq!(
        error_code(
            &safe_remove::remove(&fs, "/workspace", "/workspace/tree", true, true).unwrap_err()
        ),
        "REMOVE_LIMIT"
    );
    assert!(fs.removed.borrow().is_empty());
    let fs = Fs::new();
    let mut p = "/workspace/tree".to_string();
    for _ in 0..MAX_DEPTH + 2 {
        fs.add(&p, Kind::Directory);
        p.push_str("/d");
    }
    assert_eq!(
        error_code(
            &safe_remove::remove(&fs, "/workspace", "/workspace/tree", true, true).unwrap_err()
        ),
        "REMOVE_LIMIT"
    );
    assert!(fs.removed.borrow().is_empty());
}
#[test]
fn parent_changed_to_symlink_stops_before_deletion() {
    let fs = Fs::new();
    fs.add("/workspace/tree", Kind::Directory);
    fs.add("/workspace/tree/file", Kind::File);
    fs.add("/outside/file", Kind::File);
    *fs.swap.borrow_mut() = Some((4, "/workspace/tree".into(), "/outside".into()));
    assert_eq!(
        error_code(
            &safe_remove::remove(&fs, "/workspace", "/workspace/tree", true, true).unwrap_err()
        ),
        "PATH_CHANGED"
    );
    assert!(fs.removed.borrow().is_empty());
    assert!(fs.nodes.borrow().contains_key("/outside/file"));
}
#[test]
fn failed_recursive_deletion_reports_acknowledged_partial_progress() {
    let fs = Fs::new();
    fs.add("/workspace/tree", Kind::Directory);
    for p in ["a", "b"] {
        fs.add(&format!("/workspace/tree/{p}"), Kind::File);
    }
    *fs.fail.borrow_mut() = Some(("/workspace/tree/b".into(), "PERMISSION_DENIED", false));
    let failure =
        safe_remove::remove(&fs, "/workspace", "/workspace/tree", true, true).unwrap_err();
    let details = failure
        .downcast_ref::<file_tools::FileError>()
        .unwrap()
        .details
        .as_ref()
        .unwrap();
    assert_eq!(details["partial"], true);
    assert_eq!(details["removed_entries"], 1);
    assert_eq!(details["failed_path"], "/workspace/tree/b");
    assert!(fs.nodes.borrow().contains_key("/workspace/tree/b"));
}
#[test]
fn lost_delete_response_is_not_reported_as_a_clean_failure() {
    let fs = Fs::new();
    fs.add("/workspace/file", Kind::File);
    *fs.fail.borrow_mut() = Some(("/workspace/file".into(), "COMMIT_STATUS_UNKNOWN", true));
    let failure =
        safe_remove::remove(&fs, "/workspace", "/workspace/file", false, true).unwrap_err();
    let details = failure
        .downcast_ref::<file_tools::FileError>()
        .unwrap()
        .details
        .as_ref()
        .unwrap();
    assert_eq!(error_code(&failure), "COMMIT_STATUS_UNKNOWN");
    assert_eq!(details["outcome_unknown"], true);
    assert_eq!(details["partial"], true);
    assert!(fs.nodes.borrow().get("/workspace/file").is_none());
}
#[test]
fn deletion_permission_is_opt_in_and_respects_global_protection() {
    let op = Operation {
        kind: "remove_path".into(),
        file_request: Some(FileRequest::Remove {
            recursive: false,
            missing_ok: true,
        }),
        ..Default::default()
    };
    let server = Server {
        allow_write: true,
        allow_exec: true,
        ..Default::default()
    };
    assert!(!server.permits_mcp("remove_path"));
    let server = Server {
        mcp_permissions: Some(vec!["remove".into()]),
        ..server
    };
    assert!(!server.permits_mcp("remove_path"));
    let server = Server {
        mcp_permissions: Some(vec!["remove_path".into()]),
        ..server
    };
    assert!(server.permits_mcp("remove_path"));
    let mut settings = RuntimeSettings::default();
    assert!(settings.deletion_protection && settings.block_dangerous_commands);
    assert!(security::check(&settings, &op, "mcp").is_err());
    assert!(security::check(
        &settings,
        &Operation {
            kind: "remove".into(),
            ..Default::default()
        },
        "desktop"
    )
    .is_err());
    settings.deletion_protection = false;
    assert!(security::check(&settings, &op, "mcp").is_ok());
    assert!(security::check(
        &settings,
        &Operation {
            kind: "remove".into(),
            ..Default::default()
        },
        "desktop"
    )
    .is_ok());
    assert!(security::check_command_with_settings("rm /tmp/test", &settings).is_err());
    settings.approval_mode = "mcp".into();
    assert!(security::needs_approval(&settings, &op, "mcp"));
    settings.remote_read_only = true;
    assert!(security::check(&settings, &op, "mcp").is_err());
}
