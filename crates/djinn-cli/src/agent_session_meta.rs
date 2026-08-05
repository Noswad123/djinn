use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use djinn_memory::{
    AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionExecutionMode,
    AgentSessionId, AgentSessionLifecycleState, AgentSessionStore, JsonlAgentSessionStore,
};

use crate::prompt::prompt_title;
use crate::session_projection::AgentSessionDirProjection;

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
