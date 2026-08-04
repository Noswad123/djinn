use serde::Serialize;

use crate::{SessionStatusCandidateReport, SessionStatusLifecycleReport, SessionStatusTurnReport};

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
