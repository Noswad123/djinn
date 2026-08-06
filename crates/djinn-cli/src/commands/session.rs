use anyhow::{anyhow, Result};

use crate::agent_ask_command::session_run;
use crate::buddy::session_chat;
use crate::buddy_consolidate::session_consolidate;
use crate::promotion_cleanup::session_cleanup;
use crate::promotion_decision::{session_decide, SessionDecisionAction};
use crate::promotion_export::session_export_pattern;
use crate::promotion_session::session_promote;
use crate::promotion_validation::session_validate_candidates;
use crate::session_artifact::{session_open, SessionOpenTarget};
use crate::session_compact::session_compact;
use crate::session_context::session_context;
use crate::session_events::{session_events, session_validate_events};
use crate::session_init::session_init;
use crate::session_list::session_ls;
use crate::session_registry::{session_rename, session_shorten_names};
use crate::session_remove::session_rm;
use crate::session_status::session_status;
use crate::session_transcript::session_transcript;
use crate::session_tui::run_folder_session_tui;
use crate::session_watch::session_watch;
use crate::tui_dashboard::run_tui;
use crate::{SessionArgs, SessionCommand, SessionOpenArgs, TuiArgs, TuiView};

pub(crate) fn run_session(args: SessionArgs) -> Result<()> {
    match args.command {
        Some(command) => run_session_command(command),
        None if args.open => {
            let dir = args
                .dir
                .ok_or_else(|| anyhow!("session name, path, or Buddy id is required for --open"))?;
            session_open(SessionOpenArgs {
                dir,
                target: SessionOpenTarget::Summary,
                editor: args.editor,
            })
        }
        None if args.dir.is_some() => run_folder_session_tui(args.dir.unwrap(), args.editor),
        None => run_tui(TuiArgs {
            view: TuiView::Sessions,
            roots: Vec::new(),
            editor: args.editor,
        }),
    }
}

fn run_session_command(command: SessionCommand) -> Result<()> {
    match command {
        SessionCommand::Init(args) => session_init(args),
        SessionCommand::Run(args) => session_run(args),
        SessionCommand::Chat(args) => session_chat(args),
        SessionCommand::Consolidate(args) => session_consolidate(args),
        SessionCommand::Watch(args) => session_watch(args),
        SessionCommand::Compact(args) => session_compact(args),
        SessionCommand::Promote(args) => session_promote(args),
        SessionCommand::Accept(args) => session_decide(args, SessionDecisionAction::Accept),
        SessionCommand::Deny(args) => session_decide(args, SessionDecisionAction::Deny),
        SessionCommand::ExportPattern(args) => session_export_pattern(args),
        SessionCommand::ValidateCandidates(args) => session_validate_candidates(args),
        SessionCommand::ValidateEvents(args) => session_validate_events(args),
        SessionCommand::Transcript(args) => session_transcript(args),
        SessionCommand::Events(args) => session_events(args),
        SessionCommand::Cleanup(args) => session_cleanup(args),
        SessionCommand::Context(args) => session_context(args),
        SessionCommand::Status(args) => session_status(args),
        SessionCommand::Ls(args) => session_ls(args),
        SessionCommand::Open(args) => session_open(args),
        SessionCommand::Rename(args) => session_rename(args),
        SessionCommand::ShortenNames(args) => session_shorten_names(args),
        SessionCommand::Rm(args) => session_rm(args),
    }
}
