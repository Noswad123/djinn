use std::path::Path;

use anyhow::Result;

use crate::editor::default_editor;
use crate::session_artifact::{
    fallback_folder_session_open_target, resolve_folder_session_open_target, SessionOpenTarget,
};
use crate::shell::shell_quote;
use crate::{
    folder_session_display_name, folder_session_status, format_session_candidate_entry,
    format_session_candidate_status, latest_background_session_run_status,
    latest_event_rebuild_backup_path, latest_promotion_generation_response_path,
    read_folder_session_manifest, SessionStatusCandidateEntry,
};

pub(crate) fn folder_session_action_message(
    action: &djinn_tui::FolderSessionAction,
    session_dir: &Path,
    editor: Option<&str>,
) -> String {
    match action {
        djinn_tui::FolderSessionAction::Run => format!(
            "Run command: djinn session run {}",
            shell_quote(&session_dir.display().to_string())
        ),
        djinn_tui::FolderSessionAction::Buddy => format!(
            "Buddy chat command: djinn session chat {}",
            shell_quote(&session_dir.display().to_string())
        ),
        djinn_tui::FolderSessionAction::Watch => format!(
            "Watch command: djinn session watch {}",
            shell_quote(&session_dir.display().to_string())
        ),
        djinn_tui::FolderSessionAction::OpenSummary => folder_session_open_action_message(
            session_dir,
            SessionOpenTarget::Summary,
            "Open summary command",
            editor,
        ),
        djinn_tui::FolderSessionAction::EditRequest => folder_session_open_action_message(
            session_dir,
            SessionOpenTarget::Request,
            "Edit request command",
            editor,
        ),
        djinn_tui::FolderSessionAction::OpenContext => folder_session_open_action_message(
            session_dir,
            SessionOpenTarget::Context,
            "Open context command",
            editor,
        ),
        djinn_tui::FolderSessionAction::DiscoverContext => format!(
            "Discover context command: djinn session context discover {}",
            shell_quote(&session_dir.display().to_string())
        ),
        djinn_tui::FolderSessionAction::ValidateCandidates => {
            format!(
                "Validate candidates command: djinn session validate-candidates {}",
                shell_quote(&session_dir.display().to_string())
            )
        }
        djinn_tui::FolderSessionAction::ValidateCandidate(candidate) => {
            format!(
                "Validate candidate command: djinn session validate-candidates {} {}",
                shell_quote(&session_dir.display().to_string()),
                shell_quote(candidate)
            )
        }
        djinn_tui::FolderSessionAction::ShowPatternExportCommand(candidate) => format!(
            "Pattern export command: {}",
            pattern_export_command_hint(session_dir, candidate.as_deref())
        ),
        djinn_tui::FolderSessionAction::ShowValidateEventsCommand => format!(
            "Event validation command: {}",
            session_events_command_hint(session_dir, "validate-events", false, None)
        ),
        djinn_tui::FolderSessionAction::ShowEventsCommand => format!(
            "Event projection command: {}",
            session_events_command_hint(session_dir, "events", false, None)
        ),
        djinn_tui::FolderSessionAction::ShowEventsWriteCommand => format!(
            "Event rebuild command: {}",
            session_events_command_hint(session_dir, "events", true, None)
        ),
        djinn_tui::FolderSessionAction::ShowEventsRestoreCommand(backup) => format!(
            "Event restore command: {}",
            session_events_command_hint(session_dir, "events", true, Some(backup))
        ),
        djinn_tui::FolderSessionAction::AcceptCandidate(candidate) => {
            format!(
                "Accept candidate command: djinn session accept {} {}",
                shell_quote(&session_dir.display().to_string()),
                shell_quote(candidate)
            )
        }
        djinn_tui::FolderSessionAction::AcceptCandidateAndSyncMindweaver(candidate) => {
            format!(
                "Accept candidate + MindWeaver sync command: djinn session accept {} {} --sync-mindweaver",
                shell_quote(&session_dir.display().to_string()),
                shell_quote(candidate)
            )
        }
        djinn_tui::FolderSessionAction::DenyCandidate(candidate) => {
            format!(
                "Deny candidate command: djinn session deny {} {}",
                shell_quote(&session_dir.display().to_string()),
                shell_quote(candidate)
            )
        }
        djinn_tui::FolderSessionAction::OpenCandidate(path) => format!(
            "Open candidate command: {}",
            editor_open_command_hint(Path::new(path), editor)
        ),
        djinn_tui::FolderSessionAction::OpenPath(path) => {
            format!(
                "Open path command: {}",
                editor_open_command_hint(Path::new(path), editor)
            )
        }
    }
}

fn folder_session_open_action_message(
    session_dir: &Path,
    target: SessionOpenTarget,
    label: &str,
    editor: Option<&str>,
) -> String {
    let path = resolve_folder_session_open_target(session_dir, target)
        .unwrap_or_else(|_| fallback_folder_session_open_target(session_dir, target));
    format!("{label}: {}", editor_open_command_hint(&path, editor))
}

pub(crate) fn editor_open_command_hint(path: &Path, editor: Option<&str>) -> String {
    let editor = editor.map(str::to_string).unwrap_or_else(default_editor);
    format!("{} {}", editor, shell_quote(&path.display().to_string()))
}

fn pattern_export_command_hint(session_dir: &Path, candidate: Option<&str>) -> String {
    let mut command = format!(
        "djinn session export-pattern {}",
        shell_quote(&session_dir.display().to_string())
    );
    if let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) {
        command.push(' ');
        command.push_str(&shell_quote(candidate));
    }
    command.push_str(" --to <notes.md>");
    command
}

fn session_events_command_hint(
    session_dir: &Path,
    subcommand: &str,
    write: bool,
    restore: Option<&str>,
) -> String {
    let mut command = format!(
        "djinn session {subcommand} {}",
        shell_quote(&session_dir.display().to_string())
    );
    if let Some(restore) = restore.map(str::trim).filter(|value| !value.is_empty()) {
        command.push_str(" --restore ");
        command.push_str(&shell_quote(restore));
    }
    if write {
        command.push_str(" --write");
    }
    command
}

pub(crate) fn folder_session_status_tui_view(
    session_dir: &Path,
) -> Result<djinn_tui::FolderSessionStatusView> {
    let report = folder_session_status(session_dir)?;
    let manifest = read_folder_session_manifest(session_dir)?;
    let session_path = std::path::PathBuf::from(&report.session_dir);
    let title = session_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(folder_session_display_name)
        .unwrap_or_else(|| report.session_dir.clone());
    Ok(djinn_tui::FolderSessionStatusView {
        title,
        state: report.lifecycle.state.clone(),
        mode: report.lifecycle.mode.clone(),
        promotion_type: manifest.and_then(|manifest| manifest.promotion_type),
        session_dir: report.session_dir.clone(),
        summary_path: report
            .files
            .summary_md
            .then(|| session_path.join("summary.md").display().to_string()),
        request_path: report
            .files
            .request_md
            .then(|| session_path.join("request.md").display().to_string()),
        response_path: report
            .latest_turn
            .as_ref()
            .and_then(|turn| turn.response_path.clone()),
        turn_count: report.turn_count,
        event_count: report.event_count,
        candidate_status: report
            .candidates
            .as_ref()
            .map(format_session_candidate_status),
        candidate_details: report
            .candidates
            .as_ref()
            .map(|candidates| {
                candidates
                    .entries
                    .iter()
                    .map(format_session_candidate_entry)
                    .collect()
            })
            .unwrap_or_default(),
        candidate_entries: report
            .candidates
            .as_ref()
            .map(|candidates| candidates.entries.iter().map(tui_candidate_row).collect())
            .unwrap_or_default(),
        next_action: report.next_action.clone(),
        note: report
            .lifecycle
            .note
            .clone()
            .or(report.lifecycle.reason.clone()),
        message: None,
        latest_generation_response_path: latest_promotion_generation_response_path(&session_path)
            .map(|path| path.display().to_string()),
        latest_run_log_path: latest_background_session_run_status(&session_path)
            .and_then(|run| run.log_path),
        events_path: session_path
            .join("events.jsonl")
            .exists()
            .then(|| session_path.join("events.jsonl").display().to_string()),
        latest_event_rebuild_backup_path: latest_event_rebuild_backup_path(&session_path)
            .map(|path| path.display().to_string()),
        candidates_dir: session_path
            .join("outputs")
            .join("candidates")
            .is_dir()
            .then(|| {
                session_path
                    .join("outputs")
                    .join("candidates")
                    .display()
                    .to_string()
            }),
        source_packet_path: session_path
            .join("context/source-packet.md")
            .exists()
            .then(|| {
                session_path
                    .join("context/source-packet.md")
                    .display()
                    .to_string()
            }),
        sources_manifest_path: session_path.join("context/sources.toml").exists().then(|| {
            session_path
                .join("context/sources.toml")
                .display()
                .to_string()
        }),
    })
}

pub(crate) fn tui_candidate_row(
    entry: &SessionStatusCandidateEntry,
) -> djinn_tui::PromotionCandidateRow {
    djinn_tui::PromotionCandidateRow {
        id: entry.id.clone(),
        candidate_type: entry.candidate_type.clone(),
        status: entry.status.clone(),
        path: entry.path.clone(),
        text: entry.text.clone(),
        rationale: entry.rationale.clone(),
        evidence: entry.evidence.clone(),
        destination: entry.destination.clone(),
        writeback_path: entry.writeback_path.clone(),
    }
}
