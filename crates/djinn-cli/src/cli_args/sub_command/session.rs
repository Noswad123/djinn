use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::session::artifact::SessionOpenTarget;
use crate::session::transcript::SessionTranscriptFormat;
use crate::DEFAULT_AGENT_MAX_TOOL_ROUNDS;

#[derive(Debug, Args)]
pub(crate) struct SessionArgs {
    #[command(subcommand)]
    pub(crate) command: Option<SessionCommand>,
    /// Folder-backed session name, path, or Buddy id for convenience actions.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: Option<PathBuf>,
    /// Open the session summary without spelling `session open`.
    #[arg(long)]
    pub(crate) open: bool,
    /// Editor command for --open. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    pub(crate) editor: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    /// Scaffold a folder-backed Djinn work session.
    Init(SessionInitArgs),
    /// Start request.md for a folder-backed session in the background by default.
    Run(SessionRunArgs),
    /// Open an interactive Djinn UI chat for a folder-backed session.
    Chat(SessionChatArgs),
    /// Reconcile Djinn folder sessions and Buddy native sessions.
    Consolidate(SessionConsolidateArgs),
    /// Poll a folder-backed session until it is no longer running.
    Watch(SessionWatchArgs),
    /// Deterministically compact turn request/response evidence into context/compacted.md.
    Compact(SessionCompactArgs),
    /// Create a folder-backed promotion session with file-native provenance.
    Promote(SessionPromoteArgs),
    /// Accept a promotion session outcome and record the decision.
    Accept(SessionDecisionArgs),
    /// Deny a promotion session outcome and record the decision.
    Deny(SessionDecisionArgs),
    /// Export pattern promotion insight(s) to a Markdown notes file.
    ExportPattern(SessionExportPatternArgs),
    /// Validate generated promotion candidate TOML without accepting or rerunning the model.
    ValidateCandidates(SessionValidateCandidatesArgs),
    /// Validate that events.jsonl, optional turns/, and root summary.md agree.
    ValidateEvents(SessionValidateEventsArgs),
    /// Render a readable transcript from events.jsonl.
    Transcript(SessionTranscriptArgs),
    /// Preview the turns/ tree that would be projected from events.jsonl.
    Events(SessionEventsArgs),
    /// Permanently clean up explicit promotion-session source material.
    Cleanup(SessionCleanupArgs),
    /// Manage session-local context files and links.
    Context(SessionContextArgs),
    /// Inspect a folder-backed Djinn work session without running a model.
    Status(SessionStatusArgs),
    /// List cache-backed named folder sessions.
    Ls(SessionLsArgs),
    /// Open a folder-backed session artifact in $VISUAL/$EDITOR.
    Open(SessionOpenArgs),
    /// Rename a cache-backed folder session.
    Rename(SessionRenameArgs),
    /// Rename legacy long cache folder names to short copy-pasteable names.
    ShortenNames(SessionShortenNamesArgs),
    /// Remove a folder-backed session and its linked native session when present.
    Rm(SessionRmArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SessionWatchArgs {
    /// Folder-backed session name, path, or Buddy id to watch.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Poll interval in milliseconds while the session is running.
    #[arg(long = "interval-ms", default_value_t = 1000)]
    pub(crate) interval_ms: u64,
    /// Stop watching after this many seconds. Defaults to no timeout.
    #[arg(long = "timeout-seconds")]
    pub(crate) timeout_seconds: Option<u64>,
    /// Output compact JSON status snapshots instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionRunArgs {
    /// Folder-backed session name, path, or Buddy id to run.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Run in the foreground and block until the answer is written. Background is the default.
    #[arg(long = "fg")]
    pub(crate) foreground: bool,
    /// Internal worker mode for background session runs.
    #[arg(long = "background-worker", hide = true)]
    pub(crate) background_worker: bool,
    /// Agent profile name override.
    #[arg(long)]
    pub(crate) profile: Option<String>,
    /// Configured agent role name override.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// Model override. Prefix with copilot/ to use GitHub Copilot.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Provider API token. For copilot/* models, this is a Copilot API token.
    #[arg(long = "api-key")]
    pub(crate) api_key: Option<String>,
    /// Provider endpoint/base URL. For copilot/* models, this is the chat completions endpoint.
    #[arg(long = "base-url")]
    pub(crate) base_url: Option<String>,
    /// Maximum model/tool-call rounds before stopping.
    #[arg(long = "max-tool-rounds", default_value_t = DEFAULT_AGENT_MAX_TOOL_ROUNDS)]
    pub(crate) max_tool_rounds: usize,
    /// For promotion sessions, render the model prompt without calling a model or writing candidates.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
    /// Print the produced answer before the completion paths. Requires --fg.
    #[arg(long, conflicts_with = "json")]
    pub(crate) print: bool,
    /// Open summary.md after completion. Requires --fg.
    #[arg(long, conflicts_with = "json")]
    pub(crate) open: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionChatArgs {
    /// Folder-backed session name, path, or Buddy id to open in interactive Djinn UI chat.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Djinn UI executable/command. Defaults to DJINN_UI_BIN, legacy DJINN_BUDDY_BIN, runtime binding, then tools/buddy/bin/djinn-ui.
    #[arg(long = "buddy-bin")]
    pub(crate) buddy_bin: Option<String>,
    /// Extra argument to pass through to the Djinn UI. Repeat for multiple args.
    #[arg(long = "buddy-arg", allow_hyphen_values = true)]
    pub(crate) buddy_args: Vec<String>,
    /// Send request.md to the Djinn UI and capture the final response instead of opening interactive chat.
    #[arg(long = "capture-request", visible_alias = "capture")]
    pub(crate) capture_request: bool,
    /// With --capture-request, preview the Djinn UI command and request without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// With --capture-request, output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionConsolidateArgs {
    /// Preview reconciliation without creating Buddy sessions, folders, or bindings.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Djinn UI executable/command. Defaults to DJINN_UI_BIN, legacy DJINN_BUDDY_BIN, then tools/buddy/bin/djinn-ui.
    #[arg(long = "buddy-bin")]
    pub(crate) buddy_bin: Option<String>,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionInitArgs {
    /// Session name or directory to create or update. Bare names live under Djinn's cache session root.
    pub(crate) dir: PathBuf,
    /// Target repository to link into context/<repo-name> and use for repo-local config.
    #[arg(long = "link-repo")]
    pub(crate) link_repo: Option<PathBuf>,
    /// Do not auto-discover repo/harness breadcrumbs when --link-repo is set.
    #[arg(long = "no-discover-context")]
    pub(crate) no_discover_context: bool,
    /// Agent profile name to record. Defaults through global/repo Djinn config.
    #[arg(long, default_value = "default")]
    pub(crate) profile: String,
    /// Configured agent role name to record.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// Model to record. Defaults through profile/agent config when available.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Overwrite scaffolded files and context symlink targets when they already exist.
    #[arg(long)]
    pub(crate) force: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionCompactArgs {
    /// Folder-backed session name, path, or Buddy id containing events/context artifacts.
    #[arg(long = "session-dir", value_name = "SESSION")]
    pub(crate) session_dir: PathBuf,
    /// Output path. Defaults to <session-dir>/context/compacted.md.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionPromoteArgs {
    /// Folder-backed session names, paths, or Buddy ids to promote from.
    #[arg(required = true, value_name = "SESSION")]
    pub(crate) dirs: Vec<PathBuf>,
    /// Promotion type to prepare for.
    #[arg(long = "type", alias = "target", value_enum, default_value_t = SessionPromoteType::Memory)]
    pub(crate) promotion_type: SessionPromoteType,
    /// Promotion session folder to create. Bare names live under Djinn's cache session root.
    #[arg(long = "session-dir", alias = "output-dir")]
    pub(crate) promotion_session_dir: Option<PathBuf>,
    /// Maximum characters to include from each artifact excerpt.
    #[arg(long = "max-chars-per-artifact", default_value_t = 1200)]
    pub(crate) max_chars_per_artifact: usize,
    /// Replace generated promotion-session files if they already exist.
    #[arg(long)]
    pub(crate) force: bool,
    /// Output JSON instead of a text summary.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionDecisionArgs {
    /// Promotion session name, path, or Buddy id.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Optional candidate id/path within the promotion session. Defaults to the whole promotion outcome.
    pub(crate) candidate: Option<String>,
    /// Preview the decision without writing the decision record.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// After accepting MindWeaver todo candidates, explicitly run `mw todos sync`.
    #[arg(long = "sync-mindweaver", alias = "mw-sync")]
    pub(crate) sync_mindweaver: bool,
    /// Output JSON instead of a text summary.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionCleanupArgs {
    /// Promotion session name, path, or Buddy id whose source sessions should be removed.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Permanently delete source sessions recorded in context/sources.toml.
    #[arg(long)]
    pub(crate) delete_sources: bool,
    /// Preview source session deletion without removing anything.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output JSON instead of a text summary.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionExportPatternArgs {
    /// Pattern promotion session name, path, or Buddy id.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Optional pattern candidate id/path. Defaults to all generated pattern candidates.
    pub(crate) candidate: Option<String>,
    /// Markdown notes path to create or append to.
    #[arg(long = "to")]
    pub(crate) to: PathBuf,
    /// Append to an existing notes file. Without this, existing files are not overwritten.
    #[arg(long)]
    pub(crate) append: bool,
    /// Preview the exported Markdown without writing.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionValidateCandidatesArgs {
    /// Promotion session name, path, or Buddy id.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Optional candidate id/path within the promotion session. Defaults to all candidates.
    pub(crate) candidate: Option<String>,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionValidateEventsArgs {
    /// Folder-backed session name, path, or Buddy id to validate.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionTranscriptArgs {
    /// Folder-backed session name, path, or Buddy id to render.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = SessionTranscriptFormat::Markdown)]
    pub(crate) format: SessionTranscriptFormat,
    /// Shortcut for --format json.
    #[arg(long, conflicts_with = "format")]
    pub(crate) json: bool,
    /// Write transcript to this path instead of stdout.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Write/open the Markdown transcript. Defaults to <session>/transcript.md.
    #[arg(long)]
    pub(crate) open: bool,
    /// Editor command for --open. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    pub(crate) editor: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct SessionEventsArgs {
    /// Folder-backed session name, path, or Buddy id to project from.
    #[arg(required_unless_present = "all", value_name = "SESSION")]
    pub(crate) dir: Option<PathBuf>,
    /// Report event-ledger health for all cache-backed sessions.
    #[arg(long, conflicts_with_all = ["dir", "write", "restore"])]
    pub(crate) all: bool,
    /// Maximum cache-backed sessions to include with --all.
    #[arg(long, requires = "all")]
    pub(crate) limit: Option<usize>,
    /// With --all, include only ready, not-ready, missing, or matching issue-code sessions.
    #[arg(long = "health", requires = "all", value_name = "FILTER")]
    pub(crate) health_filter: Option<String>,
    /// With --all, exit with an error when any reported session is not ready.
    #[arg(long, requires = "all")]
    pub(crate) strict: bool,
    /// Rebuild turns/ and summary.md from events.jsonl after creating a backup.
    #[arg(long)]
    pub(crate) write: bool,
    /// Restore turns/ and summary.md from a .djinn/backups/events-rebuild-* backup. Without --write, preview only.
    #[arg(long, value_name = "BACKUP")]
    pub(crate) restore: Option<PathBuf>,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionPromoteType {
    #[value(alias = "memories")]
    Memory,
    #[value(
        alias = "suggestion",
        alias = "suggestions",
        alias = "action",
        alias = "actions"
    )]
    Todo,
    #[value(alias = "skills")]
    Skill,
    #[value(alias = "patterns")]
    Pattern,
}

#[derive(Debug, Args)]
pub(crate) struct SessionContextArgs {
    #[command(subcommand)]
    pub(crate) command: SessionContextCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionContextCommand {
    /// Discover repo and harness context breadcrumbs into this session.
    Discover(SessionContextDiscoverArgs),
    /// List session-local context entries and ingestion status.
    Ls(SessionContextLsArgs),
    /// Link a file or directory into session-local context.
    Add(SessionContextAddArgs),
    /// Remove one session-local context entry.
    Rm(SessionContextRmArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SessionContextDiscoverArgs {
    /// Folder-backed session name, path, or Buddy id to update.
    #[arg(value_name = "SESSION")]
    pub(crate) session: PathBuf,
    /// Preview discoveries without creating links or repo-index.md.
    #[arg(long = "dry-run")]
    pub(crate) dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionContextLsArgs {
    /// Folder-backed session name, path, or Buddy id to inspect.
    #[arg(value_name = "SESSION")]
    pub(crate) session: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionContextAddArgs {
    /// Folder-backed session name, path, or Buddy id to update.
    #[arg(value_name = "SESSION")]
    pub(crate) session: PathBuf,
    /// File or directory to link into context/.
    pub(crate) path: PathBuf,
    /// Context entry name. Defaults to the source basename.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Replace an existing file/link/directory under context/.
    #[arg(long)]
    pub(crate) force: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionContextRmArgs {
    /// Folder-backed session name, path, or Buddy id to update.
    #[arg(value_name = "SESSION")]
    pub(crate) session: PathBuf,
    /// Context entry name to remove.
    pub(crate) name: String,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionStatusArgs {
    /// Folder-backed session name, path, or Buddy id to inspect.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionLsArgs {
    /// Maximum folder sessions to list.
    #[arg(long)]
    pub(crate) limit: Option<usize>,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionShortenNamesArgs {
    /// Show planned renames without changing folder names.
    #[arg(long = "dry-run")]
    pub(crate) dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionRenameArgs {
    /// Folder-backed session name, path, or Buddy id to rename.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// New cache-backed session folder name.
    #[arg(value_name = "NEW_NAME")]
    pub(crate) new_name: String,
    /// Show the planned rename without changing folders.
    #[arg(long = "dry-run")]
    pub(crate) dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionOpenArgs {
    /// Folder-backed session name, path, or Buddy id to open.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Session artifact to open. Defaults to summary.md.
    #[arg(value_enum, default_value_t = SessionOpenTarget::Summary)]
    pub(crate) target: SessionOpenTarget,
    /// Editor command. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    pub(crate) editor: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct SessionRmArgs {
    /// Folder-backed session name, path, or Buddy id to remove.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_session_nouns_and_rejects_share_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "context",
            "discover",
            "./session",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Context(args)) = args.command else {
            panic!("expected session context command");
        };
        let SessionContextCommand::Discover(args) = args.command else {
            panic!("expected session context discover command");
        };
        assert_eq!(args.session, PathBuf::from("./session"));
        assert!(args.dry_run);
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "run",
            "bap-questions",
            "--fg",
            "--print",
            "--model",
            "openai/gpt-5.5",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Run(args)) = args.command else {
            panic!("expected session run command");
        };
        assert_eq!(args.dir, PathBuf::from("bap-questions"));
        assert!(args.foreground);
        assert!(args.print);
        assert!(!args.dry_run);
        assert_eq!(args.model.as_deref(), Some("openai/gpt-5.5"));

        let cli = Cli::try_parse_from(["djinn", "session", "chat", "ses_chatBuddy123"]).unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Chat(args)) = args.command else {
            panic!("expected session chat command");
        };
        assert_eq!(args.dir, PathBuf::from("ses_chatBuddy123"));

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "promote",
            "bap-questions",
            "./other-session",
            "--type",
            "pattern",
            "--session-dir",
            "./promotion-session",
            "--max-chars-per-artifact",
            "250",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Promote(args)) = args.command else {
            panic!("expected session promote command");
        };
        assert_eq!(args.promotion_type, SessionPromoteType::Pattern);
        assert_eq!(args.max_chars_per_artifact, 250);
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "events",
            "--all",
            "--limit",
            "5",
            "--health",
            "not-ready",
            "--strict",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Events(args)) = args.command else {
            panic!("expected session events all command");
        };
        assert!(args.dir.is_none());
        assert!(args.all);
        assert_eq!(args.limit, Some(5));
        assert_eq!(args.health_filter.as_deref(), Some("not-ready"));
        assert!(args.strict);
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "session", "bap-questions"]).unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        assert!(args.command.is_none());
        assert_eq!(args.dir, Some(PathBuf::from("bap-questions")));

        assert!(Cli::try_parse_from(["djinn", "session", "buddy", "bap-questions"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "share", "chats"]).is_err());
    }
}
