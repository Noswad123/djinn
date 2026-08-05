use std::path::Path;

use crate::editor::default_editor;
use crate::session_artifact::{
    fallback_folder_session_open_target, resolve_folder_session_open_target, SessionOpenTarget,
};
use crate::shell::shell_quote;

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
