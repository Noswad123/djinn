use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use djinn_memory::{lifecycle_for, AgentSession, AgentSessionEvent, AgentSessionEventKind};
use serde::Serialize;

use crate::promotion_candidate::{candidate_string_array_value, candidate_string_value};
use crate::{
    background_run::BackgroundRunStatus, inspect_folder_session_context_dir,
    load_folder_native_agent_session, parse_manifest_string_value, read_folder_session_event_turns,
    read_folder_session_manifest, read_folder_session_turns, resolve_existing_folder_session_dir,
    resolve_existing_folder_session_reference, FolderSessionManifest, FolderSessionTurnDigest,
    SessionStatusArgs,
};

pub(crate) fn session_status(args: SessionStatusArgs) -> Result<()> {
    let session_ref = resolve_existing_folder_session_reference(&args.dir)?;
    let report = folder_session_status(&session_ref.session_dir)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_folder_session_status(&report));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionStatusReport {
    pub(crate) session_dir: String,
    pub(crate) manifest_exists: bool,
    pub(crate) session_id: Option<String>,
    pub(crate) native_session_exists: bool,
    pub(crate) profile: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) workspace: Option<String>,
    pub(crate) repo: Option<SessionStatusRepoReport>,
    pub(crate) lifecycle: SessionStatusLifecycleReport,
    pub(crate) files: SessionStatusFileReport,
    pub(crate) turn_count: usize,
    pub(crate) event_count: usize,
    pub(crate) latest_turn: Option<SessionStatusTurnReport>,
    pub(crate) candidates: Option<SessionStatusCandidateReport>,
    pub(crate) context_ingestible_count: usize,
    pub(crate) context_skipped: Vec<String>,
    pub(crate) next_action: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionStatusCandidateReport {
    pub(crate) candidate_count: usize,
    pub(crate) accepted_count: usize,
    pub(crate) denied_count: usize,
    pub(crate) pending_count: usize,
    pub(crate) candidates_dir: String,
    pub(crate) candidate_index_path: Option<String>,
    pub(crate) candidate_status_path: Option<String>,
    pub(crate) entries: Vec<SessionStatusCandidateEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionStatusCandidateEntry {
    pub(crate) id: String,
    pub(crate) candidate_type: Option<String>,
    pub(crate) status: String,
    pub(crate) path: String,
    pub(crate) text: Option<String>,
    pub(crate) rationale: Option<String>,
    pub(crate) evidence: Vec<String>,
    pub(crate) destination: Option<String>,
    pub(crate) writeback_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionStatusLifecycleReport {
    pub(crate) state: String,
    pub(crate) mode: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) note: Option<String>,
}

impl Default for SessionStatusLifecycleReport {
    fn default() -> Self {
        Self {
            state: String::new(),
            mode: None,
            updated_at: None,
            reason: None,
            note: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionStatusTurnReport {
    pub(crate) id: String,
    pub(crate) request_path: Option<String>,
    pub(crate) response_path: Option<String>,
    pub(crate) has_response: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionStatusRepoReport {
    pub(crate) path: Option<String>,
    pub(crate) link: Option<String>,
    pub(crate) link_exists: bool,
    pub(crate) link_is_symlink: bool,
    pub(crate) link_target: Option<String>,
    pub(crate) link_broken: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionStatusFileReport {
    pub(crate) request_md: bool,
    pub(crate) summary_md: bool,
    pub(crate) context_dir: bool,
    pub(crate) compacted_md: bool,
    pub(crate) turns_dir: bool,
    pub(crate) events_jsonl: bool,
}

pub(crate) fn format_folder_session_status(report: &SessionStatusReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Djinn session: {}", report.session_dir));
    lines.push(format!(
        "Manifest: {}",
        crate::yes_no(report.manifest_exists)
    ));
    if let Some(session_id) = &report.session_id {
        lines.push(format!(
            "Native session: {session_id} ({})",
            if report.native_session_exists {
                "found"
            } else {
                "missing"
            }
        ));
    } else {
        lines.push("Native session: none recorded".to_string());
    }
    lines.push(format!("State: {}", report.lifecycle.state));
    if let Some(mode) = &report.lifecycle.mode {
        lines.push(format!("Mode: {mode}"));
    }
    if let Some(updated_at) = &report.lifecycle.updated_at {
        lines.push(format!("State updated: {updated_at}"));
    }
    if let Some(reason) = &report.lifecycle.reason {
        lines.push(format!("State reason: {reason}"));
    }
    if let Some(note) = &report.lifecycle.note {
        lines.push(format!("State note: {note}"));
    }
    if let Some(profile) = &report.profile {
        lines.push(format!("Profile: {profile}"));
    }
    if let Some(agent) = &report.agent {
        lines.push(format!("Agent: {agent}"));
    }
    if let Some(model) = &report.model {
        lines.push(format!("Model: {model}"));
    }
    if let Some(workspace) = &report.workspace {
        lines.push(format!("Workspace: {workspace}"));
    }
    if let Some(repo) = &report.repo {
        lines.push("Repo:".to_string());
        if let Some(path) = &repo.path {
            lines.push(format!("  path: {path}"));
        }
        if let Some(link) = &repo.link {
            lines.push(format!("  link: {link}"));
            lines.push(format!(
                "  link exists: {}",
                crate::yes_no(repo.link_exists)
            ));
            lines.push(format!(
                "  link symlink: {}",
                crate::yes_no(repo.link_is_symlink)
            ));
            if let Some(target) = &repo.link_target {
                lines.push(format!("  target: {target}"));
            }
            lines.push(format!("  broken: {}", crate::yes_no(repo.link_broken)));
        }
    }
    lines.push("Files:".to_string());
    lines.push(format!(
        "  request.md: {}",
        crate::yes_no(report.files.request_md)
    ));
    lines.push(format!(
        "  summary.md: {}",
        crate::yes_no(report.files.summary_md)
    ));
    lines.push(format!(
        "  context/: {}",
        crate::yes_no(report.files.context_dir)
    ));
    lines.push(format!(
        "  context/compacted.md: {}",
        crate::yes_no(report.files.compacted_md)
    ));
    lines.push(format!(
        "  turns/: {}",
        crate::yes_no(report.files.turns_dir)
    ));
    lines.push(format!(
        "  events.jsonl: {}",
        crate::yes_no(report.files.events_jsonl)
    ));
    lines.push(format!("Turns: {}", report.turn_count));
    lines.push(format!("Events: {}", report.event_count));
    if let Some(turn) = &report.latest_turn {
        lines.push("Latest turn:".to_string());
        lines.push(format!("  id: {}", turn.id));
        if let Some(request_path) = &turn.request_path {
            lines.push(format!("  request: {request_path}"));
        }
        if let Some(response_path) = &turn.response_path {
            lines.push(format!("  response: {response_path}"));
        }
        lines.push(format!(
            "  has response: {}",
            crate::yes_no(turn.has_response)
        ));
    }
    if let Some(candidates) = &report.candidates {
        lines.push("Candidates:".to_string());
        lines.push(format!(
            "  status: {}",
            format_session_candidate_status(candidates)
        ));
        lines.push(format!("  dir: {}", candidates.candidates_dir));
        if let Some(index_path) = &candidates.candidate_index_path {
            lines.push(format!("  index: {index_path}"));
        }
        if let Some(status_path) = &candidates.candidate_status_path {
            lines.push(format!("  decisions: {status_path}"));
        }
        if !candidates.entries.is_empty() {
            lines.push("  entries:".to_string());
            for entry in &candidates.entries {
                lines.push(format!("    - {}", format_session_candidate_entry(entry)));
            }
        }
    }
    lines.push(format!(
        "Ingestible context files: {}",
        report.context_ingestible_count
    ));
    lines.push(format!(
        "Manage context: djinn session context ls {}",
        report.session_dir
    ));
    if !report.context_skipped.is_empty() {
        lines.push("Skipped context:".to_string());
        for skipped in &report.context_skipped {
            lines.push(format!("  - {skipped}"));
        }
    }
    if let Some(next_action) = &report.next_action {
        lines.push(format!("Next: {next_action}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

pub(crate) fn count_folder_session_events_jsonl(path: &Path) -> usize {
    fs::read_to_string(path)
        .ok()
        .map(|content| {
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
        })
        .unwrap_or(0)
}

pub(crate) fn folder_session_status(dir: &Path) -> Result<SessionStatusReport> {
    let session_dir = resolve_existing_folder_session_dir(dir)?;
    let manifest_path = session_dir.join("djinn.toml");
    let manifest = read_folder_session_manifest(&session_dir)?;
    let session_id = manifest
        .as_ref()
        .and_then(|manifest| manifest.session_id.clone());
    let native_session = session_id
        .as_ref()
        .and_then(|id| load_folder_native_agent_session(&session_dir, id));
    let native_session_exists = native_session.is_some();
    let context_dir = session_dir.join("context");
    let turns_dir = session_dir.join("turns");
    let (context_ingestible_count, context_skipped) =
        inspect_folder_session_context_dir(&context_dir)?;
    let request_exists = session_dir.join("request.md").exists();
    let turns = read_folder_session_turns(&turns_dir)?;
    let events_path = session_dir.join("events.jsonl");
    let event_count = count_folder_session_events_jsonl(&events_path);
    let event_turns = read_folder_session_event_turns(&session_dir)?;
    let turn_count = if event_turns.is_empty() {
        turns.len()
    } else {
        event_turns.len()
    };
    let latest_turn = event_turns
        .last()
        .map(session_status_turn_report)
        .or_else(|| turns.last().map(session_status_turn_report));
    let candidates = session_status_candidates(&session_dir)?;
    let lifecycle = session_status_lifecycle(
        &session_dir,
        manifest.as_ref(),
        native_session.as_ref(),
        candidates.as_ref(),
    );
    let next_action = session_status_next_action(
        &session_dir,
        manifest.as_ref(),
        request_exists,
        turn_count,
        &lifecycle,
        candidates.as_ref(),
    );

    Ok(SessionStatusReport {
        session_dir: session_dir.display().to_string(),
        manifest_exists: manifest_path.exists(),
        session_id: session_id.map(|id| id.to_string()),
        native_session_exists,
        profile: manifest
            .as_ref()
            .and_then(|manifest| manifest.profile.clone()),
        agent: manifest
            .as_ref()
            .and_then(|manifest| manifest.agent.clone()),
        model: manifest
            .as_ref()
            .and_then(|manifest| manifest.model.clone()),
        workspace: manifest
            .as_ref()
            .and_then(|manifest| manifest.workspace.clone()),
        repo: manifest
            .as_ref()
            .and_then(|manifest| session_status_repo(&session_dir, manifest)),
        lifecycle,
        files: SessionStatusFileReport {
            request_md: request_exists,
            summary_md: session_dir.join("summary.md").exists(),
            context_dir: context_dir.is_dir(),
            compacted_md: context_dir.join("compacted.md").exists(),
            turns_dir: turns_dir.is_dir(),
            events_jsonl: events_path.exists(),
        },
        turn_count,
        event_count,
        latest_turn,
        candidates,
        context_ingestible_count,
        context_skipped,
        next_action,
    })
}

pub(crate) fn session_status_lifecycle(
    session_dir: &Path,
    manifest: Option<&FolderSessionManifest>,
    native_session: Option<&AgentSession>,
    candidates: Option<&SessionStatusCandidateReport>,
) -> SessionStatusLifecycleReport {
    if let Some(session) = native_session {
        let lifecycle = lifecycle_for(session);
        let report = SessionStatusLifecycleReport {
            state: lifecycle.state.as_str().to_string(),
            mode: lifecycle.mode.map(|mode| mode.as_str().to_string()),
            updated_at: crate::non_empty_string(&lifecycle.updated_at),
            reason: lifecycle.reason,
            note: lifecycle.note,
        };
        stale_background_run_lifecycle(session_dir, &report, session).unwrap_or(report)
    } else if manifest.and_then(|manifest| manifest.kind.as_deref()) == Some("promotion") {
        promotion_session_status_lifecycle(session_dir, candidates)
    } else {
        SessionStatusLifecycleReport {
            state: "not_started".to_string(),
            mode: None,
            updated_at: None,
            reason: None,
            note: None,
        }
    }
}

fn stale_background_run_lifecycle(
    session_dir: &Path,
    lifecycle: &SessionStatusLifecycleReport,
    native_session: &AgentSession,
) -> Option<SessionStatusLifecycleReport> {
    if lifecycle.state != "running" || lifecycle.mode.as_deref() != Some("background") {
        return None;
    }
    let mut run = crate::background_run::latest_background_session_run_status(session_dir)?;
    if run.alive && !background_run_unresponsive(&run) {
        return None;
    }
    run.last_observed_event = last_observed_agent_session_event(native_session);
    if run.alive {
        persist_background_run_recovery_observation(&run, "background_worker_unresponsive");
        return Some(SessionStatusLifecycleReport {
            state: "failed".to_string(),
            mode: Some("background".to_string()),
            updated_at: run
                .heartbeat_at
                .clone()
                .or(run.log_modified_at.clone())
                .or(run.started_at.clone())
                .or_else(|| lifecycle.updated_at.clone()),
            reason: Some("background_worker_unresponsive".to_string()),
            note: Some(format_unresponsive_background_run_note(&run)),
        });
    }
    persist_background_run_recovery_observation(&run, "background_worker_stale");
    Some(SessionStatusLifecycleReport {
        state: "failed".to_string(),
        mode: Some("background".to_string()),
        updated_at: run
            .log_modified_at
            .clone()
            .or(run.started_at.clone())
            .or_else(|| lifecycle.updated_at.clone()),
        reason: Some("background_worker_stale".to_string()),
        note: Some(format_stale_background_run_note(&run)),
    })
}

fn persist_background_run_recovery_observation(run: &BackgroundRunStatus, reason: &str) {
    let Some(marker_path) = run.marker_path.as_deref().map(Path::new) else {
        return;
    };
    let _ = persist_background_run_recovery_observation_to_path(marker_path, run, reason);
}

fn persist_background_run_recovery_observation_to_path(
    marker_path: &Path,
    run: &BackgroundRunStatus,
    reason: &str,
) -> Result<()> {
    let content = fs::read_to_string(marker_path)
        .with_context(|| format!("reading background run marker {}", marker_path.display()))?;
    let content = crate::upsert_toml_root_string(
        &content,
        "recovery_observed_at",
        &chrono::Local::now().to_rfc3339(),
    )?;
    let content = crate::upsert_toml_root_string(&content, "recovery_reason", reason)?;
    let content = if let Some(event) = &run.last_observed_event {
        crate::upsert_toml_root_string(&content, "last_observed_event", event)?
    } else {
        content
    };
    fs::write(marker_path, content)
        .with_context(|| format!("writing background run marker {}", marker_path.display()))
}

fn background_run_unresponsive(run: &BackgroundRunStatus) -> bool {
    run.heartbeat_age_seconds
        .is_some_and(|age| age >= crate::BACKGROUND_RUN_UNRESPONSIVE_SECONDS)
}

fn format_unresponsive_background_run_note(run: &BackgroundRunStatus) -> String {
    let mut note = format!(
        "Background worker is still alive but appears unresponsive (pid {}, run {}).",
        run.pid, run.run_id
    );
    if let Some(age) = run.heartbeat_age_seconds {
        note.push_str(&format!(" Last heartbeat was {age}s ago."));
    }
    if let Some(heartbeat_at) = &run.heartbeat_at {
        note.push_str(&format!(" Heartbeat at {heartbeat_at}."));
    }
    if let Some(phase) = &run.heartbeat_phase {
        note.push_str(&format!(" Phase: {phase}."));
    }
    if let Some(native_session_id) = &run.native_session_id {
        note.push_str(&format!(" Native session: {native_session_id}."));
    }
    if let Some(log_path) = &run.log_path {
        note.push_str(&format!(" Inspect log: {log_path}."));
    }
    if let Some(event) = &run.last_observed_event {
        note.push_str(&format!(" Last transcript event: {event}."));
    }
    if let Some(log_tail) = &run.log_tail {
        note.push_str(&format!(" Last log line: {log_tail}"));
    }
    note
}

fn format_stale_background_run_note(run: &BackgroundRunStatus) -> String {
    let mut note = format!(
        "Background worker appears stale; no live process found for pid {} (run {}).",
        run.pid, run.run_id
    );
    if let Some(native_session_id) = &run.native_session_id {
        note.push_str(&format!(" Native session: {native_session_id}."));
    }
    if let Some(log_path) = &run.log_path {
        note.push_str(&format!(" Inspect log: {log_path}."));
    }
    if let Some(command) = &run.command {
        note.push_str(&format!(" Command: {command}."));
    }
    if let Some(event) = &run.last_observed_event {
        note.push_str(&format!(" Last transcript event: {event}."));
    }
    if let Some(log_tail) = &run.log_tail {
        note.push_str(&format!(" Last log line: {log_tail}"));
    }
    note
}

fn last_observed_agent_session_event(session: &AgentSession) -> Option<String> {
    session
        .events
        .iter()
        .rev()
        .map(format_agent_session_event_summary)
        .find(|summary| !summary.trim().is_empty())
}

pub(crate) fn format_agent_session_event_summary(event: &AgentSessionEvent) -> String {
    let kind = match &event.kind {
        AgentSessionEventKind::SessionCreated { .. } => "session_created".to_string(),
        AgentSessionEventKind::SessionTitleUpdated { title } => {
            format!("session_title_updated title={}", truncate_inline(title, 80))
        }
        AgentSessionEventKind::SessionProfileUpdated { profile } => {
            format!("session_profile_updated profile={profile}")
        }
        AgentSessionEventKind::SessionModelUpdated { model } => {
            format!("session_model_updated model={model}")
        }
        AgentSessionEventKind::UserMessage { content } => {
            format!("user_message chars={}", content.chars().count())
        }
        AgentSessionEventKind::AssistantMessage { content } => {
            format!("assistant_message chars={}", content.chars().count())
        }
        AgentSessionEventKind::ModelResponseMetadata {
            model,
            round,
            elapsed_ms,
            tool_calls,
            ..
        } => format!(
            "model_response model={model} round={} elapsed_ms={elapsed_ms} tool_calls={tool_calls}",
            round
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        AgentSessionEventKind::ToolCall { id, name, .. } => {
            format!("tool_call id={id} name={name}")
        }
        AgentSessionEventKind::ToolResult { id, success, .. } => {
            format!("tool_result id={id} success={success}")
        }
        AgentSessionEventKind::ToolExecutionMetadata {
            id,
            name,
            elapsed_ms,
            success,
            ..
        } => {
            format!("tool_execution id={id} name={name} elapsed_ms={elapsed_ms} success={success}")
        }
        AgentSessionEventKind::Error { phase, message, .. } => {
            format!(
                "error phase={phase} message={}",
                truncate_inline(message, 120)
            )
        }
        AgentSessionEventKind::Summary { content } => {
            format!("summary chars={}", content.chars().count())
        }
        AgentSessionEventKind::Checkpoint { label } => {
            format!("checkpoint label={}", truncate_inline(label, 80))
        }
        AgentSessionEventKind::SessionLifecycleUpdated {
            state,
            mode,
            reason,
            ..
        } => format!(
            "lifecycle state={} mode={} reason={}",
            state.as_str(),
            mode.as_ref().map(|mode| mode.as_str()).unwrap_or("-"),
            reason.as_deref().unwrap_or("-")
        ),
        AgentSessionEventKind::ChildSessionStatusChanged {
            child_session_id,
            state,
            mode,
            ..
        } => format!(
            "child_session_status child={} state={} mode={}",
            child_session_id,
            state.as_str(),
            mode.as_ref().map(|mode| mode.as_str()).unwrap_or("-")
        ),
    };
    if event.event_id.trim().is_empty() {
        format!("{} {kind}", event.created_at)
    } else {
        format!("{} {} {kind}", event.created_at, event.event_id)
    }
}

fn truncate_inline(value: &str, max_chars: usize) -> String {
    let mut normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > max_chars {
        normalized = normalized
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect();
        normalized.push('…');
    }
    normalized
}

fn promotion_session_status_lifecycle(
    session_dir: &Path,
    candidates: Option<&SessionStatusCandidateReport>,
) -> SessionStatusLifecycleReport {
    if let Some(run) = crate::background_run::latest_background_session_run_status(session_dir)
        .filter(|run| run.alive)
    {
        return SessionStatusLifecycleReport {
            state: "running".to_string(),
            mode: Some("promotion".to_string()),
            updated_at: run.log_modified_at.clone().or(run.started_at.clone()),
            reason: Some("background_generation".to_string()),
            note: Some(format_background_promotion_run_note(&run)),
        };
    }
    if candidates.is_some_and(|candidates| candidates.candidate_count > 0) {
        return SessionStatusLifecycleReport {
            state: "completed".to_string(),
            mode: Some("promotion".to_string()),
            updated_at: latest_promotion_generation_modified_at(session_dir),
            reason: Some("candidates_generated".to_string()),
            note: Some("Promotion candidates are ready for review.".to_string()),
        };
    }
    if let Some(run) = crate::background_run::latest_background_session_run_status(session_dir) {
        return SessionStatusLifecycleReport {
            state: "failed".to_string(),
            mode: Some("promotion".to_string()),
            updated_at: run
                .started_at
                .or_else(|| latest_promotion_generation_modified_at(session_dir)),
            reason: Some("generation_failed".to_string()),
            note: Some(format!(
                "Promotion generation exited before writing valid candidates. Inspect the model response or log: {}",
                run.log_path.as_deref().unwrap_or("unknown")
            )),
        };
    }
    if promotion_generation_has_response(session_dir) {
        return SessionStatusLifecycleReport {
            state: "failed".to_string(),
            mode: Some("promotion".to_string()),
            updated_at: latest_promotion_generation_modified_at(session_dir),
            reason: Some("no_candidates".to_string()),
            note: Some(
                "Promotion generation wrote a response but no candidate TOML files.".to_string(),
            ),
        };
    }
    SessionStatusLifecycleReport {
        state: "not_started".to_string(),
        mode: Some("promotion".to_string()),
        updated_at: latest_promotion_generation_modified_at(session_dir),
        reason: None,
        note: None,
    }
}

pub(crate) fn format_background_promotion_run_note(run: &BackgroundRunStatus) -> String {
    let mut note = format!(
        "Promotion candidate generation is running in the background (run {}, pid {}, log {}, {}).",
        run.run_id,
        run.pid,
        run.log_path.as_deref().unwrap_or("unknown"),
        run.log_bytes
            .map(format_byte_count)
            .unwrap_or_else(|| "log size unknown".to_string())
    );
    if let Some(command) = &run.command {
        note.push_str(&format!(" Command: {command}."));
    }
    if let Some(updated) = &run.log_modified_at {
        note.push_str(&format!(" Log updated {updated}."));
    }
    if let Some(tail) = &run.log_tail {
        note.push_str(&format!(" Last log: {tail}"));
    }
    note
}

fn format_byte_count(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn promotion_generation_has_response(session_dir: &Path) -> bool {
    latest_promotion_generation_response_path(session_dir).is_some()
}

pub(crate) fn latest_promotion_generation_response_path(session_dir: &Path) -> Option<PathBuf> {
    session_dir
        .join("outputs")
        .join("generation")
        .read_dir()
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-response.md"))
        })
        .filter_map(|path| Some((fs::metadata(&path).ok()?.modified().ok()?, path)))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn latest_promotion_generation_modified_at(session_dir: &Path) -> Option<String> {
    let generation_dir = session_dir.join("outputs").join("generation");
    fs::read_dir(generation_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .max()
        .and_then(crate::background_run::system_time_to_rfc3339)
}

pub(crate) fn session_status_turn_report(
    turn: &FolderSessionTurnDigest,
) -> SessionStatusTurnReport {
    SessionStatusTurnReport {
        id: turn.id.clone(),
        request_path: turn
            .request_path
            .as_ref()
            .map(|path| path.display().to_string()),
        response_path: turn
            .response_path
            .as_ref()
            .map(|path| path.display().to_string()),
        has_response: turn.response_path.is_some(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PromotionCandidateDecisionStatus {
    status: String,
    destination: Option<String>,
    writeback_path: Option<String>,
}

pub(crate) fn session_status_candidates(
    session_dir: &Path,
) -> Result<Option<SessionStatusCandidateReport>> {
    let outputs_dir = session_dir.join("outputs");
    let candidates_dir = outputs_dir.join("candidates");
    let candidate_index_path = outputs_dir.join("candidate-index.toml");
    let candidate_status_path = outputs_dir.join("candidate-status.toml");
    let decisions = read_promotion_candidate_statuses(&candidate_status_path)?;
    let entries = read_session_status_candidate_entries(&candidates_dir, &decisions)?;
    let candidate_count = entries.len();
    if candidate_count == 0 && decisions.is_empty() && !candidate_index_path.exists() {
        return Ok(None);
    }
    let accepted_count = entries
        .iter()
        .filter(|entry| entry.status == "accepted")
        .count();
    let denied_count = entries
        .iter()
        .filter(|entry| entry.status == "denied")
        .count();
    let pending_count = entries
        .iter()
        .filter(|entry| entry.status == "pending")
        .count();
    Ok(Some(SessionStatusCandidateReport {
        candidate_count,
        accepted_count,
        denied_count,
        pending_count,
        candidates_dir: candidates_dir.display().to_string(),
        candidate_index_path: candidate_index_path
            .exists()
            .then(|| candidate_index_path.display().to_string()),
        candidate_status_path: candidate_status_path
            .exists()
            .then(|| candidate_status_path.display().to_string()),
        entries,
    }))
}

fn read_session_status_candidate_entries(
    candidates_dir: &Path,
    decisions: &BTreeMap<String, PromotionCandidateDecisionStatus>,
) -> Result<Vec<SessionStatusCandidateEntry>> {
    if !candidates_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(candidates_dir)
        .with_context(|| format!("reading promotion candidates {}", candidates_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|entry| {
            entry.is_file() && entry.extension().and_then(|ext| ext.to_str()) == Some("toml")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .iter()
        .map(|path| read_session_status_candidate_entry(path, decisions))
        .collect()
}

fn read_session_status_candidate_entry(
    path: &Path,
    decisions: &BTreeMap<String, PromotionCandidateDecisionStatus>,
) -> Result<SessionStatusCandidateEntry> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading promotion candidate {}", path.display()))?;
    let id = candidate_string_value(&content, "id").unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("candidate")
            .to_string()
    });
    let decision = decisions.get(&id);
    Ok(SessionStatusCandidateEntry {
        id,
        candidate_type: candidate_string_value(&content, "type"),
        status: decision
            .map(|decision| decision.status.clone())
            .filter(|status| !status.trim().is_empty())
            .unwrap_or_else(|| "pending".to_string()),
        path: path.display().to_string(),
        text: candidate_string_value(&content, "text"),
        rationale: candidate_string_value(&content, "rationale"),
        evidence: candidate_string_array_value(&content, "evidence"),
        destination: decision.and_then(|decision| decision.destination.clone()),
        writeback_path: decision.and_then(|decision| decision.writeback_path.clone()),
    })
}

fn read_promotion_candidate_statuses(
    status_path: &Path,
) -> Result<BTreeMap<String, PromotionCandidateDecisionStatus>> {
    if !status_path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = fs::read_to_string(status_path)
        .with_context(|| format!("reading {}", status_path.display()))?;
    let mut statuses = BTreeMap::new();
    let mut event = PromotionCandidateStatusEvent::default();
    for line in content.lines().map(str::trim) {
        if line.starts_with("[[") {
            record_promotion_candidate_status_event(&mut statuses, &event);
            event = PromotionCandidateStatusEvent::default();
            continue;
        }
        if let Some(value) = line
            .strip_prefix("candidate =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        {
            event.candidate = Some(value);
            continue;
        }
        if let Some(status) = line
            .strip_prefix("status =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        {
            event.status = Some(status);
            continue;
        }
        if let Some(destination) = line
            .strip_prefix("destination =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        {
            event.destination = Some(destination);
            continue;
        }
        if let Some(writeback_path) = line
            .strip_prefix("writeback_path =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        {
            event.writeback_path = Some(writeback_path);
        }
    }
    record_promotion_candidate_status_event(&mut statuses, &event);
    Ok(statuses)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PromotionCandidateStatusEvent {
    candidate: Option<String>,
    status: Option<String>,
    destination: Option<String>,
    writeback_path: Option<String>,
}

fn record_promotion_candidate_status_event(
    statuses: &mut BTreeMap<String, PromotionCandidateDecisionStatus>,
    event: &PromotionCandidateStatusEvent,
) {
    let Some(candidate) = event
        .candidate
        .as_deref()
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
    else {
        return;
    };
    let Some(status) = event
        .status
        .as_deref()
        .map(str::trim)
        .filter(|status| !status.is_empty())
    else {
        return;
    };
    statuses.insert(
        candidate.to_string(),
        PromotionCandidateDecisionStatus {
            status: status.to_string(),
            destination: event.destination.clone(),
            writeback_path: event.writeback_path.clone(),
        },
    );
}

pub(crate) fn format_session_candidate_status(candidates: &SessionStatusCandidateReport) -> String {
    format!(
        "{} total, {} accepted, {} denied, {} pending",
        candidates.candidate_count,
        candidates.accepted_count,
        candidates.denied_count,
        candidates.pending_count
    )
}

pub(crate) fn format_session_candidate_entry(entry: &SessionStatusCandidateEntry) -> String {
    let candidate_type = entry.candidate_type.as_deref().unwrap_or("unknown");
    let mut detail = format!("{} [{}] {}", entry.id, candidate_type, entry.status);
    if let Some(destination) = &entry.destination {
        detail.push_str(&format!(" -> {destination}"));
    }
    if let Some(evidence) = entry.evidence.first() {
        detail.push_str(&format!(" · evidence {evidence}"));
        if entry.evidence.len() > 1 {
            detail.push_str(&format!(" (+{})", entry.evidence.len() - 1));
        }
    }
    if let Some(path) = &entry.writeback_path {
        detail.push_str(&format!(" ({path})"));
    }
    detail
}

pub(crate) fn session_status_next_action(
    session_dir: &Path,
    manifest: Option<&FolderSessionManifest>,
    request_exists: bool,
    turn_count: usize,
    lifecycle: &SessionStatusLifecycleReport,
    candidates: Option<&SessionStatusCandidateReport>,
) -> Option<String> {
    if lifecycle.state == "running" {
        Some(format!(
            "check again: djinn session status {}",
            session_dir.display()
        ))
    } else if manifest.and_then(|manifest| manifest.kind.as_deref()) == Some("promotion")
        && candidates.is_some_and(|candidates| candidates.candidate_count > 0)
    {
        Some(format!(
            "review candidates: djinn session accept {} --dry-run",
            session_dir.display()
        ))
    } else if lifecycle.state == "failed" {
        if matches!(
            lifecycle.reason.as_deref(),
            Some("background_worker_stale" | "background_worker_unresponsive")
        ) {
            Some(format!(
                "inspect background log/transcript, then stop or rerun foreground: djinn session run {} --fg",
                session_dir.display()
            ))
        } else {
            Some("inspect the failure note, edit request.md or context, then run again".to_string())
        }
    } else if request_exists && turn_count == 0 {
        Some(format!(
            "run request.md: djinn session run {}",
            session_dir.display()
        ))
    } else if turn_count > 0 {
        Some(format!(
            "open latest summary: djinn session open {} summary",
            session_dir.display()
        ))
    } else {
        None
    }
}

pub(crate) fn session_status_repo(
    session_dir: &Path,
    manifest: &FolderSessionManifest,
) -> Option<SessionStatusRepoReport> {
    if manifest.repo_path.is_none() && manifest.repo_link.is_none() {
        return None;
    }
    let link_path = manifest.repo_link.as_ref().map(PathBuf::from).map(|link| {
        if link.is_absolute() {
            link
        } else {
            session_dir.join(link)
        }
    });
    let (link_exists, link_is_symlink, link_target, link_broken) = link_path
        .as_ref()
        .map(|link| match fs::symlink_metadata(link) {
            Ok(metadata) => {
                let is_symlink = metadata.file_type().is_symlink();
                let target = fs::read_link(link)
                    .ok()
                    .map(|target| target.display().to_string());
                let broken = is_symlink && fs::metadata(link).is_err();
                (true, is_symlink, target, broken)
            }
            Err(_) => (false, false, None, false),
        })
        .unwrap_or((false, false, None, false));
    Some(SessionStatusRepoReport {
        path: manifest.repo_path.clone(),
        link: link_path.map(|path| path.display().to_string()),
        link_exists,
        link_is_symlink,
        link_target,
        link_broken,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_session_meta::append_agent_session_lifecycle_event;
    use crate::background_run::{
        latest_background_session_run_status, write_background_session_run_marker,
    };
    use crate::session_init::create_dir_symlink;
    use crate::session_list::list_folder_sessions_in_root;
    use crate::session_native::relocate_agent_session_into_folder;
    use crate::session_projection::project_agent_session_dir;
    use crate::session_watch::{format_session_watch_snapshot, session_watch};
    use crate::{upsert_toml_root_string, SessionWatchArgs};
    use djinn_memory::{
        AgentSessionEvent, AgentSessionEventKind, AgentSessionExecutionMode,
        AgentSessionLifecycleState, AgentSessionMeta, AgentSessionStore, JsonlAgentSessionStore,
    };

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
    fn folder_session_status_reports_manifest_files_turns_and_context_skips() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-status-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let context = dir.join("context");
        let repo = root.join("repo");
        fs::create_dir_all(&context).unwrap();
        fs::create_dir_all(dir.join("turns/turn-1")).unwrap();
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            dir.join("djinn.toml"),
            format!(
                "session_id = \"agt_missing\"\nprofile = \"work\"\nmodel = \"repo-model\"\nworkspace = \"{}\"\n\n[context.repo]\npath = \"{}\"\nlink = \"context/repo\"\n",
                repo.display(),
                repo.display()
            ),
        )
        .unwrap();
        fs::write(dir.join("request.md"), "request\n").unwrap();
        fs::write(dir.join("summary.md"), "summary\n").unwrap();
        fs::write(context.join("notes.md"), "note\n").unwrap();
        fs::write(context.join("data.bin"), "binary-ish\n").unwrap();
        fs::write(dir.join("turns/turn-1/request.md"), "turn request\n").unwrap();
        fs::create_dir_all(dir.join("outputs/candidates")).unwrap();
        fs::write(
            dir.join("outputs/candidates/memory-001.toml"),
            "type = \"memory\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("outputs/candidates/memory-002.toml"),
            "type = \"memory\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("outputs/candidate-index.toml"),
            "candidate_count = 2\n",
        )
        .unwrap();
        fs::write(
            dir.join("outputs/candidate-status.toml"),
            "[[events]]\ncandidate = \"memory-001\"\nstatus = \"accepted\"\n",
        )
        .unwrap();
        create_dir_symlink(&repo, &context.join("repo")).unwrap();

        let report = folder_session_status(&dir).unwrap();
        let text = format_folder_session_status(&report);

        assert!(report.manifest_exists);
        assert_eq!(report.session_id.as_deref(), Some("agt_missing"));
        assert!(!report.native_session_exists);
        assert_eq!(report.profile.as_deref(), Some("work"));
        assert_eq!(report.model.as_deref(), Some("repo-model"));
        assert!(report.files.request_md);
        assert!(report.files.summary_md);
        assert!(report.files.context_dir);
        assert!(report.files.turns_dir);
        assert!(!report.files.events_jsonl);
        assert_eq!(report.turn_count, 1);
        assert_eq!(report.event_count, 0);
        assert_eq!(report.lifecycle.state, "not_started");
        assert_eq!(report.latest_turn.as_ref().unwrap().id, "turn-1");
        assert!(!report.latest_turn.as_ref().unwrap().has_response);
        assert_eq!(report.candidates.as_ref().unwrap().candidate_count, 2);
        assert_eq!(report.candidates.as_ref().unwrap().accepted_count, 1);
        assert_eq!(report.candidates.as_ref().unwrap().pending_count, 1);
        assert_eq!(report.candidates.as_ref().unwrap().entries.len(), 2);
        assert_eq!(
            report.candidates.as_ref().unwrap().entries[0].id,
            "memory-001"
        );
        assert_eq!(
            report.candidates.as_ref().unwrap().entries[0].status,
            "accepted"
        );
        assert_eq!(report.context_ingestible_count, 1);
        let repo_status = report.repo.as_ref().unwrap();
        assert!(repo_status.link_exists);
        assert!(repo_status.link_is_symlink);
        assert!(!repo_status.link_broken);
        assert!(text.contains("Skipped context:"));
        assert!(text.contains("State: not_started"));
        assert!(text.contains("events.jsonl: no"));
        assert!(text.contains("Events: 0"));
        assert!(text.contains("Latest turn:"));
        assert!(text.contains("Candidates:"));
        assert!(text.contains("2 total, 1 accepted, 0 denied, 1 pending"));
        assert!(text.contains("memory-001 [memory] accepted"));
        assert!(text.contains("memory-002 [memory] pending"));
        assert!(text.contains("context/data.bin: unsupported file type"));
        assert!(text.contains("context/repo: symlink directory not ingested"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_status_reports_lifecycle_and_latest_response() {
        let store = temp_agent_store("folder-status-lifecycle");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Lifecycle session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        append_agent_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Completed,
            AgentSessionExecutionMode::Foreground,
            "test completed",
            Some("all done".to_string()),
        )
        .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "request".to_string(),
                }),
            )
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "response".to_string(),
                }),
            )
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::ToolCall {
                    id: "tool-1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"path": "summary.md"}),
                }),
            )
            .unwrap();
        let root = std::env::temp_dir().join(format!(
            "djinn-session-status-lifecycle-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");
        let session = store.load_session(&id).unwrap();
        project_agent_session_dir(&session_dir, &session, "request", "response").unwrap();
        relocate_agent_session_into_folder(&store, &session_dir, &id).unwrap();

        let report = folder_session_status(&session_dir).unwrap();
        let text = format_folder_session_status(&report);

        assert!(report.native_session_exists);
        assert_eq!(report.lifecycle.state, "completed");
        assert_eq!(report.lifecycle.mode.as_deref(), Some("foreground"));
        assert_eq!(report.lifecycle.reason.as_deref(), Some("test completed"));
        assert_eq!(report.lifecycle.note.as_deref(), Some("all done"));
        assert!(report.files.events_jsonl);
        assert_eq!(report.event_count, 4);
        let latest = report.latest_turn.as_ref().unwrap();
        assert!(latest.has_response);
        assert!(latest
            .response_path
            .as_deref()
            .unwrap()
            .ends_with("events.jsonl"));
        assert!(report
            .next_action
            .as_deref()
            .unwrap()
            .contains("open latest summary"));
        assert!(text.contains("State: completed"));
        assert!(text.contains("Mode: foreground"));
        assert!(text.contains("State note: all done"));
        assert!(text.contains("events.jsonl: yes"));
        assert!(text.contains("Events: 4"));
        assert!(text.contains("summary.md"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_status_and_list_prefer_events_jsonl_without_turns() {
        let root = std::env::temp_dir().join(format!(
            "djinn-event-native-status-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("event-native");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            "title = \"Event Native\"\ncreated_at = \"2026-08-01T12:00:00Z\"\n",
        )
        .unwrap();
        fs::write(session_dir.join("summary.md"), "stale summary\n").unwrap();
        fs::write(
            session_dir.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"first question\"}\n{\"type\":\"assistant_message\",\"content\":\"first answer\"}\n{\"type\":\"user_message\",\"content\":\"latest question\"}\n{\"type\":\"assistant_message\",\"content\":\"latest event answer\"}\n",
        )
        .unwrap();

        let status = folder_session_status(&session_dir).unwrap();
        assert!(status.files.events_jsonl);
        assert!(!status.files.turns_dir);
        assert_eq!(status.event_count, 4);
        assert_eq!(status.turn_count, 2);
        let latest = status.latest_turn.as_ref().unwrap();
        assert_eq!(latest.id, "event-turn-0002");
        assert!(latest
            .request_path
            .as_deref()
            .unwrap()
            .ends_with("events.jsonl"));
        assert!(latest
            .response_path
            .as_deref()
            .unwrap()
            .ends_with("events.jsonl"));

        let list = list_folder_sessions_in_root(&root, None).unwrap();
        assert_eq!(list.sessions.len(), 1);
        assert_eq!(list.sessions[0].turn_count, 2);
        assert_eq!(
            list.sessions[0].summary_preview.as_deref(),
            Some("latest event answer")
        );
        assert_eq!(
            list.sessions[0].latest_turn.as_ref().unwrap().id,
            "event-turn-0002"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_status_marks_dead_background_worker_as_failed() {
        let store = temp_agent_store("folder-status-stale-background");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Stale background session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        append_agent_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Running,
            AgentSessionExecutionMode::Background,
            "djinn session run started",
            None,
        )
        .unwrap();
        let root = std::env::temp_dir().join(format!(
            "djinn-session-status-stale-background-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");
        let session = store.load_session(&id).unwrap();
        project_agent_session_dir(&session_dir, &session, "request", "in progress").unwrap();
        relocate_agent_session_into_folder(&store, &session_dir, &id).unwrap();
        let log_path = session_dir.join(".djinn/runs/session-run-stale.log");
        fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        fs::write(&log_path, "worker started\n").unwrap();
        write_background_session_run_marker(
            &session_dir,
            &log_path,
            4_294_967_295,
            "djinn session run /tmp/session --background-worker",
            Some(id.as_str()),
        )
        .unwrap();

        let report = folder_session_status(&session_dir).unwrap();
        let rendered = format_session_watch_snapshot(&report);
        let run = latest_background_session_run_status(&session_dir).unwrap();

        assert_eq!(run.run_id, "session-run-stale");
        assert_eq!(run.native_session_id.as_deref(), Some(id.as_str()));
        assert_eq!(
            run.command.as_deref(),
            Some("djinn session run /tmp/session --background-worker")
        );
        assert_eq!(report.lifecycle.state, "failed");
        assert_eq!(report.lifecycle.mode.as_deref(), Some("background"));
        assert_eq!(
            report.lifecycle.reason.as_deref(),
            Some("background_worker_stale")
        );
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("no live process found"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("worker started"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("session-run-stale"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains(id.as_str()));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("Last transcript event"));
        assert!(format_agent_session_event_summary(&AgentSessionEvent::new(
            AgentSessionEventKind::ToolCall {
                id: "tool-1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"path": "summary.md"}),
            }
        ))
        .contains("tool_call id=tool-1 name=read"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("lifecycle state=running"));
        let marker = fs::read_to_string(log_path.with_extension("toml")).unwrap();
        assert!(marker.contains("recovery_reason = \"background_worker_stale\""));
        assert!(marker.contains("recovery_observed_at ="));
        assert!(marker.contains("last_observed_event ="));
        assert!(marker.contains("lifecycle state=running"));
        assert!(report
            .next_action
            .as_deref()
            .unwrap()
            .contains("djinn session run"));
        assert!(report.next_action.as_deref().unwrap().contains("--fg"));
        assert!(rendered.contains("State: failed (background)"));
        assert!(rendered.contains("Reason: background_worker_stale"));
        assert!(rendered.contains("Next:"));
        session_watch(SessionWatchArgs {
            dir: session_dir.clone(),
            interval_ms: 1,
            timeout_seconds: Some(1),
            json: false,
        })
        .unwrap();

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_status_marks_stale_heartbeat_worker_as_unresponsive() {
        let store = temp_agent_store("folder-status-unresponsive-background");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Unresponsive background session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        append_agent_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Running,
            AgentSessionExecutionMode::Background,
            "djinn session run started",
            None,
        )
        .unwrap();
        let root = std::env::temp_dir().join(format!(
            "djinn-session-status-unresponsive-background-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");
        let session = store.load_session(&id).unwrap();
        project_agent_session_dir(&session_dir, &session, "request", "in progress").unwrap();
        relocate_agent_session_into_folder(&store, &session_dir, &id).unwrap();
        let log_path = session_dir.join(".djinn/runs/session-run-unresponsive.log");
        fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        fs::write(&log_path, "waiting for model\n").unwrap();
        write_background_session_run_marker(
            &session_dir,
            &log_path,
            std::process::id(),
            "djinn session run /tmp/session --background-worker",
            Some(id.as_str()),
        )
        .unwrap();
        let marker_path = log_path.with_extension("toml");
        let marker = fs::read_to_string(&marker_path).unwrap();
        let marker =
            upsert_toml_root_string(&marker, "heartbeat_at", "2000-01-01T00:00:00Z").unwrap();
        let marker = upsert_toml_root_string(&marker, "heartbeat_phase", "model_call").unwrap();
        fs::write(&marker_path, marker).unwrap();

        let report = folder_session_status(&session_dir).unwrap();
        let run = latest_background_session_run_status(&session_dir).unwrap();

        assert!(run.alive);
        assert!(run.heartbeat_age_seconds.unwrap() >= crate::BACKGROUND_RUN_UNRESPONSIVE_SECONDS);
        assert_eq!(report.lifecycle.state, "failed");
        assert_eq!(
            report.lifecycle.reason.as_deref(),
            Some("background_worker_unresponsive")
        );
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("still alive but appears unresponsive"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("Phase: model_call"));
        assert!(report.next_action.as_deref().unwrap().contains("--fg"));
        let marker = fs::read_to_string(marker_path).unwrap();
        assert!(marker.contains("recovery_reason = \"background_worker_unresponsive\""));
        assert!(marker.contains("recovery_observed_at ="));
        assert!(marker.contains("last_observed_event ="));

        let _ = fs::remove_dir_all(&root);
    }
}
