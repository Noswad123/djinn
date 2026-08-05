use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    compact_text_snippet, read_folder_session_event_turns, read_folder_session_turns,
    resolve_existing_folder_session_dir, FolderSessionTurnDigest, SessionCompactArgs,
    FOLDER_SESSION_COMPACT_END_MARKER, FOLDER_SESSION_COMPACT_SNIPPET_CHARS,
    FOLDER_SESSION_COMPACT_START_MARKER,
};

pub(crate) fn session_compact(args: SessionCompactArgs) -> Result<()> {
    let report = compact_folder_session(&args.session_dir, args.output.as_deref())?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Compacted {} turns", report.turn_count);
        println!("Output: {}", report.output_path);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionCompactReport {
    pub(crate) session_dir: String,
    pub(crate) output_path: String,
    pub(crate) turn_count: usize,
    pub(crate) turns: Vec<CompactedTurnReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct CompactedTurnReport {
    pub(crate) id: String,
    pub(crate) request_path: Option<String>,
    pub(crate) response_path: Option<String>,
}

pub(crate) fn compact_folder_session(
    session_dir: &Path,
    output: Option<&Path>,
) -> Result<SessionCompactReport> {
    let session_dir = resolve_existing_folder_session_dir(session_dir)?;
    let turns_dir = session_dir.join("turns");
    let context_dir = session_dir.join("context");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    let output_path = output
        .map(PathBuf::from)
        .unwrap_or_else(|| context_dir.join("compacted.md"));
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("creating compaction output directory {}", parent.display())
        })?;
    }

    let mut turns = read_folder_session_turns(&turns_dir)?;
    if turns.is_empty() {
        turns = read_folder_session_event_turns(&session_dir)?;
    }
    let generated = render_folder_session_compaction_generated(&session_dir, &turns);
    let existing = fs::read_to_string(&output_path).ok();
    let content = merge_folder_session_compaction_document(existing.as_deref(), &generated);
    fs::write(&output_path, content)
        .with_context(|| format!("writing {}", output_path.display()))?;

    Ok(SessionCompactReport {
        session_dir: session_dir.display().to_string(),
        output_path: output_path.display().to_string(),
        turn_count: turns.len(),
        turns: turns
            .into_iter()
            .map(|turn| CompactedTurnReport {
                id: turn.id,
                request_path: turn.request_path.map(|path| path.display().to_string()),
                response_path: turn.response_path.map(|path| path.display().to_string()),
            })
            .collect(),
    })
}

fn compaction_evidence_link(session_dir: &Path, path: &Path, label: &str) -> String {
    let target = path
        .strip_prefix(session_dir)
        .map(|relative| format!("../{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string());
    format!("[{label}]({target})")
}

fn render_folder_session_compaction_generated(
    session_dir: &Path,
    turns: &[FolderSessionTurnDigest],
) -> String {
    let mut output = String::new();
    output.push_str(&format!("Session: `{}`\n", session_dir.display()));
    output.push_str(&format!(
        "Generated: `{}`\n\n",
        chrono::Local::now().to_rfc3339()
    ));
    if turns.is_empty() {
        output.push_str("No turn history found in `events.jsonl` or projected `turns/`.\n");
        return output;
    }
    output.push_str("## Turn digest\n\n");
    for turn in turns {
        output.push_str(&format!("### {}\n\n", turn.id));
        if let Some(request) = &turn.request {
            output.push_str("**Request**\n\n");
            output.push_str(&markdown_quote_block(&compact_text_snippet(
                request,
                FOLDER_SESSION_COMPACT_SNIPPET_CHARS,
            )));
            output.push_str("\n\n");
        }
        if let Some(response) = &turn.response {
            output.push_str("**Response**\n\n");
            output.push_str(&markdown_quote_block(&compact_text_snippet(
                response,
                FOLDER_SESSION_COMPACT_SNIPPET_CHARS,
            )));
            output.push_str("\n\n");
        }
        let mut links = Vec::new();
        if let Some(path) = &turn.request_path {
            links.push(compaction_evidence_link(session_dir, path, "request"));
        }
        if let Some(path) = &turn.response_path {
            let link = compaction_evidence_link(session_dir, path, "response");
            if !links.iter().any(|existing| existing == &link) {
                links.push(link);
            }
        }
        if !links.is_empty() {
            output.push_str(&format!("Evidence: {}\n\n", links.join(", ")));
        }
    }
    output
}

fn merge_folder_session_compaction_document(existing: Option<&str>, generated: &str) -> String {
    let generated_block = format!(
        "{FOLDER_SESSION_COMPACT_START_MARKER}\n{}\n{FOLDER_SESSION_COMPACT_END_MARKER}",
        generated.trim_end()
    );
    let Some(existing) = existing else {
        return initial_folder_session_compaction_document(&generated_block);
    };
    if let Some(start) = existing.find(FOLDER_SESSION_COMPACT_START_MARKER) {
        if let Some(relative_end) = existing[start..].find(FOLDER_SESSION_COMPACT_END_MARKER) {
            let end = start + relative_end + FOLDER_SESSION_COMPACT_END_MARKER.len();
            let mut output = String::new();
            output.push_str(existing[..start].trim_end());
            output.push_str("\n");
            output.push_str(&generated_block);
            let suffix = existing[end..].trim_start_matches(|ch| ch == '\r' || ch == '\n');
            if !suffix.trim().is_empty() {
                output.push_str("\n\n");
                output.push_str(suffix.trim_end());
            }
            output.push('\n');
            return output;
        }
    }

    let mut output = existing.trim_end().to_string();
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str("## Generated digest\n\n");
    output.push_str(&generated_block);
    output.push('\n');
    output
}

fn initial_folder_session_compaction_document(generated_block: &str) -> String {
    format!(
        "# Compacted session context\n\n## User notes\n\nAdd durable facts, decisions, open questions, and edited summaries here. Djinn preserves this section when regenerating the digest.\n\n## Generated digest\n\n{generated_block}\n"
    )
}

fn markdown_quote_block(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_projection::project_agent_session_dir;
    use djinn_memory::{
        AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionId, AgentSessionMeta,
    };

    #[test]
    fn session_compact_writes_deterministic_turn_digest_with_evidence_links() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-compact-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let turn = dir.join("turns/20260727T120000-1");
        fs::create_dir_all(&turn).unwrap();
        fs::write(turn.join("request.md"), "Decide storage shape\n\nDetails").unwrap();
        fs::write(
            turn.join("response.md"),
            "Use context for durable notes and turns for evidence.\n",
        )
        .unwrap();

        let report = compact_folder_session(&dir, None).unwrap();
        let compacted = fs::read_to_string(dir.join("context/compacted.md")).unwrap();

        assert_eq!(report.turn_count, 1);
        assert_eq!(report.turns[0].id, "20260727T120000-1");
        assert!(compacted.contains("# Compacted session context"));
        assert!(compacted.contains("## User notes"));
        assert!(compacted.contains(FOLDER_SESSION_COMPACT_START_MARKER));
        assert!(compacted.contains(FOLDER_SESSION_COMPACT_END_MARKER));
        assert!(compacted.contains("### 20260727T120000-1"));
        assert!(compacted.contains("> Decide storage shape"));
        assert!(compacted.contains("> Use context for durable notes"));
        assert!(compacted.contains("[request](../turns/20260727T120000-1/request.md)"));
        assert!(compacted.contains("[response](../turns/20260727T120000-1/response.md)"));
        assert!(!dir.join("logs/transcript.md").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_compact_reads_event_turns_when_turn_projection_is_absent() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-compact-events-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let session = AgentSession {
            id: AgentSessionId::new("agt_compact_events"),
            meta: AgentSessionMeta {
                title: "Compact events".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "Use events for history".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "Keep turns as projection only".to_string(),
                }),
            ],
        };
        project_agent_session_dir(
            &dir,
            &session,
            "Use events for history",
            "Keep turns as projection only",
        )
        .unwrap();

        let report = compact_folder_session(&dir, None).unwrap();
        let compacted = fs::read_to_string(dir.join("context/compacted.md")).unwrap();

        assert_eq!(report.turn_count, 1);
        assert_eq!(report.turns[0].id, "event-turn-0001");
        assert!(compacted.contains("### event-turn-0001"));
        assert!(compacted.contains("> Use events for history"));
        assert!(compacted.contains("> Keep turns as projection only"));
        assert!(compacted.contains("[request](../events.jsonl)"));
        assert!(!dir.join("turns").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_compact_replaces_generated_block_and_preserves_user_notes() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-compact-preserve-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let turn = dir.join("turns/20260727T120000-1");
        let context = dir.join("context");
        fs::create_dir_all(&turn).unwrap();
        fs::create_dir_all(&context).unwrap();
        fs::write(turn.join("request.md"), "Initial request\n").unwrap();
        fs::write(turn.join("response.md"), "Fresh response\n").unwrap();
        fs::write(
            context.join("compacted.md"),
            format!(
                "# Compacted session context\n\n## User notes\n\nKeep this decision.\n\n## Generated digest\n\n{FOLDER_SESSION_COMPACT_START_MARKER}\nOld generated response\n{FOLDER_SESSION_COMPACT_END_MARKER}\n\n## User appendix\n\nKeep appendix.\n"
            ),
        )
        .unwrap();

        compact_folder_session(&dir, None).unwrap();
        let compacted = fs::read_to_string(context.join("compacted.md")).unwrap();

        assert!(compacted.contains("Keep this decision."));
        assert!(compacted.contains("Keep appendix."));
        assert!(compacted.contains("> Fresh response"));
        assert!(!compacted.contains("Old generated response"));
        assert_eq!(
            compacted
                .matches(FOLDER_SESSION_COMPACT_START_MARKER)
                .count(),
            1
        );
        assert_eq!(
            compacted.matches(FOLDER_SESSION_COMPACT_END_MARKER).count(),
            1
        );

        let _ = fs::remove_dir_all(&root);
    }
}
