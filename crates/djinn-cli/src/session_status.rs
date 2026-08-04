use serde::Serialize;

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
            crate::format_session_candidate_status(candidates)
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
                lines.push(format!(
                    "    - {}",
                    crate::format_session_candidate_entry(entry)
                ));
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
