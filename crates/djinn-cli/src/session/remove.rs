use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
#[cfg(test)]
use djinn_memory::AgentSessionMeta;
use djinn_memory::{AgentSessionStore, JsonlAgentSessionStore};
use serde::Serialize;

use crate::session::manifest::session_id_from_session_dir;
use crate::session::native::folder_agent_session_store;
use crate::session::reference::{
    default_folder_session_root, is_named_folder_session_reference,
    resolve_existing_folder_session_reference,
};
use crate::storage::stores::agent_session_store;
use crate::SessionRmArgs;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionRmReport {
    pub(crate) session_dir: String,
    pub(crate) removed_folder: bool,
    pub(crate) session_id: Option<String>,
    pub(crate) removed_native_session: bool,
}

pub(crate) fn session_rm(args: SessionRmArgs) -> Result<()> {
    let report = remove_folder_session(&args.dir)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Removed folder session: {}", report.session_dir);
        if let Some(session_id) = &report.session_id {
            println!(
                "Native session {session_id}: {}",
                if report.removed_native_session {
                    "removed"
                } else {
                    "not found"
                }
            );
        }
    }
    Ok(())
}

pub(crate) fn remove_folder_session(dir: &Path) -> Result<SessionRmReport> {
    remove_folder_session_with_store(dir, &agent_session_store())
}

fn remove_folder_session_with_store(
    dir: &Path,
    store: &JsonlAgentSessionStore,
) -> Result<SessionRmReport> {
    let named_reference = is_named_folder_session_reference(dir);
    let session_dir = resolve_existing_folder_session_reference(dir)?.session_dir;
    let manifest_exists = session_dir.join("djinn.toml").exists();
    if !manifest_exists && !named_reference_under_cache_root(&session_dir) && !named_reference {
        bail!(
            "refusing to remove explicit directory without djinn.toml: {}",
            session_dir.display()
        );
    }
    let session_id = session_id_from_session_dir(&session_dir)?;
    let removed_native_session = if let Some(id) = &session_id {
        let folder_store = folder_agent_session_store(&session_dir);
        if folder_store.load_session(id).is_ok() {
            true
        } else if store.load_session(id).is_ok() {
            store.delete_session(id)?;
            true
        } else {
            false
        }
    } else {
        false
    };
    fs::remove_dir_all(&session_dir)
        .with_context(|| format!("removing folder session {}", session_dir.display()))?;
    Ok(SessionRmReport {
        session_dir: session_dir.display().to_string(),
        removed_folder: true,
        session_id: session_id.map(|id| id.to_string()),
        removed_native_session,
    })
}

fn named_reference_under_cache_root(path: &Path) -> bool {
    let root = default_folder_session_root();
    path.parent().is_some_and(|parent| parent == root)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn temp_agent_store(name: &str) -> JsonlAgentSessionStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-cli-agent-chat-{name}-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        JsonlAgentSessionStore::default_in(&dir)
    }

    #[test]
    fn folder_session_rm_removes_folder_and_linked_native_session_without_force() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-rm-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        fs::create_dir_all(&dir).unwrap();
        let store = temp_agent_store("folder-session-rm");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Folder rm".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        fs::write(dir.join("djinn.toml"), format!("session_id = \"{}\"\n", id)).unwrap();

        let report = remove_folder_session_with_store(&dir, &store).unwrap();

        assert!(report.removed_folder);
        assert_eq!(report.session_id, Some(id.to_string()));
        assert!(report.removed_native_session);
        assert!(!dir.exists());
        assert!(store.load_session(&id).is_err());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_rm_rejects_explicit_non_session_directory() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-rm-guard-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let store = temp_agent_store("folder-session-rm-guard");

        let error = remove_folder_session_with_store(&root, &store).unwrap_err();

        assert!(error
            .to_string()
            .contains("refusing to remove explicit directory without djinn.toml"));
        assert!(root.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
