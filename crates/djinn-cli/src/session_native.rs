use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use djinn_memory::{AgentSession, AgentSessionId, AgentSessionStore, JsonlAgentSessionStore};

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
        .or_else(|| crate::agent_session_store().load_session(id).ok())
}

pub(crate) fn agent_session_store_for_folder_session(
    session_dir: &Path,
    id: &AgentSessionId,
) -> JsonlAgentSessionStore {
    let folder_store = folder_agent_session_store(session_dir);
    if folder_store.load_session(id).is_ok() {
        folder_store
    } else {
        crate::agent_session_store()
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
