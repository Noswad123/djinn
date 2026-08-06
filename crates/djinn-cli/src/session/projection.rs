use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use djinn_memory::{
    AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionId, AgentSessionStore,
    JsonlAgentSessionStore,
};

use crate::session::init::session_context_readme;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::messages::agent_model_messages;
    use crate::session::manifest::read_folder_session_manifest;
    use djinn_memory::{AgentSessionId, AgentSessionMeta};

    #[test]
    fn folder_backed_session_projection_writes_events_and_context_without_duplicate_logs() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-folder-session-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("summary.md"), "old summary\n").unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("agt_folder"),
            meta: AgentSessionMeta {
                title: "Folder session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "new request".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "new summary".to_string(),
                }),
            ],
        };

        let projection =
            project_agent_session_dir(&dir, &session, "new request", "new summary").unwrap();

        assert_eq!(fs::read_to_string(dir.join("request.md")).unwrap(), "");
        assert_eq!(
            fs::read_to_string(dir.join("summary.md")).unwrap(),
            "new summary\n"
        );
        assert!(projection.context_dir.exists());
        assert!(projection.turn_dir.is_none());
        assert!(!dir.join("turns").exists());
        assert!(dir.join("djinn.toml").exists());
        let events_jsonl = fs::read_to_string(dir.join("events.jsonl")).unwrap();
        assert_eq!(events_jsonl.lines().count(), 2);
        assert!(events_jsonl.contains("\"type\":\"user_message\""));
        assert!(events_jsonl.contains("\"type\":\"assistant_message\""));
        write_folder_session_events_jsonl(&dir, &session).unwrap();
        let events_jsonl_after_second_shadow =
            fs::read_to_string(dir.join("events.jsonl")).unwrap();
        assert_eq!(events_jsonl_after_second_shadow.lines().count(), 2);
        assert!(!dir.join("logs/summary-history.md").exists());
        assert!(!dir.join("logs/events.jsonl").exists());
        assert!(!dir.join("logs/transcript.md").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_session_events_jsonl_hydrates_native_history_for_continuation() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-events-first-session-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let id = AgentSessionId::new("agt_events_first");
        fs::write(
            dir.join("djinn.toml"),
            "session_id = \"agt_events_first\"\ntitle = \"Events First\"\nworkspace = \"/tmp/workspace\"\nprofile = \"default\"\n",
        )
        .unwrap();
        let events = vec![
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "event request".to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::AssistantMessage {
                    content: "event response".to_string(),
                },
            ),
        ];
        let events_jsonl = events
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n")
            + "\n";
        fs::write(dir.join("events.jsonl"), events_jsonl).unwrap();

        let stale = AgentSession {
            id: id.clone(),
            meta: AgentSessionMeta {
                title: "stale".to_string(),
                source: "djinn".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "stale native request".to_string(),
                },
            )],
        };
        write_agent_session_native_jsonl(&dir, &stale).unwrap();

        let manifest = read_folder_session_manifest(&dir).unwrap();
        assert!(
            hydrate_folder_agent_session_from_events_jsonl(&dir, &id, manifest.as_ref()).unwrap()
        );
        let loaded = folder_agent_session_store(&dir).load_session(&id).unwrap();
        let messages = agent_model_messages(&loaded, "/tmp/workspace", &[]);

        assert_eq!(loaded.events.len(), 2);
        assert_eq!(loaded.meta.workspace, "/tmp/workspace");
        assert!(messages
            .iter()
            .any(|message| message.content == "event request"));
        assert!(messages
            .iter()
            .any(|message| message.content == "event response"));
        assert!(!messages
            .iter()
            .any(|message| message.content.contains("stale native")));

        let _ = fs::remove_dir_all(&dir);
    }
}
