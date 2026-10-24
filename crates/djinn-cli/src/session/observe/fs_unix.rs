use std::ffi::CString;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::{EvidenceResult, EVENTS_LIMIT, MANIFEST_LIMIT};

pub(super) struct Snapshot {
    pub resolved: String,
    pub manifest: Vec<u8>,
    pub events: Option<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Stamp {
    fn of(meta: &Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}

struct Artifact {
    file: File,
    stamp: Stamp,
    bytes: Vec<u8>,
}

fn open_at(root: &File, name: &str) -> io::Result<File> {
    let name = CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points to writable storage; name is NUL terminated.
    if unsafe {
        libc::fstatat(
            root.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat succeeded and initialized stat.
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG || stat.st_nlink != 1 {
        return Err(io::Error::other("unsafe source"));
    }
    // Names are fixed single components. NONBLOCK prevents FIFO/device opens from waiting.
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn regular(file: &File) -> EvidenceResult<Metadata> {
    let meta = file.metadata().map_err(|_| "source_io")?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err("unsafe_source");
    }
    Ok(meta)
}

fn validate(root: &File, name: &str, artifact: &Option<Artifact>) -> EvidenceResult<()> {
    match artifact {
        Some(artifact) => {
            if Stamp::of(&regular(&artifact.file)?) != artifact.stamp {
                return Err("source_changed");
            }
            let current = open_at(root, name).map_err(|_| "source_changed")?;
            if Stamp::of(&regular(&current)?) != artifact.stamp {
                return Err("source_changed");
            }
        }
        None => match open_at(root, name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            _ => return Err("source_changed"),
        },
    }
    Ok(())
}

fn read_artifact(
    root: &File,
    name: &str,
    cap: usize,
    optional: bool,
) -> EvidenceResult<Option<Artifact>> {
    let file = match open_at(root, name) {
        Ok(file) => file,
        Err(error) if optional && error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("source_open_failed"),
    };
    let stamp = Stamp::of(&regular(&file)?);
    if stamp.size > cap as u64 {
        return Err("source_limit");
    }
    let mut artifact = Some(Artifact {
        file,
        stamp,
        bytes: Vec::new(),
    });
    validate(root, name, &artifact)?;
    let entry = artifact.as_mut().ok_or("source_open_failed")?;
    (&mut entry.file)
        .take(cap as u64 + 1)
        .read_to_end(&mut entry.bytes)
        .map_err(|_| "source_io")?;
    if entry.bytes.len() > cap {
        return Err("source_limit");
    }
    validate(root, name, &artifact)?;
    Ok(artifact)
}

fn resolve(path: &Path) -> EvidenceResult<PathBuf> {
    // Resolve ancestor aliases only; the leaf must itself be a real directory.
    let parent = path.parent().ok_or("invalid_folder")?;
    let leaf = path.file_name().ok_or("invalid_folder")?;
    let parent = fs::canonicalize(parent).map_err(|_| "folder_unavailable")?;
    Ok(parent.join(leaf))
}

pub(super) fn read(path: &Path) -> EvidenceResult<Snapshot> {
    read_with_hook(path, || {})
}

fn read_with_hook(path: &Path, after_reads: impl FnOnce()) -> EvidenceResult<Snapshot> {
    let resolved = resolve(path)?;
    let resolved_text = resolved.to_str().ok_or("non_utf8_resolution")?.to_owned();
    let root = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(&resolved)
        .map_err(|_| "unsafe_folder")?;
    let initial = root.metadata().map_err(|_| "folder_unavailable")?;
    let same_root = || -> EvidenceResult<()> {
        if resolve(path)? != resolved {
            return Err("root_changed");
        }
        let current = fs::symlink_metadata(&resolved).map_err(|_| "root_changed")?;
        let fd = root.metadata().map_err(|_| "root_changed")?;
        if !current.is_dir()
            || current.dev() != initial.dev()
            || current.ino() != initial.ino()
            || fd.dev() != initial.dev()
            || fd.ino() != initial.ino()
            || fd.nlink() == 0
        {
            return Err("root_changed");
        }
        Ok(())
    };
    same_root()?;
    let manifest = read_artifact(&root, "djinn.toml", MANIFEST_LIMIT, false)?;
    let events = read_artifact(&root, "events.jsonl", EVENTS_LIMIT, true)?;
    after_reads();
    validate(&root, "djinn.toml", &manifest)?;
    validate(&root, "events.jsonl", &events)?;
    same_root()?;
    Ok(Snapshot {
        resolved: resolved_text,
        manifest: manifest.ok_or("source_open_failed")?.bytes,
        events: events.map(|artifact| artifact.bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "djinn-observe-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::write(path.join("djinn.toml"), "session_id = 'agt_fixture'\n").unwrap();
            Self(path)
        }
        fn report(&self) -> serde_json::Value {
            super::super::observe(&self.0).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn unavailable(report: serde_json::Value) {
        assert_eq!(report["assessment"], "unavailable");
        assert!(report["observation"].is_null());
        assert_eq!(report["diagnostics"].as_array().unwrap().len(), 1);
        assert_eq!(report["native_lifecycle"], "unestablished");
        assert_eq!(report["dispatchable"], false);
    }

    #[test]
    fn real_manifest_missing_and_empty_events() {
        let fixture = Fixture::new();
        let report = fixture.report();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["contract"], "djinn.session.observation.v1");
        assert_eq!(report["assessment"], "observed");
        assert_eq!(
            report["resolved_folder"],
            fs::canonicalize(&fixture.0).unwrap().to_str().unwrap()
        );
        assert_eq!(report["diagnostics"], serde_json::json!([]));
        assert_eq!(
            report["observation"]["sources"]["folder_events"]["status"],
            "missing"
        );
        assert_eq!(report.as_object().unwrap().len(), 16);
        assert_eq!(report["observation"].as_object().unwrap().len(), 7);
        fs::write(fixture.0.join("events.jsonl"), "").unwrap();
        let report = fixture.report();
        assert_eq!(report["observation"]["event_count"], 0);
        assert_eq!(
            report["observation"]["sources"]["folder_events"]["status"],
            "observed"
        );
    }

    #[test]
    fn observes_native_writer_artifacts_without_modifying_them() {
        use djinn_memory::{
            AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionExecutionMode,
            AgentSessionId, AgentSessionLifecycleState, AgentSessionMeta,
        };
        use serde_json::json;
        use sha2::{Digest, Sha256};

        let fixture = Fixture::new();
        let mut session = AgentSession::new(
            AgentSessionId::fresh(),
            AgentSessionMeta {
                title: "PRIVATE_NATIVE_TITLE".to_owned(),
                created_at: chrono::Utc::now().to_rfc3339(),
                ..AgentSessionMeta::default()
            },
        );
        session.events = vec![
            AgentSessionEvent::with_session(
                session.id.clone(),
                AgentSessionEventKind::SessionCreated {
                    id: session.id.clone(),
                    meta: session.meta.clone(),
                },
            ),
            AgentSessionEvent::with_session(
                session.id.clone(),
                AgentSessionEventKind::SessionLifecycleUpdated {
                    state: AgentSessionLifecycleState::Running,
                    mode: Some(AgentSessionExecutionMode::Background),
                    reason: Some("PRIVATE_NATIVE_REASON".to_owned()),
                    note: Some("PRIVATE_NATIVE_NOTE".to_owned()),
                },
            ),
            AgentSessionEvent::with_session(
                session.id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "PRIVATE_NATIVE_CONTENT".to_owned(),
                },
            ),
            AgentSessionEvent::with_session(
                session.id.clone(),
                AgentSessionEventKind::ChildSessionStatusChanged {
                    child_session_id: AgentSessionId::fresh(),
                    state: AgentSessionLifecycleState::Completed,
                    mode: Some(AgentSessionExecutionMode::Foreground),
                    reason: None,
                    summary_pointer: Some("PRIVATE_NATIVE_SUMMARY".to_owned()),
                },
            ),
        ];
        crate::session::manifest::write_agent_session_toml(&fixture.0, &session).unwrap();
        crate::session::projection::write_folder_session_events_jsonl(&fixture.0, &session)
            .unwrap();

        let snapshot = || {
            let mut entries = fs::read_dir(&fixture.0)
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    let metadata = fs::symlink_metadata(&path).unwrap();
                    let bytes = fs::read(&path).unwrap();
                    (path, Stamp::of(&metadata), bytes)
                })
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            (Stamp::of(&fs::metadata(&fixture.0).unwrap()), entries)
        };
        let before = snapshot();
        assert_eq!(before.1.len(), 2);
        let report = fixture.report();
        assert_eq!(report["assessment"], "observed");
        assert_eq!(report["diagnostics"], json!([]));
        assert_eq!(report["native_lifecycle"], "unestablished");
        let observation = &report["observation"];
        assert_eq!(
            observation["recorded_native_session_id"],
            session.id.as_str()
        );
        assert_eq!(observation["recorded_created_at"], session.meta.created_at);
        assert_eq!(observation["event_count"], 4);
        let lifecycle = &session.events[1];
        assert_eq!(
            observation["last_lifecycle"],
            json!({
                "state": "running", "mode": "background", "event_id": lifecycle.event_id,
                "session_id": session.id.as_str(), "recorded_at": lifecycle.created_at
            })
        );
        let last = &session.events[3];
        assert_eq!(
            observation["last_event"],
            json!({
                "kind": "child_session_status_changed", "event_id": last.event_id,
                "session_id": session.id.as_str(), "recorded_at": last.created_at
            })
        );
        for (path, _, bytes) in &before.1 {
            let source = if path.file_name().unwrap() == "djinn.toml" {
                "manifest"
            } else {
                "folder_events"
            };
            assert_eq!(observation["sources"][source]["bytes"], bytes.len() as u64);
            assert_eq!(
                observation["sources"][source]["sha256"],
                format!("{:x}", Sha256::digest(bytes))
            );
        }
        assert!(!report.to_string().contains("PRIVATE_NATIVE_"));
        assert_eq!(before, snapshot());
    }

    #[test]
    fn malformed_source_and_missing_manifest_have_no_partial_facts() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("events.jsonl"), "{\"type\":\"x\"}\n{bad}\n").unwrap();
        unavailable(fixture.report());
        fs::remove_file(fixture.0.join("djinn.toml")).unwrap();
        unavailable(fixture.report());
    }

    #[test]
    fn source_symlinks_directories_and_hardlinks_are_refused() {
        for name in ["djinn.toml", "events.jsonl"] {
            for kind in ["symlink", "directory", "hardlink"] {
                let fixture = Fixture::new();
                let target = fixture.0.join(name);
                if target.exists() {
                    fs::remove_file(&target).unwrap();
                }
                let other = fixture.0.join("other");
                fs::write(&other, "").unwrap();
                match kind {
                    "symlink" => symlink(&other, &target).unwrap(),
                    "directory" => fs::create_dir(&target).unwrap(),
                    _ => fs::hard_link(&other, &target).unwrap(),
                }
                unavailable(fixture.report());
            }
        }
    }

    #[test]
    fn fifos_and_unix_sockets_are_refused_without_waiting() {
        for name in ["djinn.toml", "events.jsonl"] {
            let fixture = Fixture::new();
            let target = fixture.0.join(name);
            if target.exists() {
                fs::remove_file(&target).unwrap();
            }
            let name = CString::new(target.as_os_str().as_encoded_bytes()).unwrap();
            // SAFETY: name is a valid NUL-terminated path; mkfifo creates a test-only FIFO.
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            unavailable(fixture.report());
            fs::remove_file(&target).unwrap();
            let _socket = std::os::unix::net::UnixListener::bind(&target).unwrap();
            unavailable(fixture.report());
        }
    }

    #[test]
    fn leaf_root_symlink_refused_but_parent_alias_allowed() {
        let fixture = Fixture::new();
        let child = fixture.0.join("child");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("djinn.toml"), "").unwrap();
        let leaf = fixture.0.join("leaf");
        symlink(&child, &leaf).unwrap();
        unavailable(super::super::observe(&leaf).unwrap());
        let parent = fixture.0.join("parent");
        symlink(&fixture.0, &parent).unwrap();
        let report = super::super::observe(&parent.join("child")).unwrap();
        assert_eq!(report["assessment"], "observed");
        assert_eq!(
            report["resolved_folder"],
            fs::canonicalize(child).unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn final_check_rejects_mutation_replacement_and_missing_appearance() {
        for change in [
            "modify_manifest",
            "replace_manifest",
            "modify_events",
            "replace_events",
            "missing_appears",
            "events_disappear",
            "missing_symlink",
        ] {
            let fixture = Fixture::new();
            let events = fixture.0.join("events.jsonl");
            if !["missing_appears", "missing_symlink"].contains(&change) {
                fs::write(&events, "").unwrap();
            }
            let result = read_with_hook(&fixture.0, || match change {
                "modify_manifest" => {
                    fs::write(fixture.0.join("djinn.toml"), "# changed\n").unwrap()
                }
                "replace_manifest" => {
                    fs::rename(fixture.0.join("djinn.toml"), fixture.0.join("old")).unwrap();
                    fs::write(fixture.0.join("djinn.toml"), "session_id = 'agt_fixture'\n")
                        .unwrap();
                }
                "modify_events" => fs::write(&events, "{\"type\":\"x\"}\n").unwrap(),
                "replace_events" => {
                    fs::rename(&events, fixture.0.join("old")).unwrap();
                    fs::write(&events, "").unwrap();
                }
                "events_disappear" => fs::remove_file(&events).unwrap(),
                "missing_symlink" => symlink("nonexistent", &events).unwrap(),
                _ => fs::write(&events, "").unwrap(),
            });
            assert!(result.is_err(), "{change}");
        }
    }

    #[test]
    fn final_check_rejects_root_replacement_and_renaming() {
        for replace in [false, true] {
            let fixture = Fixture::new();
            let child = fixture.0.join("child");
            fs::create_dir(&child).unwrap();
            fs::write(child.join("djinn.toml"), "").unwrap();
            assert!(read_with_hook(&child, || {
                fs::rename(&child, fixture.0.join("moved")).unwrap();
                if replace {
                    fs::create_dir(&child).unwrap();
                    fs::write(child.join("djinn.toml"), "").unwrap();
                }
            })
            .is_err());
        }
    }

    #[test]
    fn final_check_rejects_parent_alias_retargeting() {
        let fixture = Fixture::new();
        for dir in ["a", "b"] {
            fs::create_dir_all(fixture.0.join(dir).join("child")).unwrap();
            fs::write(fixture.0.join(dir).join("child/djinn.toml"), "").unwrap();
        }
        let alias = fixture.0.join("alias");
        symlink(fixture.0.join("a"), &alias).unwrap();
        assert!(read_with_hook(&alias.join("child"), || {
            fs::remove_file(&alias).unwrap();
            symlink(fixture.0.join("b"), &alias).unwrap();
        })
        .is_err());
    }

    #[test]
    fn filesystem_byte_caps_are_enforced() {
        for (name, cap) in [
            ("djinn.toml", MANIFEST_LIMIT),
            ("events.jsonl", EVENTS_LIMIT),
        ] {
            let fixture = Fixture::new();
            fs::write(fixture.0.join(name), vec![b' '; cap + 1]).unwrap();
            unavailable(fixture.report());
        }
    }

    #[test]
    fn read_only_and_unrelated_runtime_config_and_payloads_are_ignored() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.0.join(".djinn/runtime")).unwrap();
        fs::write(fixture.0.join(".djinn/runtime/stale"), "running pid=1").unwrap();
        fs::write(fixture.0.join("config.toml"), "[invalid TOML").unwrap();
        fs::write(
            fixture.0.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"SECRET\"}\n",
        )
        .unwrap();
        // A runtime FIFO would hang if this observer attempted runtime hydration.
        let runtime = CString::new(
            fixture
                .0
                .join(".djinn/runtime/job.json")
                .as_os_str()
                .as_encoded_bytes(),
        )
        .unwrap();
        // SAFETY: valid test-only path.
        assert_eq!(unsafe { libc::mkfifo(runtime.as_ptr(), 0o600) }, 0);
        fn tree(path: &Path) -> Vec<(PathBuf, Stamp, u64, Option<Vec<u8>>)> {
            let mut entries = Vec::new();
            let mut children = fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                let meta = fs::symlink_metadata(&child).unwrap();
                let bytes = meta.is_file().then(|| fs::read(&child).unwrap());
                if meta.is_dir() {
                    entries.extend(tree(&child));
                }
                entries.push((child, Stamp::of(&meta), meta.nlink(), bytes));
            }
            entries
        }
        let before = tree(&fixture.0);
        let report = fixture.report();
        assert_eq!(report["assessment"], "observed");
        assert_eq!(report["native_lifecycle"], "unestablished");
        assert!(!report.to_string().contains("SECRET"));
        assert!(!report.to_string().contains("pid=1"));
        assert_eq!(before, tree(&fixture.0));
    }
}
