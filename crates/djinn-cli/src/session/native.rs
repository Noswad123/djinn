use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use djinn_memory::{AgentSession, AgentSessionId, AgentSessionStore, JsonlAgentSessionStore};

use crate::storage::stores::agent_session_store;

const FOLDER_NATIVE_SESSION_DIR: &str = ".djinn";

pub(crate) fn folder_agent_session_store(session_dir: &Path) -> JsonlAgentSessionStore {
    JsonlAgentSessionStore::new(session_dir.join(FOLDER_NATIVE_SESSION_DIR))
}

pub(crate) fn load_folder_native_agent_session(
    session_dir: &Path,
    id: &AgentSessionId,
) -> Option<AgentSession> {
    folder_agent_session_store(session_dir)
        .load_session(id)
        .ok()
        .or_else(|| agent_session_store().load_session(id).ok())
}

pub(crate) fn agent_session_store_for_folder_session(
    session_dir: &Path,
    id: &AgentSessionId,
) -> JsonlAgentSessionStore {
    let folder_store = folder_agent_session_store(session_dir);
    if folder_store.load_session(id).is_ok() {
        folder_store
    } else {
        agent_session_store()
    }
}

pub(crate) fn relocate_agent_session_into_folder(
    source_store: &JsonlAgentSessionStore,
    session_dir: &Path,
    id: &AgentSessionId,
) -> Result<JsonlAgentSessionStore> {
    let folder_store = folder_agent_session_store(session_dir);
    let target_path = folder_store.session_file_path(id);
    if target_path.exists() {
        return Ok(folder_store);
    }

    let source_path = source_store.session_file_path(id);
    if !source_path.exists() {
        source_store
            .load_session(id)
            .with_context(|| format!("loading agent session {id} before moving into folder"))?;
        bail!(
            "agent session {id} exists but its JSONL path is missing: {}",
            source_path.display()
        );
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating native session directory {}", parent.display()))?;
    }
    fs::rename(&source_path, &target_path).or_else(|rename_error| {
        fs::copy(&source_path, &target_path).with_context(|| {
            format!(
                "copying agent session {} to {} after rename failed: {rename_error}",
                source_path.display(),
                target_path.display()
            )
        })?;
        fs::remove_file(&source_path).with_context(|| {
            format!(
                "removing original agent session {} after copying to {}",
                source_path.display(),
                target_path.display()
            )
        })?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(folder_store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use djinn_memory::{AgentSessionMeta, AgentSessionStore};

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
    fn relocates_native_jsonl_into_folder_session() {
        let store = temp_agent_store("folder-native-relocate");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Move me".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let source_path = store.session_file_path(&id);
        let root = std::env::temp_dir().join(format!(
            "djinn-folder-native-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");

        let folder_store = relocate_agent_session_into_folder(&store, &session_dir, &id).unwrap();
        let target_path = folder_store.session_file_path(&id);

        assert!(!source_path.exists());
        assert_eq!(
            target_path,
            session_dir.join(".djinn").join(format!("{id}.jsonl"))
        );
        assert!(target_path.exists());
        assert_eq!(
            folder_store.load_session(&id).unwrap().meta.title,
            "Move me"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
