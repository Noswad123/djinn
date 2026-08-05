use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use djinn_memory::{
    AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionId, AgentSessionStore,
    JsonlAgentSessionStore,
};

use crate::session_init::session_context_readme;
use crate::{
    ensure_trailing_newline, folder_agent_session_store, folder_session_manifest_meta,
    write_agent_session_toml, FolderSessionManifest,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentSessionDirProjection {
    pub(crate) session_dir: PathBuf,
    pub(crate) turn_dir: Option<PathBuf>,
    pub(crate) context_dir: PathBuf,
    pub(crate) summary_path: PathBuf,
    pub(crate) request_path: PathBuf,
}

pub(crate) fn ensure_folder_session_readme(session_dir: &Path) -> Result<()> {
    let context_dir = session_dir.join("context");
    let readme_path = context_dir.join("djinn-context.md");
    if readme_path.exists() {
        return Ok(());
    }
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    fs::write(&readme_path, session_context_readme(None, Path::new("")))
        .with_context(|| format!("writing {}", readme_path.display()))
}

pub(crate) fn project_agent_session_dir(
    session_dir: &Path,
    session: &AgentSession,
    _prompt: &str,
    summary: &str,
) -> Result<AgentSessionDirProjection> {
    fs::create_dir_all(session_dir)
        .with_context(|| format!("creating session directory {}", session_dir.display()))?;
    let context_dir = session_dir.join("context");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;

    let summary_path = session_dir.join("summary.md");

    let request_path = session_dir.join("request.md");
    fs::write(&request_path, "").with_context(|| format!("writing {}", request_path.display()))?;
    fs::write(&summary_path, ensure_trailing_newline(summary))
        .with_context(|| format!("writing {}", summary_path.display()))?;

    write_agent_session_toml(session_dir, session)?;
    write_folder_session_events_jsonl(session_dir, session)?;

    Ok(AgentSessionDirProjection {
        session_dir: session_dir.to_path_buf(),
        turn_dir: None,
        context_dir,
        summary_path,
        request_path,
    })
}

pub(crate) fn write_folder_session_events_jsonl(
    session_dir: &Path,
    session: &AgentSession,
) -> Result<PathBuf> {
    let path = session_dir.join("events.jsonl");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut existing_event_ids = HashSet::new();
    let mut existing_lines = HashSet::new();
    for line in existing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        existing_lines.insert(line.to_string());
        if let Ok(event) = serde_json::from_str::<AgentSessionEvent>(line) {
            if !event.event_id.trim().is_empty() {
                existing_event_ids.insert(event.event_id);
            }
        }
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    for event in &session.events {
        let line = serde_json::to_string(event)?;
        let event_id = event.event_id.trim();
        if !event_id.is_empty() {
            if existing_event_ids.contains(event_id) {
                continue;
            }
            existing_event_ids.insert(event_id.to_string());
        } else if existing_lines.contains(&line) {
            continue;
        }
        writeln!(file, "{line}").with_context(|| format!("appending {}", path.display()))?;
        existing_lines.insert(line);
    }
    Ok(path)
}

pub(crate) fn hydrate_folder_agent_session_from_events_jsonl(
    session_dir: &Path,
    id: &AgentSessionId,
    manifest: Option<&FolderSessionManifest>,
) -> Result<bool> {
    let Some(session) = read_folder_session_from_events_jsonl(session_dir, id, manifest)? else {
        return Ok(false);
    };
    write_agent_session_native_jsonl(session_dir, &session)?;
    Ok(true)
}

pub(crate) fn sync_folder_session_events_jsonl_from_store(
    session_dir: Option<&Path>,
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
) -> Result<()> {
    let Some(session_dir) = session_dir else {
        return Ok(());
    };
    let session = store.load_session(id)?;
    write_folder_session_events_jsonl(session_dir, &session)?;
    Ok(())
}

fn read_folder_session_from_events_jsonl(
    session_dir: &Path,
    id: &AgentSessionId,
    manifest: Option<&FolderSessionManifest>,
) -> Result<Option<AgentSession>> {
    let path = session_dir.join("events.jsonl");
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        bail!("events.jsonl exists but is not a file: {}", path.display());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut meta = folder_session_manifest_meta(session_dir, manifest);
    let mut events = Vec::new();
    for (idx, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut event = serde_json::from_str::<AgentSessionEvent>(trimmed)
            .with_context(|| format!("parsing {} line {}", path.display(), idx + 1))?;
        if event.event_id.trim().is_empty() {
            event.event_id = format!("events-jsonl-{}-{}", id.as_str(), idx + 1);
        }
        if event.session_id.as_str().trim().is_empty() {
            event.session_id = id.clone();
        }
        if event.session_id != *id {
            bail!(
                "events.jsonl session id mismatch at line {}: manifest/session is {}, event is {}",
                idx + 1,
                id,
                event.session_id
            );
        }
        match event.kind.clone() {
            AgentSessionEventKind::SessionCreated {
                id: created_id,
                meta: created_meta,
            } => {
                if created_id != *id {
                    bail!(
                        "events.jsonl session_created id mismatch at line {}: manifest/session is {}, event is {}",
                        idx + 1,
                        id,
                        created_id
                    );
                }
                meta = created_meta;
            }
            _ => events.push(event),
        }
    }
    Ok(Some(AgentSession {
        id: id.clone(),
        meta,
        events,
    }))
}

pub(crate) fn write_agent_session_native_jsonl(
    session_dir: &Path,
    session: &AgentSession,
) -> Result<PathBuf> {
    let path = folder_agent_session_store(session_dir).session_file_path(&session.id);
    djinn_core::ensure_parent(&path)?;
    let mut output = String::new();
    output.push_str(&serde_json::to_string(&AgentSessionEvent::with_session(
        session.id.clone(),
        AgentSessionEventKind::SessionCreated {
            id: session.id.clone(),
            meta: session.meta.clone(),
        },
    ))?);
    output.push('\n');
    for event in &session.events {
        output.push_str(&serde_json::to_string(event)?);
        output.push('\n');
    }
    fs::write(&path, output).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}
