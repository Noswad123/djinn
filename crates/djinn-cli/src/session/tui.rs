use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::buddy::session_chat;
use crate::cli_args::{
    SessionChatArgs, SessionContextDiscoverArgs, SessionDecisionArgs, SessionOpenArgs,
    SessionRunArgs, SessionValidateCandidatesArgs, SessionWatchArgs,
};
use crate::commands::agent_ask::session_run;
use crate::promotion::decision::{session_decide, SessionDecisionAction};
use crate::promotion::validation::session_validate_candidates;
use crate::runtime::background_run::latest_background_session_run_status;
use crate::session::artifact::session_open;
use crate::session::artifact::{
    fallback_folder_session_open_target, resolve_folder_session_open_target, SessionOpenTarget,
};
use crate::session::context::session_context_discover;
use crate::session::events::latest_event_rebuild_backup_path;
use crate::session::manifest::read_folder_session_manifest;
use crate::session::reference::{
    folder_session_display_name, resolve_existing_folder_session_reference,
};
use crate::session::status::{
    folder_session_status, format_session_candidate_entry, format_session_candidate_status,
    latest_promotion_generation_response_path, SessionStatusCandidateEntry,
};
use crate::session::watch::session_watch;
use crate::util::editor::{default_editor, open_editor_path};
use crate::util::shell::shell_quote;
use crate::DEFAULT_AGENT_MAX_TOOL_ROUNDS;

pub(crate) fn run_folder_session_tui(dir: PathBuf, editor: Option<String>) -> Result<()> {
    let session_dir = resolve_existing_folder_session_reference(&dir)?.session_dir;
    let mut tui = djinn_tui::TuiSession::enter()?;
    let mut message = None::<String>;
    loop {
        let action = tui.run_folder_session_status(|| {
            let mut view = folder_session_status_tui_view(&session_dir)?;
            view.message = message.clone();
            Ok(view)
        })?;
        let Some(action) = action else {
            tui.finish()?;
            return Ok(());
        };
        let action_message =
            folder_session_action_message(&action, &session_dir, editor.as_deref());
        tui.suspend()?;
        println!("{action_message}");
        io::stdout().flush()?;
        let action_result =
            handle_folder_session_tui_action(action, session_dir.clone(), editor.as_deref());
        tui.resume()?;
        message = Some(match action_result {
            Ok(()) => action_message,
            Err(err) => format!("Error: {err:#}"),
        });
    }
}

fn handle_folder_session_tui_action(
    action: djinn_tui::FolderSessionAction,
    session_dir: PathBuf,
    editor: Option<&str>,
) -> Result<()> {
    match action {
        djinn_tui::FolderSessionAction::Run => session_run(SessionRunArgs {
            dir: session_dir,
            foreground: false,
            background_worker: false,
            profile: None,
            agent: None,
            model: None,
            api_key: None,
            base_url: None,
            max_tool_rounds: DEFAULT_AGENT_MAX_TOOL_ROUNDS,
            dry_run: false,
            json: false,
            print: false,
            open: false,
        }),
        djinn_tui::FolderSessionAction::Buddy => session_chat(SessionChatArgs {
            dir: session_dir,
            buddy_bin: None,
            buddy_args: Vec::new(),
            capture_request: false,
            dry_run: false,
            json: false,
        }),
        djinn_tui::FolderSessionAction::Watch => session_watch(SessionWatchArgs {
            dir: session_dir,
            interval_ms: 1000,
            timeout_seconds: None,
            json: false,
        }),
        djinn_tui::FolderSessionAction::OpenSummary => session_open(SessionOpenArgs {
            dir: session_dir,
            target: SessionOpenTarget::Summary,
            editor: editor.map(str::to_string),
        }),
        djinn_tui::FolderSessionAction::EditRequest => session_open(SessionOpenArgs {
            dir: session_dir,
            target: SessionOpenTarget::Request,
            editor: editor.map(str::to_string),
        }),
        djinn_tui::FolderSessionAction::OpenContext => session_open(SessionOpenArgs {
            dir: session_dir,
            target: SessionOpenTarget::Context,
            editor: editor.map(str::to_string),
        }),
        djinn_tui::FolderSessionAction::DiscoverContext => {
            session_context_discover(SessionContextDiscoverArgs {
                session: session_dir,
                dry_run: false,
                json: false,
            })
        }
        djinn_tui::FolderSessionAction::ValidateCandidates => {
            session_validate_candidates(SessionValidateCandidatesArgs {
                dir: session_dir,
                candidate: None,
                json: false,
            })
        }
        djinn_tui::FolderSessionAction::ValidateCandidate(candidate) => {
            session_validate_candidates(SessionValidateCandidatesArgs {
                dir: session_dir,
                candidate: Some(candidate),
                json: false,
            })
        }
        djinn_tui::FolderSessionAction::ShowPatternExportCommand(_) => Ok(()),
        djinn_tui::FolderSessionAction::ShowValidateEventsCommand
        | djinn_tui::FolderSessionAction::ShowEventsCommand
        | djinn_tui::FolderSessionAction::ShowEventsWriteCommand
        | djinn_tui::FolderSessionAction::ShowEventsRestoreCommand(_) => Ok(()),
        djinn_tui::FolderSessionAction::AcceptCandidate(candidate) => session_decide(
            SessionDecisionArgs {
                dir: session_dir,
                candidate: Some(candidate),
                dry_run: false,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
        ),
        djinn_tui::FolderSessionAction::AcceptCandidateAndSyncMindweaver(candidate) => {
            session_decide(
                SessionDecisionArgs {
                    dir: session_dir,
                    candidate: Some(candidate),
                    dry_run: false,
                    sync_mindweaver: true,
                    json: false,
                },
                SessionDecisionAction::Accept,
            )
        }
        djinn_tui::FolderSessionAction::DenyCandidate(candidate) => session_decide(
            SessionDecisionArgs {
                dir: session_dir,
                candidate: Some(candidate),
                dry_run: false,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Deny,
        ),
        djinn_tui::FolderSessionAction::OpenCandidate(path) => {
            open_editor_path(Path::new(&path), editor.map(str::to_string))
        }
        djinn_tui::FolderSessionAction::OpenPath(path) => {
            open_editor_path(Path::new(&path), editor.map(str::to_string))
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn folder_session_status_tui_view_projects_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-tui-view-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("bap-questions");
        let turn = session_dir.join("turns/turn-1");
        fs::create_dir_all(&turn).unwrap();
        fs::write(session_dir.join("djinn.toml"), "title = \"BAP\"\n").unwrap();
        fs::write(session_dir.join("request.md"), "question\n").unwrap();
        fs::write(session_dir.join("summary.md"), "answer\n").unwrap();
        fs::write(turn.join("request.md"), "question\n").unwrap();
        fs::write(turn.join("response.md"), "answer\n").unwrap();
        fs::write(
            session_dir.join("events.jsonl"),
            "{\"type\":\"user_message\"}\n{\"type\":\"assistant_message\"}\n",
        )
        .unwrap();
        fs::create_dir_all(session_dir.join("outputs/candidates")).unwrap();
        fs::create_dir_all(session_dir.join("outputs/generation")).unwrap();
        fs::create_dir_all(session_dir.join("context")).unwrap();
        fs::create_dir_all(session_dir.join(".djinn/runs")).unwrap();
        fs::write(
            session_dir.join("outputs/generation/1-response.md"),
            "model response\n",
        )
        .unwrap();
        fs::write(session_dir.join("context/source-packet.md"), "packet\n").unwrap();
        fs::write(
            session_dir.join("context/sources.toml"),
            "source_count = 0\n",
        )
        .unwrap();
        fs::write(
            session_dir.join(".djinn/runs/session-run-test.log"),
            "log\n",
        )
        .unwrap();
        fs::write(
            session_dir.join(".djinn/runs/session-run-test.toml"),
            format!(
                "version = 1\nstarted_at = \"2026-07-30T12:00:00Z\"\npid = 4294967295\nlog_path = \"{}\"\n",
                session_dir.join(".djinn/runs/session-run-test.log").display()
            ),
        )
        .unwrap();
        fs::write(
            session_dir.join("outputs/candidates/todo-001.toml"),
            "type = \"todo\"\n",
        )
        .unwrap();
        let event_backup = session_dir.join(".djinn/backups/events-rebuild-test");
        fs::create_dir_all(&event_backup).unwrap();
        fs::write(event_backup.join("backup.toml"), "source = \"test\"\n").unwrap();

        let view = folder_session_status_tui_view(&session_dir).unwrap();

        assert_eq!(view.title, "bap-questions");
        assert_eq!(view.state, "not_started");
        assert_eq!(view.turn_count, 1);
        assert_eq!(view.event_count, 2);
        assert_eq!(
            view.candidate_status.as_deref(),
            Some("1 total, 0 accepted, 0 denied, 1 pending")
        );
        assert_eq!(view.candidate_details, vec!["todo-001 [todo] pending"]);
        assert_eq!(view.candidate_entries.len(), 1);
        assert_eq!(view.candidate_entries[0].id, "todo-001");
        assert!(view.candidate_entries[0].path.ends_with("todo-001.toml"));
        assert!(view.message.is_none());
        assert!(view
            .latest_generation_response_path
            .as_deref()
            .unwrap()
            .ends_with("1-response.md"));
        assert!(view
            .latest_run_log_path
            .as_deref()
            .unwrap()
            .ends_with("session-run-test.log"));
        assert!(view
            .events_path
            .as_deref()
            .unwrap()
            .ends_with("events.jsonl"));
        assert!(view
            .latest_event_rebuild_backup_path
            .as_deref()
            .unwrap()
            .ends_with("events-rebuild-test"));
        assert!(view
            .candidates_dir
            .as_deref()
            .unwrap()
            .ends_with("candidates"));
        assert!(view
            .source_packet_path
            .as_deref()
            .unwrap()
            .ends_with("source-packet.md"));
        assert!(view
            .sources_manifest_path
            .as_deref()
            .unwrap()
            .ends_with("sources.toml"));
        assert!(view
            .request_path
            .as_deref()
            .unwrap()
            .ends_with("request.md"));
        assert!(view
            .summary_path
            .as_deref()
            .unwrap()
            .ends_with("summary.md"));
        assert!(view
            .response_path
            .as_deref()
            .unwrap()
            .ends_with("response.md"));
        assert_eq!(
            folder_session_action_message(&djinn_tui::FolderSessionAction::Run, &session_dir, None),
            format!("Run command: djinn session run '{}'", session_dir.display())
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::Buddy,
                &session_dir,
                None
            ),
            format!(
                "Buddy chat command: djinn session chat '{}'",
                session_dir.display()
            )
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::OpenSummary,
                &session_dir,
                None,
            ),
            format!(
                "Open summary command: {}",
                editor_open_command_hint(&session_dir.join("summary.md"), None)
            )
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::EditRequest,
                &session_dir,
                Some("code --wait"),
            ),
            format!(
                "Edit request command: code --wait '{}'",
                session_dir.join("request.md").display()
            )
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::AcceptCandidate("todo-001".to_string()),
                &session_dir,
                None,
            ),
            format!(
                "Accept candidate command: djinn session accept '{}' 'todo-001'",
                session_dir.display()
            )
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::OpenCandidate(
                    view.candidate_entries[0].path.clone()
                ),
                &session_dir,
                None,
            ),
            format!(
                "Open candidate command: {}",
                editor_open_command_hint(Path::new(&view.candidate_entries[0].path), None)
            )
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::ShowPatternExportCommand(Some(
                    "pattern-001".to_string(),
                )),
                &session_dir,
                None,
            ),
            format!(
                "Pattern export command: djinn session export-pattern '{}' 'pattern-001' --to <notes.md>",
                session_dir.display()
            )
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::ShowValidateEventsCommand,
                &session_dir,
                None,
            ),
            format!(
                "Event validation command: djinn session validate-events '{}'",
                session_dir.display()
            )
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::ShowEventsWriteCommand,
                &session_dir,
                None,
            ),
            format!(
                "Event rebuild command: djinn session events '{}' --write",
                session_dir.display()
            )
        );
        assert_eq!(
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::ShowEventsRestoreCommand(
                    "events-rebuild-test".to_string(),
                ),
                &session_dir,
                None,
            ),
            format!(
                "Event restore command: djinn session events '{}' --restore 'events-rebuild-test' --write",
                session_dir.display()
            )
        );

        let _ = fs::remove_dir_all(&root);
    }
}
