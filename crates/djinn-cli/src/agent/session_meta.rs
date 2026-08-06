use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use djinn_memory::{
    AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionExecutionMode,
    AgentSessionId, AgentSessionLifecycleState, AgentSessionStore, JsonlAgentSessionStore,
};

use crate::session::projection::AgentSessionDirProjection;
use crate::util::prompt::prompt_title;

const AGENT_CHILD_SESSION_MAX_DEPTH: usize = 3;

pub(crate) fn append_agent_session_lifecycle_event(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    state: AgentSessionLifecycleState,
    mode: AgentSessionExecutionMode,
    reason: impl Into<String>,
    note: Option<String>,
) -> Result<()> {
    store.append_event(
        id,
        AgentSessionEvent::new(AgentSessionEventKind::SessionLifecycleUpdated {
            state,
            mode: Some(mode),
            reason: Some(reason.into()),
            note,
        }),
    )
}

pub(crate) fn format_session_run_completion(
    id: &AgentSessionId,
    projection: Option<&AgentSessionDirProjection>,
    session_dir: Option<&Path>,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Completed Djinn session run: {id}"));
    if let Some(projection) = projection {
        lines.push(format!("  session: {}", projection.session_dir.display()));
        lines.push(format!("  summary: {}", projection.summary_path.display()));
        if let Some(turn_dir) = &projection.turn_dir {
            lines.push(format!(
                "  response: {}",
                turn_dir.join("response.md").display()
            ));
        } else {
            lines.push("  response: summary.md (turns/ projection not written)".to_string());
        }
        lines.push(format!("  request: {}", projection.request_path.display()));
    } else if let Some(session_dir) = session_dir {
        lines.push(format!("  session: {}", session_dir.display()));
    }
    lines.push(String::new());
    lines.join("\n")
}

pub(crate) fn validate_agent_child_session_depth(
    store: &JsonlAgentSessionStore,
    parent_session_id: Option<&AgentSessionId>,
) -> Result<()> {
    let Some(parent_session_id) = parent_session_id else {
        return Ok(());
    };

    let parent_depth = agent_session_depth(store, parent_session_id)?;
    if parent_depth >= AGENT_CHILD_SESSION_MAX_DEPTH {
        bail!(
            "child session depth limit exceeded: parent session {parent_session_id} is at depth \
             {parent_depth}; maximum child-session depth is {AGENT_CHILD_SESSION_MAX_DEPTH} \
             levels below the root"
        );
    }

    Ok(())
}

fn agent_session_depth(
    store: &JsonlAgentSessionStore,
    session_id: &AgentSessionId,
) -> Result<usize> {
    let mut depth = 0;
    let mut current = session_id.clone();
    let mut seen = HashSet::new();

    loop {
        if !seen.insert(current.clone()) {
            bail!("cycle detected in agent session parent chain at {current}");
        }

        let session = store
            .load_session(&current)
            .with_context(|| format!("loading parent agent session {current}"))?;
        let Some(parent) = session.meta.parent_session_id else {
            return Ok(depth);
        };

        depth += 1;
        current = parent;
    }
}

pub(crate) fn maybe_auto_title_agent_session(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    prompt: &str,
) -> Result<()> {
    let session = store.load_session(id)?;
    if !should_auto_title_agent_session(&session) {
        return Ok(());
    }
    let title = infer_agent_session_title(prompt);
    if title.trim().is_empty() || title == session.meta.title {
        return Ok(());
    }
    store.append_event(
        id,
        AgentSessionEvent::new(AgentSessionEventKind::SessionTitleUpdated { title }),
    )
}

fn should_auto_title_agent_session(session: &AgentSession) -> bool {
    let title = session.meta.title.trim();
    let default_title =
        title.is_empty() || title == "Agent chat" || title == "Untitled agent session";
    default_title
        && session
            .events
            .iter()
            .filter(|event| matches!(event.kind, AgentSessionEventKind::UserMessage { .. }))
            .count()
            == 1
}

fn infer_agent_session_title(prompt: &str) -> String {
    let title = prompt_title(prompt, "Djinn session")
        .trim_matches(|ch: char| ch.is_ascii_punctuation() || ch.is_whitespace())
        .trim()
        .to_string();
    if title.is_empty() {
        "Djinn session".to_string()
    } else {
        title
    }
}

pub(crate) fn latest_session_model(session: &AgentSession) -> Option<String> {
    for event in session.events.iter().rev() {
        match &event.kind {
            AgentSessionEventKind::SessionModelUpdated { model } => {
                let model = model.trim();
                if !model.is_empty() {
                    return Some(model.to_string());
                }
            }
            AgentSessionEventKind::SessionProfileUpdated { .. } => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use djinn_memory::AgentSessionMeta;

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
    fn format_session_run_completion_reports_output_paths() {
        let session_dir = PathBuf::from("/tmp/djinn/session");
        let projection = AgentSessionDirProjection {
            session_dir: session_dir.clone(),
            turn_dir: Some(session_dir.join("turns/20260728T120000-1")),
            context_dir: session_dir.join("context"),
            summary_path: session_dir.join("summary.md"),
            request_path: session_dir.join("request.md"),
        };

        let rendered = format_session_run_completion(
            &AgentSessionId::new("agt_run_test"),
            Some(&projection),
            Some(&session_dir),
        );

        assert!(rendered.contains("Completed Djinn session run: agt_run_test"));
        assert!(rendered.contains("summary.md"));
        assert!(rendered.contains("turns/20260728T120000-1/response.md"));
        assert!(rendered.contains("request.md"));
    }

    #[test]
    fn latest_session_model_uses_latest_model_until_profile_changes() {
        let mut session = AgentSession {
            id: AgentSessionId::new("agt_model"),
            meta: AgentSessionMeta::default(),
            events: vec![AgentSessionEvent::new(
                AgentSessionEventKind::SessionModelUpdated {
                    model: "openai/gpt-5.5".to_string(),
                },
            )],
        };

        assert_eq!(
            latest_session_model(&session).as_deref(),
            Some("openai/gpt-5.5")
        );

        session.events.push(AgentSessionEvent::new(
            AgentSessionEventKind::SessionProfileUpdated {
                profile: "architect".to_string(),
            },
        ));

        assert_eq!(latest_session_model(&session), None);

        session.events.push(AgentSessionEvent::new(
            AgentSessionEventKind::SessionModelUpdated {
                model: "openai/gpt-5.4-mini".to_string(),
            },
        ));

        assert_eq!(
            latest_session_model(&session).as_deref(),
            Some("openai/gpt-5.4-mini")
        );
    }

    #[test]
    fn child_session_depth_limit_allows_three_levels_below_root() {
        let store = temp_agent_store("child-depth-allow");
        let root = store
            .create_session(AgentSessionMeta {
                title: "root".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let child = store
            .create_session(AgentSessionMeta {
                title: "child".to_string(),
                parent_session_id: Some(root),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let grandchild = store
            .create_session(AgentSessionMeta {
                title: "grandchild".to_string(),
                parent_session_id: Some(child),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        validate_agent_child_session_depth(&store, Some(&grandchild)).unwrap();
    }

    #[test]
    fn child_session_depth_limit_rejects_fourth_level_below_root() {
        let store = temp_agent_store("child-depth-reject");
        let root = store
            .create_session(AgentSessionMeta {
                title: "root".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let child = store
            .create_session(AgentSessionMeta {
                title: "child".to_string(),
                parent_session_id: Some(root),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let grandchild = store
            .create_session(AgentSessionMeta {
                title: "grandchild".to_string(),
                parent_session_id: Some(child),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let great_grandchild = store
            .create_session(AgentSessionMeta {
                title: "great grandchild".to_string(),
                parent_session_id: Some(grandchild),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        let err = validate_agent_child_session_depth(&store, Some(&great_grandchild)).unwrap_err();

        assert!(err
            .to_string()
            .contains("child session depth limit exceeded"));
        assert!(err.to_string().contains("maximum child-session depth is 3"));
    }

    #[test]
    fn maybe_auto_title_agent_session_titles_first_default_session_prompt() {
        let store = temp_agent_store("auto-title");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Agent chat".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "Implement session auto title\nwith extra details".to_string(),
                }),
            )
            .unwrap();

        maybe_auto_title_agent_session(
            &store,
            &id,
            "Implement session auto title\nwith extra details",
        )
        .unwrap();

        let loaded = store.load_session(&id).unwrap();
        assert_eq!(loaded.meta.title, "Implement session auto title");
        assert!(loaded.events.iter().any(|event| matches!(
            &event.kind,
            AgentSessionEventKind::SessionTitleUpdated { title } if title == "Implement session auto title"
        )));
    }

    #[test]
    fn maybe_auto_title_agent_session_preserves_explicit_title() {
        let store = temp_agent_store("auto-title-explicit");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Explicit title".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "Different first prompt".to_string(),
                }),
            )
            .unwrap();

        maybe_auto_title_agent_session(&store, &id, "Different first prompt").unwrap();

        let loaded = store.load_session(&id).unwrap();
        assert_eq!(loaded.meta.title, "Explicit title");
    }
}
