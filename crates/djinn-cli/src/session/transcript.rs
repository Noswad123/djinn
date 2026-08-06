use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::Serialize;

use crate::session::events::read_event_turn_pairs;
use crate::session::reference::resolve_existing_folder_session_dir;
use crate::util::editor::open_editor_path;
use crate::util::shell::shell_quote;
use crate::util::text::ensure_trailing_newline;
use crate::SessionTranscriptArgs;

pub(crate) fn session_transcript(args: SessionTranscriptArgs) -> Result<()> {
    run_session_transcript(SessionTranscriptOptions {
        dir: args.dir,
        format: args.format,
        json: args.json,
        output: args.output,
        open: args.open,
        editor: args.editor,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionTranscriptFormat {
    Markdown,
    Json,
}

pub(crate) struct SessionTranscriptOptions {
    pub(crate) dir: PathBuf,
    pub(crate) format: SessionTranscriptFormat,
    pub(crate) json: bool,
    pub(crate) output: Option<PathBuf>,
    pub(crate) open: bool,
    pub(crate) editor: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionTranscriptReport {
    pub(crate) session_dir: String,
    pub(crate) events_path: String,
    pub(crate) format: SessionTranscriptFormat,
    pub(crate) turn_count: usize,
    pub(crate) output_path: Option<String>,
    pub(crate) turns: Vec<SessionTranscriptTurnReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionTranscriptTurnReport {
    pub(crate) index: usize,
    pub(crate) user: String,
    pub(crate) assistant: String,
    pub(crate) request_line: usize,
    pub(crate) response_line: usize,
}

pub(crate) fn run_session_transcript(options: SessionTranscriptOptions) -> Result<()> {
    let format = if options.json {
        SessionTranscriptFormat::Json
    } else {
        options.format
    };
    if options.open && format != SessionTranscriptFormat::Markdown {
        bail!("--open is only supported for Markdown transcripts");
    }

    let mut report = build_session_transcript(&options.dir, format)?;
    if options.open {
        let session_dir = PathBuf::from(&report.session_dir);
        let output_path = options
            .output
            .clone()
            .unwrap_or_else(|| session_dir.join("transcript.md"));
        write_text_output(&output_path, &render_session_transcript_markdown(&report))?;
        report.output_path = Some(output_path.display().to_string());
        return open_editor_path(&output_path, options.editor);
    }

    if let Some(output_path) = options.output {
        match format {
            SessionTranscriptFormat::Markdown => {
                write_text_output(&output_path, &render_session_transcript_markdown(&report))?;
            }
            SessionTranscriptFormat::Json => {
                report.output_path = Some(output_path.display().to_string());
                write_text_output(
                    &output_path,
                    &(serde_json::to_string_pretty(&report)? + "\n"),
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
        }
        report.output_path = Some(output_path.display().to_string());
        println!("Wrote transcript: {}", output_path.display());
        return Ok(());
    }

    match format {
        SessionTranscriptFormat::Markdown => {
            print!("{}", render_session_transcript_markdown(&report))
        }
        SessionTranscriptFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
    }
    Ok(())
}

pub(crate) fn build_session_transcript(
    dir: &Path,
    format: SessionTranscriptFormat,
) -> Result<SessionTranscriptReport> {
    let session_dir = resolve_existing_folder_session_dir(dir)?;
    let events_path = session_dir.join("events.jsonl");
    if !events_path.is_file() {
        bail!(
            "session has no events.jsonl transcript source: {}",
            events_path.display()
        );
    }
    let raw = fs::read_to_string(&events_path)
        .with_context(|| format!("reading {}", events_path.display()))?;
    let mut issues = Vec::new();
    let pairs = read_event_turn_pairs(&events_path, &raw, &mut issues);
    if !issues.is_empty() {
        bail!(
            "cannot render transcript because events.jsonl has {} issue(s); run `djinn session validate-events {}`",
            issues.len(),
            shell_quote(&session_dir.display().to_string())
        );
    }
    let turns = pairs
        .into_iter()
        .enumerate()
        .map(|(index, pair)| SessionTranscriptTurnReport {
            index: index + 1,
            user: pair.request,
            assistant: pair.response,
            request_line: pair.request_line,
            response_line: pair.response_line,
        })
        .collect::<Vec<_>>();
    Ok(SessionTranscriptReport {
        session_dir: session_dir.display().to_string(),
        events_path: events_path.display().to_string(),
        format,
        turn_count: turns.len(),
        output_path: None,
        turns,
    })
}

pub(crate) fn render_session_transcript_markdown(report: &SessionTranscriptReport) -> String {
    let mut output = String::new();
    output.push_str("# Session Transcript\n\n");
    output.push_str(&format!("Session: `{}`\n\n", report.session_dir));
    output.push_str(&format!("Source: `{}`\n\n", report.events_path));
    output.push_str(&format!("Turns: {}\n\n", report.turn_count));
    for turn in &report.turns {
        output.push_str(&format!("## Turn {}\n\n", turn.index));
        output.push_str(&format!("### User\n\n{}\n\n", turn.user.trim_end()));
        output.push_str(&format!(
            "### Assistant\n\n{}\n\n",
            turn.assistant.trim_end()
        ));
    }
    output
}

fn write_text_output(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, ensure_trailing_newline(content))
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use djinn_memory::{
        AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionId, AgentSessionMeta,
    };

    #[test]
    fn session_transcript_renders_markdown_from_events_jsonl() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-session-transcript-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("transcript-session"),
            meta: AgentSessionMeta {
                title: "Transcript Session".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![
                AgentSessionEvent::with_session(
                    AgentSessionId::new("transcript-session"),
                    AgentSessionEventKind::UserMessage {
                        content: "What is structured programming?".to_string(),
                    },
                ),
                AgentSessionEvent::with_session(
                    AgentSessionId::new("transcript-session"),
                    AgentSessionEventKind::AssistantMessage {
                        content: "It emphasizes clear control flow.".to_string(),
                    },
                ),
            ],
        };
        crate::session::projection::write_folder_session_events_jsonl(&dir, &session).unwrap();

        let report = build_session_transcript(&dir, SessionTranscriptFormat::Markdown).unwrap();
        let rendered = render_session_transcript_markdown(&report);

        assert_eq!(report.turn_count, 1);
        assert_eq!(report.turns[0].request_line, 1);
        assert_eq!(report.turns[0].response_line, 2);
        assert!(rendered.contains("# Session Transcript"));
        assert!(rendered.contains("## Turn 1"));
        assert!(rendered.contains("### User"));
        assert!(rendered.contains("What is structured programming?"));
        assert!(rendered.contains("### Assistant"));
        assert!(rendered.contains("It emphasizes clear control flow."));

        let _ = fs::remove_dir_all(&dir);
    }
}
