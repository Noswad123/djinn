use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use djinn_memory::AgentSession;
use serde::Serialize;

use crate::session_status::{
    session_status_candidates, session_status_lifecycle, session_status_next_action,
    session_status_turn_report, SessionStatusCandidateReport, SessionStatusLifecycleReport,
    SessionStatusTurnReport,
};
use crate::{
    default_folder_session_root, folder_session_display_name, folder_session_reference_name,
    load_folder_native_agent_session, non_empty_string, read_buddy_runtime_state,
    read_folder_session_event_turns, read_folder_session_manifest, read_folder_session_turns,
    validate_folder_session_events, FolderSessionTurnDigest,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionLsReport {
    pub(crate) root: String,
    pub(crate) sessions: Vec<FolderSessionSummary>,
    pub(crate) groups: Vec<FolderSessionGroup>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct FolderSessionGroup {
    pub(crate) repo: String,
    pub(crate) sessions: Vec<FolderSessionSummary>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct FolderSessionSummary {
    pub(crate) name: String,
    pub(crate) display_name: String,
    pub(crate) reference_name: String,
    pub(crate) path: String,
    pub(crate) manifest_exists: bool,
    pub(crate) session_id: Option<String>,
    pub(crate) native_session_exists: bool,
    pub(crate) lifecycle: SessionStatusLifecycleReport,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) workspace: Option<String>,
    pub(crate) repo_path: Option<String>,
    pub(crate) request_md: bool,
    pub(crate) summary_md: bool,
    pub(crate) summary_preview: Option<String>,
    pub(crate) turn_count: usize,
    pub(crate) event_health: FolderSessionEventHealth,
    pub(crate) buddy: Option<FolderSessionBuddySummary>,
    pub(crate) latest_turn: Option<SessionStatusTurnReport>,
    pub(crate) candidates: Option<SessionStatusCandidateReport>,
    pub(crate) next_action: Option<String>,
    pub(crate) modified_at: Option<String>,
    pub(crate) modified_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct FolderSessionBuddySummary {
    pub(crate) buddy_session: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) last_run_at: Option<String>,
    pub(crate) runtime_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct FolderSessionEventHealth {
    pub(crate) ready: bool,
    pub(crate) events_exists: bool,
    pub(crate) event_count: usize,
    pub(crate) event_turn_count: usize,
    pub(crate) issue_count: usize,
    pub(crate) issue_codes: Vec<String>,
}

pub(crate) fn list_cache_folder_sessions(limit: Option<usize>) -> Result<SessionLsReport> {
    let root = default_folder_session_root();
    list_folder_sessions_in_root(&root, limit)
}

pub(crate) fn list_folder_sessions_in_root(
    root: &Path,
    limit: Option<usize>,
) -> Result<SessionLsReport> {
    let mut summaries = Vec::new();
    if root.is_dir() {
        let mut entries = fs::read_dir(root)
            .with_context(|| format!("reading folder session root {}", root.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            summaries.push(folder_session_summary(&path)?);
        }
        summaries.sort_by(folder_session_summary_order);
        if let Some(limit) = limit {
            summaries.truncate(limit);
        }
    }
    let groups = group_folder_session_summaries(&summaries);
    Ok(SessionLsReport {
        root: root.display().to_string(),
        sessions: summaries,
        groups,
    })
}

fn folder_session_summary(path: &Path) -> Result<FolderSessionSummary> {
    let manifest = read_folder_session_manifest(path)?;
    let session_id = manifest
        .as_ref()
        .and_then(|manifest| manifest.session_id.clone());
    let native_session = session_id
        .as_ref()
        .and_then(|id| load_folder_native_agent_session(path, id));
    let native_session_exists = native_session.is_some();
    let created_at = native_session
        .as_ref()
        .and_then(|session| non_empty_string(&session.meta.created_at))
        .or_else(|| {
            manifest
                .as_ref()
                .and_then(|manifest| non_empty_string(manifest.created_at.as_deref().unwrap_or("")))
        });
    let updated_at = native_session
        .as_ref()
        .and_then(latest_agent_session_event_created_at)
        .or_else(|| created_at.clone())
        .or_else(|| folder_session_modified_at(path));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("session")
        .to_string();
    let turns = read_folder_session_turns(&path.join("turns"))?;
    let event_turns = read_folder_session_event_turns(path)?;
    let turn_count = if event_turns.is_empty() {
        turns.len()
    } else {
        event_turns.len()
    };
    let event_health = folder_session_event_health(path)?;
    let buddy = folder_session_buddy_summary(path)?;
    let latest_turn = event_turns
        .last()
        .map(session_status_turn_report)
        .or_else(|| turns.last().map(session_status_turn_report));
    let request_md = path.join("request.md").exists();
    let candidates = session_status_candidates(path)?;
    let lifecycle = session_status_lifecycle(
        path,
        manifest.as_ref(),
        native_session.as_ref(),
        candidates.as_ref(),
    );
    let next_action = session_status_next_action(
        path,
        manifest.as_ref(),
        request_md,
        turn_count,
        &lifecycle,
        candidates.as_ref(),
    );
    Ok(FolderSessionSummary {
        display_name: folder_session_display_name(&name),
        reference_name: folder_session_reference_name(&name),
        name,
        path: path.display().to_string(),
        manifest_exists: path.join("djinn.toml").exists(),
        session_id: session_id.map(|id| id.to_string()),
        native_session_exists,
        lifecycle,
        created_at,
        updated_at,
        workspace: manifest
            .as_ref()
            .and_then(|manifest| manifest.workspace.clone()),
        repo_path: manifest
            .as_ref()
            .and_then(|manifest| manifest.repo_path.clone()),
        request_md,
        summary_md: path.join("summary.md").exists(),
        summary_preview: folder_session_summary_preview(path, &event_turns),
        turn_count,
        event_health,
        buddy,
        latest_turn,
        candidates,
        next_action,
        modified_at: folder_session_modified_at(path),
        modified_at_ms: folder_session_modified_at_ms(path),
    })
}

fn folder_session_buddy_summary(path: &Path) -> Result<Option<FolderSessionBuddySummary>> {
    let runtime_path = path.join("runtime/buddy.json");
    let Some(runtime) = read_buddy_runtime_state(&runtime_path)? else {
        return Ok(None);
    };
    Ok(Some(FolderSessionBuddySummary {
        buddy_session: runtime.buddy_session,
        command: runtime.command,
        last_run_at: runtime.last_run_at,
        runtime_path: runtime_path.display().to_string(),
    }))
}

fn folder_session_event_health(path: &Path) -> Result<FolderSessionEventHealth> {
    let report = validate_folder_session_events(path)?;
    Ok(FolderSessionEventHealth {
        ready: report.all_valid,
        events_exists: report.events_exists,
        event_count: report.event_count,
        event_turn_count: report.event_turn_count,
        issue_count: report.issues.len(),
        issue_codes: report.issues.into_iter().map(|issue| issue.code).collect(),
    })
}

pub(crate) fn folder_session_event_health_label(health: &FolderSessionEventHealth) -> String {
    if health.ready {
        format!("ready:{}/{}", health.event_turn_count, health.event_count)
    } else if !health.events_exists {
        "missing".to_string()
    } else if let Some(code) = health.issue_codes.first() {
        if health.issue_count > 1 {
            format!("{code}+{}", health.issue_count - 1)
        } else {
            code.clone()
        }
    } else {
        "not_ready".to_string()
    }
}

fn folder_session_summary_order(
    left: &FolderSessionSummary,
    right: &FolderSessionSummary,
) -> std::cmp::Ordering {
    folder_session_repo_sort_key(left)
        .cmp(&folder_session_repo_sort_key(right))
        .then_with(|| {
            folder_session_recency_sort_key(right).cmp(&folder_session_recency_sort_key(left))
        })
        .then_with(|| left.name.cmp(&right.name))
}

fn folder_session_repo_sort_key(session: &FolderSessionSummary) -> String {
    session
        .repo_path
        .as_deref()
        .unwrap_or("~")
        .to_ascii_lowercase()
}

fn folder_session_recency_sort_key(session: &FolderSessionSummary) -> i64 {
    session
        .updated_at
        .as_deref()
        .and_then(parse_session_list_datetime_ms)
        .or(session.modified_at_ms)
        .unwrap_or(0)
}

fn folder_session_summary_preview(
    path: &Path,
    event_turns: &[FolderSessionTurnDigest],
) -> Option<String> {
    event_turns
        .last()
        .and_then(|turn| turn.response.as_deref())
        .and_then(first_non_empty_preview)
        .or_else(|| {
            fs::read_to_string(path.join("summary.md"))
                .ok()
                .and_then(|summary| first_non_empty_preview(&summary))
        })
}

fn first_non_empty_preview(value: &str) -> Option<String> {
    let preview = value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .chars()
        .take(80)
        .collect::<String>();
    Some(preview)
}

fn group_folder_session_summaries(sessions: &[FolderSessionSummary]) -> Vec<FolderSessionGroup> {
    let mut groups = Vec::<FolderSessionGroup>::new();
    for session in sessions {
        let repo = folder_session_repo_label(session);
        if let Some(group) = groups.last_mut().filter(|group| group.repo == repo) {
            group.sessions.push(session.clone());
        } else {
            groups.push(FolderSessionGroup {
                repo,
                sessions: vec![session.clone()],
            });
        }
    }
    groups
}

fn folder_session_repo_label(session: &FolderSessionSummary) -> String {
    session
        .repo_path
        .as_deref()
        .map(short_folder_session_path)
        .unwrap_or_else(|| "-".to_string())
}

fn latest_agent_session_event_created_at(session: &AgentSession) -> Option<String> {
    session
        .events
        .iter()
        .rev()
        .map(|event| event.created_at.trim().to_string())
        .find(|created_at| !created_at.is_empty())
}

fn folder_session_modified_at(path: &Path) -> Option<String> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(|modified| {
            let datetime: chrono::DateTime<chrono::Local> = modified.into();
            datetime.to_rfc3339()
        })
}

fn folder_session_modified_at_ms(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

pub(crate) fn parse_session_list_datetime_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|datetime| datetime.timestamp_millis())
}

fn short_folder_session_path(value: &str) -> String {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(value)
        .to_string()
}

pub(crate) fn format_folder_session_ls(report: &SessionLsReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Cache folder sessions: {}", report.root));
    if report.sessions.is_empty() {
        lines.push("No cache-backed folder sessions found.".to_string());
        lines.push(String::new());
        return lines.join("\n");
    }
    for (index, group) in report.groups.iter().enumerate() {
        if index > 0 {
            lines.push(String::new());
        }
        lines.push(format!("Repo: {}", group.repo));
        lines.push(format!(
            "  {:<20} {:<12} {:<34} {}",
            "UPDATED", "STATE", "BUDDY", "NAME"
        ));
        lines.push(format!("  {}", "-".repeat(86)));
        for session in &group.sessions {
            let updated = session
                .updated_at
                .as_deref()
                .or(session.modified_at.as_deref())
                .map(compact_session_list_datetime)
                .unwrap_or_else(|| "-".to_string());
            let summary = session.summary_preview.as_deref().unwrap_or("");
            let state = folder_session_summary_state_label(session);
            lines.push(format!(
                "  {:<20} {:<12} {:<34} {}",
                crate::truncate_table_cell(&updated, 20),
                crate::truncate_table_cell(&state, 12),
                folder_session_buddy_label(session.buddy.as_ref()),
                format!(
                    "{}{}",
                    session.reference_name,
                    if session.manifest_exists {
                        ""
                    } else {
                        " (no manifest)"
                    }
                ),
            ));
            if !summary.is_empty() {
                lines.push(format!("      summary: {summary}"));
            }
        }
    }
    lines.push(format!(
        "\nTotal: {} folder sessions",
        report.sessions.len()
    ));
    lines.push(String::new());
    lines.join("\n")
}

fn folder_session_buddy_label(buddy: Option<&FolderSessionBuddySummary>) -> String {
    buddy
        .and_then(|buddy| buddy.buddy_session.as_deref())
        .filter(|session| !session.trim().is_empty())
        .unwrap_or("-")
        .to_string()
}

fn folder_session_summary_state_label(session: &FolderSessionSummary) -> String {
    session
        .lifecycle
        .mode
        .as_deref()
        .map(|mode| format!("{}/{}", session.lifecycle.state, mode))
        .unwrap_or_else(|| session.lifecycle.state.clone())
}

pub(crate) fn compact_session_list_datetime(value: &str) -> String {
    value
        .split_once('.')
        .map(|(prefix, suffix)| {
            let timezone = if suffix.ends_with('Z') {
                "Z"
            } else {
                suffix
                    .rfind('+')
                    .or_else(|| suffix.rfind('-'))
                    .map(|idx| &suffix[idx..])
                    .unwrap_or("")
            };
            format!("{prefix}{timezone}")
        })
        .unwrap_or_else(|| value.to_string())
}
