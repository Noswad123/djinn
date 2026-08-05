#[cfg(test)]
use std::fs;
use std::io::{self, IsTerminal};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
#[cfg(test)]
use djinn_memory::AgentSession;
#[cfg(test)]
use djinn_memory::{
    ActionStore, AgentSessionEvent, AgentSessionEventKind, AgentSessionId, AgentSessionMeta,
};
#[cfg(test)]
use djinn_skills::SkillStore;
use serde::Serialize;

mod agent_ask_command;
mod agent_commands;
mod agent_config;
mod agent_file_history;
mod agent_instructions;
mod agent_messages;
mod agent_roles;
mod agent_runtime_config;
mod agent_session_meta;
mod agent_workspace;
mod background_run;
mod buddy;
mod buddy_consolidate;
mod config_commands;
mod config_doctor;
mod config_format;
mod config_model;
mod config_native;
mod config_preview;
mod config_write;
mod context_commands;
mod copilot_auth;
mod doctor_commands;
mod editor;
mod memory_commands;
mod model_completion;
mod model_resolution;
mod openai_auth;
mod path_util;
mod permission_gate;
mod policy_resolution;
mod promotion_candidate;
mod promotion_cleanup;
mod promotion_decision;
mod promotion_export;
mod promotion_generation;
mod promotion_session;
mod promotion_validation;
mod prompt;
mod session_artifact;
mod session_commands;
mod session_compact;
mod session_context;
mod session_events;
mod session_init;
mod session_list;
mod session_manifest;
mod session_native;
mod session_projection;
mod session_reference;
mod session_registry;
mod session_remove;
mod session_run_support;
mod session_status;
mod session_transcript;
mod session_tui;
mod session_turns;
mod session_watch;
mod shell;
mod skills_commands;
mod stores;
mod text;
mod toml_util;
mod tools_commands;
mod top_level_commands;
mod tui_dashboard;
pub(crate) use agent_ask_command::session_run;
use agent_ask_command::top_level_ask;
pub(crate) use agent_commands::warn_legacy_agent_command;
use agent_commands::{run_agent, run_agents};
use agent_instructions::ResolvedAgentInstruction;
use agent_roles::resolve_agent_role_selection_from_config;
pub(crate) use agent_roles::AgentRoleSelection;
pub(crate) use agent_workspace::{
    clean_unique_paths, load_djinn_config_for_workspace, resolve_agent_workspace,
};
use background_run::latest_background_session_run_status;
#[cfg(test)]
use background_run::BackgroundRunStatus;
use buddy::*;
#[cfg(test)]
use buddy_consolidate::*;
use config_commands::run_config;
#[cfg(test)]
use config_commands::validate_config_import_mode;
use config_doctor::*;
use config_model::*;
use config_native::*;
pub(crate) use context_commands::context_store;
use copilot_auth::*;
use doctor_commands::run_doctor;
pub(crate) use memory_commands::accept_memory;
pub(crate) use memory_commands::{remove_memories_silent, remove_suggestions};
use model_completion::resolve_openai_client;
use model_resolution::*;
use openai_auth::*;
pub(crate) use path_util::expand_tilde_path;
use policy_resolution::*;
#[cfg(test)]
use promotion_candidate::parse_promotion_candidate;
#[cfg(test)]
use promotion_decision::decide_promotion_session;
#[cfg(test)]
use promotion_decision::SessionDecisionAction;
#[cfg(test)]
pub(crate) use promotion_decision::{
    candidate_duplicate_similarity, decide_promotion_session_with_stores, PromotionWritebackStores,
};
#[cfg(test)]
use promotion_generation::{
    render_promotion_candidate_generation_prompt, render_promotion_generation_summary,
    write_generated_promotion_candidates, write_promotion_candidate_index,
    write_promotion_generation_summary, PromotionGeneratedCandidateReport,
};
pub(crate) use promotion_session::{create_promotion_session, session_promote_type_label};
pub(crate) use promotion_validation::SessionValidateCandidateEntry;
pub(crate) use prompt::prompt_title;
use session_artifact::SessionOpenTarget;
use session_commands::run_session;
use session_context::inspect_folder_session_context_dir;
use session_events::{
    latest_event_rebuild_backup_path, projected_event_turn_id, read_event_turn_pairs,
    validate_folder_session_events,
};
pub(crate) use session_list::list_folder_sessions_in_root;
pub(crate) use session_list::FolderSessionSummary;
use session_list::{folder_session_event_health_label, list_cache_folder_sessions};
pub(crate) use session_manifest::{
    folder_session_manifest_meta, manifest_root_string_value, parse_manifest_string_value,
    read_folder_session_manifest, session_id_from_session_dir, session_manifest_workspace_path,
    toml_string, write_agent_session_toml, FolderSessionManifest,
};
pub(crate) use session_native::{folder_agent_session_store, load_folder_native_agent_session};
#[cfg(test)]
use session_projection::project_agent_session_dir;
pub(crate) use session_projection::write_folder_session_events_jsonl;
#[cfg(test)]
use session_reference::resolve_buddy_session_reference_in_root;
pub(crate) use session_reference::{
    default_folder_session_root, folder_session_display_name, folder_session_reference_name,
    folder_session_slug, is_named_folder_session_reference, resolve_existing_folder_session_dir,
    resolve_existing_folder_session_reference, resolve_existing_folder_session_reference_in_root,
    resolve_session_dir, safe_folder_session_slug,
};
#[cfg(test)]
use session_status::format_background_promotion_run_note;
use session_status::{
    folder_session_status, format_session_candidate_entry, format_session_candidate_status,
    latest_promotion_generation_response_path, SessionStatusCandidateEntry,
};
use session_transcript::SessionTranscriptFormat;
pub(crate) use session_tui::{run_folder_session_tui, tui_candidate_row};
pub(crate) use session_turns::{
    compact_text_snippet, read_folder_session_event_turns, read_folder_session_turns,
    read_optional_markdown_file, FolderSessionTurnDigest,
};
pub(crate) use skills_commands::{open_skill_entry, skill_records, skill_store};
pub(crate) use stores::{
    action_store, agent_session_store, file_history_store, idea_store, memory_store,
    suggestion_store,
};
pub(crate) use text::{
    ensure_trailing_newline, non_empty_string, output_format, plural_suffix, push_unique_string,
    truncate, truncate_table_cell, yes_no,
};
pub(crate) use toml_util::upsert_toml_root_string;
pub(crate) use tools_commands::{open_tool_entry, scan_tools, tool_roots};
use top_level_commands::{
    run_accept, run_add, run_clear, run_index, run_ingest, run_list, run_open, run_reject,
    run_review, run_rm, run_scan, run_search, run_show, run_switch,
};
#[cfg(test)]
use tui_dashboard::dashboard_tab;
use tui_dashboard::{default_dashboard_tui_args, run_tui};

const DEFAULT_AGENT_MAX_TOOL_ROUNDS: usize = 128;
const BACKGROUND_RUN_UNRESPONSIVE_SECONDS: i64 = 30 * 60;
pub(crate) const FOLDER_SESSION_COMPACT_SNIPPET_CHARS: usize = 1_200;
pub(crate) const FOLDER_SESSION_COMPACT_START_MARKER: &str = "<!-- djinn:generated:start -->";
pub(crate) const FOLDER_SESSION_COMPACT_END_MARKER: &str = "<!-- djinn:generated:end -->";

#[derive(Debug, Parser)]
#[command(name = "djinn")]
#[command(about = "Local-first companion for OpenCode and other AI coding agents")]
struct Cli {
    /// Open Buddy mode immediately instead of the Djinn dashboard.
    #[arg(short = 'b', long = "buddy")]
    buddy: bool,
    /// Folder-backed session name, path, or Buddy id to open.
    #[arg(short = 's', long = "session", value_name = "SESSION")]
    session: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
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
struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Debug, Args)]
struct SessionArgs {
    #[command(subcommand)]
    command: Option<SessionCommand>,
    /// Folder-backed session name, path, or Buddy id for convenience actions.
    #[arg(value_name = "SESSION")]
    dir: Option<PathBuf>,
    /// Open the session summary without spelling `session open`.
    #[arg(long)]
    open: bool,
    /// Editor command for --open. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    editor: Option<String>,
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
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
struct SessionWatchArgs {
    /// Folder-backed session name, path, or Buddy id to watch.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Poll interval in milliseconds while the session is running.
    #[arg(long = "interval-ms", default_value_t = 1000)]
    interval_ms: u64,
    /// Stop watching after this many seconds. Defaults to no timeout.
    #[arg(long = "timeout-seconds")]
    timeout_seconds: Option<u64>,
    /// Output compact JSON status snapshots instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionRunArgs {
    /// Folder-backed session name, path, or Buddy id to run.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Run in the foreground and block until the answer is written. Background is the default.
    #[arg(long = "fg")]
    foreground: bool,
    /// Internal worker mode for background session runs.
    #[arg(long = "background-worker", hide = true)]
    background_worker: bool,
    /// Agent profile name override.
    #[arg(long)]
    profile: Option<String>,
    /// Configured agent role name override.
    #[arg(long)]
    agent: Option<String>,
    /// Model override. Prefix with copilot/ to use GitHub Copilot.
    #[arg(long)]
    model: Option<String>,
    /// Provider API token. For copilot/* models, this is a Copilot API token.
    #[arg(long = "api-key")]
    api_key: Option<String>,
    /// Provider endpoint/base URL. For copilot/* models, this is the chat completions endpoint.
    #[arg(long = "base-url")]
    base_url: Option<String>,
    /// Maximum model/tool-call rounds before stopping.
    #[arg(long = "max-tool-rounds", default_value_t = DEFAULT_AGENT_MAX_TOOL_ROUNDS)]
    max_tool_rounds: usize,
    /// For promotion sessions, render the model prompt without calling a model or writing candidates.
    #[arg(long)]
    dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
    /// Print the produced answer before the completion paths. Requires --fg.
    #[arg(long, conflicts_with = "json")]
    print: bool,
    /// Open summary.md after completion. Requires --fg.
    #[arg(long, conflicts_with = "json")]
    open: bool,
}

#[derive(Debug, Args)]
struct SessionChatArgs {
    /// Folder-backed session name, path, or Buddy id to open in interactive Buddy chat.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Buddy executable/command. Defaults to DJINN_BUDDY_BIN, runtime binding, then tools/buddy/bin/buddy.
    #[arg(long = "buddy-bin")]
    buddy_bin: Option<String>,
    /// Extra argument to pass through to Buddy. Repeat for multiple args.
    #[arg(long = "buddy-arg", allow_hyphen_values = true)]
    buddy_args: Vec<String>,
    /// Send request.md to Buddy and capture the final response instead of opening interactive chat.
    #[arg(long = "capture-request", visible_alias = "capture")]
    capture_request: bool,
    /// With --capture-request, preview the Buddy command and request without writing files.
    #[arg(long)]
    dry_run: bool,
    /// With --capture-request, output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionConsolidateArgs {
    /// Preview reconciliation without creating Buddy sessions, folders, or bindings.
    #[arg(long)]
    dry_run: bool,
    /// Buddy executable/command. Defaults to DJINN_BUDDY_BIN, tools/buddy/bin/buddy, then buddy.
    #[arg(long = "buddy-bin")]
    buddy_bin: Option<String>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionInitArgs {
    /// Session name or directory to create or update. Bare names live under Djinn's cache session root.
    dir: PathBuf,
    /// Target repository to link into context/<repo-name> and use for repo-local config.
    #[arg(long = "link-repo")]
    link_repo: Option<PathBuf>,
    /// Do not auto-discover repo/harness breadcrumbs when --link-repo is set.
    #[arg(long = "no-discover-context")]
    no_discover_context: bool,
    /// Agent profile name to record. Defaults through global/repo Djinn config.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Configured agent role name to record.
    #[arg(long)]
    agent: Option<String>,
    /// Model to record. Defaults through profile/agent config when available.
    #[arg(long)]
    model: Option<String>,
    /// Overwrite scaffolded files and context symlink targets when they already exist.
    #[arg(long)]
    force: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionCompactArgs {
    /// Folder-backed session name, path, or Buddy id containing events/context artifacts.
    #[arg(long = "session-dir", value_name = "SESSION")]
    session_dir: PathBuf,
    /// Output path. Defaults to <session-dir>/context/compacted.md.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
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
struct SessionDecisionArgs {
    /// Promotion session name, path, or Buddy id.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Optional candidate id/path within the promotion session. Defaults to the whole promotion outcome.
    candidate: Option<String>,
    /// Preview the decision without writing the decision record.
    #[arg(long)]
    dry_run: bool,
    /// After accepting MindWeaver todo candidates, explicitly run `mw todos sync`.
    #[arg(long = "sync-mindweaver", alias = "mw-sync")]
    sync_mindweaver: bool,
    /// Output JSON instead of a text summary.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionCleanupArgs {
    /// Promotion session name, path, or Buddy id whose source sessions should be removed.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Permanently delete source sessions recorded in context/sources.toml.
    #[arg(long)]
    delete_sources: bool,
    /// Preview source session deletion without removing anything.
    #[arg(long)]
    dry_run: bool,
    /// Output JSON instead of a text summary.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionExportPatternArgs {
    /// Pattern promotion session name, path, or Buddy id.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Optional pattern candidate id/path. Defaults to all generated pattern candidates.
    candidate: Option<String>,
    /// Markdown notes path to create or append to.
    #[arg(long = "to")]
    to: PathBuf,
    /// Append to an existing notes file. Without this, existing files are not overwritten.
    #[arg(long)]
    append: bool,
    /// Preview the exported Markdown without writing.
    #[arg(long)]
    dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionValidateCandidatesArgs {
    /// Promotion session name, path, or Buddy id.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Optional candidate id/path within the promotion session. Defaults to all candidates.
    candidate: Option<String>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionValidateEventsArgs {
    /// Folder-backed session name, path, or Buddy id to validate.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionTranscriptArgs {
    /// Folder-backed session name, path, or Buddy id to render.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = SessionTranscriptFormat::Markdown)]
    format: SessionTranscriptFormat,
    /// Shortcut for --format json.
    #[arg(long, conflicts_with = "format")]
    json: bool,
    /// Write transcript to this path instead of stdout.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Write/open the Markdown transcript. Defaults to <session>/transcript.md.
    #[arg(long)]
    open: bool,
    /// Editor command for --open. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    editor: Option<String>,
}

#[derive(Debug, Args)]
struct SessionEventsArgs {
    /// Folder-backed session name, path, or Buddy id to project from.
    #[arg(required_unless_present = "all", value_name = "SESSION")]
    dir: Option<PathBuf>,
    /// Report event-ledger health for all cache-backed sessions.
    #[arg(long, conflicts_with_all = ["dir", "write", "restore"])]
    all: bool,
    /// Maximum cache-backed sessions to include with --all.
    #[arg(long, requires = "all")]
    limit: Option<usize>,
    /// With --all, include only ready, not-ready, missing, or matching issue-code sessions.
    #[arg(long = "health", requires = "all", value_name = "FILTER")]
    health_filter: Option<String>,
    /// With --all, exit with an error when any reported session is not ready.
    #[arg(long, requires = "all")]
    strict: bool,
    /// Rebuild turns/ and summary.md from events.jsonl after creating a backup.
    #[arg(long)]
    write: bool,
    /// Restore turns/ and summary.md from a .djinn/backups/events-rebuild-* backup. Without --write, preview only.
    #[arg(long, value_name = "BACKUP")]
    restore: Option<PathBuf>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
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
struct SessionContextArgs {
    #[command(subcommand)]
    command: SessionContextCommand,
}

#[derive(Debug, Subcommand)]
enum SessionContextCommand {
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
struct SessionContextDiscoverArgs {
    /// Folder-backed session name, path, or Buddy id to update.
    #[arg(value_name = "SESSION")]
    session: PathBuf,
    /// Preview discoveries without creating links or repo-index.md.
    #[arg(long = "dry-run")]
    dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionContextLsArgs {
    /// Folder-backed session name, path, or Buddy id to inspect.
    #[arg(value_name = "SESSION")]
    session: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionContextAddArgs {
    /// Folder-backed session name, path, or Buddy id to update.
    #[arg(value_name = "SESSION")]
    session: PathBuf,
    /// File or directory to link into context/.
    path: PathBuf,
    /// Context entry name. Defaults to the source basename.
    #[arg(long)]
    name: Option<String>,
    /// Replace an existing file/link/directory under context/.
    #[arg(long)]
    force: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionContextRmArgs {
    /// Folder-backed session name, path, or Buddy id to update.
    #[arg(value_name = "SESSION")]
    session: PathBuf,
    /// Context entry name to remove.
    name: String,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionStatusArgs {
    /// Folder-backed session name, path, or Buddy id to inspect.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionLsArgs {
    /// Maximum folder sessions to list.
    #[arg(long)]
    limit: Option<usize>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionShortenNamesArgs {
    /// Show planned renames without changing folder names.
    #[arg(long = "dry-run")]
    dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionRenameArgs {
    /// Folder-backed session name, path, or Buddy id to rename.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// New cache-backed session folder name.
    #[arg(value_name = "NEW_NAME")]
    new_name: String,
    /// Show the planned rename without changing folders.
    #[arg(long = "dry-run")]
    dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionOpenArgs {
    /// Folder-backed session name, path, or Buddy id to open.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Session artifact to open. Defaults to summary.md.
    #[arg(value_enum, default_value_t = SessionOpenTarget::Summary)]
    target: SessionOpenTarget,
    /// Editor command. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    editor: Option<String>,
}

#[derive(Debug, Args)]
struct SessionRmArgs {
    /// Folder-backed session name, path, or Buddy id to remove.
    #[arg(value_name = "SESSION")]
    dir: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Add or update a provider credential.
    Login(AuthLoginArgs),
}

#[derive(Debug, Args)]
struct AuthLoginArgs {
    /// Provider id. Defaults to an interactive provider picker.
    #[arg(long, value_enum)]
    provider: Option<AuthProvider>,
    /// Login method. Defaults to an interactive method picker.
    #[arg(long, value_enum)]
    method: Option<OpenAiLoginMethod>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AuthProvider {
    Openai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OpenAiLoginMethod {
    Browser,
    Headless,
    ApiKey,
}

#[derive(Debug, Args)]
struct ListArgs {
    #[command(subcommand)]
    noun: ListNoun,
}

#[derive(Debug, Subcommand)]
enum ListNoun {
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
struct ShowArgs {
    #[command(subcommand)]
    noun: ShowNoun,
}

#[derive(Debug, Subcommand)]
enum ShowNoun {
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
struct AddArgs {
    #[command(subcommand)]
    noun: AddNoun,
}

#[derive(Debug, Subcommand)]
enum AddNoun {
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
struct AcceptArgs {
    #[command(subcommand)]
    noun: AcceptNoun,
}

#[derive(Debug, Subcommand)]
enum AcceptNoun {
    /// Review a memory and produce suggestions.
    Memory(AcceptMemoryArgs),
    /// Mark a suggestion as done and remove it from the suggestion list.
    Suggestion { id: String },
}

#[derive(Debug, Args)]
struct RejectArgs {
    #[command(subcommand)]
    noun: RejectNoun,
}

#[derive(Debug, Subcommand)]
enum RejectNoun {
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
struct IngestArgs {
    #[command(subcommand)]
    noun: IngestNoun,
}

#[derive(Debug, Subcommand)]
enum IngestNoun {
    /// Route active memories into the right downstream collection.
    Memories(IngestMemoriesArgs),
    /// Route one active memory into the right downstream collection.
    Memory(IngestMemoriesArgs),
}

#[derive(Debug, Args)]
struct IngestMemoriesArgs {
    /// Memory ids or text fragments to ingest.
    #[arg(required = true)]
    ids: Vec<String>,
    /// Destination collection. `auto` uses memory kind text.
    #[arg(long = "as", value_enum, default_value_t = IngestTarget::Auto)]
    target: IngestTarget,
    /// Keep memories after ingesting instead of consuming them.
    #[arg(long)]
    keep: bool,
    /// Overwrite an existing Djinn-managed skill when ingesting as a skill.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum IngestTarget {
    Auto,
    Memory,
    Suggestion,
    Skill,
    Idea,
    Action,
}

#[derive(Debug, Args)]
struct ReviewArgs {
    #[command(subcommand)]
    source: ReviewSource,
}

#[derive(Debug, Subcommand)]
enum ReviewSource {
    /// Ask OpenCode to review one or more memories and create suggestions.
    Memories(ReviewMemoriesArgs),
    /// Ask OpenCode to review one memory and create suggestions.
    Memory(ReviewMemoriesArgs),
}

#[derive(Debug, Args)]
struct ReviewMemoriesArgs {
    /// Optional memory ids or text fragments to review.
    ids: Vec<String>,
    /// Maximum memories to include unless --all is used.
    #[arg(long, default_value_t = 100)]
    limit: usize,
    /// Review all matching memories instead of applying --limit.
    #[arg(long)]
    all: bool,
    /// Optional query filter over memory id, text, metadata, and evidence.
    #[arg(long)]
    query: Option<String>,
    /// OpenCode agent to use for the review.
    #[arg(long)]
    agent: Option<String>,
    /// OpenCode run title.
    #[arg(long, default_value = "djinn memory curation review")]
    title: String,
    /// OpenCode binary to execute.
    #[arg(long, default_value = "opencode")]
    opencode_bin: String,
    /// Print the prompt instead of running OpenCode.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct RmArgs {
    #[command(subcommand)]
    noun: RmNoun,
}

#[derive(Debug, Subcommand)]
enum RmNoun {
    /// Remove a memory matching a keyword.
    Memory { keyword: String },
    /// Remove or archive a skill.
    Skill(RmSkillArgs),
}

#[derive(Debug, Args)]
struct ClearArgs {
    #[command(subcommand)]
    noun: ClearNoun,
}

#[derive(Debug, Subcommand)]
enum ClearNoun {
    /// Clear all memories after interactive confirmation.
    Memories {
        /// Skip creating memories.backup-*.jsonl before clearing.
        #[arg(long)]
        no_backup: bool,
    },
}

#[derive(Debug, Args)]
struct ScanArgs {
    #[command(subcommand)]
    noun: ScanNoun,
}

#[derive(Debug, Subcommand)]
enum ScanNoun {
    /// Scan local tools and print a summary.
    Tools(ToolsScope),
}

#[derive(Debug, Args)]
struct IndexArgs {
    #[command(subcommand)]
    noun: IndexNoun,
}

#[derive(Debug, Subcommand)]
enum IndexNoun {
    /// Write the local tools JSON index.
    Tools(IndexToolsArgs),
}

#[derive(Debug, Args)]
struct SearchArgs {
    #[command(subcommand)]
    noun: SearchNoun,
}

#[derive(Debug, Subcommand)]
enum SearchNoun {
    /// Search local tools.
    Tools(SearchToolsArgs),
    /// Search memories.
    Memories { query: String },
    /// Search suggestions.
    Suggestions { query: String },
}

#[derive(Debug, Args)]
struct SwitchArgs {
    #[command(subcommand)]
    noun: SwitchNoun,
}

#[derive(Debug, Subcommand)]
enum SwitchNoun {
    /// Switch the active context.
    Ctx {
        /// Context name, case-insensitive. Falls back to substring matching.
        name: String,
    },
}

#[derive(Debug, Args)]
struct OpenArgs {
    #[command(subcommand)]
    noun: OpenNoun,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[command(subcommand)]
    command: DoctorCommand,
}

#[derive(Debug, Subcommand)]
enum DoctorCommand {
    /// Show which Buddy command Djinn will use without launching Buddy.
    Buddy(DoctorBuddyArgs),
}

#[derive(Debug, Args)]
struct DoctorBuddyArgs {
    /// Folder-backed session name or directory whose runtime/buddy.json should be considered.
    #[arg(short = 's', long = "session", value_name = "SESSION")]
    session: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Show Djinn's native config, merged from discovered config files.
    Show(ConfigShowArgs),
    /// Diagnose how an external harness config maps into Djinn concepts.
    Doctor(ConfigDoctorArgs),
    /// Preview importing an external harness config into Djinn-native config.
    Import(ConfigImportArgs),
    /// Preview exporting Djinn-native config into an external harness format.
    Export(ConfigExportArgs),
}

#[derive(Debug, Args)]
struct ConfigShowArgs {
    /// Djinn config file path to load. Defaults to discovered native config paths.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ConfigImportArgs {
    #[command(subcommand)]
    source: ConfigImportSource,
}

#[derive(Debug, Args)]
struct ConfigExportArgs {
    #[command(subcommand)]
    target: ConfigExportTarget,
}

#[derive(Debug, Subcommand)]
enum ConfigExportTarget {
    /// Export native Djinn config as GitHub Copilot CLI config.
    Copilot(ConfigExportCopilotArgs),
    /// Export native Djinn config as OpenCode config.
    Opencode(ConfigExportOpencodeArgs),
}

#[derive(Debug, Args)]
struct ConfigExportCopilotArgs {
    /// Djinn config file path to export. Defaults to discovered native config paths.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Preview the export without writing files.
    #[arg(long)]
    dry_run: bool,
    /// Write the exported Copilot config.
    #[arg(long)]
    write: bool,
    /// Destination Copilot config file. Defaults to ~/.config/github-copilot/config.json.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Allow --write to replace an existing destination file.
    #[arg(long)]
    force: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ConfigExportOpencodeArgs {
    /// Djinn config file path to export. Defaults to discovered native config paths.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Preview the export without writing files.
    #[arg(long)]
    dry_run: bool,
    /// Write the exported OpenCode config.
    #[arg(long)]
    write: bool,
    /// Destination OpenCode config file. Defaults to ~/.config/opencode/opencode.json.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Allow --write to replace an existing destination file.
    #[arg(long)]
    force: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum ConfigImportSource {
    /// Import GitHub Copilot CLI config.
    Copilot(ConfigImportCopilotArgs),
    /// Import OpenCode config.
    Opencode(ConfigImportOpencodeArgs),
}

#[derive(Debug, Args)]
struct ConfigImportCopilotArgs {
    /// Copilot config file path to inspect. Defaults to discovered GitHub Copilot config paths.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Preview the import without writing files.
    #[arg(long)]
    dry_run: bool,
    /// Write the imported Djinn-native config.
    #[arg(long)]
    write: bool,
    /// Destination Djinn config file. Defaults to ~/.config/djinn/config.json.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Explicitly merge into an existing destination file. This is the default write behavior.
    #[arg(long, requires = "write", conflicts_with = "force")]
    merge: bool,
    /// Allow --write to replace an existing destination file.
    #[arg(long)]
    force: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ConfigImportOpencodeArgs {
    /// OpenCode config file path to inspect. Defaults to Djinn's discovered source paths.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Preview the import without writing files.
    #[arg(long)]
    dry_run: bool,
    /// Write the imported Djinn-native config.
    #[arg(long)]
    write: bool,
    /// Destination Djinn config file. Defaults to ~/.config/djinn/config.json.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Explicitly merge into an existing destination file. This is the default write behavior.
    #[arg(long, requires = "write", conflicts_with = "force")]
    merge: bool,
    /// Allow --write to replace an existing destination file.
    #[arg(long)]
    force: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ConfigDoctorArgs {
    /// External config source to inspect.
    #[arg(long, value_enum, default_value_t = ConfigSource::Djinn)]
    source: ConfigSource,
    /// Config file path to inspect. Defaults to Djinn's discovered source paths.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ConfigSource {
    Copilot,
    Djinn,
    Opencode,
}

#[derive(Debug, Subcommand)]
enum OpenNoun {
    /// Open a local tool source by name.
    Tool(OpenToolArgs),
}

#[derive(Debug, Args)]
struct AgentArgs {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Debug, Args)]
struct AgentsArgs {
    #[command(subcommand)]
    command: AgentsCommand,
}

#[derive(Debug, Subcommand)]
enum AgentsCommand {
    /// List configured Djinn agent roles.
    List(AgentsListArgs),
    /// Show one configured Djinn agent role.
    Show(AgentsShowArgs),
}

#[derive(Debug, Args)]
struct AgentsListArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentsShowArgs {
    /// Agent role name, case-insensitive. Falls back to substring matching.
    name: String,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
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
struct AgentConfigArgs {
    #[command(subcommand)]
    command: AgentConfigCommand,
}

#[derive(Debug, Args)]
struct AgentToolsArgs {
    #[command(subcommand)]
    command: AgentToolsCommand,
}

#[derive(Debug, Args)]
struct AgentPolicyArgs {
    #[command(subcommand)]
    command: AgentPolicyCommand,
}

#[derive(Debug, Subcommand)]
enum AgentConfigCommand {
    /// List discovered agent profiles and models.
    List(AgentConfigListArgs),
    /// Show the effective agent runtime configuration.
    Show(AgentConfigShowArgs),
}

#[derive(Debug, Subcommand)]
enum AgentToolsCommand {
    /// List built-in tools exposed to the agent runtime.
    List(AgentToolsListArgs),
    /// Show one built-in agent tool spec.
    Show(AgentToolsShowArgs),
}

#[derive(Debug, Subcommand)]
enum AgentPolicyCommand {
    /// List the effective read/permission policy and guardrails.
    List(AgentPolicyListArgs),
    /// Audit effective policy for durable grants and high-attention behavior.
    Audit(AgentPolicyAuditArgs),
    /// Revoke stored durable approvals. Currently reports no-op until durable approvals exist.
    Revoke(AgentPolicyRevokeArgs),
}

#[derive(Debug, Args)]
struct AgentPolicyListArgs {
    /// Workspace path to resolve. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Configured agent role name.
    #[arg(long)]
    agent: Option<String>,
    /// OpenAI model to use. Defaults the same way as folder-backed asks.
    #[arg(long)]
    model: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentPolicyAuditArgs {
    /// Workspace path to resolve. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Configured agent role name.
    #[arg(long)]
    agent: Option<String>,
    /// OpenAI model to use. Defaults the same way as folder-backed asks.
    #[arg(long)]
    model: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentPolicyRevokeArgs {
    /// Optional action selector for future durable approvals, such as shell or write.
    #[arg(long)]
    action: Option<String>,
    /// Optional resource/path selector for future durable approvals.
    #[arg(long)]
    resource: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentFileHistoryArgs {
    #[command(subcommand)]
    command: AgentFileHistoryCommand,
}

#[derive(Debug, Args)]
struct AgentConfigListArgs {
    /// Agent profile to treat as current.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Model to treat as current. Defaults the same way as folder-backed asks.
    #[arg(long)]
    model: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentConfigShowArgs {
    /// Workspace path to resolve. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Configured agent role name.
    #[arg(long)]
    agent: Option<String>,
    /// OpenAI model to use. Defaults the same way as folder-backed asks.
    #[arg(long)]
    model: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentToolsListArgs {
    /// Workspace path used to resolve profile permissions. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Agent profile name used for read/permission policy resolution.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Configured agent role name.
    #[arg(long)]
    agent: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentToolsShowArgs {
    /// Tool name, case-insensitive. Falls back to substring matching.
    name: String,
    /// Workspace path used to resolve profile permissions. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Agent profile name used for read/permission policy resolution.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Configured agent role name.
    #[arg(long)]
    agent: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum AgentFileHistoryCommand {
    /// List apply_patch file-history entries.
    List(AgentFileHistoryListArgs),
    /// Restore one apply_patch preimage entry.
    Restore(AgentFileHistoryRestoreArgs),
}

#[derive(Debug, Args, Clone)]
struct ToolsScope {
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct IndexToolsArgs {
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Index JSON path. Defaults under the scanned root.
    #[arg(long)]
    index: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ToolLookupArgs {
    /// Tool name, case-insensitive. Falls back to substring matching.
    name: String,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SearchToolsArgs {
    query: String,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ListSkillsArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ShowSkillArgs {
    /// Skill name, case-insensitive. Falls back to substring matching.
    name: String,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AddSkillArgs {
    /// Skill name to scaffold under ~/.config/djinn/skills.
    name: String,
    /// One-line skill description.
    #[arg(long)]
    description: Option<String>,
    /// Overwrite an existing Djinn-managed skill scaffold.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct RmSkillArgs {
    /// Skill name, case-insensitive. Only Djinn-managed skills can be removed.
    name: String,
}

#[derive(Debug, Args)]
struct ListCtxArgs {
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ShowCtxArgs {
    /// Context name. Defaults to the active context.
    name: Option<String>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AddCtxArgs {
    /// Context name.
    name: String,
    /// Human-friendly description.
    #[arg(long)]
    description: Option<String>,
    /// Tool/project root for this context. Repeatable.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Skill root for this context. Repeatable.
    #[arg(long = "skill-root")]
    skill_roots: Vec<PathBuf>,
    /// Default memory scope, for example: project:djinn.
    #[arg(long = "memory-scope")]
    memory_scope: Option<String>,
    /// Make this context active after adding/updating it.
    #[arg(long)]
    switch: bool,
}

#[derive(Debug, Args)]
struct OpenToolArgs {
    /// Tool name, case-insensitive. Falls back to substring matching.
    name: String,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Editor command. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    editor: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TuiArgs {
    /// TUI view to open. Defaults to sessions.
    #[arg(value_enum, default_value_t = TuiView::Sessions)]
    view: TuiView,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
    /// Editor command for opening tools. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    editor: Option<String>,
}

#[derive(Debug, Args)]
struct AgentFileHistoryListArgs {
    /// Filter by exact patch id.
    #[arg(long = "patch-id")]
    patch_id: Option<String>,
    /// Filter by exact workspace string.
    #[arg(long)]
    workspace: Option<String>,
    /// Maximum entries to list.
    #[arg(long)]
    limit: Option<usize>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentFileHistoryRestoreArgs {
    /// File-history entry id to restore.
    id: String,
    /// Overwrite an existing preimage target, or remove an existing tombstone target.
    #[arg(long)]
    force: bool,
    /// For move entries, also remove the recorded new_path file if it exists.
    #[arg(long = "remove-new-path")]
    remove_new_path: bool,
    /// Validate and show what would happen without changing files.
    #[arg(long = "dry-run")]
    dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentAskArgs {
    /// Prompt to send to the configured agent provider.
    prompt: Option<String>,
    /// Existing Djinn agent session id to append this ask turn to.
    #[arg(long = "session-id")]
    session_id: Option<String>,
    /// Folder-backed session name or directory. Bare names live under Djinn's cache session root.
    #[arg(long = "session-dir", visible_alias = "session")]
    session_dir: Option<PathBuf>,
    /// Human-friendly session title. Defaults to a trimmed prompt preview.
    #[arg(long)]
    title: Option<String>,
    /// Workspace path for the session. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long)]
    profile: Option<String>,
    /// Configured agent role name.
    #[arg(long)]
    agent: Option<String>,
    /// Parent agent session id for explicit related-session workflows.
    #[arg(long = "parent-session")]
    parent_session: Option<String>,
    /// Model to use. Prefix with copilot/ to use GitHub Copilot.
    #[arg(long)]
    model: Option<String>,
    /// Provider API token. For copilot/* models, this is a Copilot API token.
    #[arg(long = "api-key")]
    api_key: Option<String>,
    /// Provider endpoint/base URL. For copilot/* models, this is the chat completions endpoint.
    #[arg(long = "base-url")]
    base_url: Option<String>,
    /// Maximum model/tool-call rounds before stopping.
    #[arg(long = "max-tool-rounds", default_value_t = DEFAULT_AGENT_MAX_TOOL_ROUNDS)]
    max_tool_rounds: usize,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
    /// Print the produced answer instead of the default folder path output.
    #[arg(long, conflicts_with = "json")]
    print: bool,
    /// Open the produced summary.md after an auto-created folder-backed ask completes.
    #[arg(long, conflicts_with_all = ["json", "session_id", "session_dir"])]
    open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TuiView {
    Tools,
    Sessions,
    Memories,
    Suggestions,
    Skills,
}

#[derive(Debug, Args)]
struct AddMemoryArgs {
    /// Durable memory text.
    text: String,
    /// Scope for the memory, for example: global, project, repo, work, personal.
    #[arg(long)]
    scope: Option<String>,
    /// Memory kind, for example: preference, convention, workaround, correction.
    #[arg(long)]
    kind: Option<String>,
    /// Confidence label, for example: low, medium, high.
    #[arg(long)]
    confidence: Option<String>,
    /// Do not act on this memory before this date, for example: 2026-10-01.
    #[arg(long = "not-before")]
    not_before: Option<String>,
    /// Durable copied evidence explaining why this memory exists. Repeatable.
    #[arg(long = "evidence")]
    evidence: Vec<String>,
}

#[derive(Debug, Args)]
struct AddSuggestionArgs {
    /// Suggested action or artifact to consider.
    text: String,
    /// Suggested target, for example: skill, action, idea, config, code, docs.
    #[arg(long)]
    target: Option<String>,
    /// Why this suggestion is worth considering.
    #[arg(long)]
    rationale: Option<String>,
    /// Optional draft content or implementation sketch.
    #[arg(long)]
    draft: Option<String>,
    /// Copied evidence supporting this suggestion. Repeatable.
    #[arg(long = "evidence")]
    evidence: Vec<String>,
    /// Memory id or text fragment to attach as evidence. Repeatable.
    #[arg(long = "source-memory")]
    source_memories: Vec<String>,
}

#[derive(Debug, Args)]
struct AcceptMemoryArgs {
    /// Memory id or text fragment.
    id: String,
    /// OpenCode agent to use for the review.
    #[arg(long)]
    agent: Option<String>,
    /// OpenCode run title.
    #[arg(long, default_value = "djinn memory suggestion review")]
    title: String,
    /// OpenCode binary to execute.
    #[arg(long, default_value = "opencode")]
    opencode_bin: String,
    /// Print the prompt instead of running OpenCode.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.buddy {
        if cli.command.is_some() {
            bail!("-b/--buddy opens Buddy mode and cannot be combined with a Djinn subcommand");
        }
        return run_top_level_buddy_mode(cli.session);
    }
    if let Some(session) = cli.session {
        if cli.command.is_some() {
            bail!("-s/--session opens a focused folder session and cannot be combined with a Djinn subcommand unless -b/--buddy is also set");
        }
        return run_session(SessionArgs {
            command: None,
            dir: Some(session),
            open: false,
            editor: None,
        });
    }
    let Some(command) = cli.command else {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            return run_tui(default_dashboard_tui_args());
        }
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };
    match command {
        Command::List(args) => run_list(args),
        Command::Show(args) => run_show(args),
        Command::Add(args) => run_add(args),
        Command::Accept(args) => run_accept(args),
        Command::Reject(args) => run_reject(args),
        Command::Ingest(args) => run_ingest(args),
        Command::Review(args) => run_review(args),
        Command::Rm(args) => run_rm(args),
        Command::Clear(args) => run_clear(args),
        Command::Scan(args) => run_scan(args),
        Command::Index(args) => run_index(args),
        Command::Search(args) => run_search(args),
        Command::Switch(args) => run_switch(args),
        Command::Open(args) => run_open(args),
        Command::Config(args) => run_config(args),
        Command::Doctor(args) => run_doctor(args),
        Command::Auth(args) => run_auth(args),
        Command::Ask(args) => top_level_ask(args),
        Command::Session(args) => run_session(args),
        Command::Agent(args) => run_agent(args),
        Command::Agents(args) => run_agents(args),
        Command::Tui(args) => run_tui(args),
    }
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
    fn parses_config_doctor_opencode_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "doctor",
            "--source",
            "opencode",
            "--path",
            "/tmp/opencode.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Doctor(args) = args.command else {
            panic!("expected config doctor command");
        };

        assert_eq!(args.source, ConfigSource::Opencode);
        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/opencode.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_doctor_copilot_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "doctor",
            "--source",
            "copilot",
            "--path",
            "/tmp/copilot.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Doctor(args) = args.command else {
            panic!("expected config doctor command");
        };

        assert_eq!(args.source, ConfigSource::Copilot);
        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/copilot.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_doctor_buddy_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "doctor",
            "buddy",
            "--session",
            "rebrand-opencode",
            "--json",
        ])
        .unwrap();

        let Some(Command::Doctor(args)) = cli.command else {
            panic!("expected doctor command");
        };
        let DoctorCommand::Buddy(args) = args.command;

        assert_eq!(args.session.as_deref(), Some(Path::new("rebrand-opencode")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_show_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "show",
            "--path",
            "/tmp/djinn.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Show(args) = args.command else {
            panic!("expected config show command");
        };

        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_auth_login_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "auth",
            "login",
            "--provider",
            "openai",
            "--method",
            "api-key",
        ])
        .unwrap();

        let Some(Command::Auth(args)) = cli.command else {
            panic!("expected auth command");
        };
        let AuthCommand::Login(args) = args.command;

        assert_eq!(args.provider, Some(AuthProvider::Openai));
        assert_eq!(args.method, Some(OpenAiLoginMethod::ApiKey));
    }

    #[test]
    fn parses_agents_list_and_show_commands() {
        let cli = Cli::try_parse_from(["djinn", "agents", "list", "--json"]).unwrap();
        let Some(Command::Agents(args)) = cli.command else {
            panic!("expected agents command");
        };
        let AgentsCommand::List(args) = args.command else {
            panic!("expected agents list command");
        };
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "agents", "show", "reviewer", "--json"]).unwrap();
        let Some(Command::Agents(args)) = cli.command else {
            panic!("expected agents command");
        };
        let AgentsCommand::Show(args) = args.command else {
            panic!("expected agents show command");
        };
        assert_eq!(args.name, "reviewer");
        assert!(args.json);
    }

    #[test]
    fn parses_config_import_opencode_dry_run_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "import",
            "opencode",
            "--dry-run",
            "--path",
            "/tmp/opencode.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Import(args) = args.command else {
            panic!("expected config import command");
        };
        let ConfigImportSource::Opencode(args) = args.source else {
            panic!("expected opencode import source");
        };

        assert!(args.dry_run);
        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/opencode.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_import_copilot_write_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "import",
            "copilot",
            "--write",
            "--output",
            "/tmp/djinn.json",
            "--force",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Import(args) = args.command else {
            panic!("expected config import command");
        };
        let ConfigImportSource::Copilot(args) = args.source else {
            panic!("expected copilot import source");
        };

        assert!(args.write);
        assert!(!args.merge);
        assert!(args.force);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_import_merge_alias_and_rejects_force_conflict() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "import",
            "opencode",
            "--write",
            "--merge",
            "--output",
            "/tmp/djinn.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Import(args) = args.command else {
            panic!("expected config import command");
        };
        let ConfigImportSource::Opencode(args) = args.source else {
            panic!("expected opencode import source");
        };

        assert!(args.write);
        assert!(args.merge);
        assert!(!args.force);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);

        let conflict = Cli::try_parse_from([
            "djinn", "config", "import", "copilot", "--write", "--merge", "--force",
        ]);
        assert!(conflict.is_err());

        assert!(validate_config_import_mode(false, true, true, false).is_ok());
        assert!(validate_config_import_mode(false, true, true, true).is_err());
        assert!(validate_config_import_mode(true, false, true, false).is_err());
    }

    #[test]
    fn parses_config_import_opencode_write_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "import",
            "opencode",
            "--write",
            "--output",
            "/tmp/djinn.json",
            "--force",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Import(args) = args.command else {
            panic!("expected config import command");
        };
        let ConfigImportSource::Opencode(args) = args.source else {
            panic!("expected opencode import source");
        };

        assert!(args.write);
        assert!(!args.merge);
        assert!(args.force);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_export_opencode_dry_run_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "export",
            "opencode",
            "--dry-run",
            "--path",
            "/tmp/djinn.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Export(args) = args.command else {
            panic!("expected config export command");
        };
        let ConfigExportTarget::Opencode(args) = args.target else {
            panic!("expected opencode export target");
        };

        assert!(args.dry_run);
        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_export_copilot_write_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "export",
            "copilot",
            "--write",
            "--output",
            "/tmp/copilot.json",
            "--force",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Export(args) = args.command else {
            panic!("expected config export command");
        };
        let ConfigExportTarget::Copilot(args) = args.target else {
            panic!("expected copilot export target");
        };

        assert!(args.write);
        assert!(args.force);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/copilot.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_export_opencode_write_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "export",
            "opencode",
            "--write",
            "--output",
            "/tmp/opencode.json",
            "--force",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Export(args) = args.command else {
            panic!("expected config export command");
        };
        let ConfigExportTarget::Opencode(args) = args.target else {
            panic!("expected opencode export target");
        };

        assert!(args.write);
        assert!(args.force);
        assert_eq!(
            args.output.as_deref(),
            Some(Path::new("/tmp/opencode.json"))
        );
        assert!(args.json);
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
    fn session_buddy_captures_final_response_into_folder_session() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "djinn-buddy-session-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("djinn.toml"),
            "session_id = \"agt_buddy\"\ntitle = \"Buddy Test\"\nworkspace = \"/tmp/workspace\"\n",
        )
        .unwrap();
        fs::write(dir.join("request.md"), "Please answer from Buddy.\n").unwrap();
        fs::write(dir.join("summary.md"), "old summary\n").unwrap();

        let prompt_seen = dir.join("prompt-seen.txt");
        let args_seen = dir.join("args-seen.txt");
        let buddy_bin = dir.join("buddy-test.sh");
        fs::write(
            &buddy_bin,
            format!(
                "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'Buddy final response.\\n'\n",
                prompt_seen.display(),
                args_seen.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&buddy_bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&buddy_bin, permissions).unwrap();
        }

        let report = run_session_buddy(&SessionBuddyRunArgs {
            dir: dir.clone(),
            buddy_bin: Some(buddy_bin.display().to_string()),
            buddy_session: Some("bud_test".to_string()),
            buddy_args: vec!["--final".to_string()],
            dry_run: false,
        })
        .unwrap();

        assert!(!report.dry_run);
        assert!(report.wrote_summary);
        assert!(report.appended_events);
        assert!(report.cleared_request);
        assert_eq!(report.buddy_session.as_deref(), Some("bud_test"));
        assert_eq!(fs::read_to_string(dir.join("request.md")).unwrap(), "");
        assert_eq!(
            fs::read_to_string(dir.join("summary.md")).unwrap(),
            "Buddy final response.\n"
        );
        assert_eq!(
            fs::read_to_string(&prompt_seen).unwrap(),
            "Please answer from Buddy.\n"
        );
        assert_eq!(
            fs::read_to_string(&args_seen).unwrap(),
            "-s\nbud_test\n--final\n"
        );

        let events = fs::read_to_string(dir.join("events.jsonl")).unwrap();
        assert_eq!(events.lines().count(), 2);
        assert!(events.contains("Please answer from Buddy."));
        assert!(events.contains("Buddy final response."));
        let runtime = fs::read_to_string(dir.join("runtime/buddy.json")).unwrap();
        assert!(runtime.contains("bud_test"));
        assert!(runtime.contains("--final"));
        assert!(format_session_buddy_report(&report).contains("Buddy capture:"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_consolidate_reconciles_djinn_and_buddy_sessions() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-consolidate-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        fs::write(
            alpha.join("djinn.toml"),
            "title = \"Alpha\"\nworkspace = \"/tmp/repo-a\"\n\n[context.repo]\npath = \"/tmp/repo-a\"\n",
        )
        .unwrap();
        fs::write(alpha.join("summary.md"), "alpha summary\n").unwrap();
        fs::write(
            beta.join("djinn.toml"),
            "title = \"Beta\"\nworkspace = \"/tmp/repo-c\"\n\n[context.repo]\npath = \"/tmp/repo-c\"\n",
        )
        .unwrap();
        fs::write(beta.join("summary.md"), "beta summary\n").unwrap();

        let create_log = root.join("create-log.txt");
        let buddy_bin = root.join("buddy-json.sh");
        fs::write(
            &buddy_bin,
            "#!/bin/sh\nif [ \"$1\" = \"session\" ] && [ \"$2\" = \"list\" ] && [ \"$3\" = \"--format\" ] && [ \"$4\" = \"json\" ]; then\n  cat <<'JSON'\n[{\"id\":\"bud_alpha\",\"title\":\"Alpha\",\"updated\":1785599577905,\"created\":1785081429401,\"projectId\":\"project-a\",\"directory\":\"/tmp/repo-a\"},{\"id\":\"bud_orphan\",\"title\":\"Orphan Buddy\",\"updated\":1785595306273,\"created\":1785595040658,\"projectId\":\"project-b\",\"directory\":\"/tmp/repo-b\"}]\nJSON\n  exit 0\nfi\nif [ \"$1\" = \"session\" ] && [ \"$2\" = \"create\" ]; then\n  printf '%s|%s\\n' \"$6\" \"$8\" >> '__CREATE_LOG__'\n  printf '{\"id\":\"bud_created_beta\",\"title\":\"%s\",\"repo_path\":\"%s\",\"created_at\":\"2026-08-01T12:00:00Z\"}\\n' \"$6\" \"$8\"\n  exit 0\nfi\necho unexpected buddy args: \"$@\" >&2\nexit 2\n"
                .replace("__CREATE_LOG__", &create_log.display().to_string()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&buddy_bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&buddy_bin, permissions).unwrap();
        }

        let dry_run = consolidate_sessions_in_root(
            &root,
            &SessionConsolidateArgs {
                dry_run: true,
                buddy_bin: Some(buddy_bin.display().to_string()),
                json: false,
            },
        )
        .unwrap();

        assert!(dry_run.dry_run);
        assert_eq!(dry_run.total_djinn_sessions, 2);
        assert_eq!(dry_run.total_buddy_sessions, 2);
        assert_eq!(dry_run.matched_existing, 1);
        assert_eq!(dry_run.created_buddy_sessions, 1);
        assert_eq!(dry_run.adopted_buddy_sessions, 1);
        assert!(dry_run
            .entries
            .iter()
            .any(|entry| entry.action == "would_match_existing_buddy"
                && entry.buddy_session.as_deref() == Some("bud_alpha")));
        assert!(!alpha.join("runtime/buddy.json").exists());
        assert!(!create_log.exists());

        let report = consolidate_sessions_in_root(
            &root,
            &SessionConsolidateArgs {
                dry_run: false,
                buddy_bin: Some(buddy_bin.display().to_string()),
                json: false,
            },
        )
        .unwrap();

        assert!(!report.dry_run);
        assert_eq!(report.matched_existing, 1);
        assert_eq!(report.created_buddy_sessions, 1);
        assert_eq!(report.adopted_buddy_sessions, 1);
        assert!(fs::read_to_string(alpha.join("runtime/buddy.json"))
            .unwrap()
            .contains("bud_alpha"));
        assert!(fs::read_to_string(beta.join("runtime/buddy.json"))
            .unwrap()
            .contains("bud_created_beta"));
        assert_eq!(
            fs::read_to_string(&create_log).unwrap(),
            "beta|/tmp/repo-c\n"
        );
        let orphan = root.join("orphan_buddy-bud_orphan");
        assert!(orphan.join("djinn.toml").exists());
        assert!(orphan.join("summary.md").exists());
        assert!(fs::read_to_string(orphan.join("runtime/buddy.json"))
            .unwrap()
            .contains("bud_orphan"));
        assert!(format_session_consolidate_report(&report).contains("created_buddy_for_folder"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn buddy_session_reference_resolves_to_bound_folder_session() {
        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-ref-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("from-buddy");
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        fs::write(
            session_dir.join("runtime/buddy.json"),
            serde_json::json!({
                "buddy_session": "ses_boundBuddy123",
                "command": "buddy",
                "args": [],
                "last_run_at": null,
                "last_prompt_chars": 0,
                "last_response_chars": 0
            })
            .to_string(),
        )
        .unwrap();

        let resolved =
            resolve_buddy_session_reference_in_root(&root, Path::new("ses_boundBuddy123")).unwrap();

        assert_eq!(
            resolved,
            Some((session_dir.clone(), "ses_boundBuddy123".to_string()))
        );

        let missing =
            resolve_buddy_session_reference_in_root(&root, Path::new("ses_missing")).unwrap();
        assert_eq!(missing, None);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn existing_folder_session_reference_resolves_current_and_stale_buddy_ids() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-ref-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("from-buddy");
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        fs::write(
            session_dir.join("runtime/buddy.json"),
            serde_json::json!({
                "buddy_session": "ses_currentBuddy123",
                "stale_buddy_sessions": ["ses_staleBuddy123"]
            })
            .to_string(),
        )
        .unwrap();

        let current = resolve_existing_folder_session_reference_in_root(
            Path::new("ses_currentBuddy123"),
            &root,
        )
        .unwrap();
        let stale = resolve_existing_folder_session_reference_in_root(
            Path::new("ses_staleBuddy123"),
            &root,
        )
        .unwrap();

        assert_eq!(current.session_dir, session_dir);
        assert_eq!(
            current.buddy_session.as_deref(),
            Some("ses_currentBuddy123")
        );
        assert_eq!(stale.session_dir, session_dir);
        assert_eq!(stale.buddy_session.as_deref(), Some("ses_currentBuddy123"));

        let _ = fs::remove_dir_all(&root);
    }

    #[derive(Clone)]
    struct TestBuddyBackend {
        command: String,
        runtime_command_override: Option<String>,
        create_id: String,
        creates: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl BuddySessionBackend for TestBuddyBackend {
        fn command(&self) -> &str {
            &self.command
        }

        fn runtime_command_override(&self) -> Option<String> {
            self.runtime_command_override.clone()
        }

        fn list_sessions(&self) -> Result<Vec<BuddySessionListRecord>> {
            Ok(Vec::new())
        }

        fn get_session(&self, session_id: &str) -> Result<BuddySessionListRecord> {
            Ok(BuddySessionListRecord {
                id: session_id.to_string(),
                title: session_id.to_string(),
                repo_path: String::new(),
                created_at: "2026-08-01T12:00:00Z".to_string(),
                updated_at: "2026-08-01T12:00:00Z".to_string(),
                summary: String::new(),
            })
        }

        fn create_session(&self, title: &str, repo_path: &str) -> Result<BuddySessionCreateRecord> {
            self.creates
                .lock()
                .unwrap()
                .push((title.to_string(), repo_path.to_string()));
            Ok(BuddySessionCreateRecord {
                id: self.create_id.clone(),
                title: title.to_string(),
                repo_path: repo_path.to_string(),
                created_at: "2026-08-01T12:00:00Z".to_string(),
            })
        }

        fn delete_session(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn ensure_buddy_session_binding_creates_runtime_without_default_command_override() {
        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-ensure-binding-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            format!(
                "title = \"Custom Buddy Title\"\nworkspace = {}\n",
                serde_json::to_string(&workspace.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        let manifest = read_folder_session_manifest(&session_dir).unwrap();
        let creates = Arc::new(Mutex::new(Vec::new()));
        let backend = TestBuddyBackend {
            command: "in-tree-buddy".to_string(),
            runtime_command_override: None,
            create_id: "ses_auto_bound".to_string(),
            creates: creates.clone(),
        };

        let binding = ensure_buddy_session_binding(
            &backend,
            BuddyBindingInput {
                session_dir: session_dir.clone(),
                title: manifest
                    .as_ref()
                    .and_then(|manifest| manifest.title.clone()),
                requested_workspace: Some(workspace.clone()),
                previous_runtime: None,
            },
        )
        .unwrap();

        assert_eq!(binding.buddy_session, "ses_auto_bound");
        assert_eq!(binding.repo_path, workspace);
        assert_eq!(
            creates.lock().unwrap().as_slice(),
            &[(
                "Custom Buddy Title".to_string(),
                binding.repo_path.display().to_string()
            )]
        );
        let runtime = fs::read_to_string(session_dir.join("runtime/buddy.json")).unwrap();
        assert!(runtime.contains("ses_auto_bound"));
        assert!(!runtime.contains("command"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ask_auto_folder_session_creates_buddy_binding() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ask-buddy-binding-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("ask-auto-session"),
            meta: AgentSessionMeta {
                title: "Ask auto title".to_string(),
                workspace: workspace.display().to_string(),
                profile: "default".to_string(),
                source: "djinn".to_string(),
                ..AgentSessionMeta::default()
            },
            events: Vec::new(),
        };
        let creates = Arc::new(Mutex::new(Vec::new()));
        let backend = TestBuddyBackend {
            command: "in-tree-buddy".to_string(),
            runtime_command_override: None,
            create_id: "ses_ask_bound".to_string(),
            creates: creates.clone(),
        };

        let binding = ensure_folder_session_buddy_binding_for_ask(
            &session_dir,
            &session,
            &workspace,
            &backend,
        )
        .unwrap();

        assert_eq!(binding.buddy_session, "ses_ask_bound");
        assert_eq!(binding.repo_path, workspace);
        assert_eq!(
            creates.lock().unwrap().as_slice(),
            &[(
                "Ask auto title".to_string(),
                binding.repo_path.display().to_string()
            )]
        );
        let runtime = fs::read_to_string(session_dir.join("runtime/buddy.json")).unwrap();
        assert!(runtime.contains("ses_ask_bound"));
        assert!(!runtime.contains("command"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ask_auto_folder_session_reuses_existing_buddy_binding() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ask-buddy-reuse-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        write_buddy_runtime_state(
            &session_dir.join("runtime/buddy.json"),
            &BuddyRuntimeState {
                buddy_session: Some("ses_existing_ask".to_string()),
                stale_buddy_sessions: Vec::new(),
                command: None,
                args: Vec::new(),
                last_run_at: None,
                last_prompt_chars: 0,
                last_response_chars: 0,
            },
        )
        .unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("ask-auto-session"),
            meta: AgentSessionMeta {
                title: "Ask auto title".to_string(),
                workspace: workspace.display().to_string(),
                profile: "default".to_string(),
                source: "djinn".to_string(),
                ..AgentSessionMeta::default()
            },
            events: Vec::new(),
        };
        let creates = Arc::new(Mutex::new(Vec::new()));
        let backend = TestBuddyBackend {
            command: "in-tree-buddy".to_string(),
            runtime_command_override: None,
            create_id: "ses_should_not_create".to_string(),
            creates: creates.clone(),
        };

        let binding = ensure_folder_session_buddy_binding_for_ask(
            &session_dir,
            &session,
            &workspace,
            &backend,
        )
        .unwrap();

        assert_eq!(binding.buddy_session, "ses_existing_ask");
        assert_eq!(binding.repo_path, workspace);
        assert!(creates.lock().unwrap().is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn top_level_buddy_session_plans_interactive_resume_even_with_pending_request() {
        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-behavior-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            format!(
                "title = \"Session\"\nworkspace = {}\n",
                serde_json::to_string(&workspace.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        fs::write(session_dir.join("request.md"), "pending prompt\n").unwrap();
        fs::write(
            session_dir.join("runtime/buddy.json"),
            serde_json::json!({
                "buddy_session": "ses_resume",
                "command": "buddy",
                "args": [],
                "last_run_at": null,
                "last_prompt_chars": 0,
                "last_response_chars": 0
            })
            .to_string(),
        )
        .unwrap();

        let behavior = top_level_buddy_session_behavior(&session_dir, None).unwrap();
        assert_eq!(behavior.buddy_session.as_deref(), Some("ses_resume"));
        assert_eq!(behavior.cwd.as_deref(), Some(workspace.as_path()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn top_level_buddy_session_auto_binds_unbound_folder_session() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-auto-bind-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        let create_log = root.join("create-log.txt");
        let buddy_bin = root.join("buddy-json.sh");
        fs::write(
            &buddy_bin,
            "#!/bin/sh\nif [ \"$1\" = \"session\" ] && [ \"$2\" = \"create\" ]; then\n  printf '%s|%s\n' \"$6\" \"$8\" >> '__CREATE_LOG__'\n  printf '{\"id\":\"ses_auto_bound\",\"title\":\"%s\",\"repo_path\":\"%s\",\"created_at\":\"2026-08-01T12:00:00Z\"}\n' \"$6\" \"$8\"\n  exit 0\nfi\necho unexpected buddy args: \"$@\" >&2\nexit 2\n"
                .replace("__CREATE_LOG__", &create_log.display().to_string()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&buddy_bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&buddy_bin, permissions).unwrap();
        }
        fs::write(
            session_dir.join("djinn.toml"),
            format!(
                "title = \"Auto Bound Session\"\nworkspace = {}\n",
                serde_json::to_string(&workspace.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        fs::write(
            session_dir.join("runtime/buddy.json"),
            serde_json::json!({
                "command": buddy_bin.display().to_string(),
                "args": [],
                "last_run_at": null,
                "last_prompt_chars": 0,
                "last_response_chars": 0
            })
            .to_string(),
        )
        .unwrap();

        let behavior = top_level_buddy_session_behavior(&session_dir, None).unwrap();
        assert_eq!(behavior.buddy_session.as_deref(), Some("ses_auto_bound"));
        assert_eq!(behavior.cwd.as_deref(), Some(workspace.as_path()));
        assert_eq!(
            fs::read_to_string(&create_log).unwrap(),
            format!("Auto Bound Session|{}\n", workspace.display())
        );
        let runtime = fs::read_to_string(session_dir.join("runtime/buddy.json")).unwrap();
        assert!(runtime.contains("ses_auto_bound"));
        assert!(runtime.contains(&buddy_bin.display().to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn top_level_buddy_session_promotes_stale_bound_workspace() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-stale-workspace-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let missing_workspace = root.join("missing-workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        let create_log = root.join("create-log.txt");
        let buddy_bin = root.join("buddy-json.sh");
        fs::write(
            &buddy_bin,
            "#!/bin/sh\nif [ \"$1\" = \"session\" ] && [ \"$2\" = \"create\" ]; then\n  printf '%s|%s\\n' \"$6\" \"$8\" >> '__CREATE_LOG__'\n  printf '{\"id\":\"ses_promoted\",\"title\":\"%s\",\"repo_path\":\"%s\",\"created_at\":\"2026-08-01T12:00:00Z\"}\\n' \"$6\" \"$8\"\n  exit 0\nfi\necho unexpected buddy args: \"$@\" >&2\nexit 2\n"
                .replace("__CREATE_LOG__", &create_log.display().to_string()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&buddy_bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&buddy_bin, permissions).unwrap();
        }
        fs::write(
            session_dir.join("djinn.toml"),
            format!(
                "title = \"Session\"\nworkspace = {}\n\n[context.repo]\npath = {}\nlink = \"/tmp/link\"\n",
                serde_json::to_string(&missing_workspace.display().to_string()).unwrap(),
                serde_json::to_string(&missing_workspace.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        fs::write(
            session_dir.join("runtime/buddy.json"),
            serde_json::json!({
                "buddy_session": "ses_stale",
                "command": buddy_bin.display().to_string(),
                "args": [],
                "last_run_at": null,
                "last_prompt_chars": 0,
                "last_response_chars": 0
            })
            .to_string(),
        )
        .unwrap();

        let behavior = top_level_buddy_session_behavior(&session_dir, None).unwrap();
        assert_eq!(behavior.buddy_session.as_deref(), Some("ses_promoted"));
        assert_eq!(behavior.cwd.as_deref(), Some(session_dir.as_path()));
        assert_eq!(
            fs::read_to_string(&create_log).unwrap(),
            format!("session|{}\n", session_dir.display())
        );
        let manifest = fs::read_to_string(session_dir.join("djinn.toml")).unwrap();
        assert!(!manifest.contains("workspace ="));
        assert!(!manifest.contains("[context.repo]"));
        assert!(!manifest.contains(&missing_workspace.display().to_string()));
        let runtime = fs::read_to_string(session_dir.join("runtime/buddy.json")).unwrap();
        assert!(runtime.contains("ses_promoted"));
        assert!(runtime.contains("ses_stale"));

        let resolved =
            resolve_buddy_session_reference_in_root(&root, Path::new("ses_stale")).unwrap();
        assert_eq!(
            resolved,
            Some((session_dir.clone(), "ses_promoted".to_string()))
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn interactive_buddy_summary_refresh_uses_latest_event_pair() {
        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-summary-refresh-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("summary.md"), "stale summary\n").unwrap();
        let id = AgentSessionId::new("agt_buddy_summary_refresh");
        let events = vec![
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "first request".to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::AssistantMessage {
                    content: "first response".to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "interactive request".to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id,
                AgentSessionEventKind::AssistantMessage {
                    content: "fresh interactive summary".to_string(),
                },
            ),
        ];
        let events_jsonl = events
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n")
            + "\n";
        fs::write(session_dir.join("events.jsonl"), events_jsonl).unwrap();

        let sync = refresh_folder_summary_from_latest_event(&session_dir)
            .unwrap()
            .expect("expected summary refresh");

        assert_eq!(sync.summary_path, session_dir.join("summary.md"));
        assert_eq!(
            sync.response_chars,
            "fresh interactive summary".chars().count()
        );
        assert_eq!(
            fs::read_to_string(session_dir.join("summary.md")).unwrap(),
            "fresh interactive summary\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn interactive_buddy_sync_status_reports_synced_or_unchanged() {
        let session_dir = PathBuf::from("/tmp/djinn-session");
        let sync = BuddyInteractiveSummarySync {
            summary_path: session_dir.join("summary.md"),
            response_chars: 42,
        };

        let synced = format_interactive_buddy_sync_status(&session_dir, Some(&sync));
        assert!(synced.contains("Buddy session completed."));
        assert!(synced.contains("Synced /tmp/djinn-session/summary.md"));
        assert!(synced.contains("42 chars"));

        let unchanged = format_interactive_buddy_sync_status(&session_dir, None);
        assert!(unchanged.contains("Buddy session completed."));
        assert!(unchanged.contains("No valid event pair found in /tmp/djinn-session/events.jsonl"));
        assert!(unchanged.contains("summary.md unchanged"));
    }

    #[test]
    fn default_no_args_tui_opens_sessions_dashboard() {
        let args = default_dashboard_tui_args();

        assert_eq!(args.view, TuiView::Sessions);
        assert!(args.roots.is_empty());
        assert!(args.editor.is_none());
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
        assert_eq!(dashboard_tab(args.view), djinn_tui::DashboardTab::Sessions);
    }

    #[test]
    fn rejects_removed_tui_workspaces_view() {
        assert!(Cli::try_parse_from(["djinn", "tui", "workspaces"]).is_err());
    }

    #[test]
    fn session_promote_renders_folder_artifacts_with_file_provenance() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-promote-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let turn = dir.join("turns/turn-1");
        let context = dir.join("context");
        fs::create_dir_all(&turn).unwrap();
        fs::create_dir_all(&context).unwrap();
        fs::write(dir.join("request.md"), "Find durable lessons\n").unwrap();
        fs::write(
            dir.join("summary.md"),
            "Use folder sessions as source material.\n",
        )
        .unwrap();
        fs::write(
            context.join("compacted.md"),
            "Compacted decision evidence.\n",
        )
        .unwrap();
        fs::write(turn.join("request.md"), "What should promotion cite?\n").unwrap();
        fs::write(
            turn.join("response.md"),
            "Cite summary.md and turns/turn-1/response.md.\n",
        )
        .unwrap();

        let promotion_dir = root.join("promotion-memory");
        let report = create_promotion_session(&SessionPromoteArgs {
            dirs: vec![dir.clone()],
            promotion_type: SessionPromoteType::Memory,
            promotion_session_dir: Some(promotion_dir.clone()),
            max_chars_per_artifact: 200,
            force: false,
            json: false,
        })
        .unwrap();

        assert_eq!(report.promotion_type, SessionPromoteType::Memory);
        assert_eq!(
            report.promotion_session_dir,
            promotion_dir.display().to_string()
        );
        assert_eq!(report.session_count, 1);
        assert_eq!(report.sessions[0].turn_count, 1);
        assert_eq!(report.sessions[0].artifact_count, 5);
        assert!(report
            .packet
            .starts_with("# Djinn Folder Session Promotion Packet"));
        assert!(report.packet.contains("Promotion type: `memory`"));
        assert!(report.packet.contains("`summary`: `summary.md`"));
        assert!(report
            .packet
            .contains("`compacted_context`: `context/compacted.md`"));
        assert!(report
            .packet
            .contains("`turn:turn-1:response`: `turns/turn-1/response.md`"));
        assert!(report
            .packet
            .contains("Use folder sessions as source material."));
        assert!(report
            .packet
            .contains("Cite summary.md and turns/turn-1/response.md."));

        let source_packet = fs::read_to_string(promotion_dir.join("context/source-packet.md"))
            .expect("source packet should be written");
        assert_eq!(source_packet, report.packet);
        let sources = fs::read_to_string(promotion_dir.join("context/sources.toml"))
            .expect("sources manifest should be written");
        assert!(sources.contains("promotion_type = \"memory\""));
        assert!(sources.contains(&format!("session_dir = \"{}\"", dir.display())));
        assert!(sources.contains("relative_path = \"summary.md\""));
        let manifest = fs::read_to_string(promotion_dir.join("djinn.toml"))
            .expect("promotion manifest should be written");
        assert!(manifest.contains("kind = \"promotion\""));
        assert!(manifest.contains("promotion_type = \"memory\""));
        let request = fs::read_to_string(promotion_dir.join("request.md"))
            .expect("promotion request should be written");
        assert!(request.contains("Use `context/source-packet.md`"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_promote_includes_structured_event_turn_artifacts_without_turn_projection() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-promote-events-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let session = AgentSession {
            id: AgentSessionId::new("agt_promote_events"),
            meta: AgentSessionMeta {
                title: "Promote events".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "What should promotion cite?".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "Cite events.jsonl event turns.".to_string(),
                }),
            ],
        };
        project_agent_session_dir(
            &dir,
            &session,
            "What should promotion cite?",
            "Cite events.jsonl event turns.",
        )
        .unwrap();
        let promotion_dir = root.join("promotion-memory");

        let report = create_promotion_session(&SessionPromoteArgs {
            dirs: vec![dir.clone()],
            promotion_type: SessionPromoteType::Memory,
            promotion_session_dir: Some(promotion_dir),
            max_chars_per_artifact: 400,
            force: false,
            json: false,
        })
        .unwrap();

        assert_eq!(report.sessions[0].turn_count, 1);
        assert!(report
            .packet
            .contains("`event_turn:event-turn-0001`: `events.jsonl#event-turn-0001`"));
        assert!(report.packet.contains("## Request"));
        assert!(report.packet.contains("What should promotion cite?"));
        assert!(report.packet.contains("Cite events.jsonl event turns."));
        assert!(!dir.join("turns").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_accept_and_deny_record_promotion_decisions_with_dry_run() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-decision-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let source = root.join("source-session");
        fs::create_dir_all(source.join("context")).unwrap();
        fs::write(source.join("summary.md"), "A useful lesson.\n").unwrap();

        let promotion_dir = root.join("promotion-memory");
        create_promotion_session(&SessionPromoteArgs {
            dirs: vec![source.clone()],
            promotion_type: SessionPromoteType::Memory,
            promotion_session_dir: Some(promotion_dir.clone()),
            max_chars_per_artifact: 200,
            force: false,
            json: false,
        })
        .unwrap();

        let dry_run = decide_promotion_session(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: None,
                dry_run: true,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
        )
        .unwrap();
        assert!(dry_run.dry_run);
        assert!(!dry_run.wrote_decision);
        assert!(!promotion_dir.join("outputs/decisions").exists());

        let accepted = decide_promotion_session(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: None,
                dry_run: false,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
        )
        .unwrap();
        assert_eq!(accepted.action, SessionDecisionAction::Accept);
        assert_eq!(accepted.promotion_type, "memory");
        assert!(accepted.wrote_decision);
        assert!(!accepted.durable_writeback);
        let accepted_record = fs::read_to_string(&accepted.decision_path).unwrap();
        assert!(accepted_record.contains("action = \"accept\""));
        assert!(accepted_record.contains("durable_writeback = false"));

        let denied = decide_promotion_session(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: None,
                dry_run: false,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Deny,
        )
        .unwrap();
        let denied_record = fs::read_to_string(&denied.decision_path).unwrap();
        assert!(denied_record.contains("action = \"deny\""));
        assert!(!denied_record.contains("candidate ="));

        let normal = root.join("normal-session");
        fs::create_dir_all(&normal).unwrap();
        fs::write(normal.join("djinn.toml"), "version = 1\n").unwrap();
        let err = decide_promotion_session(
            &SessionDecisionArgs {
                dir: normal,
                candidate: None,
                dry_run: false,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not a promotion session"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn candidate_duplicate_similarity_matches_generated_candidate_variants() {
        let duplicate_pairs = [
            (
                "Keep source sessions as durable promotion provenance.",
                "Keep source sessions as durable provenance for promotion writeback.",
            ),
            (
                "Wire promotion todos into the MindWeaver inbox capture.",
                "Wire Djinn promotion todo candidates into MindWeaver inbox capture.",
            ),
            (
                "Add direct promotion-candidate review actions in Sessions TUI.",
                "Add direct review actions for promotion candidates in the Sessions TUI.",
            ),
            (
                "Promotion candidate rows should show status, evidence, and destination previews.",
                "Show promotion candidates with status, evidence links, and destination preview rows.",
            ),
        ];

        for (existing, candidate) in duplicate_pairs {
            assert!(
                candidate_duplicate_similarity(candidate, existing).is_some(),
                "expected duplicate match for generated variant: {candidate}"
            );
        }
    }

    #[test]
    fn candidate_duplicate_similarity_allows_related_but_distinct_work() {
        let distinct_pairs = [
            (
                "Add direct review actions for promotion candidates in Sessions TUI.",
                "Tune fuzzy duplicate thresholds from generated promotion candidates.",
            ),
            (
                "Wire Djinn promotion todo candidates into MindWeaver inbox capture.",
                "Run mw todos sync after accepting MindWeaver inbox todos.",
            ),
            (
                "Keep source sessions as durable promotion provenance.",
                "Link high signal project files during session context discovery.",
            ),
        ];

        for (existing, candidate) in distinct_pairs {
            assert_eq!(
                candidate_duplicate_similarity(candidate, existing),
                None,
                "expected related candidate to remain distinct: {candidate}"
            );
        }
    }

    #[test]
    fn session_accept_writes_stable_memory_candidate_to_durable_store() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-writeback-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let data = root.join("data");
        let source = root.join("source-session");
        fs::create_dir_all(source.join("context")).unwrap();
        fs::write(source.join("summary.md"), "A useful promotion lesson.\n").unwrap();

        let promotion_dir = root.join("promotion-memory");
        create_promotion_session(&SessionPromoteArgs {
            dirs: vec![source.clone()],
            promotion_type: SessionPromoteType::Memory,
            promotion_session_dir: Some(promotion_dir.clone()),
            max_chars_per_artifact: 200,
            force: false,
            json: false,
        })
        .unwrap();
        let candidates_dir = promotion_dir.join("outputs/candidates");
        fs::create_dir_all(&candidates_dir).unwrap();
        fs::write(
            candidates_dir.join("memory-001.toml"),
            format!(
                "type = \"memory\"\ntext = \"Keep source sessions as promotion provenance.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\"{}/summary.md\"]\n",
                source.display()
            ),
        )
        .unwrap();

        let stores = PromotionWritebackStores {
            memory: djinn_memory::MemoryStore::default_in(&data),
            action: ActionStore::default_in(&data),
            skill: SkillStore::default_in(&data),
            mindweaver_inbox: None,
            mindweaver_sync_command: None,
        };

        let dry_run = decide_promotion_session_with_stores(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: Some("memory-001".to_string()),
                dry_run: true,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
            stores.clone(),
        )
        .unwrap();
        assert_eq!(dry_run.candidate_count, 1);
        assert_eq!(dry_run.writebacks.len(), 1);
        assert!(!dry_run.durable_writeback);
        assert!(stores.memory.list().unwrap().is_empty());

        let accepted = decide_promotion_session_with_stores(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: Some("memory-001".to_string()),
                dry_run: false,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
            stores.clone(),
        )
        .unwrap();
        assert!(accepted.durable_writeback);
        assert_eq!(accepted.writebacks[0].destination, "memory");
        let memories = stores.memory.list().unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(
            memories[0].text,
            "Keep source sessions as promotion provenance."
        );
        assert_eq!(memories[0].scope, "project:djinn");
        assert_eq!(memories[0].evidence.len(), 1);
        let record = fs::read_to_string(&accepted.decision_path).unwrap();
        assert!(record.contains("durable_writeback = true"));
        assert!(record.contains("destination = \"memory\""));
        let status = fs::read_to_string(&accepted.candidate_status_path).unwrap();
        assert!(status.contains("status = \"accepted\""));
        assert!(status.contains("durable_writeback = true"));

        let duplicate = decide_promotion_session_with_stores(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: Some("memory-001".to_string()),
                dry_run: true,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
            stores.clone(),
        )
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate memory candidate"));

        fs::write(
            candidates_dir.join("memory-002.toml"),
            format!(
                "type = \"memory\"\nid = \"memory-002\"\ntext = \"Keep source sessions as durable promotion provenance.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\"{}/summary.md\"]\n",
                source.display()
            ),
        )
        .unwrap();
        let near_duplicate = decide_promotion_session_with_stores(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: Some("memory-002".to_string()),
                dry_run: true,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
            stores.clone(),
        )
        .unwrap_err();
        assert!(near_duplicate
            .to_string()
            .contains("near-duplicate memory candidate"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_accept_writes_mindweaver_todo_to_inbox() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-mw-todo-preview-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let data = root.join("data");
        let inbox = root.join("notes/introspection/inbox.md");
        let sync_marker = root.join("mindweaver-sync-ran");
        let source = root.join("source-session");
        fs::create_dir_all(source.join("turns/turn-1")).unwrap();
        fs::write(
            source.join("turns/turn-1/response.md"),
            "A concrete follow-up exists.\n",
        )
        .unwrap();

        let promotion_dir = root.join("promotion-todo");
        create_promotion_session(&SessionPromoteArgs {
            dirs: vec![source.clone()],
            promotion_type: SessionPromoteType::Todo,
            promotion_session_dir: Some(promotion_dir.clone()),
            max_chars_per_artifact: 200,
            force: false,
            json: false,
        })
        .unwrap();
        let candidates_dir = promotion_dir.join("outputs/candidates");
        fs::create_dir_all(&candidates_dir).unwrap();
        fs::write(
            candidates_dir.join("todo-001.toml"),
            format!(
                "type = \"todo\"\nid = \"todo-001\"\ntext = \"Wire Djinn promotion todos into MindWeaver inbox capture.\"\nkind = \"follow-up\"\nconfidence = \"medium\"\ntodo_adapter = \"mindweaver\"\narea = \"Code\"\npriority = \"p2\"\nenergy = \"m\"\ndue = \"2026-08-01\"\nestimate = \"30\"\nevidence = [\"{}/turns/turn-1/response.md\"]\n",
                source.display()
            ),
        )
        .unwrap();

        let stores = PromotionWritebackStores {
            memory: djinn_memory::MemoryStore::default_in(&data),
            action: ActionStore::default_in(&data),
            skill: SkillStore::default_in(&data),
            mindweaver_inbox: Some(inbox.clone()),
            mindweaver_sync_command: Some(vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf synced > \"$1\"".to_string(),
                "sh".to_string(),
                sync_marker.display().to_string(),
            ]),
        };

        let dry_run = decide_promotion_session_with_stores(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: Some("todo-001".to_string()),
                dry_run: true,
                sync_mindweaver: true,
                json: false,
            },
            SessionDecisionAction::Accept,
            stores.clone(),
        )
        .unwrap();
        assert_eq!(dry_run.writebacks.len(), 1);
        assert_eq!(
            dry_run.writebacks[0].destination,
            "mindweaver_inbox_preview"
        );
        assert_eq!(
            dry_run.writebacks[0].preview.as_deref(),
            Some(
                "- [ ] Wire Djinn promotion todos into MindWeaver inbox capture.\n  - p2 e:m due:2026-08-01 est:30 area:Code"
            )
        );
        assert!(!dry_run.durable_writeback);
        assert_eq!(dry_run.post_writebacks.len(), 1);
        assert_eq!(dry_run.post_writebacks[0].status, "dry_run");
        assert!(stores.action.list().unwrap().is_empty());
        assert!(!promotion_dir.join("outputs/decisions").exists());
        assert!(!sync_marker.exists());

        let accepted = decide_promotion_session_with_stores(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: Some("todo-001".to_string()),
                dry_run: false,
                sync_mindweaver: true,
                json: false,
            },
            SessionDecisionAction::Accept,
            stores.clone(),
        )
        .unwrap();
        assert!(accepted.durable_writeback);
        assert_eq!(accepted.post_writebacks.len(), 1);
        assert_eq!(accepted.post_writebacks[0].status, "completed");
        assert_eq!(accepted.writebacks[0].destination, "mindweaver_inbox");
        assert_eq!(
            accepted.writebacks[0].path.as_deref(),
            Some(inbox.to_str().unwrap())
        );
        let inbox_content = fs::read_to_string(&inbox).unwrap();
        assert!(inbox_content.contains("domains: [task-index]"));
        assert!(inbox_content.contains("### Inbox\n- [ ] Wire Djinn promotion todos into MindWeaver inbox capture.\n  - p2 e:m due:2026-08-01 est:30 area:Code\n### Next"));
        let decision = fs::read_to_string(&accepted.decision_path).unwrap();
        assert!(decision.contains("destination = \"mindweaver_inbox\""));
        assert!(decision.contains("preview = "));
        assert!(decision.contains("[[post_writebacks]]"));
        assert_eq!(fs::read_to_string(&sync_marker).unwrap(), "synced");

        fs::write(
            candidates_dir.join("todo-002.toml"),
            format!(
                "type = \"todo\"\nid = \"todo-002\"\ntext = \"Polish explicit MindWeaver sync handoff UX.\"\nkind = \"follow-up\"\nconfidence = \"medium\"\ntodo_adapter = \"mindweaver\"\narea = \"Code\"\npriority = \"p3\"\nevidence = [\"{}/turns/turn-1/response.md\"]\n",
                source.display()
            ),
        )
        .unwrap();
        let accepted_without_sync = decide_promotion_session_with_stores(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: Some("todo-002".to_string()),
                dry_run: false,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
            stores.clone(),
        )
        .unwrap();
        assert_eq!(accepted_without_sync.post_writebacks.len(), 1);
        assert_eq!(accepted_without_sync.post_writebacks[0].status, "pending");
        assert_eq!(
            accepted_without_sync.post_writebacks[0].command,
            stores.mindweaver_sync_command.clone().unwrap().join(" ")
        );
        assert!(accepted_without_sync
            .note
            .contains("run the listed follow-up command"));
        let pending_decision = fs::read_to_string(&accepted_without_sync.decision_path).unwrap();
        assert!(pending_decision.contains("status = \"pending\""));
        assert!(fs::read_to_string(&inbox)
            .unwrap()
            .contains("Polish explicit MindWeaver sync handoff UX."));

        let duplicate = decide_promotion_session_with_stores(
            &SessionDecisionArgs {
                dir: promotion_dir.clone(),
                candidate: Some("todo-001".to_string()),
                dry_run: true,
                sync_mindweaver: false,
                json: false,
            },
            SessionDecisionAction::Accept,
            stores,
        )
        .unwrap_err();
        assert!(duplicate
            .to_string()
            .contains("duplicate MindWeaver todo candidate"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn promotion_candidate_validation_requires_type_specific_fields() {
        let root = std::env::temp_dir().join(format!(
            "djinn-promotion-validation-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let candidates = root.join("outputs/candidates");
        fs::create_dir_all(&candidates).unwrap();
        let evidence = "/tmp/source/summary.md";

        let memory_err = parse_promotion_candidate(
            &root,
            &candidates.join("memory.toml"),
            &format!(
                "type = \"memory\"\ntext = \"A lesson.\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\"{evidence}\"]\n"
            ),
        )
        .unwrap_err();
        assert!(memory_err.to_string().contains("must include `scope`"));

        let todo_err = parse_promotion_candidate(
            &root,
            &candidates.join("todo.toml"),
            &format!(
                "type = \"todo\"\ntext = \"Do the thing.\"\nconfidence = \"medium\"\nevidence = [\"{evidence}\"]\n"
            ),
        )
        .unwrap_err();
        assert!(todo_err.to_string().contains("must include `kind`"));

        let skill_err = parse_promotion_candidate(
            &root,
            &candidates.join("skill.toml"),
            &format!(
                "type = \"skill\"\nname = \"workflow\"\nbody = \"# Skill: workflow\"\nevidence = [\"{evidence}\"]\n"
            ),
        )
        .unwrap_err();
        assert!(skill_err.to_string().contains("must include `description`"));

        let pattern_err = parse_promotion_candidate(
            &root,
            &candidates.join("pattern.toml"),
            &format!(
                "type = \"pattern\"\ntext = \"A repeated theme.\"\nevidence = [\"{evidence}\"]\n"
            ),
        )
        .unwrap_err();
        assert!(pattern_err.to_string().contains("must include `rationale`"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn promotion_generation_writes_model_toml_blocks_as_candidate_files() {
        let root = std::env::temp_dir().join(format!(
            "djinn-promotion-generation-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("promotion-memory");
        let candidates_dir = session_dir.join("outputs/candidates");
        fs::create_dir_all(&candidates_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            "version = 1\nkind = \"promotion\"\npromotion_type = \"memory\"\n",
        )
        .unwrap();
        fs::write(session_dir.join("request.md"), "promote memories\n").unwrap();
        let model_output = "Here are candidates:\n\n```toml\ntype = \"memory\"\ntext = \"Promotion sessions should preserve source provenance.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\n  \"/tmp/source/summary.md\"\n]\n```\n";

        let reports = write_generated_promotion_candidates(
            &session_dir,
            "memory",
            model_output,
            &candidates_dir,
        )
        .unwrap();

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].id, "memory-001");
        assert_eq!(reports[0].candidate_type, "memory");
        assert_eq!(reports[0].evidence_count, 1);
        let candidate = fs::read_to_string(&reports[0].path).unwrap();
        assert!(candidate.contains("id = \"memory-001\""));
        assert!(candidate.contains("type = \"memory\""));
        let index_path = write_promotion_candidate_index(&session_dir, &reports).unwrap();
        let index = fs::read_to_string(index_path).unwrap();
        assert!(index.contains("candidate_count = 1"));
        assert!(index.contains("status = \"candidate\""));
        let summary_path =
            write_promotion_generation_summary(&session_dir, "memory", &reports).unwrap();
        let summary = fs::read_to_string(summary_path).unwrap();
        assert!(summary.contains("# Promotion candidates"));
        assert!(summary.contains("Promotion sessions should preserve source provenance."));
        assert!(summary.contains("/tmp/source/summary.md"));
        let status = folder_session_status(&session_dir).unwrap();
        assert_eq!(status.lifecycle.state, "completed");
        assert_eq!(status.lifecycle.mode.as_deref(), Some("promotion"));
        assert_eq!(
            status.lifecycle.reason.as_deref(),
            Some("candidates_generated")
        );
        assert!(status
            .next_action
            .as_deref()
            .unwrap_or_default()
            .contains("djinn session accept"));

        let prompt = render_promotion_candidate_generation_prompt("memory", "Packet evidence");
        assert!(prompt.contains("Promotion type: `memory`"));
        assert!(prompt.contains("Return one fenced `toml` block per candidate"));
        assert!(prompt.contains("Packet evidence"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pattern_promotion_summary_is_standalone_synthesis() {
        let candidates = vec![
            PromotionGeneratedCandidateReport {
                id: "pattern-001".to_string(),
                candidate_type: "pattern".to_string(),
                path: "/tmp/promotion/outputs/candidates/pattern-001.toml".to_string(),
                text: "Keep pattern insights in notes after review.".to_string(),
                rationale: Some(
                    "Patterns are synthesis across sessions, not durable Djinn records."
                        .to_string(),
                ),
                evidence: vec![
                    "/tmp/source-a/summary.md".to_string(),
                    "/tmp/source-b/turns/turn-1/response.md".to_string(),
                ],
                evidence_count: 2,
            },
            PromotionGeneratedCandidateReport {
                id: "pattern-002".to_string(),
                candidate_type: "pattern".to_string(),
                path: "/tmp/promotion/outputs/candidates/pattern-002.toml".to_string(),
                text: "Prefer explicit cleanup after exporting insights.".to_string(),
                rationale: Some(
                    "The workflow keeps provenance until the user intentionally deletes sources."
                        .to_string(),
                ),
                evidence: vec!["/tmp/source-c/context/source-packet.md".to_string()],
                evidence_count: 1,
            },
        ];

        let summary = render_promotion_generation_summary("pattern", &candidates);

        assert!(summary.starts_with("# Pattern synthesis"));
        assert!(summary.contains("## Executive summary"));
        assert!(summary.contains("## Patterns to evaluate"));
        assert!(summary.contains("## Review checklist"));
        assert!(summary.contains("**pattern-001** — Keep pattern insights in notes"));
        assert!(summary.contains("**Why it matters:** Patterns are synthesis"));
        assert!(summary.contains("/tmp/source-b/turns/turn-1/response.md"));
        assert!(summary.contains(
            "djinn session export-pattern <promotion-session> [candidate] --to <notes.md>"
        ));
        assert!(!summary.contains("# Promotion candidates"));
    }

    #[test]
    fn promotion_status_reports_failed_background_generation_without_candidates() {
        let root = std::env::temp_dir().join(format!(
            "djinn-promotion-bg-status-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("promotion-pattern");
        let run_dir = session_dir.join(".djinn/runs");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            "version = 1\nkind = \"promotion\"\npromotion_type = \"pattern\"\n",
        )
        .unwrap();
        fs::write(session_dir.join("request.md"), "promote patterns\n").unwrap();
        let log_path = run_dir.join("session-run-test.log");
        fs::write(&log_path, "candidate validation failed\n").unwrap();
        fs::write(
            run_dir.join("session-run-test.toml"),
            format!(
                "version = 1\nstarted_at = \"2026-07-30T12:00:00Z\"\nsession_dir = \"{}\"\npid = 4294967295\nlog_path = \"{}\"\n",
                session_dir.display(),
                log_path.display()
            ),
        )
        .unwrap();

        let status = folder_session_status(&session_dir).unwrap();

        assert_eq!(status.lifecycle.state, "failed");
        assert_eq!(status.lifecycle.mode.as_deref(), Some("promotion"));
        assert_eq!(
            status.lifecycle.reason.as_deref(),
            Some("generation_failed")
        );
        assert!(status
            .lifecycle
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("Inspect the model response or log"));
        let run = latest_background_session_run_status(&session_dir).unwrap();
        assert_eq!(run.log_tail.as_deref(), Some("candidate validation failed"));
        assert!(run.log_bytes.unwrap_or_default() > 0);
        let running_note =
            format_background_promotion_run_note(&BackgroundRunStatus { alive: true, ..run });
        assert!(running_note.contains("pid 4294967295"));
        assert!(running_note.contains("Last log: candidate validation failed"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_init_repo_config_overrides_global_profile_model() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-config-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let global = root.join("global.json");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            &global,
            r#"{
  "version": 1,
  "default_profile": "work",
  "profiles": { "work": { "model": "global-model" } }
}"#,
        )
        .unwrap();
        fs::write(
            repo.join(".djinn.json"),
            r#"{
  "version": 1,
  "profiles": { "work": { "model": "repo-model" } }
}"#,
        )
        .unwrap();

        let load = load_djinn_config_from_paths(vec![global, repo.join(".djinn.json")]).unwrap();
        let selection =
            resolve_agent_role_selection_from_config(&load.effective, None, "default", None)
                .unwrap();
        let model = resolve_agent_model_from_config(None, &load.effective, &selection.profile);

        assert_eq!(selection.profile, "work");
        assert_eq!(model, "repo-model");

        let _ = fs::remove_dir_all(&root);
    }
}
