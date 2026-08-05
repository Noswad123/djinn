#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::session_artifact::SessionOpenTarget;
use crate::session_transcript::SessionTranscriptFormat;
use crate::DEFAULT_AGENT_MAX_TOOL_ROUNDS;

mod agents;
mod auth;
mod config;
mod doctor;
mod tui;
pub(crate) use agents::*;
pub(crate) use auth::*;
pub(crate) use config::*;
pub(crate) use doctor::*;
pub(crate) use tui::{TuiArgs, TuiView};

#[derive(Debug, Parser)]
#[command(name = "djinn")]
#[command(about = "Local-first companion for OpenCode and other AI coding agents")]
pub(crate) struct Cli {
    /// Open Buddy mode immediately instead of the Djinn dashboard.
    #[arg(short = 'b', long = "buddy")]
    pub(crate) buddy: bool,
    /// Folder-backed session name, path, or Buddy id to open.
    #[arg(short = 's', long = "session", value_name = "SESSION")]
    pub(crate) session: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

pub(crate) fn parse_cli() -> Cli {
    Cli::parse()
}

pub(crate) fn print_cli_help() -> std::io::Result<()> {
    Cli::command().print_help()?;
    println!();
    Ok(())
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// List a collection for humans.
    List(ListArgs),
    /// Show detailed information for one item.
    Show(ShowArgs),
    /// Add one item.
    Add(AddArgs),
    /// Accept/complete an item.
    Accept(AcceptArgs),
    /// Reject/remove an item.
    Reject(RejectArgs),
    /// Route memories into suggestions, skills, ideas, or actions.
    Ingest(IngestArgs),
    /// Run an external review to create or activate durable knowledge.
    Review(ReviewArgs),
    /// Remove one item.
    Rm(RmArgs),
    /// Clear a collection after confirmation.
    Clear(ClearArgs),
    /// Discover without writing durable state.
    Scan(ScanArgs),
    /// Write a machine-readable cache/index.
    Index(IndexArgs),
    /// Search a collection.
    Search(SearchArgs),
    /// Switch active context.
    Switch(SwitchArgs),
    /// Open an item in the user's editor.
    Open(OpenArgs),
    /// Inspect Djinn configuration and external harness config adapters.
    Config(ConfigArgs),
    /// Diagnose Djinn runtime integration points.
    Doctor(DoctorArgs),
    /// Manage provider credentials.
    Auth(AuthArgs),
    /// Ask Djinn from a new or existing session without the legacy agent prefix.
    Ask(AgentAskArgs),
    /// Manage folder-backed Djinn work sessions.
    Session(SessionArgs),
    /// Deprecated compatibility shim for old agent-prefixed commands.
    Agent(AgentArgs),
    /// Inspect configured Djinn agent roles.
    Agents(AgentsArgs),
    /// Open the unified terminal dashboard.
    Tui(TuiArgs),
}

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
    /// Open an interactive Buddy chat for a folder-backed session.
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
    /// Folder-backed session name, path, or Buddy id to open in interactive Buddy chat.
    #[arg(value_name = "SESSION")]
    pub(crate) dir: PathBuf,
    /// Buddy executable/command. Defaults to DJINN_BUDDY_BIN, runtime binding, then tools/buddy/bin/buddy.
    #[arg(long = "buddy-bin")]
    pub(crate) buddy_bin: Option<String>,
    /// Extra argument to pass through to Buddy. Repeat for multiple args.
    #[arg(long = "buddy-arg", allow_hyphen_values = true)]
    pub(crate) buddy_args: Vec<String>,
    /// Send request.md to Buddy and capture the final response instead of opening interactive chat.
    #[arg(long = "capture-request", visible_alias = "capture")]
    pub(crate) capture_request: bool,
    /// With --capture-request, preview the Buddy command and request without writing files.
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
    /// Buddy executable/command. Defaults to DJINN_BUDDY_BIN, tools/buddy/bin/buddy, then buddy.
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

#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    #[command(subcommand)]
    pub(crate) noun: ListNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ListNoun {
    /// List discovered local aliases, functions, scripts, and wrappers.
    Tools(ToolsScope),
    /// List active memories.
    Memories,
    /// List open suggestions.
    Suggestions,
    /// List saved ideas.
    Ideas,
    /// List open user actions.
    Actions,
    /// List agent skills known to Djinn.
    Skills(ListSkillsArgs),
    /// List available contexts.
    Contexts(ListCtxArgs),
    /// Alias for contexts; ctx has no plural form.
    Ctx(ListCtxArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ShowArgs {
    #[command(subcommand)]
    pub(crate) noun: ShowNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ShowNoun {
    /// Show an active memory by id or text fragment.
    Memory { id: String },
    /// Show a suggestion by id or text fragment.
    Suggestion { id: String },
    /// Show a saved idea by id or text fragment.
    Idea { id: String },
    /// Show a user action by id or text fragment.
    Action { id: String },
    /// Show the active context.
    Ctx(ShowCtxArgs),
    /// Show a tool by name.
    Tool(ToolLookupArgs),
    /// Show a skill by name.
    Skill(ShowSkillArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    #[command(subcommand)]
    pub(crate) noun: AddNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AddNoun {
    /// Add an active memory.
    Memory(AddMemoryArgs),
    /// Add a suggestion.
    Suggestion(AddSuggestionArgs),
    /// Add a saved idea.
    Idea(AddMemoryArgs),
    /// Add a user action.
    Action(AddMemoryArgs),
    /// Add or scaffold a skill.
    Skill(AddSkillArgs),
    /// Add or update a context.
    Ctx(AddCtxArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AcceptArgs {
    #[command(subcommand)]
    pub(crate) noun: AcceptNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AcceptNoun {
    /// Review a memory and produce suggestions.
    Memory(AcceptMemoryArgs),
    /// Mark a suggestion as done and remove it from the suggestion list.
    Suggestion { id: String },
}

#[derive(Debug, Args)]
pub(crate) struct RejectArgs {
    #[command(subcommand)]
    pub(crate) noun: RejectNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RejectNoun {
    /// Remove memories permanently.
    Memory {
        /// Memory ids or text fragments.
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Reject suggestions and remove them permanently.
    Suggestion {
        /// Suggestion ids or text fragments.
        #[arg(required = true)]
        ids: Vec<String>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct IngestArgs {
    #[command(subcommand)]
    pub(crate) noun: IngestNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum IngestNoun {
    /// Route active memories into the right downstream collection.
    Memories(IngestMemoriesArgs),
    /// Route one active memory into the right downstream collection.
    Memory(IngestMemoriesArgs),
}

#[derive(Debug, Args)]
pub(crate) struct IngestMemoriesArgs {
    /// Memory ids or text fragments to ingest.
    #[arg(required = true)]
    pub(crate) ids: Vec<String>,
    /// Destination collection. `auto` uses memory kind text.
    #[arg(long = "as", value_enum, default_value_t = IngestTarget::Auto)]
    pub(crate) target: IngestTarget,
    /// Keep memories after ingesting instead of consuming them.
    #[arg(long)]
    pub(crate) keep: bool,
    /// Overwrite an existing Djinn-managed skill when ingesting as a skill.
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum IngestTarget {
    Auto,
    Memory,
    Suggestion,
    Skill,
    Idea,
    Action,
}

#[derive(Debug, Args)]
pub(crate) struct ReviewArgs {
    #[command(subcommand)]
    pub(crate) source: ReviewSource,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ReviewSource {
    /// Ask OpenCode to review one or more memories and create suggestions.
    Memories(ReviewMemoriesArgs),
    /// Ask OpenCode to review one memory and create suggestions.
    Memory(ReviewMemoriesArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ReviewMemoriesArgs {
    /// Optional memory ids or text fragments to review.
    pub(crate) ids: Vec<String>,
    /// Maximum memories to include unless --all is used.
    #[arg(long, default_value_t = 100)]
    pub(crate) limit: usize,
    /// Review all matching memories instead of applying --limit.
    #[arg(long)]
    pub(crate) all: bool,
    /// Optional query filter over memory id, text, metadata, and evidence.
    #[arg(long)]
    pub(crate) query: Option<String>,
    /// OpenCode agent to use for the review.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// OpenCode run title.
    #[arg(long, default_value = "djinn memory curation review")]
    pub(crate) title: String,
    /// OpenCode binary to execute.
    #[arg(long, default_value = "opencode")]
    pub(crate) opencode_bin: String,
    /// Print the prompt instead of running OpenCode.
    #[arg(long)]
    pub(crate) dry_run: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RmArgs {
    #[command(subcommand)]
    pub(crate) noun: RmNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RmNoun {
    /// Remove a memory matching a keyword.
    Memory { keyword: String },
    /// Remove or archive a skill.
    Skill(RmSkillArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ClearArgs {
    #[command(subcommand)]
    pub(crate) noun: ClearNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClearNoun {
    /// Clear all memories after interactive confirmation.
    Memories {
        /// Skip creating memories.backup-*.jsonl before clearing.
        #[arg(long)]
        no_backup: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ScanArgs {
    #[command(subcommand)]
    pub(crate) noun: ScanNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ScanNoun {
    /// Scan local tools and print a summary.
    Tools(ToolsScope),
}

#[derive(Debug, Args)]
pub(crate) struct IndexArgs {
    #[command(subcommand)]
    pub(crate) noun: IndexNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum IndexNoun {
    /// Write the local tools JSON index.
    Tools(IndexToolsArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    #[command(subcommand)]
    pub(crate) noun: SearchNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SearchNoun {
    /// Search local tools.
    Tools(SearchToolsArgs),
    /// Search memories.
    Memories { query: String },
    /// Search suggestions.
    Suggestions { query: String },
}

#[derive(Debug, Args)]
pub(crate) struct SwitchArgs {
    #[command(subcommand)]
    pub(crate) noun: SwitchNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SwitchNoun {
    /// Switch the active context.
    Ctx {
        /// Context name, case-insensitive. Falls back to substring matching.
        name: String,
    },
}

#[derive(Debug, Args)]
pub(crate) struct OpenArgs {
    #[command(subcommand)]
    pub(crate) noun: OpenNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum OpenNoun {
    /// Open a local tool source by name.
    Tool(OpenToolArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AgentArgs {
    #[command(subcommand)]
    pub(crate) command: AgentCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentCommand {
    /// Inspect discovered agent profiles and models.
    Config(AgentConfigArgs),
    /// Inspect built-in agent runtime tools.
    Tools(AgentToolsArgs),
    /// Inspect, audit, and revoke effective agent policy grants.
    Policy(AgentPolicyArgs),
    /// Inspect or restore apply_patch file-history entries.
    FileHistory(AgentFileHistoryArgs),
    /// Deprecated alias for top-level `djinn ask`.
    Ask(AgentAskArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AgentConfigArgs {
    #[command(subcommand)]
    pub(crate) command: AgentConfigCommand,
}

#[derive(Debug, Args)]
pub(crate) struct AgentToolsArgs {
    #[command(subcommand)]
    pub(crate) command: AgentToolsCommand,
}

#[derive(Debug, Args)]
pub(crate) struct AgentPolicyArgs {
    #[command(subcommand)]
    pub(crate) command: AgentPolicyCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentConfigCommand {
    /// List discovered agent profiles and models.
    List(AgentConfigListArgs),
    /// Show the effective agent runtime configuration.
    Show(AgentConfigShowArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentToolsCommand {
    /// List built-in tools exposed to the agent runtime.
    List(AgentToolsListArgs),
    /// Show one built-in agent tool spec.
    Show(AgentToolsShowArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentPolicyCommand {
    /// List the effective read/permission policy and guardrails.
    List(AgentPolicyListArgs),
    /// Audit effective policy for durable grants and high-attention behavior.
    Audit(AgentPolicyAuditArgs),
    /// Revoke stored durable approvals. Currently reports no-op until durable approvals exist.
    Revoke(AgentPolicyRevokeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AgentPolicyListArgs {
    /// Workspace path to resolve. Defaults to the current directory.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    pub(crate) profile: String,
    /// Configured agent role name.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// OpenAI model to use. Defaults the same way as folder-backed asks.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentPolicyAuditArgs {
    /// Workspace path to resolve. Defaults to the current directory.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    pub(crate) profile: String,
    /// Configured agent role name.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// OpenAI model to use. Defaults the same way as folder-backed asks.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentPolicyRevokeArgs {
    /// Optional action selector for future durable approvals, such as shell or write.
    #[arg(long)]
    pub(crate) action: Option<String>,
    /// Optional resource/path selector for future durable approvals.
    #[arg(long)]
    pub(crate) resource: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentFileHistoryArgs {
    #[command(subcommand)]
    pub(crate) command: AgentFileHistoryCommand,
}

#[derive(Debug, Args)]
pub(crate) struct AgentConfigListArgs {
    /// Agent profile to treat as current.
    #[arg(long, default_value = "default")]
    pub(crate) profile: String,
    /// Model to treat as current. Defaults the same way as folder-backed asks.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentConfigShowArgs {
    /// Workspace path to resolve. Defaults to the current directory.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    pub(crate) profile: String,
    /// Configured agent role name.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// OpenAI model to use. Defaults the same way as folder-backed asks.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentToolsListArgs {
    /// Workspace path used to resolve profile permissions. Defaults to the current directory.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Agent profile name used for read/permission policy resolution.
    #[arg(long, default_value = "default")]
    pub(crate) profile: String,
    /// Configured agent role name.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentToolsShowArgs {
    /// Tool name, case-insensitive. Falls back to substring matching.
    pub(crate) name: String,
    /// Workspace path used to resolve profile permissions. Defaults to the current directory.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Agent profile name used for read/permission policy resolution.
    #[arg(long, default_value = "default")]
    pub(crate) profile: String,
    /// Configured agent role name.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentFileHistoryCommand {
    /// List apply_patch file-history entries.
    List(AgentFileHistoryListArgs),
    /// Restore one apply_patch preimage entry.
    Restore(AgentFileHistoryRestoreArgs),
}

#[derive(Debug, Args, Clone)]
pub(crate) struct ToolsScope {
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct IndexToolsArgs {
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Index JSON path. Defaults under the scanned root.
    #[arg(long)]
    pub(crate) index: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ToolLookupArgs {
    /// Tool name, case-insensitive. Falls back to substring matching.
    pub(crate) name: String,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SearchToolsArgs {
    pub(crate) query: String,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ListSkillsArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ShowSkillArgs {
    /// Skill name, case-insensitive. Falls back to substring matching.
    pub(crate) name: String,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AddSkillArgs {
    /// Skill name to scaffold under ~/.config/djinn/skills.
    pub(crate) name: String,
    /// One-line skill description.
    #[arg(long)]
    pub(crate) description: Option<String>,
    /// Overwrite an existing Djinn-managed skill scaffold.
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RmSkillArgs {
    /// Skill name, case-insensitive. Only Djinn-managed skills can be removed.
    pub(crate) name: String,
}

#[derive(Debug, Args)]
pub(crate) struct ListCtxArgs {
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ShowCtxArgs {
    /// Context name. Defaults to the active context.
    pub(crate) name: Option<String>,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AddCtxArgs {
    /// Context name.
    pub(crate) name: String,
    /// Human-friendly description.
    #[arg(long)]
    pub(crate) description: Option<String>,
    /// Tool/project root for this context. Repeatable.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Skill root for this context. Repeatable.
    #[arg(long = "skill-root")]
    pub(crate) skill_roots: Vec<PathBuf>,
    /// Default memory scope, for example: project:djinn.
    #[arg(long = "memory-scope")]
    pub(crate) memory_scope: Option<String>,
    /// Make this context active after adding/updating it.
    #[arg(long)]
    pub(crate) switch: bool,
}

#[derive(Debug, Args)]
pub(crate) struct OpenToolArgs {
    /// Tool name, case-insensitive. Falls back to substring matching.
    pub(crate) name: String,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Editor command. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    pub(crate) editor: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AgentFileHistoryListArgs {
    /// Filter by exact patch id.
    #[arg(long = "patch-id")]
    pub(crate) patch_id: Option<String>,
    /// Filter by exact workspace string.
    #[arg(long)]
    pub(crate) workspace: Option<String>,
    /// Maximum entries to list.
    #[arg(long)]
    pub(crate) limit: Option<usize>,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentFileHistoryRestoreArgs {
    /// File-history entry id to restore.
    pub(crate) id: String,
    /// Overwrite an existing preimage target, or remove an existing tombstone target.
    #[arg(long)]
    pub(crate) force: bool,
    /// For move entries, also remove the recorded new_path file if it exists.
    #[arg(long = "remove-new-path")]
    pub(crate) remove_new_path: bool,
    /// Validate and show what would happen without changing files.
    #[arg(long = "dry-run")]
    pub(crate) dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentAskArgs {
    /// Prompt to send to the configured agent provider.
    pub(crate) prompt: Option<String>,
    /// Existing Djinn agent session id to append this ask turn to.
    #[arg(long = "session-id")]
    pub(crate) session_id: Option<String>,
    /// Folder-backed session name or directory. Bare names live under Djinn's cache session root.
    #[arg(long = "session-dir", visible_alias = "session")]
    pub(crate) session_dir: Option<PathBuf>,
    /// Human-friendly session title. Defaults to a trimmed prompt preview.
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// Workspace path for the session. Defaults to the current directory.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long)]
    pub(crate) profile: Option<String>,
    /// Configured agent role name.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// Parent agent session id for explicit related-session workflows.
    #[arg(long = "parent-session")]
    pub(crate) parent_session: Option<String>,
    /// Model to use. Prefix with copilot/ to use GitHub Copilot.
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
    /// Output JSON instead of text.
    #[arg(long)]
    pub(crate) json: bool,
    /// Print the produced answer instead of the default folder path output.
    #[arg(long, conflicts_with = "json")]
    pub(crate) print: bool,
    /// Open the produced summary.md after an auto-created folder-backed ask completes.
    #[arg(long, conflicts_with_all = ["json", "session_id", "session_dir"])]
    pub(crate) open: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AddMemoryArgs {
    /// Durable memory text.
    pub(crate) text: String,
    /// Scope for the memory, for example: global, project, repo, work, personal.
    #[arg(long)]
    pub(crate) scope: Option<String>,
    /// Memory kind, for example: preference, convention, workaround, correction.
    #[arg(long)]
    pub(crate) kind: Option<String>,
    /// Confidence label, for example: low, medium, high.
    #[arg(long)]
    pub(crate) confidence: Option<String>,
    /// Do not act on this memory before this date, for example: 2026-10-01.
    #[arg(long = "not-before")]
    pub(crate) not_before: Option<String>,
    /// Durable copied evidence explaining why this memory exists. Repeatable.
    #[arg(long = "evidence")]
    pub(crate) evidence: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AddSuggestionArgs {
    /// Suggested action or artifact to consider.
    pub(crate) text: String,
    /// Suggested target, for example: skill, action, idea, config, code, docs.
    #[arg(long)]
    pub(crate) target: Option<String>,
    /// Why this suggestion is worth considering.
    #[arg(long)]
    pub(crate) rationale: Option<String>,
    /// Optional draft content or implementation sketch.
    #[arg(long)]
    pub(crate) draft: Option<String>,
    /// Copied evidence supporting this suggestion. Repeatable.
    #[arg(long = "evidence")]
    pub(crate) evidence: Vec<String>,
    /// Memory id or text fragment to attach as evidence. Repeatable.
    #[arg(long = "source-memory")]
    pub(crate) source_memories: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AcceptMemoryArgs {
    /// Memory id or text fragment.
    pub(crate) id: String,
    /// OpenCode agent to use for the review.
    #[arg(long)]
    pub(crate) agent: Option<String>,
    /// OpenCode run title.
    #[arg(long, default_value = "djinn memory suggestion review")]
    pub(crate) title: String,
    /// OpenCode binary to execute.
    #[arg(long, default_value = "opencode")]
    pub(crate) opencode_bin: String,
    /// Print the prompt instead of running OpenCode.
    #[arg(long)]
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Json,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_removed_agent_session_command() {
        assert!(Cli::try_parse_from(["djinn", "agent", "session"]).is_err());
        assert!(Cli::try_parse_from([
            "djinn", "agent", "session", "delete", "agt_test", "--force", "--json",
        ])
        .is_err());
        assert!(
            Cli::try_parse_from(["djinn", "agent", "session", "stats", "agt_test", "--json",])
                .is_err()
        );
    }

    #[test]
    fn parses_agent_policy_commands() {
        let cli = Cli::try_parse_from([
            "djinn",
            "agent",
            "policy",
            "list",
            "--profile",
            "architect",
            "--agent",
            "reviewer",
            "--json",
        ])
        .unwrap();
        let Some(Command::Agent(agent_args)) = cli.command else {
            panic!("expected agent command");
        };
        let AgentCommand::Policy(policy_args) = agent_args.command else {
            panic!("expected agent policy command");
        };
        let AgentPolicyCommand::List(list_args) = policy_args.command else {
            panic!("expected agent policy list command");
        };
        assert_eq!(list_args.profile, "architect");
        assert_eq!(list_args.agent.as_deref(), Some("reviewer"));
        assert!(list_args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "agent",
            "policy",
            "revoke",
            "--action",
            "shell",
            "--resource",
            "printf hello",
            "--json",
        ])
        .unwrap();
        let Some(Command::Agent(agent_args)) = cli.command else {
            panic!("expected agent command");
        };
        let AgentCommand::Policy(policy_args) = agent_args.command else {
            panic!("expected agent policy command");
        };
        let AgentPolicyCommand::Revoke(revoke_args) = policy_args.command else {
            panic!("expected agent policy revoke command");
        };
        assert_eq!(revoke_args.action.as_deref(), Some("shell"));
        assert_eq!(revoke_args.resource.as_deref(), Some("printf hello"));
        assert!(revoke_args.json);
    }

    #[test]
    fn rejects_removed_agent_session_relationship_and_child_commands() {
        assert!(Cli::try_parse_from([
            "djinn",
            "agent",
            "session",
            "list",
            "--agent",
            "reviewer",
            "--parent-session",
            "agt_parent",
            "--state",
            "running",
            "--json",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "djinn",
            "agent",
            "session",
            "children",
            "agt_parent",
            "--limit",
            "5",
            "--state",
            "completed",
            "--json",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "djinn",
            "agent",
            "session",
            "child",
            "start",
            "agt_parent",
            "--prompt",
            "review this diff",
            "--agent",
            "reviewer",
            "--title",
            "Review diff",
            "--json",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "djinn",
            "agent",
            "session",
            "lifecycle",
            "show",
            "agt_child",
            "--json",
        ])
        .is_err());
    }

    #[test]
    fn parses_agent_role_selection_flags_for_runtime_commands() {
        let cli = Cli::try_parse_from([
            "djinn",
            "agent",
            "ask",
            "hello",
            "--agent",
            "reviewer",
            "--parent-session",
            "agt_parent",
            "--json",
        ])
        .unwrap();
        let Some(Command::Agent(agent_args)) = cli.command else {
            panic!("expected agent command");
        };
        let AgentCommand::Ask(args) = agent_args.command else {
            panic!("expected agent ask command");
        };
        assert_eq!(args.prompt.as_deref(), Some("hello"));
        assert_eq!(args.agent.as_deref(), Some("reviewer"));
        assert_eq!(args.parent_session.as_deref(), Some("agt_parent"));
        assert_eq!(args.max_tool_rounds, DEFAULT_AGENT_MAX_TOOL_ROUNDS);
        assert!(args.json);

        assert!(Cli::try_parse_from([
            "djinn",
            "agent",
            "chat",
            "--agent",
            "planner",
            "--parent-session",
            "agt_parent",
            "--max-tool-rounds",
            "8",
        ])
        .is_err());

        let cli = Cli::try_parse_from([
            "djinn",
            "agent",
            "ask",
            "--session-dir",
            "/tmp/djinn-session",
        ])
        .unwrap();
        let Some(Command::Agent(agent_args)) = cli.command else {
            panic!("expected agent command");
        };
        let AgentCommand::Ask(args) = agent_args.command else {
            panic!("expected agent ask command");
        };
        assert!(args.prompt.is_none());
        assert_eq!(
            args.session_dir.as_deref(),
            Some(Path::new("/tmp/djinn-session"))
        );

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "init",
            "/tmp/folder-session",
            "--link-repo",
            "/tmp/repo",
            "--profile",
            "work",
            "--agent",
            "architect",
            "--model",
            "repo-model",
            "--force",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Init(args)) = session_args.command else {
            panic!("expected session init command");
        };
        assert_eq!(args.dir, PathBuf::from("/tmp/folder-session"));
        assert_eq!(args.link_repo.as_deref(), Some(Path::new("/tmp/repo")));
        assert_eq!(args.profile, "work");
        assert_eq!(args.agent.as_deref(), Some("architect"));
        assert_eq!(args.model.as_deref(), Some("repo-model"));
        assert!(args.force);
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "ask",
            "continue here",
            "--session-id",
            "agt_existing",
            "--session-dir",
            "/tmp/folder-session",
            "--profile",
            "work",
        ])
        .unwrap();
        let Some(Command::Ask(args)) = cli.command else {
            panic!("expected top-level ask command");
        };
        assert_eq!(args.prompt.as_deref(), Some("continue here"));
        assert_eq!(args.session_id.as_deref(), Some("agt_existing"));
        assert_eq!(args.profile.as_deref(), Some("work"));
        assert_eq!(
            args.session_dir.as_deref(),
            Some(Path::new("/tmp/folder-session"))
        );
        assert!(!args.print);
        assert!(!args.open);

        let cli = Cli::try_parse_from(["djinn", "ask", "hi", "--session", "quick-note", "--print"])
            .unwrap();
        let Some(Command::Ask(args)) = cli.command else {
            panic!("expected top-level ask command");
        };
        assert_eq!(args.session_dir.as_deref(), Some(Path::new("quick-note")));
        assert!(args.print);
        assert!(!args.open);

        let cli = Cli::try_parse_from(["djinn", "ask", "hi", "--print", "--open"]).unwrap();
        let Some(Command::Ask(args)) = cli.command else {
            panic!("expected top-level ask command");
        };
        assert!(args.print);
        assert!(args.open);
        assert!(Cli::try_parse_from(["djinn", "ask", "hi", "--print", "--json"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "ask", "hi", "--open", "--json"]).is_err());
        assert!(
            Cli::try_parse_from(["djinn", "ask", "hi", "--session", "quick-note", "--open",])
                .is_err()
        );
        assert!(Cli::try_parse_from([
            "djinn",
            "ask",
            "hi",
            "--session-id",
            "agt_existing",
            "--open",
        ])
        .is_err());

        assert!(Cli::try_parse_from(["djinn", "session", "list", "--limit", "5"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "session", "show", "/tmp/folder-session"]).is_err());
        assert!(
            Cli::try_parse_from(["djinn", "session", "delete", "agt_existing", "--force",])
                .is_err()
        );

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "compact",
            "--session-dir",
            "/tmp/folder-session",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Compact(args)) = session_args.command else {
            panic!("expected session compact command");
        };
        assert_eq!(args.session_dir, PathBuf::from("/tmp/folder-session"));
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "context",
            "ls",
            "small-question",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Context(context_args)) = session_args.command else {
            panic!("expected session context command");
        };
        let SessionContextCommand::Ls(args) = context_args.command else {
            panic!("expected session context ls command");
        };
        assert_eq!(args.session, PathBuf::from("small-question"));
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "context",
            "add",
            "small-question",
            "/tmp/notes.md",
            "--name",
            "notes.md",
            "--force",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Context(context_args)) = session_args.command else {
            panic!("expected session context command");
        };
        let SessionContextCommand::Add(args) = context_args.command else {
            panic!("expected session context add command");
        };
        assert_eq!(args.session, PathBuf::from("small-question"));
        assert_eq!(args.path, PathBuf::from("/tmp/notes.md"));
        assert_eq!(args.name.as_deref(), Some("notes.md"));
        assert!(args.force);
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "context",
            "rm",
            "small-question",
            "notes.md",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Context(context_args)) = session_args.command else {
            panic!("expected session context command");
        };
        let SessionContextCommand::Rm(args) = context_args.command else {
            panic!("expected session context rm command");
        };
        assert_eq!(args.session, PathBuf::from("small-question"));
        assert_eq!(args.name, "notes.md");
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "session", "shorten-names", "--dry-run", "--json"])
            .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::ShortenNames(args)) = session_args.command else {
            panic!("expected session shorten-names command");
        };
        assert!(args.dry_run);
        assert!(args.json);

        let cli =
            Cli::try_parse_from(["djinn", "session", "status", "/tmp/folder-session"]).unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Status(args)) = session_args.command else {
            panic!("expected session status command");
        };
        assert_eq!(args.dir, PathBuf::from("/tmp/folder-session"));

        let cli = Cli::try_parse_from(["djinn", "session", "ls", "--limit", "10"]).unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Ls(args)) = session_args.command else {
            panic!("expected session ls command");
        };
        assert_eq!(args.limit, Some(10));

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "open",
            "small-question",
            "compacted",
            "--editor",
            "nvim",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Open(args)) = session_args.command else {
            panic!("expected session open command");
        };
        assert_eq!(args.dir, PathBuf::from("small-question"));
        assert_eq!(args.target, SessionOpenTarget::Compacted);
        assert_eq!(args.editor.as_deref(), Some("nvim"));

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "rename",
            "ses_abc123",
            "structured-programming",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Rename(args)) = session_args.command else {
            panic!("expected session rename command");
        };
        assert_eq!(args.dir, PathBuf::from("ses_abc123"));
        assert_eq!(args.new_name, "structured-programming");
        assert!(args.dry_run);
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "small-question",
            "--open",
            "--editor",
            "nvim",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        assert!(session_args.command.is_none());
        assert_eq!(
            session_args.dir.as_deref(),
            Some(Path::new("small-question"))
        );
        assert!(session_args.open);
        assert_eq!(session_args.editor.as_deref(), Some("nvim"));

        let cli =
            Cli::try_parse_from(["djinn", "session", "rm", "small-question", "--json"]).unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Rm(args)) = session_args.command else {
            panic!("expected session rm command");
        };
        assert_eq!(args.dir, PathBuf::from("small-question"));
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "cleanup",
            "promotion-memory",
            "--delete-sources",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Cleanup(args)) = session_args.command else {
            panic!("expected session cleanup command");
        };
        assert_eq!(args.dir, PathBuf::from("promotion-memory"));
        assert!(args.delete_sources);
        assert!(args.dry_run);
        assert!(args.json);

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
            "export-pattern",
            "promotion-pattern",
            "pattern-001",
            "--to",
            "/tmp/notes/pattern.md",
            "--append",
            "--dry-run",
        ])
        .unwrap();
        let Some(Command::Session(session_args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::ExportPattern(args)) = session_args.command else {
            panic!("expected session export-pattern command");
        };
        assert_eq!(args.dir, PathBuf::from("promotion-pattern"));
        assert_eq!(args.candidate.as_deref(), Some("pattern-001"));
        assert_eq!(args.to, PathBuf::from("/tmp/notes/pattern.md"));
        assert!(args.append);
        assert!(args.dry_run);
    }

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

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "run",
            "promotion-memory",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Run(args)) = args.command else {
            panic!("expected session run command");
        };
        assert_eq!(args.dir, PathBuf::from("promotion-memory"));
        assert!(args.dry_run);
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "chat",
            "bap-questions",
            "--capture-request",
            "--buddy-bin",
            "buddy-dev",
            "--buddy-arg",
            "--no-stream",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Chat(args)) = args.command else {
            panic!("expected session chat command");
        };
        assert_eq!(args.dir, PathBuf::from("bap-questions"));
        assert!(args.capture_request);
        assert_eq!(args.buddy_bin.as_deref(), Some("buddy-dev"));
        assert_eq!(args.buddy_args, vec!["--no-stream".to_string()]);
        assert!(args.dry_run);
        assert!(args.json);

        assert!(Cli::try_parse_from([
            "djinn",
            "session",
            "chat",
            "bap-questions",
            "--buddy-session",
            "bud_123",
        ])
        .is_err());

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "consolidate",
            "--dry-run",
            "--buddy-bin",
            "buddy-dev",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Consolidate(args)) = args.command else {
            panic!("expected session consolidate command");
        };
        assert!(args.dry_run);
        assert_eq!(args.buddy_bin.as_deref(), Some("buddy-dev"));
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "watch",
            "bap-questions",
            "--interval-ms",
            "250",
            "--timeout-seconds",
            "5",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Watch(args)) = args.command else {
            panic!("expected session watch command");
        };
        assert_eq!(args.dir, PathBuf::from("bap-questions"));
        assert_eq!(args.interval_ms, 250);
        assert_eq!(args.timeout_seconds, Some(5));
        assert!(args.json);

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
        assert_eq!(
            args.dirs,
            vec![
                PathBuf::from("bap-questions"),
                PathBuf::from("./other-session")
            ]
        );
        assert_eq!(args.promotion_type, SessionPromoteType::Pattern);
        assert_eq!(
            args.promotion_session_dir,
            Some(PathBuf::from("./promotion-session"))
        );
        assert_eq!(args.max_chars_per_artifact, 250);
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "promote",
            "bap-questions",
            "--target",
            "memories",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Promote(args)) = args.command else {
            panic!("expected session promote command");
        };
        assert_eq!(args.promotion_type, SessionPromoteType::Memory);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "accept",
            "./promotion-session",
            "memory-001",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Accept(args)) = args.command else {
            panic!("expected session accept command");
        };
        assert_eq!(args.dir, PathBuf::from("./promotion-session"));
        assert_eq!(args.candidate.as_deref(), Some("memory-001"));
        assert!(args.dry_run);
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "session", "deny", "./promotion-session"]).unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Deny(args)) = args.command else {
            panic!("expected session deny command");
        };
        assert_eq!(args.dir, PathBuf::from("./promotion-session"));
        assert!(args.candidate.is_none());
        assert!(!args.dry_run);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "validate-candidates",
            "./promotion-session",
            "memory-001",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::ValidateCandidates(args)) = args.command else {
            panic!("expected session validate-candidates command");
        };
        assert_eq!(args.dir, PathBuf::from("./promotion-session"));
        assert_eq!(args.candidate.as_deref(), Some("memory-001"));
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "validate-events",
            "./debugging-session",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::ValidateEvents(args)) = args.command else {
            panic!("expected session validate-events command");
        };
        assert_eq!(args.dir, PathBuf::from("./debugging-session"));
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "transcript",
            "./debugging-session",
            "--format",
            "json",
            "--output",
            "transcript.json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Transcript(args)) = args.command else {
            panic!("expected session transcript command");
        };
        assert_eq!(args.dir, PathBuf::from("./debugging-session"));
        assert_eq!(args.format, SessionTranscriptFormat::Json);
        assert_eq!(args.output, Some(PathBuf::from("transcript.json")));

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "events",
            "./debugging-session",
            "--write",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Events(args)) = args.command else {
            panic!("expected session events command");
        };
        assert_eq!(args.dir, Some(PathBuf::from("./debugging-session")));
        assert!(!args.all);
        assert!(args.write);
        assert!(!args.strict);
        assert!(args.restore.is_none());
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "session",
            "events",
            "./debugging-session",
            "--restore",
            "events-rebuild-test",
            "--write",
            "--json",
        ])
        .unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        let Some(SessionCommand::Events(args)) = args.command else {
            panic!("expected session events restore command");
        };
        assert_eq!(args.dir, Some(PathBuf::from("./debugging-session")));
        assert_eq!(args.restore, Some(PathBuf::from("events-rebuild-test")));
        assert!(!args.all);
        assert!(args.write);
        assert!(!args.strict);
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
        assert!(!args.write);
        assert!(args.restore.is_none());
        assert!(args.json);
        assert!(Cli::try_parse_from(["djinn", "session", "events", "--all", "--gate"]).is_err());
        assert!(Cli::try_parse_from([
            "djinn",
            "session",
            "events",
            "./scratch-debugging-session",
            "--authority-trial",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "djinn",
            "session",
            "events",
            "./scratch-debugging-session",
            "--authority-read",
        ])
        .is_err());

        assert!(Cli::try_parse_from([
            "djinn",
            "session",
            "project-events",
            "./debugging-session",
            "--json",
        ])
        .is_err());

        let cli = Cli::try_parse_from(["djinn", "session", "bap-questions"]).unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        assert!(args.command.is_none());
        assert_eq!(args.dir, Some(PathBuf::from("bap-questions")));

        assert!(Cli::try_parse_from(["djinn", "session", "buddy", "bap-questions"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "share", "chats"]).is_err());
    }

    #[test]
    fn rejects_removed_archive_command() {
        assert!(Cli::try_parse_from(["djinn", "archive", "list"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "archive", "sessions", "--dry-run"]).is_err());
    }

    #[test]
    fn rejects_removed_saved_row_review_commands() {
        assert!(Cli::try_parse_from(["djinn", "review", "sessions", "--dry-run"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "review", "opencode"]).is_err());
    }

    #[test]
    fn rejects_removed_prune_sessions_command() {
        assert!(
            Cli::try_parse_from(["djinn", "prune", "sessions", "--older-than", "30d",]).is_err()
        );
    }

    #[test]
    fn rejects_removed_legacy_saved_row_commands() {
        assert!(Cli::try_parse_from(["djinn", "add", "session", "./session.md"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "list", "sessions"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "show", "session", "abc"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "search", "sessions", "rust"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "rm", "session", "abc"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "clear", "sessions"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "promote", "sessions"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "promote", "session", "abc"]).is_err());
    }

    #[test]
    fn parses_top_level_buddy_mode_flags() {
        let cli = Cli::try_parse_from(["djinn", "-b"]).unwrap();
        assert!(cli.buddy);
        assert!(cli.session.is_none());
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["djinn", "-b", "-s", "bap-questions"]).unwrap();
        assert!(cli.buddy);
        assert_eq!(cli.session, Some(PathBuf::from("bap-questions")));
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["djinn", "-bs", "bap-questions"]).unwrap();
        assert!(cli.buddy);
        assert_eq!(cli.session, Some(PathBuf::from("bap-questions")));
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["djinn", "-s", "bap-questions"]).unwrap();
        assert!(!cli.buddy);
        assert_eq!(cli.session, Some(PathBuf::from("bap-questions")));
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_tui_without_view_defaults_to_sessions() {
        let cli = Cli::try_parse_from(["djinn", "tui"]).unwrap();
        let Some(Command::Tui(args)) = cli.command else {
            panic!("expected tui command");
        };

        assert_eq!(args.view, TuiView::Sessions);
    }

    #[test]
    fn parses_tui_sessions_view() {
        let cli = Cli::try_parse_from(["djinn", "tui", "sessions"]).unwrap();
        let Some(Command::Tui(args)) = cli.command else {
            panic!("expected tui command");
        };

        assert_eq!(args.view, TuiView::Sessions);
    }

    #[test]
    fn rejects_removed_tui_workspaces_view() {
        assert!(Cli::try_parse_from(["djinn", "tui", "workspaces"]).is_err());
    }
}
