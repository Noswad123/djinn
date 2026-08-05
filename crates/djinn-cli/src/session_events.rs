use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::{env, fs};

use anyhow::{bail, Context, Result};
use djinn_memory::{AgentSessionEvent, AgentSessionEventKind};
use serde::Serialize;

use crate::shell::shell_quote;
use crate::{
    compact_text_snippet, default_folder_session_root, ensure_trailing_newline,
    folder_session_display_name, read_folder_session_turns, read_optional_markdown_file,
    resolve_existing_folder_session_dir, toml_string, yes_no, FolderSessionTurnDigest,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionValidateEventsReport {
    pub(crate) session_dir: String,
    pub(crate) events_path: String,
    pub(crate) events_exists: bool,
    pub(crate) event_count: usize,
    pub(crate) event_turn_count: usize,
    pub(crate) turn_count: usize,
    pub(crate) root_summary_matches_latest_turn: Option<bool>,
    pub(crate) all_valid: bool,
    pub(crate) issues: Vec<SessionValidateEventsIssue>,
    pub(crate) note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionValidateEventsIssue {
    pub(crate) severity: String,
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionEventTurnPair {
    pub(crate) request: String,
    pub(crate) response: String,
    pub(crate) request_line: usize,
    pub(crate) response_line: usize,
}

pub(crate) fn latest_event_rebuild_backup_path(session_dir: &Path) -> Option<PathBuf> {
    let backup_root = session_dir.join(".djinn/backups");
    let mut entries = fs::read_dir(&backup_root)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("events-rebuild-"))
                    .unwrap_or(false)
                && path.join("backup.toml").is_file()
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries.pop()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingSessionUserMessage {
    content: String,
    line: usize,
}

pub(crate) fn validate_folder_session_events(dir: &Path) -> Result<SessionValidateEventsReport> {
    let session_dir = resolve_existing_folder_session_dir(dir)?;
    let events_path = session_dir.join("events.jsonl");
    let turns = read_folder_session_turns(&session_dir.join("turns"))?;
    let mut issues = Vec::new();
    let mut event_turns = Vec::new();
    let mut event_count = 0;
    let mut events_exists = events_path.exists();

    if events_exists && !events_path.is_file() {
        push_session_event_validation_issue(
            &mut issues,
            "invalid_events_path",
            format!(
                "events.jsonl exists but is not a file: {}",
                events_path.display()
            ),
            Some(&events_path),
            None,
        );
        events_exists = false;
    }

    if events_exists {
        let raw = fs::read_to_string(&events_path)
            .with_context(|| format!("reading {}", events_path.display()))?;
        event_count = raw.lines().filter(|line| !line.trim().is_empty()).count();
        event_turns = read_event_turn_pairs(&events_path, &raw, &mut issues);
    } else {
        push_session_event_validation_issue(
            &mut issues,
            "missing_events_jsonl",
            format!(
                "missing folder-local event ledger: {}",
                events_path.display()
            ),
            Some(&events_path),
            None,
        );
    }

    validate_event_turn_pairs_against_turns(&session_dir, &event_turns, &turns, &mut issues);
    let root_summary_matches_latest_turn =
        validate_root_summary_against_latest_turn(&session_dir, &event_turns, &turns, &mut issues)?;

    let all_valid = issues.is_empty();
    let note = if all_valid {
        "events.jsonl, optional turns/, and summary.md agree. events.jsonl is the folder-session history source."
    } else {
        "One or more event/turn agreement issues were found. Treat turns/ as a stale compatibility projection until it is regenerated from events."
    }
    .to_string();

    Ok(SessionValidateEventsReport {
        session_dir: session_dir.display().to_string(),
        events_path: events_path.display().to_string(),
        events_exists,
        event_count,
        event_turn_count: event_turns.len(),
        turn_count: turns.len(),
        root_summary_matches_latest_turn,
        all_valid,
        issues,
        note,
    })
}

pub(crate) fn read_event_turn_pairs(
    events_path: &Path,
    raw: &str,
    issues: &mut Vec<SessionValidateEventsIssue>,
) -> Vec<SessionEventTurnPair> {
    let mut pending_user: Option<PendingSessionUserMessage> = None;
    let mut pairs = Vec::new();
    let mut event_ids: BTreeMap<String, usize> = BTreeMap::new();
    for (index, line) in raw.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event = match serde_json::from_str::<AgentSessionEvent>(trimmed) {
            Ok(event) => event,
            Err(err) => {
                push_session_event_validation_issue(
                    issues,
                    "invalid_event_json",
                    format!("line {line_number} is not a valid Djinn session event: {err}"),
                    Some(events_path),
                    Some(line_number),
                );
                continue;
            }
        };
        if !event.event_id.trim().is_empty() {
            if let Some(first_line) = event_ids.get(&event.event_id) {
                push_session_event_validation_issue(
                    issues,
                    "duplicate_event_id",
                    format!(
                        "event_id `{}` on line {line_number} duplicates line {first_line}",
                        event.event_id
                    ),
                    Some(events_path),
                    Some(line_number),
                );
            } else {
                event_ids.insert(event.event_id.clone(), line_number);
            }
        }
        match event.kind {
            AgentSessionEventKind::UserMessage { content } => {
                if let Some(previous) = pending_user.take() {
                    push_session_event_validation_issue(
                        issues,
                        "user_message_without_assistant",
                        format!(
                            "user message on line {} was not followed by an assistant message before line {line_number}",
                            previous.line
                        ),
                        Some(events_path),
                        Some(previous.line),
                    );
                }
                pending_user = Some(PendingSessionUserMessage {
                    content,
                    line: line_number,
                });
            }
            AgentSessionEventKind::AssistantMessage { content } => {
                if let Some(user) = pending_user.take() {
                    pairs.push(SessionEventTurnPair {
                        request: user.content,
                        response: content,
                        request_line: user.line,
                        response_line: line_number,
                    });
                } else {
                    push_session_event_validation_issue(
                        issues,
                        "assistant_message_without_user",
                        format!(
                            "assistant message on line {line_number} has no preceding user message"
                        ),
                        Some(events_path),
                        Some(line_number),
                    );
                }
            }
            _ => {}
        }
    }
    if let Some(user) = pending_user {
        push_session_event_validation_issue(
            issues,
            "user_message_without_assistant",
            format!(
                "user message on line {} was not followed by an assistant message",
                user.line
            ),
            Some(events_path),
            Some(user.line),
        );
    }
    pairs
}

pub(crate) fn validate_event_turn_pairs_against_turns(
    session_dir: &Path,
    event_turns: &[SessionEventTurnPair],
    turns: &[FolderSessionTurnDigest],
    issues: &mut Vec<SessionValidateEventsIssue>,
) {
    if turns.is_empty() {
        return;
    }
    if event_turns.len() != turns.len() {
        push_session_event_validation_issue(
            issues,
            "turn_count_mismatch",
            format!(
                "events.jsonl has {} user/assistant turn pair(s), but turns/ has {} turn folder(s)",
                event_turns.len(),
                turns.len()
            ),
            Some(&session_dir.join("events.jsonl")),
            None,
        );
    }

    for (index, (event_turn, turn)) in event_turns.iter().zip(turns.iter()).enumerate() {
        let turn_number = index + 1;
        match &turn.request {
            Some(request) if same_session_text(request, &event_turn.request) => {}
            Some(_) => push_session_event_validation_issue(
                issues,
                "turn_request_mismatch",
                format!(
                    "turn {} ({}) request.md does not match user_message from events.jsonl line {}",
                    turn_number, turn.id, event_turn.request_line
                ),
                turn.request_path.as_deref(),
                None,
            ),
            None => push_session_event_validation_issue(
                issues,
                "missing_turn_request",
                format!("turn {} ({}) is missing request.md", turn_number, turn.id),
                Some(&session_dir.join("turns").join(&turn.id).join("request.md")),
                None,
            ),
        }
        match &turn.response {
            Some(response) if same_session_text(response, &event_turn.response) => {}
            Some(_) => push_session_event_validation_issue(
                issues,
                "turn_response_mismatch",
                format!(
                    "turn {} ({}) response.md does not match assistant_message from events.jsonl line {}",
                    turn_number, turn.id, event_turn.response_line
                ),
                turn.response_path.as_deref(),
                None,
            ),
            None => push_session_event_validation_issue(
                issues,
                "missing_turn_response",
                format!("turn {} ({}) is missing response.md", turn_number, turn.id),
                Some(&session_dir.join("turns").join(&turn.id).join("response.md")),
                None,
            ),
        }
    }
}

pub(crate) fn validate_root_summary_against_latest_turn(
    session_dir: &Path,
    event_turns: &[SessionEventTurnPair],
    turns: &[FolderSessionTurnDigest],
    issues: &mut Vec<SessionValidateEventsIssue>,
) -> Result<Option<bool>> {
    let latest_response = turns
        .last()
        .and_then(|turn| turn.response.as_deref())
        .or_else(|| event_turns.last().map(|turn| turn.response.as_str()));
    let Some(latest_response) = latest_response else {
        return Ok(None);
    };
    let summary_path = session_dir.join("summary.md");
    let summary = read_optional_markdown_file(&summary_path)?;
    let matches_latest = summary
        .as_deref()
        .map(|summary| same_session_text(summary, latest_response))
        .unwrap_or(false);
    if summary.is_none() {
        push_session_event_validation_issue(
            issues,
            "missing_root_summary",
            "summary.md is missing or empty while the latest response exists".to_string(),
            Some(&summary_path),
            None,
        );
    } else if !matches_latest {
        push_session_event_validation_issue(
            issues,
            "root_summary_mismatch",
            "summary.md does not match the latest turn response.md".to_string(),
            Some(&summary_path),
            None,
        );
    }
    Ok(Some(matches_latest))
}

pub(crate) fn same_session_text(left: &str, right: &str) -> bool {
    left.trim_end() == right.trim_end()
}

pub(crate) fn push_session_event_validation_issue(
    issues: &mut Vec<SessionValidateEventsIssue>,
    code: &str,
    message: String,
    path: Option<&Path>,
    line: Option<usize>,
) {
    issues.push(SessionValidateEventsIssue {
        severity: "error".to_string(),
        code: code.to_string(),
        message,
        path: path.map(|path| path.display().to_string()),
        line,
    });
}

pub(crate) fn format_session_validate_events_report(
    report: &SessionValidateEventsReport,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Validated session event ledger: {}",
        report.session_dir
    ));
    lines.push(format!(
        "  status: {}",
        if report.all_valid { "valid" } else { "invalid" }
    ));
    lines.push(format!(
        "  events.jsonl: {} ({})",
        yes_no(report.events_exists),
        report.events_path
    ));
    lines.push(format!("  events: {}", report.event_count));
    lines.push(format!("  event turn pairs: {}", report.event_turn_count));
    lines.push(format!("  turn folders: {}", report.turn_count));
    let summary_status = report
        .root_summary_matches_latest_turn
        .map(yes_no)
        .unwrap_or("n/a");
    lines.push(format!("  summary matches latest turn: {summary_status}"));
    if report.issues.is_empty() {
        lines.push("  issues: none".to_string());
    } else {
        lines.push("  issues:".to_string());
        for issue in &report.issues {
            let mut suffix = String::new();
            if let Some(path) = &issue.path {
                suffix.push_str(&format!(" ({path}"));
                if let Some(line) = issue.line {
                    suffix.push_str(&format!(":{line}"));
                }
                suffix.push(')');
            }
            lines.push(format!(
                "    - [{}] {}{}",
                issue.code, issue.message, suffix
            ));
        }
    }
    lines.push(format!("  note: {}", report.note));
    lines.push(String::new());
    lines.join("\n")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionProjectEventsReport {
    pub(crate) session_dir: String,
    pub(crate) events_path: String,
    pub(crate) events_exists: bool,
    pub(crate) event_count: usize,
    pub(crate) projected_turn_count: usize,
    pub(crate) existing_turn_count: usize,
    pub(crate) writes: bool,
    pub(crate) backup_dir: Option<String>,
    pub(crate) turns: Vec<SessionProjectedEventTurn>,
    pub(crate) summary: Option<SessionProjectedSummary>,
    pub(crate) issues: Vec<SessionValidateEventsIssue>,
    pub(crate) note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionProjectedEventTurn {
    pub(crate) index: usize,
    pub(crate) id: String,
    pub(crate) existing_turn_id: Option<String>,
    pub(crate) request_path: String,
    pub(crate) response_path: String,
    pub(crate) request_chars: usize,
    pub(crate) response_chars: usize,
    pub(crate) request_preview: String,
    pub(crate) response_preview: String,
    pub(crate) request_state: String,
    pub(crate) response_state: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionProjectedSummary {
    pub(crate) path: String,
    pub(crate) source_turn_id: String,
    pub(crate) response_chars: usize,
    pub(crate) response_preview: String,
    pub(crate) state: String,
}

pub(crate) fn project_folder_session_events(dir: &Path) -> Result<SessionProjectEventsReport> {
    let session_dir = resolve_existing_folder_session_dir(dir)?;
    let events_path = session_dir.join("events.jsonl");
    let turns = read_folder_session_turns(&session_dir.join("turns"))?;
    let mut issues = Vec::new();
    let mut event_count = 0;
    let mut event_turns = Vec::new();
    let mut events_exists = events_path.exists();

    if events_exists && !events_path.is_file() {
        push_session_event_validation_issue(
            &mut issues,
            "invalid_events_path",
            format!(
                "events.jsonl exists but is not a file: {}",
                events_path.display()
            ),
            Some(&events_path),
            None,
        );
        events_exists = false;
    }

    if events_exists {
        let raw = fs::read_to_string(&events_path)
            .with_context(|| format!("reading {}", events_path.display()))?;
        event_count = raw.lines().filter(|line| !line.trim().is_empty()).count();
        event_turns = read_event_turn_pairs(&events_path, &raw, &mut issues);
    } else {
        push_session_event_validation_issue(
            &mut issues,
            "missing_events_jsonl",
            format!(
                "missing folder-local event ledger: {}",
                events_path.display()
            ),
            Some(&events_path),
            None,
        );
    }

    let projected_turns = event_turns
        .iter()
        .enumerate()
        .map(|(index, event_turn)| {
            let existing_turn = turns.get(index);
            let id = existing_turn
                .map(|turn| turn.id.clone())
                .unwrap_or_else(|| projected_event_turn_id(index));
            let turn_dir = session_dir.join("turns").join(&id);
            SessionProjectedEventTurn {
                index: index + 1,
                id: id.clone(),
                existing_turn_id: existing_turn.map(|turn| turn.id.clone()),
                request_path: turn_dir.join("request.md").display().to_string(),
                response_path: turn_dir.join("response.md").display().to_string(),
                request_chars: event_turn.request.chars().count(),
                response_chars: event_turn.response.chars().count(),
                request_preview: compact_text_snippet(&event_turn.request, 160),
                response_preview: compact_text_snippet(&event_turn.response, 160),
                request_state: projected_content_state(
                    existing_turn.and_then(|turn| turn.request.as_deref()),
                    &event_turn.request,
                ),
                response_state: projected_content_state(
                    existing_turn.and_then(|turn| turn.response.as_deref()),
                    &event_turn.response,
                ),
            }
        })
        .collect::<Vec<_>>();

    let summary_path = session_dir.join("summary.md");
    let existing_summary = read_optional_markdown_file(&summary_path)?;
    let latest_event_response = event_turns.last().map(|turn| turn.response.as_str());
    let summary = projected_turns.last().map(|turn| SessionProjectedSummary {
        path: summary_path.display().to_string(),
        source_turn_id: turn.id.clone(),
        response_chars: latest_event_response
            .map(|response| response.chars().count())
            .unwrap_or_default(),
        response_preview: latest_event_response
            .map(|response| compact_text_snippet(response, 160))
            .unwrap_or_default(),
        state: projected_content_state(
            existing_summary.as_deref(),
            latest_event_response.unwrap_or_default(),
        ),
    });

    let note = if issues.is_empty() {
        "Dry-run only: no files were written. This projection can be used to review a future event-to-turn regeneration."
    } else {
        "Dry-run only: no files were written. Repair event issues before using this projection as a regeneration source."
    }
    .to_string();

    Ok(SessionProjectEventsReport {
        session_dir: session_dir.display().to_string(),
        events_path: events_path.display().to_string(),
        events_exists,
        event_count,
        projected_turn_count: projected_turns.len(),
        existing_turn_count: turns.len(),
        writes: false,
        backup_dir: None,
        turns: projected_turns,
        summary,
        issues,
        note,
    })
}

pub(crate) fn rebuild_folder_session_from_events(dir: &Path) -> Result<SessionProjectEventsReport> {
    let session_dir = resolve_existing_folder_session_dir(dir)?;
    let events_path = session_dir.join("events.jsonl");
    let raw = fs::read_to_string(&events_path)
        .with_context(|| format!("reading {}", events_path.display()))?;
    let mut issues = Vec::new();
    let event_turns = read_event_turn_pairs(&events_path, &raw, &mut issues);
    if !issues.is_empty() {
        bail!(
            "cannot rebuild turns from events with {} event issue(s); run `djinn session events {}` to inspect them",
            issues.len(),
            shell_quote(&session_dir.display().to_string())
        );
    }
    if event_turns.is_empty() {
        bail!(
            "cannot rebuild turns from events because {} contains no complete user/assistant turn pairs",
            events_path.display()
        );
    }

    let existing_turns = read_folder_session_turns(&session_dir.join("turns"))?;
    let turn_ids = event_turns
        .iter()
        .enumerate()
        .map(|(index, _)| {
            existing_turns
                .get(index)
                .map(|turn| turn.id.clone())
                .unwrap_or_else(|| projected_event_turn_id(index))
        })
        .collect::<Vec<_>>();
    let backup_dir =
        backup_folder_session_event_rebuild_targets(&session_dir, "djinn session events --write")?;

    let turns_dir = session_dir.join("turns");
    if turns_dir.exists() {
        fs::rename(&turns_dir, backup_dir.join("turns"))
            .with_context(|| format!("backing up {}", turns_dir.display()))?;
    }
    fs::create_dir_all(&turns_dir)
        .with_context(|| format!("creating turns directory {}", turns_dir.display()))?;
    for (index, event_turn) in event_turns.iter().enumerate() {
        let turn_dir = turns_dir.join(&turn_ids[index]);
        fs::create_dir_all(&turn_dir)
            .with_context(|| format!("creating projected turn directory {}", turn_dir.display()))?;
        fs::write(
            turn_dir.join("request.md"),
            ensure_trailing_newline(&event_turn.request),
        )
        .with_context(|| format!("writing projected request in {}", turn_dir.display()))?;
        fs::write(
            turn_dir.join("response.md"),
            ensure_trailing_newline(&event_turn.response),
        )
        .with_context(|| format!("writing projected response in {}", turn_dir.display()))?;
    }
    if let Some(latest) = event_turns.last() {
        let summary_path = session_dir.join("summary.md");
        fs::write(&summary_path, ensure_trailing_newline(&latest.response))
            .with_context(|| format!("writing projected {}", summary_path.display()))?;
    }

    let mut report = project_folder_session_events(&session_dir)?;
    report.writes = true;
    report.backup_dir = Some(backup_dir.display().to_string());
    report.note = "Rebuilt optional turns/ projection and summary.md from events.jsonl after preserving a backup."
        .to_string();
    Ok(report)
}

pub(crate) fn backup_folder_session_event_rebuild_targets(
    session_dir: &Path,
    source: &str,
) -> Result<PathBuf> {
    let backup_root = session_dir.join(".djinn/backups");
    fs::create_dir_all(&backup_root)
        .with_context(|| format!("creating backup root {}", backup_root.display()))?;
    let backup_dir = backup_root.join(format!(
        "events-rebuild-{}-{}",
        chrono::Local::now().format("%Y%m%dT%H%M%S"),
        chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
    ));
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("creating backup directory {}", backup_dir.display()))?;

    let summary_path = session_dir.join("summary.md");
    if summary_path.exists() {
        fs::copy(&summary_path, backup_dir.join("summary.md")).with_context(|| {
            format!(
                "backing up {} to {}",
                summary_path.display(),
                backup_dir.display()
            )
        })?;
    }
    let manifest = format!(
        "created_at = {}\nsource = {}\nincludes_turns = {}\nincludes_summary = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?,
        toml_string(source)?,
        session_dir.join("turns").exists(),
        summary_path.exists()
    );
    fs::write(backup_dir.join("backup.toml"), manifest)
        .with_context(|| format!("writing backup manifest in {}", backup_dir.display()))?;
    Ok(backup_dir)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionRestoreEventsReport {
    pub(crate) session_dir: String,
    pub(crate) backup_dir: String,
    pub(crate) writes: bool,
    pub(crate) safety_backup_dir: Option<String>,
    pub(crate) backup_has_turns: bool,
    pub(crate) backup_has_summary: bool,
    pub(crate) current_turn_count: usize,
    pub(crate) restored_turn_count: usize,
    pub(crate) restored_summary: bool,
    pub(crate) note: String,
}

pub(crate) fn restore_folder_session_event_backup(
    dir: &Path,
    backup: &Path,
    write: bool,
) -> Result<SessionRestoreEventsReport> {
    let session_dir = resolve_existing_folder_session_dir(dir)?;
    let backup_dir = resolve_folder_session_event_backup(&session_dir, backup)?;
    let backup_turns_dir = backup_dir.join("turns");
    let backup_summary_path = backup_dir.join("summary.md");
    let backup_has_turns = backup_turns_dir.is_dir();
    let backup_has_summary = backup_summary_path.is_file();
    if !backup_has_turns && !backup_has_summary {
        bail!(
            "event rebuild backup contains neither turns/ nor summary.md: {}",
            backup_dir.display()
        );
    }

    let current_turn_count = read_folder_session_turns(&session_dir.join("turns"))?.len();
    let restored_turn_count = if backup_has_turns {
        read_folder_session_turns(&backup_turns_dir)?.len()
    } else {
        0
    };
    let mut safety_backup_dir = None;

    if write {
        let safety_backup = backup_folder_session_event_rebuild_targets(
            &session_dir,
            "djinn session events --restore --write",
        )?;
        let current_turns_dir = session_dir.join("turns");
        if current_turns_dir.exists() {
            fs::rename(&current_turns_dir, safety_backup.join("turns"))
                .with_context(|| format!("backing up {}", current_turns_dir.display()))?;
        }

        if backup_has_turns {
            copy_dir_recursive(&backup_turns_dir, &current_turns_dir)?;
        }

        let summary_path = session_dir.join("summary.md");
        if backup_has_summary {
            fs::copy(&backup_summary_path, &summary_path).with_context(|| {
                format!(
                    "restoring {} from {}",
                    summary_path.display(),
                    backup_summary_path.display()
                )
            })?;
        } else if summary_path.exists() {
            fs::remove_file(&summary_path)
                .with_context(|| format!("removing {}", summary_path.display()))?;
        }
        safety_backup_dir = Some(safety_backup.display().to_string());
    }

    let note = if write {
        "Restored turns/ and summary.md from an event rebuild backup after preserving the previous current state."
    } else {
        "Preview only: no files were written. Add --write to restore this backup."
    }
    .to_string();

    Ok(SessionRestoreEventsReport {
        session_dir: session_dir.display().to_string(),
        backup_dir: backup_dir.display().to_string(),
        writes: write,
        safety_backup_dir,
        backup_has_turns,
        backup_has_summary,
        current_turn_count,
        restored_turn_count,
        restored_summary: backup_has_summary,
        note,
    })
}

pub(crate) fn resolve_folder_session_event_backup(
    session_dir: &Path,
    backup: &Path,
) -> Result<PathBuf> {
    let backup_root = session_dir.join(".djinn/backups");
    let candidate = if is_single_component_path(backup) {
        backup_root.join(backup)
    } else if backup.is_absolute() {
        backup.to_path_buf()
    } else {
        env::current_dir()
            .context("resolving current directory")?
            .join(backup)
    };
    let backup_root = backup_root.canonicalize().with_context(|| {
        format!(
            "resolving event rebuild backup root {}",
            backup_root.display()
        )
    })?;
    let candidate = candidate
        .canonicalize()
        .with_context(|| format!("resolving event rebuild backup {}", candidate.display()))?;
    if !candidate.starts_with(&backup_root) {
        bail!(
            "event rebuild backup must be under {}: {}",
            backup_root.display(),
            candidate.display()
        );
    }
    if !candidate.is_dir() {
        bail!(
            "event rebuild backup is not a directory: {}",
            candidate.display()
        );
    }
    if !candidate.join("backup.toml").is_file() {
        bail!(
            "event rebuild backup is missing backup.toml: {}",
            candidate.display()
        );
    }
    Ok(candidate)
}

pub(crate) fn is_single_component_path(path: &Path) -> bool {
    !path.is_absolute()
        && matches!(path.components().next(), Some(Component::Normal(_)))
        && path.components().nth(1).is_none()
}

pub(crate) fn copy_dir_recursive(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("reading {}", source.display()))? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if source_path.is_file() {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "copying {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}

pub(crate) fn format_session_restore_events_report(report: &SessionRestoreEventsReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} event rebuild backup: {}",
        if report.writes {
            "Restored"
        } else {
            "Previewed"
        },
        report.session_dir
    ));
    lines.push(format!("  writes: {}", yes_no(report.writes)));
    lines.push(format!("  backup: {}", report.backup_dir));
    if let Some(safety_backup) = &report.safety_backup_dir {
        lines.push(format!("  safety backup: {safety_backup}"));
    }
    lines.push(format!(
        "  backup turns/: {}",
        yes_no(report.backup_has_turns)
    ));
    lines.push(format!(
        "  backup summary.md: {}",
        yes_no(report.backup_has_summary)
    ));
    lines.push(format!(
        "  current turn folders: {}",
        report.current_turn_count
    ));
    lines.push(format!(
        "  restored turn folders: {}",
        report.restored_turn_count
    ));
    lines.push(format!(
        "  restored summary.md: {}",
        yes_no(report.restored_summary)
    ));
    lines.push(format!("  note: {}", report.note));
    lines.push(String::new());
    lines.join("\n")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionEventsHealthReport {
    pub(crate) root: String,
    pub(crate) filter: Option<String>,
    pub(crate) total: usize,
    pub(crate) ready: usize,
    pub(crate) not_ready: usize,
    pub(crate) sessions: Vec<SessionEventsHealthEntry>,
    pub(crate) note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionEventsHealthEntry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) ready: bool,
    pub(crate) events_exists: bool,
    pub(crate) event_count: usize,
    pub(crate) event_turn_count: usize,
    pub(crate) turn_count: usize,
    pub(crate) summary_matches_latest_turn: Option<bool>,
    pub(crate) issue_count: usize,
    pub(crate) issue_codes: Vec<String>,
    pub(crate) latest_event_rebuild_backup: Option<String>,
}

pub(crate) fn event_health_report_for_cache_sessions(
    limit: Option<usize>,
    health_filter: Option<&str>,
) -> Result<SessionEventsHealthReport> {
    let root = default_folder_session_root();
    event_health_report_for_folder_session_root(&root, limit, health_filter)
}

pub(crate) fn event_health_report_for_folder_session_root(
    root: &Path,
    limit: Option<usize>,
    health_filter: Option<&str>,
) -> Result<SessionEventsHealthReport> {
    let health_filter = health_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut sessions = Vec::new();
    if root.is_dir() {
        let mut entries = fs::read_dir(&root)
            .with_context(|| format!("reading folder session root {}", root.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let report = validate_folder_session_events(&path)?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(folder_session_display_name)
                .unwrap_or_else(|| path.display().to_string());
            let issue_codes = report
                .issues
                .iter()
                .map(|issue| issue.code.clone())
                .collect::<Vec<_>>();
            let entry = SessionEventsHealthEntry {
                name,
                path: path.display().to_string(),
                ready: report.all_valid,
                events_exists: report.events_exists,
                event_count: report.event_count,
                event_turn_count: report.event_turn_count,
                turn_count: report.turn_count,
                summary_matches_latest_turn: report.root_summary_matches_latest_turn,
                issue_count: report.issues.len(),
                issue_codes,
                latest_event_rebuild_backup: latest_event_rebuild_backup_path(&path)
                    .map(|path| path.display().to_string()),
            };
            if event_health_entry_matches_filter(&entry, health_filter.as_deref()) {
                sessions.push(entry);
            }
        }
        sessions.sort_by(|left, right| {
            left.ready
                .cmp(&right.ready)
                .then_with(|| left.name.cmp(&right.name))
        });
        if let Some(limit) = limit {
            sessions.truncate(limit);
        }
    }
    let ready = sessions.iter().filter(|entry| entry.ready).count();
    let total = sessions.len();
    let not_ready = total.saturating_sub(ready);
    let note = if total == 0 {
        "No cache-backed folder sessions found.".to_string()
    } else if not_ready == 0 {
        "All reported cache-backed sessions have event ledgers that agree with optional turn files."
            .to_string()
    } else {
        "Some cache-backed sessions have event ledger issues. Inspect issue codes with `djinn session events <session>` and `djinn session validate-events <session>`.".to_string()
    };
    let report = SessionEventsHealthReport {
        root: root.display().to_string(),
        filter: health_filter,
        total,
        ready,
        not_ready,
        sessions,
        note,
    };
    Ok(report)
}

pub(crate) fn event_health_entry_matches_filter(
    entry: &SessionEventsHealthEntry,
    filter: Option<&str>,
) -> bool {
    let Some(filter) = filter.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let normalized = normalize_event_health_filter(filter);
    match normalized.as_str() {
        "ready" => entry.ready,
        "not-ready" | "not_ready" | "notready" => !entry.ready,
        "missing" | "missing-events" | "missing_events_jsonl" => !entry.events_exists,
        value => entry
            .issue_codes
            .iter()
            .any(|code| normalize_event_health_filter(code) == value),
    }
}

pub(crate) fn normalize_event_health_filter(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .map(|ch| if ch == '_' || ch == ' ' { '-' } else { ch })
        .collect()
}

pub(crate) fn format_event_health_report(report: &SessionEventsHealthReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Event ledger health: {}", report.root));
    if let Some(filter) = &report.filter {
        lines.push(format!("  filter: {filter}"));
    }
    lines.push(format!("  total: {}", report.total));
    lines.push(format!("  ready: {}", report.ready));
    lines.push(format!("  not ready: {}", report.not_ready));
    if report.sessions.is_empty() {
        lines.push("  sessions: none".to_string());
    } else {
        lines.push("  sessions:".to_string());
        for session in &report.sessions {
            lines.push(format!(
                "    - {} [{}]",
                session.name,
                if session.ready { "ready" } else { "not_ready" }
            ));
            lines.push(format!("      path: {}", session.path));
            lines.push(format!(
                "      events: {} rows, {} turn pairs, exists: {}",
                session.event_count,
                session.event_turn_count,
                yes_no(session.events_exists)
            ));
            lines.push(format!("      turn folders: {}", session.turn_count));
            let summary = session
                .summary_matches_latest_turn
                .map(yes_no)
                .unwrap_or("n/a");
            lines.push(format!("      summary matches latest turn: {summary}"));
            if session.issue_codes.is_empty() {
                lines.push("      issues: none".to_string());
            } else {
                lines.push(format!(
                    "      issues: {} ({})",
                    session.issue_count,
                    session.issue_codes.join(", ")
                ));
            }
            if let Some(backup) = &session.latest_event_rebuild_backup {
                lines.push(format!("      latest rebuild backup: {backup}"));
            }
        }
    }
    lines.push(format!("  note: {}", report.note));
    lines.push(String::new());
    lines.join("\n")
}

pub(crate) fn ensure_event_health_strict(report: &SessionEventsHealthReport) -> Result<()> {
    if report.not_ready > 0 {
        bail!(
            "event ledger strict check failed: {} of {} reported session(s) not ready",
            report.not_ready,
            report.total
        );
    }
    Ok(())
}

pub(crate) fn projected_event_turn_id(index: usize) -> String {
    format!("event-turn-{number:04}", number = index + 1)
}

pub(crate) fn projected_content_state(existing: Option<&str>, projected: &str) -> String {
    match existing {
        Some(existing) if same_session_text(existing, projected) => "matches".to_string(),
        Some(_) => "would_update".to_string(),
        None => "would_create".to_string(),
    }
}

pub(crate) fn format_session_project_events_report(report: &SessionProjectEventsReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} session event ledger: {}",
        if report.writes {
            "Rebuilt"
        } else {
            "Projected"
        },
        report.session_dir
    ));
    lines.push(format!("  writes: {}", yes_no(report.writes)));
    if let Some(backup_dir) = &report.backup_dir {
        lines.push(format!("  backup: {backup_dir}"));
    }
    lines.push(format!(
        "  events.jsonl: {} ({})",
        yes_no(report.events_exists),
        report.events_path
    ));
    lines.push(format!("  events: {}", report.event_count));
    lines.push(format!(
        "  projected turns: {}",
        report.projected_turn_count
    ));
    lines.push(format!(
        "  existing turn folders: {}",
        report.existing_turn_count
    ));
    if report.turns.is_empty() {
        lines.push("  turns/: none projected".to_string());
    } else {
        lines.push("  turns/:".to_string());
        for turn in &report.turns {
            lines.push(format!("    - {} (event pair {})", turn.id, turn.index));
            if let Some(existing) = &turn.existing_turn_id {
                lines.push(format!("      existing: {existing}"));
            }
            lines.push(format!(
                "      request.md: {} ({} chars)",
                turn.request_state, turn.request_chars
            ));
            lines.push(format!("        path: {}", turn.request_path));
            lines.push(format!("        preview: {}", turn.request_preview));
            lines.push(format!(
                "      response.md: {} ({} chars)",
                turn.response_state, turn.response_chars
            ));
            lines.push(format!("        path: {}", turn.response_path));
            lines.push(format!("        preview: {}", turn.response_preview));
        }
    }
    if let Some(summary) = &report.summary {
        lines.push("  summary.md:".to_string());
        lines.push(format!("    state: {}", summary.state));
        lines.push(format!("    path: {}", summary.path));
        lines.push(format!("    source turn: {}", summary.source_turn_id));
        lines.push(format!("    response chars: {}", summary.response_chars));
        lines.push(format!("    preview: {}", summary.response_preview));
    }
    if !report.issues.is_empty() {
        lines.push("  issues:".to_string());
        for issue in &report.issues {
            let mut suffix = String::new();
            if let Some(path) = &issue.path {
                suffix.push_str(&format!(" ({path}"));
                if let Some(line) = issue.line {
                    suffix.push_str(&format!(":{line}"));
                }
                suffix.push(')');
            }
            lines.push(format!(
                "    - [{}] {}{}",
                issue.code, issue.message, suffix
            ));
        }
    }
    lines.push(format!("  note: {}", report.note));
    lines.push(String::new());
    lines.join("\n")
}
