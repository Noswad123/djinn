use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use djinn_agent::{
    tools_with_policies_file_history_and_gate, AgentProgressEvent, AgentRuntime, CopilotClient,
    ModelClient, ModelMessage, ModelRequest, ModelRole, OpenAiAuth, OpenAiClient, OpenAiOAuth,
    PermissionEffect, PermissionGate, PermissionPolicy, PermissionRule, ReadAccessEffect,
    ReadAccessPolicy, ReadAccessRule, ToolSpec,
};
use djinn_contexts::{resolve_context, ContextInput, ContextRecord, ContextStore};
use djinn_memory::{
    ActionRecord, ActionStore, AgentSession, AgentSessionEvent, AgentSessionEventKind,
    AgentSessionExecutionMode, AgentSessionId, AgentSessionLifecycleState, AgentSessionMeta,
    AgentSessionPolicyRule, AgentSessionPolicySnapshot, AgentSessionRuntimeConfig,
    AgentSessionStore, FileHistoryEntryId, FileHistoryFilter, FileHistoryRestoreOptions,
    IdeaRecord, IdeaStore, JsonlAgentSessionStore, JsonlFileHistoryStore, MemoryInput,
    MemoryRecord, MemorySource, SuggestionInput, SuggestionRecord, SuggestionStore,
};
use djinn_skills::{
    list_skills as discover_skills, read_skill_content, resolve_skill, SkillRecord, SkillRoot,
    SkillStore,
};
use djinn_tools::ToolEntry;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

mod background_run;
mod buddy;
mod buddy_consolidate;
mod editor;
mod path_util;
mod permission_gate;
mod promotion_candidate;
mod promotion_cleanup;
mod promotion_decision;
mod promotion_export;
mod promotion_generation;
mod promotion_session;
mod promotion_validation;
mod prompt;
mod session_artifact;
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
mod session_status;
mod session_transcript;
mod session_tui;
mod session_turns;
mod session_watch;
mod shell;
mod text;
#[cfg(test)]
use background_run::BackgroundRunStatus;
use background_run::{
    background_session_run_log_path, latest_background_session_run_status,
    touch_background_run_marker, write_background_session_run_marker,
};
use buddy::*;
use buddy_consolidate::*;
use editor::{open_editor_at, open_editor_path};
pub(crate) use path_util::expand_tilde_path;
use permission_gate::TerminalPermissionGate;
#[cfg(test)]
use promotion_candidate::parse_promotion_candidate;
use promotion_cleanup::session_cleanup;
#[cfg(test)]
pub(crate) use promotion_decision::{
    candidate_duplicate_similarity, decide_promotion_session_with_stores, PromotionWritebackStores,
};
use promotion_decision::{
    decide_promotion_session, session_decision_action_label, SessionDecisionAction,
};
use promotion_export::session_export_pattern;
use promotion_generation::*;
pub(crate) use promotion_session::create_promotion_session;
use promotion_validation::session_validate_candidates;
pub(crate) use promotion_validation::SessionValidateCandidateEntry;
pub(crate) use prompt::{prompt_title, resolve_agent_request_prompt};
#[cfg(test)]
use session_artifact::resolve_folder_session_open_target_in_root;
use session_artifact::{resolve_folder_session_open_target, SessionOpenTarget};
use session_compact::compact_folder_session;
#[cfg(test)]
use session_context::validate_context_entry_name;
use session_context::{
    add_folder_session_context_entry, discover_folder_session_context,
    format_folder_session_context_discover, format_folder_session_context_ls,
    inspect_folder_session_context_dir, list_folder_session_context,
    remove_folder_session_context_entry, resolve_folder_session_context_instructions,
};
use session_events::{
    ensure_event_health_strict, event_health_report_for_cache_sessions, format_event_health_report,
    format_session_project_events_report, format_session_restore_events_report,
    format_session_validate_events_report, latest_event_rebuild_backup_path,
    project_folder_session_events, projected_event_turn_id, read_event_turn_pairs,
    rebuild_folder_session_from_events, restore_folder_session_event_backup,
    validate_folder_session_events,
};
#[cfg(test)]
use session_events::{event_health_report_for_folder_session_root, SessionEventsHealthReport};
use session_init::session_init;
#[cfg(test)]
use session_init::{
    create_dir_symlink, initialize_folder_session, initialize_folder_session_with_buddy,
    SessionInitBuddyReport,
};
pub(crate) use session_list::list_folder_sessions_in_root;
pub(crate) use session_list::FolderSessionSummary;
#[cfg(test)]
use session_list::{compact_session_list_datetime, parse_session_list_datetime_ms};
use session_list::{
    folder_session_event_health_label, format_folder_session_ls, list_cache_folder_sessions,
};
#[cfg(test)]
use session_manifest::parse_folder_session_manifest;
pub(crate) use session_manifest::{
    folder_session_manifest_meta, manifest_root_string_value, parse_manifest_string_value,
    read_folder_session_manifest, session_id_from_session_dir, session_manifest_workspace_path,
    toml_string, write_agent_session_toml, FolderSessionManifest,
};
pub(crate) use session_native::{
    agent_session_store_for_folder_session, folder_agent_session_store,
    load_folder_native_agent_session, relocate_agent_session_into_folder,
};
#[cfg(test)]
use session_projection::write_agent_session_native_jsonl;
pub(crate) use session_projection::{
    ensure_folder_session_readme, hydrate_folder_agent_session_from_events_jsonl,
    project_agent_session_dir, sync_folder_session_events_jsonl_from_store,
    write_folder_session_events_jsonl, AgentSessionDirProjection,
};
pub(crate) use session_reference::{
    auto_folder_session_dir, default_folder_session_root, folder_session_display_name,
    folder_session_reference_name, folder_session_slug, is_named_folder_session_reference,
    resolve_existing_folder_session_dir, resolve_existing_folder_session_reference,
    resolve_existing_folder_session_reference_in_root, resolve_session_dir,
    resolve_session_dir_in_root, safe_folder_session_slug,
};
#[cfg(test)]
use session_reference::{
    resolve_buddy_session_reference_in_root, resolve_folder_session_reference_name,
    short_agent_session_suffix, short_agent_session_suffix_from_str,
};
#[cfg(test)]
use session_registry::shorten_folder_session_names_in_root;
use session_registry::{
    format_session_rename_report, format_session_shorten_names_report,
    rename_folder_session_in_root, shorten_cache_folder_session_names,
};
use session_remove::session_rm;
#[cfg(test)]
use session_status::SessionStatusLifecycleReport;
#[cfg(test)]
use session_status::SessionStatusTurnReport;
use session_status::{
    folder_session_status, format_folder_session_status, format_session_candidate_entry,
    format_session_candidate_status, latest_promotion_generation_response_path,
    SessionStatusCandidateEntry,
};
#[cfg(test)]
use session_status::{format_agent_session_event_summary, format_background_promotion_run_note};
#[cfg(test)]
use session_status::{SessionStatusFileReport, SessionStatusReport};
#[cfg(test)]
use session_transcript::{build_session_transcript, render_session_transcript_markdown};
use session_transcript::{SessionTranscriptFormat, SessionTranscriptOptions};
#[cfg(test)]
use session_tui::editor_open_command_hint;
use session_tui::{
    folder_session_action_message, folder_session_status_tui_view, tui_candidate_row,
};
pub(crate) use session_turns::{
    compact_text_snippet, read_folder_session_event_turns, read_folder_session_turns,
    read_optional_markdown_file, FolderSessionTurnDigest,
};
use session_watch::session_watch;
#[cfg(test)]
use session_watch::{format_session_watch_snapshot, session_watch_snapshot_key};
use shell::shell_quote;
pub(crate) use text::{
    ensure_trailing_newline, non_empty_string, plural_suffix, truncate, truncate_table_cell,
};

const AGENT_CHILD_SESSION_MAX_DEPTH: usize = 3;
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
            return run_tui_command(default_dashboard_tui_args());
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
        Command::Tui(args) => run_tui_command(args),
    }
}

fn run_tui_command(args: TuiArgs) -> Result<()> {
    run_tui(args)
}

fn run_list(args: ListArgs) -> Result<()> {
    match args.noun {
        ListNoun::Tools(scope) => list_tools(scope),
        ListNoun::Memories => list_memories(),
        ListNoun::Suggestions => list_suggestions(),
        ListNoun::Ideas => list_ideas(),
        ListNoun::Actions => list_actions(),
        ListNoun::Skills(args) => list_skills(args),
        ListNoun::Contexts(args) | ListNoun::Ctx(args) => list_contexts(args),
    }
}

fn run_show(args: ShowArgs) -> Result<()> {
    match args.noun {
        ShowNoun::Memory { id } => show_memory(&id),
        ShowNoun::Suggestion { id } => show_suggestion(&id),
        ShowNoun::Idea { id } => show_idea(&id),
        ShowNoun::Action { id } => show_action(&id),
        ShowNoun::Ctx(args) => show_context(args),
        ShowNoun::Tool(args) => show_tool(args),
        ShowNoun::Skill(args) => show_skill(args),
    }
}

fn run_add(args: AddArgs) -> Result<()> {
    match args.noun {
        AddNoun::Memory(args) => {
            let record = add_memory(args)?;
            println!("Memory saved [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Suggestion(args) => add_suggestion(args),
        AddNoun::Idea(args) => {
            let record = add_idea(args)?;
            println!("Idea saved [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Action(args) => {
            let record = add_action(args)?;
            println!("Action saved [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Skill(args) => add_skill(args),
        AddNoun::Ctx(args) => add_context(args),
    }
}

fn run_accept(args: AcceptArgs) -> Result<()> {
    match args.noun {
        AcceptNoun::Memory(args) => accept_memory(args),
        AcceptNoun::Suggestion { id } => complete_suggestions(&[id]),
    }
}

fn run_reject(args: RejectArgs) -> Result<()> {
    match args.noun {
        RejectNoun::Memory { ids } => reject_memories(&ids),
        RejectNoun::Suggestion { ids } => reject_suggestions(&ids),
    }
}

fn run_ingest(args: IngestArgs) -> Result<()> {
    match args.noun {
        IngestNoun::Memories(args) | IngestNoun::Memory(args) => ingest_memories(args),
    }
}

fn run_review(args: ReviewArgs) -> Result<()> {
    match args.source {
        ReviewSource::Memory(args) | ReviewSource::Memories(args) => review_memories(args),
    }
}

fn run_rm(args: RmArgs) -> Result<()> {
    match args.noun {
        RmNoun::Memory { keyword } => rm_memory(&keyword),
        RmNoun::Skill(args) => rm_skill(args),
    }
}

fn run_clear(args: ClearArgs) -> Result<()> {
    match args.noun {
        ClearNoun::Memories { no_backup } => clear_memories(no_backup),
    }
}

fn run_scan(args: ScanArgs) -> Result<()> {
    match args.noun {
        ScanNoun::Tools(scope) => {
            let roots = tool_roots(scope.roots);
            let entries = scan_tools(&roots)?;
            println!(
                "Scanned {} tools under {}",
                entries.len(),
                format_roots(&roots)
            );
            Ok(())
        }
    }
}

fn run_index(args: IndexArgs) -> Result<()> {
    match args.noun {
        IndexNoun::Tools(args) => {
            let roots = tool_roots(args.roots);
            let root = roots
                .first()
                .cloned()
                .unwrap_or_else(djinn_core::default_dotfiles_root);
            let index_path = args
                .index
                .unwrap_or_else(|| djinn_core::default_index_path(&root));
            let entries = scan_tools(&roots)?;
            let changed = write_tools_index(&roots, &entries, &index_path)?;
            let count = entries.len();
            let status = if changed { "updated" } else { "unchanged" };
            eprintln!(
                "djinn index tools: {status} {} ({count} entries)",
                index_path.display()
            );
            Ok(())
        }
    }
}

fn run_search(args: SearchArgs) -> Result<()> {
    match args.noun {
        SearchNoun::Tools(args) => search_tools(args),
        SearchNoun::Memories { query } => search_memories(&query),
        SearchNoun::Suggestions { query } => search_suggestions(&query),
    }
}

fn run_switch(args: SwitchArgs) -> Result<()> {
    match args.noun {
        SwitchNoun::Ctx { name } => switch_context(&name),
    }
}

fn run_open(args: OpenArgs) -> Result<()> {
    match args.noun {
        OpenNoun::Tool(args) => open_tool(args),
    }
}

fn run_config(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommand::Show(args) => config_show(args),
        ConfigCommand::Doctor(args) => config_doctor(args),
        ConfigCommand::Import(args) => config_import(args),
        ConfigCommand::Export(args) => config_export(args),
    }
}

fn run_doctor(args: DoctorArgs) -> Result<()> {
    match args.command {
        DoctorCommand::Buddy(args) => doctor_buddy(args),
    }
}

fn doctor_buddy(args: DoctorBuddyArgs) -> Result<()> {
    let report = buddy_command_doctor_report(args.session.as_deref())?;
    print!(
        "{}",
        format_buddy_command_doctor_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

fn run_auth(args: AuthArgs) -> Result<()> {
    match args.command {
        AuthCommand::Login(args) => auth_login(args),
    }
}

fn auth_login(args: AuthLoginArgs) -> Result<()> {
    let provider = args.provider.unwrap_or_else(prompt_auth_provider);
    match provider {
        AuthProvider::Openai => {
            run_openai_login_method(args.method.unwrap_or_else(prompt_openai_login_method))
        }
    }
}

fn config_show(args: ConfigShowArgs) -> Result<()> {
    let report = load_djinn_config(args.path)?;
    print!(
        "{}",
        format_djinn_config_load_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

fn config_import(args: ConfigImportArgs) -> Result<()> {
    match args.source {
        ConfigImportSource::Copilot(args) => config_import_copilot(args),
        ConfigImportSource::Opencode(args) => config_import_opencode(args),
    }
}

fn config_export(args: ConfigExportArgs) -> Result<()> {
    match args.target {
        ConfigExportTarget::Copilot(args) => config_export_copilot(args),
        ConfigExportTarget::Opencode(args) => config_export_opencode(args),
    }
}

fn config_export_copilot(args: ConfigExportCopilotArgs) -> Result<()> {
    match (args.dry_run, args.write) {
        (true, true) => bail!("choose either --dry-run or --write, not both"),
        (false, false) => bail!("config export is safe by default; pass --dry-run to preview or --write to create a Copilot config file"),
        (true, false) => {
            let preview = copilot_config_export_preview(args.path)?;
            print!(
                "{}",
                format_config_export_preview(&preview, output_format(args.format, args.json))?
            );
        }
        (false, true) => {
            let preview = copilot_config_export_preview(args.path)?;
            let output = args.output.unwrap_or_else(default_copilot_config_path);
            let report = write_config_export_preview(&preview, &output, args.force)?;
            print!(
                "{}",
                format_config_export_write_report(&report, output_format(args.format, args.json))?
            );
        }
    }
    Ok(())
}

fn config_export_opencode(args: ConfigExportOpencodeArgs) -> Result<()> {
    match (args.dry_run, args.write) {
        (true, true) => bail!("choose either --dry-run or --write, not both"),
        (false, false) => bail!("config export is safe by default; pass --dry-run to preview or --write to create an OpenCode config file"),
        (true, false) => {
            let preview = opencode_config_export_preview(args.path)?;
            print!(
                "{}",
                format_config_export_preview(&preview, output_format(args.format, args.json))?
            );
        }
        (false, true) => {
            let preview = opencode_config_export_preview(args.path)?;
            let output = args.output.unwrap_or_else(default_opencode_config_path);
            let report = write_config_export_preview(&preview, &output, args.force)?;
            print!(
                "{}",
                format_config_export_write_report(&report, output_format(args.format, args.json))?
            );
        }
    }
    Ok(())
}

fn config_import_opencode(args: ConfigImportOpencodeArgs) -> Result<()> {
    validate_config_import_mode(args.dry_run, args.write, args.merge, args.force)?;
    match (args.dry_run, args.write) {
        (true, true) => bail!("choose either --dry-run or --write, not both"),
        (false, false) => bail!("config import is safe by default; pass --dry-run to preview or --write to create a Djinn config file"),
        (true, false) => {
            let preview = opencode_config_import_preview(args.path)?;
            print!(
                "{}",
                format_config_import_preview(&preview, output_format(args.format, args.json))?
            );
        }
        (false, true) => {
            let preview = opencode_config_import_preview(args.path)?;
            let output = args.output.unwrap_or_else(default_djinn_config_path);
            let report = write_config_import_preview(&preview, &output, args.force)?;
            print!(
                "{}",
                format_config_import_write_report(&report, output_format(args.format, args.json))?
            );
        }
    }
    Ok(())
}

fn config_import_copilot(args: ConfigImportCopilotArgs) -> Result<()> {
    validate_config_import_mode(args.dry_run, args.write, args.merge, args.force)?;
    match (args.dry_run, args.write) {
        (true, true) => bail!("choose either --dry-run or --write, not both"),
        (false, false) => bail!("config import is safe by default; pass --dry-run to preview or --write to create a Djinn config file"),
        (true, false) => {
            let preview = copilot_config_import_preview(args.path)?;
            print!(
                "{}",
                format_config_import_preview(&preview, output_format(args.format, args.json))?
            );
        }
        (false, true) => {
            let preview = copilot_config_import_preview(args.path)?;
            let output = args.output.unwrap_or_else(default_djinn_config_path);
            let report = write_config_import_preview(&preview, &output, args.force)?;
            print!(
                "{}",
                format_config_import_write_report(&report, output_format(args.format, args.json))?
            );
        }
    }
    Ok(())
}

fn validate_config_import_mode(dry_run: bool, write: bool, merge: bool, force: bool) -> Result<()> {
    if dry_run && write {
        bail!("choose either --dry-run or --write, not both");
    }
    if merge && !write {
        bail!("--merge is only meaningful with --write");
    }
    if merge && force {
        bail!("choose either --merge or --force, not both");
    }
    if !dry_run && !write {
        bail!("config import is safe by default; pass --dry-run to preview or --write to create a Djinn config file");
    }
    Ok(())
}

fn config_doctor(args: ConfigDoctorArgs) -> Result<()> {
    let report = match args.source {
        ConfigSource::Copilot => copilot_config_doctor(args.path)?,
        ConfigSource::Djinn => djinn_config_doctor(args.path)?,
        ConfigSource::Opencode => opencode_config_doctor(args.path)?,
    };
    print!(
        "{}",
        format_config_doctor_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ConfigDoctorReport {
    source: String,
    checked_paths: Vec<String>,
    files: Vec<ConfigDoctorFileReport>,
    summary: ConfigDoctorSummary,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
struct ConfigDoctorSummary {
    checked_path_count: usize,
    readable_file_count: usize,
    mapped_count: usize,
    unsupported_count: usize,
    unknown_count: usize,
    secret_count: usize,
    error_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ConfigDoctorFileReport {
    path: String,
    exists: bool,
    readable: bool,
    mapped: Vec<ConfigDoctorFinding>,
    unsupported: Vec<ConfigDoctorFinding>,
    unknown: Vec<ConfigDoctorFinding>,
    secrets: Vec<ConfigDoctorFinding>,
    errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ConfigDoctorFinding {
    pointer: String,
    concept: String,
    djinn_mapping: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DjinnConfig {
    #[serde(default = "default_djinn_config_version")]
    version: u16,
    #[serde(default)]
    default_profile: Option<String>,
    #[serde(default)]
    providers: BTreeMap<String, DjinnConfigProvider>,
    #[serde(default)]
    profiles: BTreeMap<String, DjinnConfigProfile>,
    #[serde(default)]
    permissions: Vec<DjinnConfigPermission>,
    #[serde(default)]
    instructions: BTreeMap<String, DjinnConfigInstruction>,
    #[serde(default)]
    commands: BTreeMap<String, DjinnConfigCommandTemplate>,
    #[serde(default)]
    tools: BTreeMap<String, DjinnConfigTool>,
    #[serde(default)]
    agents: BTreeMap<String, DjinnConfigAgent>,
}

impl Default for DjinnConfig {
    fn default() -> Self {
        Self {
            version: default_djinn_config_version(),
            default_profile: None,
            providers: BTreeMap::new(),
            profiles: BTreeMap::new(),
            permissions: Vec::new(),
            instructions: BTreeMap::new(),
            commands: BTreeMap::new(),
            tools: BTreeMap::new(),
            agents: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct DjinnConfigProvider {
    #[serde(rename = "type")]
    provider_type: String,
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct DjinnConfigProfile {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    instructions: Vec<String>,
    #[serde(default)]
    permissions: Vec<DjinnConfigPermission>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DjinnConfigPermission {
    action: String,
    #[serde(default = "default_permission_resource")]
    resource: String,
    effect: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct DjinnConfigInstruction {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct DjinnConfigCommandTemplate {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct DjinnConfigTool {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    permission: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct DjinnConfigAgent {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    instructions: Vec<String>,
    #[serde(default)]
    tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DjinnConfigLoadReport {
    checked_paths: Vec<String>,
    files: Vec<DjinnConfigFileReport>,
    effective: DjinnConfig,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DjinnConfigFileReport {
    path: String,
    exists: bool,
    readable: bool,
    errors: Vec<String>,
}

fn default_djinn_config_version() -> u16 {
    1
}

fn default_permission_resource() -> String {
    "*".to_string()
}

fn default_djinn_config_path() -> PathBuf {
    djinn_config_dir().join("config.json")
}

fn djinn_config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| djinn_core::home_dir().join(".config"))
        .join("djinn")
}

fn djinn_config_paths(cwd: &Path) -> Vec<PathBuf> {
    clean_unique_paths(vec![default_djinn_config_path(), cwd.join(".djinn.json")])
}

fn load_djinn_config(path: Option<PathBuf>) -> Result<DjinnConfigLoadReport> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(|| djinn_config_paths(&cwd)),
    );
    load_djinn_config_from_paths(paths)
}

fn load_djinn_config_from_paths(paths: Vec<PathBuf>) -> Result<DjinnConfigLoadReport> {
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut configs = Vec::new();
    let mut warnings = Vec::new();

    for path in paths {
        if !path.exists() {
            files.push(DjinnConfigFileReport {
                path: path.display().to_string(),
                exists: false,
                readable: false,
                errors: Vec::new(),
            });
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                files.push(DjinnConfigFileReport {
                    path: path.display().to_string(),
                    exists: true,
                    readable: false,
                    errors: vec![format!("read failed: {error}")],
                });
                continue;
            }
        };
        match parse_djinn_config(&content) {
            Ok(config) => {
                files.push(DjinnConfigFileReport {
                    path: path.display().to_string(),
                    exists: true,
                    readable: true,
                    errors: Vec::new(),
                });
                configs.push(config);
            }
            Err(error) => files.push(DjinnConfigFileReport {
                path: path.display().to_string(),
                exists: true,
                readable: true,
                errors: vec![format!("parse failed: {error}")],
            }),
        }
    }

    if configs.is_empty() {
        warnings.push(
            "no readable Djinn config files found; using built-in empty defaults".to_string(),
        );
    }

    Ok(DjinnConfigLoadReport {
        checked_paths,
        files,
        effective: merge_djinn_configs(configs),
        warnings,
    })
}

fn effective_djinn_config() -> Result<DjinnConfig> {
    Ok(load_djinn_config(None)?.effective)
}

fn parse_djinn_config(content: &str) -> Result<DjinnConfig> {
    let config: DjinnConfig = serde_json::from_str(content)?;
    validate_djinn_config(&config)?;
    Ok(config)
}

fn validate_djinn_config(config: &DjinnConfig) -> Result<()> {
    if config.version != 1 {
        bail!(
            "unsupported Djinn config version {}; expected 1",
            config.version
        );
    }
    Ok(())
}

fn merge_djinn_configs(configs: Vec<DjinnConfig>) -> DjinnConfig {
    let mut effective = DjinnConfig::default();
    for config in configs {
        if config.default_profile.is_some() {
            effective.default_profile = config.default_profile;
        }
        effective.providers.extend(config.providers);
        effective.profiles.extend(config.profiles);
        effective.permissions.extend(config.permissions);
        effective.instructions.extend(config.instructions);
        effective.commands.extend(config.commands);
        effective.tools.extend(config.tools);
        effective.agents.extend(config.agents);
    }
    effective
}

fn djinn_config_doctor(path: Option<PathBuf>) -> Result<ConfigDoctorReport> {
    let load = load_djinn_config(path)?;
    let mut files = Vec::new();
    for file in &load.files {
        let mut report = ConfigDoctorFileReport {
            path: file.path.clone(),
            exists: file.exists,
            readable: file.readable,
            mapped: Vec::new(),
            unsupported: Vec::new(),
            unknown: Vec::new(),
            secrets: Vec::new(),
            errors: file.errors.clone(),
        };
        if file.readable && file.errors.is_empty() {
            let content = fs::read_to_string(&file.path).unwrap_or_default();
            if let Ok(value) = serde_json::from_str::<Value>(&content) {
                report = djinn_config_doctor_from_value(Path::new(&file.path), &value);
            }
        }
        files.push(report);
    }
    Ok(ConfigDoctorReport {
        source: "djinn".to_string(),
        checked_paths: load.checked_paths,
        summary: config_doctor_summary(&files),
        files,
    })
}

fn djinn_config_doctor_from_value(path: &Path, value: &Value) -> ConfigDoctorFileReport {
    let mut file = ConfigDoctorFileReport {
        path: path.display().to_string(),
        exists: true,
        readable: true,
        mapped: Vec::new(),
        unsupported: Vec::new(),
        unknown: Vec::new(),
        secrets: Vec::new(),
        errors: Vec::new(),
    };

    collect_config_secrets(value, "", &mut file.secrets);
    let Some(object) = value.as_object() else {
        file.errors
            .push("Djinn config root must be a JSON object".to_string());
        return file;
    };
    for key in object.keys() {
        let pointer = format!("/{}", json_pointer_escape(key));
        match key.as_str() {
            "version" => push_mapped(
                &mut file,
                &pointer,
                "Djinn config schema version",
                "native schema migration guard",
                "Version 1 is the current native config schema.",
            ),
            "default_profile" => push_mapped(
                &mut file,
                &pointer,
                "Djinn default profile",
                "native default profile",
                "Used when no command/session profile is specified.",
            ),
            "providers" => push_mapped(
                &mut file,
                &pointer,
                "Djinn providers",
                "native provider registry",
                "Defines provider types, endpoints, and secret references.",
            ),
            "profiles" => push_mapped(
                &mut file,
                &pointer,
                "Djinn profiles",
                "native profile registry",
                "Defines profile model, instructions, tools, and permissions.",
            ),
            "permissions" => push_mapped(
                &mut file,
                &pointer,
                "Djinn shared permissions",
                "native permission defaults",
                "Defines shared read/write/shell policy before profile overrides.",
            ),
            "instructions" => push_mapped(
                &mut file,
                &pointer,
                "Djinn instruction sources",
                "native context/instruction registry",
                "Defines reusable instruction sources by path or inline text.",
            ),
            "commands" => push_mapped(
                &mut file,
                &pointer,
                "Djinn command templates",
                "native prompt template registry",
                "Defines reusable prompt templates for future command palette flows.",
            ),
            "tools" => push_mapped(
                &mut file,
                &pointer,
                "Djinn tool policy",
                "native tool registry settings",
                "Defines tool enablement and permission hints.",
            ),
            "agents" => push_mapped(
                &mut file,
                &pointer,
                "Djinn agents",
                "native sub-agent registry",
                "Reserved for future constrained agent definitions.",
            ),
            _ if is_secret_key(key) => push_secret(
                &mut file.secrets,
                &pointer,
                "Secret-like Djinn config field",
                "secret reference only",
                "Value intentionally redacted; native config should prefer secret references.",
            ),
            _ => push_unknown(
                &mut file,
                &pointer,
                "Unknown Djinn config field",
                "no native mapping",
                "Field is not part of Djinn config schema version 1.",
            ),
        }
    }
    dedupe_config_findings(&mut file.secrets);
    file
}

fn copilot_config_doctor(path: Option<PathBuf>) -> Result<ConfigDoctorReport> {
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(copilot_model_config_paths),
    );
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    for path in paths {
        if !path.exists() {
            files.push(ConfigDoctorFileReport {
                path: path.display().to_string(),
                exists: false,
                readable: false,
                mapped: Vec::new(),
                unsupported: Vec::new(),
                unknown: Vec::new(),
                secrets: Vec::new(),
                errors: Vec::new(),
            });
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                files.push(ConfigDoctorFileReport {
                    path: path.display().to_string(),
                    exists: true,
                    readable: false,
                    mapped: Vec::new(),
                    unsupported: Vec::new(),
                    unknown: Vec::new(),
                    secrets: Vec::new(),
                    errors: vec![format!("read failed: {error}")],
                });
                continue;
            }
        };
        match serde_json::from_str::<Value>(&content) {
            Ok(value) => files.push(copilot_config_doctor_from_value(&path, &value)),
            Err(error) => files.push(ConfigDoctorFileReport {
                path: path.display().to_string(),
                exists: true,
                readable: true,
                mapped: Vec::new(),
                unsupported: Vec::new(),
                unknown: Vec::new(),
                secrets: Vec::new(),
                errors: vec![format!("parse failed: {error}")],
            }),
        }
    }
    Ok(ConfigDoctorReport {
        source: "copilot".to_string(),
        checked_paths,
        summary: config_doctor_summary(&files),
        files,
    })
}

fn copilot_config_doctor_from_value(path: &Path, value: &Value) -> ConfigDoctorFileReport {
    let mut file = ConfigDoctorFileReport {
        path: path.display().to_string(),
        exists: true,
        readable: true,
        mapped: Vec::new(),
        unsupported: Vec::new(),
        unknown: Vec::new(),
        secrets: Vec::new(),
        errors: Vec::new(),
    };
    collect_config_secrets(value, "", &mut file.secrets);
    if !copilot_model_options_from_value(value).is_empty() {
        push_mapped(
            &mut file,
            "/",
            "Copilot model configuration",
            "Djinn copilot provider/default profile model",
            "Model-like entries can be imported into Djinn native provider/profile config.",
        );
    }
    if !file.secrets.is_empty() {
        push_mapped(
            &mut file,
            "/",
            "Copilot auth configuration",
            "Djinn provider auth = auto",
            "Token-like fields are detected as secret references and are not printed or copied raw.",
        );
    }
    let Some(object) = value.as_object() else {
        file.errors
            .push("Copilot config root must be a JSON object".to_string());
        return file;
    };
    for key in object.keys() {
        let pointer = format!("/{}", json_pointer_escape(key));
        match key.as_str() {
            "model" | "models" | "model_id" | "modelId" | "selected_model" | "selectedModel"
            | "default_model" | "defaultModel" | "available_models" | "availableModels"
            | "chat_models" | "chatModels" | "model_choices" | "modelChoices" | "custom_models"
            | "customModels" => push_mapped(
                &mut file,
                &pointer,
                "Copilot model field",
                "Djinn profile model option",
                "Recognized as a Copilot model source.",
            ),
            "github.com" | "apps" | "github" | "oauth_token" | "oauthToken" => push_mapped(
                &mut file,
                &pointer,
                "Copilot auth field",
                "Djinn provider auth = auto",
                "Recognized as Copilot/GitHub auth state; values are not exported raw.",
            ),
            _ if is_secret_key(key) => push_secret(
                &mut file.secrets,
                &pointer,
                "Secret-like Copilot field",
                "secret reference only",
                "Value intentionally redacted and not imported/exported raw.",
            ),
            _ => push_unknown(
                &mut file,
                &pointer,
                "Unknown Copilot config field",
                "no Djinn mapping yet",
                "Not recognized by the current Copilot adapter.",
            ),
        }
    }
    dedupe_config_findings(&mut file.secrets);
    file
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ConfigImportPreview {
    source: String,
    mode: String,
    checked_paths: Vec<String>,
    readable_files: Vec<String>,
    patch: DjinnConfigPatchPreview,
    unsupported: Vec<ConfigDoctorFinding>,
    unknown: Vec<ConfigDoctorFinding>,
    secrets: Vec<ConfigDoctorFinding>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ConfigImportWriteReport {
    source: String,
    mode: String,
    path: String,
    overwritten: bool,
    merged: bool,
    summary: ConfigImportWriteSummary,
    config: DjinnConfig,
    unsupported: Vec<ConfigDoctorFinding>,
    unknown: Vec<ConfigDoctorFinding>,
    secrets: Vec<ConfigDoctorFinding>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
struct ConfigImportWriteSummary {
    applied_default_profile: Option<String>,
    preserved_default_profile: Option<String>,
    skipped_import_default_profile: Option<String>,
    added_providers: Vec<String>,
    skipped_providers: Vec<String>,
    added_profiles: Vec<String>,
    skipped_profiles: Vec<String>,
    added_shared_permissions: usize,
    skipped_shared_permissions: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ConfigExportPreview {
    target: String,
    mode: String,
    checked_paths: Vec<String>,
    readable_files: Vec<String>,
    config: Value,
    unsupported: Vec<ConfigDoctorFinding>,
    secrets: Vec<ConfigDoctorFinding>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ConfigExportWriteReport {
    target: String,
    mode: String,
    path: String,
    overwritten: bool,
    config: Value,
    unsupported: Vec<ConfigDoctorFinding>,
    secrets: Vec<ConfigDoctorFinding>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DjinnConfigPatchPreview {
    version: u16,
    default_profile: Option<String>,
    providers: BTreeMap<String, DjinnProviderPatchPreview>,
    profiles: BTreeMap<String, DjinnProfilePatchPreview>,
    permissions: Vec<DjinnPermissionPatchPreview>,
}

impl Default for DjinnConfigPatchPreview {
    fn default() -> Self {
        Self {
            version: 1,
            default_profile: None,
            providers: BTreeMap::new(),
            profiles: BTreeMap::new(),
            permissions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
struct DjinnProviderPatchPreview {
    #[serde(rename = "type")]
    provider_type: String,
    auth: Option<String>,
    source_pointers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
struct DjinnProfilePatchPreview {
    model: Option<String>,
    instructions: Vec<String>,
    permissions: Vec<DjinnPermissionPatchPreview>,
    source_pointers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DjinnPermissionPatchPreview {
    action: String,
    resource: String,
    effect: String,
    source_pointer: String,
}

fn opencode_config_import_preview(path: Option<PathBuf>) -> Result<ConfigImportPreview> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(|| opencode_model_config_paths(&cwd)),
    );
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut sources = Vec::new();
    let mut warnings = Vec::new();

    for path in &paths {
        if !path.exists() {
            continue;
        }
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) => {
                warnings.push(format!("{}: read failed: {error}", path.display()));
                continue;
            }
        };
        match serde_json::from_str::<Value>(&content) {
            Ok(value) => sources.push((path.clone(), value)),
            Err(error) => warnings.push(format!("{}: parse failed: {error}", path.display())),
        }
    }

    Ok(opencode_config_import_preview_from_values(
        checked_paths,
        sources,
        warnings,
    ))
}

fn opencode_config_export_preview(path: Option<PathBuf>) -> Result<ConfigExportPreview> {
    let report = load_djinn_config(path)?;
    Ok(opencode_config_export_preview_from_load_report(report))
}

fn copilot_config_import_preview(path: Option<PathBuf>) -> Result<ConfigImportPreview> {
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(copilot_model_config_paths),
    );
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut sources = Vec::new();
    let mut warnings = Vec::new();
    for path in &paths {
        if !path.exists() {
            continue;
        }
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) => {
                warnings.push(format!("{}: read failed: {error}", path.display()));
                continue;
            }
        };
        match serde_json::from_str::<Value>(&content) {
            Ok(value) => sources.push((path.clone(), value)),
            Err(error) => warnings.push(format!("{}: parse failed: {error}", path.display())),
        }
    }
    Ok(copilot_config_import_preview_from_values(
        checked_paths,
        sources,
        warnings,
    ))
}

fn copilot_config_import_preview_from_values(
    checked_paths: Vec<String>,
    sources: Vec<(PathBuf, Value)>,
    mut warnings: Vec<String>,
) -> ConfigImportPreview {
    let mut patch = DjinnConfigPatchPreview::default();
    let mut unsupported = Vec::new();
    let mut unknown = Vec::new();
    let mut secrets = Vec::new();
    let readable_files = sources
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();
    if sources.is_empty() {
        warnings.push("no readable Copilot config files found".to_string());
    }

    for (path, value) in &sources {
        let doctor = copilot_config_doctor_from_value(path, value);
        let model_options = copilot_model_options_from_value(value);
        let has_auth = !doctor.secrets.is_empty();
        unsupported.extend(doctor.unsupported);
        unknown.extend(doctor.unknown);
        secrets.extend(doctor.secrets);
        if has_auth || !model_options.is_empty() {
            let provider = patch.providers.entry("copilot".to_string()).or_default();
            provider.provider_type = "copilot".to_string();
            if has_auth {
                provider.auth = Some("auto".to_string());
            }
            push_unique_string(&mut provider.source_pointers, &path.display().to_string());
        }
        for model in model_options {
            let profile = patch.profiles.entry("default".to_string()).or_default();
            if profile.model.is_none() {
                profile.model = Some(model);
                push_unique_string(&mut profile.source_pointers, &path.display().to_string());
            }
        }
    }
    if patch.default_profile.is_none() && patch.profiles.contains_key("default") {
        patch.default_profile = Some("default".to_string());
    }
    dedupe_config_findings(&mut unsupported);
    dedupe_config_findings(&mut unknown);
    dedupe_config_findings(&mut secrets);
    ConfigImportPreview {
        source: "copilot".to_string(),
        mode: "dry-run".to_string(),
        checked_paths,
        readable_files,
        patch,
        unsupported,
        unknown,
        secrets,
        warnings,
    }
}

fn copilot_config_export_preview(path: Option<PathBuf>) -> Result<ConfigExportPreview> {
    let report = load_djinn_config(path)?;
    Ok(copilot_config_export_preview_from_load_report(report))
}

fn copilot_config_export_preview_from_load_report(
    report: DjinnConfigLoadReport,
) -> ConfigExportPreview {
    let native = &report.effective;
    let mut config = Map::new();
    let mut models = Vec::new();
    let mut unsupported = Vec::new();
    let mut secrets = Vec::new();
    let mut warnings = report.warnings.clone();

    if let Some(default_profile) = native.default_profile.as_deref() {
        if let Some(model) = profile_model_from_config(native, default_profile)
            .and_then(|model| copilot_export_model_id(&model))
        {
            config.insert("model".to_string(), Value::String(model.clone()));
            models.push(Value::String(model));
        }
    }
    for profile in native.profiles.values() {
        if let Some(model) = profile.model.as_deref().and_then(copilot_export_model_id) {
            if !models
                .iter()
                .any(|value| value.as_str() == Some(model.as_str()))
            {
                models.push(Value::String(model));
            }
        }
        if !profile.permissions.is_empty()
            || !profile.instructions.is_empty()
            || !profile.tools.is_empty()
            || profile.agent.is_some()
        {
            unsupported.push(config_finding(
                "/profiles",
                "Djinn profile metadata",
                "Copilot model-only export",
                "Copilot export currently maps provider/model choices only; profile metadata remains Djinn-native.",
            ));
        }
    }
    if !models.is_empty() {
        config.insert("models".to_string(), Value::Array(models));
    }
    if native.providers.contains_key("copilot") || native.providers.contains_key("github-copilot") {
        config.insert(
            "provider".to_string(),
            Value::String("github-copilot".to_string()),
        );
    }
    for (name, provider) in &native.providers {
        if name == "copilot" || name == "github-copilot" {
            if let Some(auth) = provider
                .auth
                .as_deref()
                .filter(|auth| !auth.trim().is_empty())
            {
                secrets.push(config_finding(
                    &format!("/providers/{}/auth", json_pointer_escape(name)),
                    "Djinn Copilot auth reference",
                    "not exported raw",
                    &format!("Copilot export omits auth reference `{}`; authenticate the target Copilot CLI separately.", redact_secret_reference(auth)),
                ));
            }
            if provider.endpoint.is_some() {
                unsupported.push(config_finding(
                    &format!("/providers/{}", json_pointer_escape(name)),
                    "Djinn Copilot endpoint",
                    "Copilot endpoint not exported",
                    "Endpoint export needs a concrete Copilot CLI schema mapping.",
                ));
            }
        }
    }
    if !native.permissions.is_empty()
        || !native.instructions.is_empty()
        || !native.commands.is_empty()
        || !native.tools.is_empty()
        || !native.agents.is_empty()
    {
        unsupported.push(config_finding(
            "/",
            "Djinn native-only config",
            "Copilot model/provider export only",
            "Shared permissions, instructions, commands, tools, and agents are not represented in the current Copilot export shape.",
        ));
    }
    dedupe_config_findings(&mut unsupported);
    dedupe_config_findings(&mut secrets);
    if report.files.iter().all(|file| !file.readable) {
        warnings.push("export preview used built-in empty Djinn config defaults".to_string());
    }
    ConfigExportPreview {
        target: "copilot".to_string(),
        mode: "dry-run".to_string(),
        checked_paths: report.checked_paths,
        readable_files: report
            .files
            .iter()
            .filter(|file| file.readable && file.errors.is_empty())
            .map(|file| file.path.clone())
            .collect(),
        config: Value::Object(config),
        unsupported,
        secrets,
        warnings,
    }
}

fn copilot_export_model_id(model: &str) -> Option<String> {
    let model = model.trim();
    if !is_copilot_model(model) {
        return None;
    }
    Some(
        model
            .strip_prefix("copilot/")
            .or_else(|| model.strip_prefix("github-copilot/"))
            .unwrap_or(model)
            .to_string(),
    )
}

fn opencode_config_export_preview_from_load_report(
    report: DjinnConfigLoadReport,
) -> ConfigExportPreview {
    let mut config = Map::new();
    let mut unsupported = Vec::new();
    let mut secrets = Vec::new();
    let mut warnings = report.warnings.clone();
    let native = &report.effective;

    if let Some(default_profile) = native
        .default_profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
    {
        config.insert(
            "default_agent".to_string(),
            Value::String(default_profile.to_string()),
        );
        if let Some(model) = profile_model_from_config(native, default_profile) {
            config.insert("model".to_string(), Value::String(model));
        }
    }

    let mut enabled_providers = Vec::new();
    for (name, provider) in &native.providers {
        enabled_providers.push(Value::String(name.clone()));
        if let Some(auth) = provider
            .auth
            .as_deref()
            .map(str::trim)
            .filter(|auth| !auth.is_empty())
        {
            secrets.push(config_finding(
                &format!("/providers/{}/auth", json_pointer_escape(name)),
                "Djinn provider auth reference",
                "not exported raw",
                &format!(
                    "OpenCode export omits provider `{name}` auth reference `{}`; configure secrets in the target harness.",
                    redact_secret_reference(auth)
                ),
            ));
        }
        if provider.endpoint.is_some() {
            unsupported.push(config_finding(
                &format!("/providers/{}", json_pointer_escape(name)),
                "Djinn provider endpoint",
                "OpenCode provider endpoint not exported",
                "Endpoint export needs a target-specific provider schema decision.",
            ));
        }
    }
    if !enabled_providers.is_empty() {
        config.insert(
            "enabled_providers".to_string(),
            Value::Array(enabled_providers),
        );
    }

    let mut agent_map = Map::new();
    for (name, profile) in &native.profiles {
        let mut agent = Map::new();
        if let Some(model) = profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            agent.insert("model".to_string(), Value::String(model.to_string()));
        }
        if !profile.permissions.is_empty() {
            agent.insert(
                "permissions".to_string(),
                Value::Array(opencode_permission_values_from_djinn_permissions(
                    &profile.permissions,
                )),
            );
        }
        if !profile.instructions.is_empty() {
            unsupported.push(config_finding(
                &format!("/profiles/{}/instructions", json_pointer_escape(name)),
                "Djinn profile instructions",
                "OpenCode instructions not exported yet",
                "Instruction precedence and path semantics need a target-specific export decision.",
            ));
        }
        if !profile.tools.is_empty() {
            unsupported.push(config_finding(
                &format!("/profiles/{}/tools", json_pointer_escape(name)),
                "Djinn profile tools",
                "OpenCode tools not exported yet",
                "Tool export needs a target-specific tool/MCP mapping decision.",
            ));
        }
        if profile.agent.is_some() {
            unsupported.push(config_finding(
                &format!("/profiles/{}/agent", json_pointer_escape(name)),
                "Djinn profile agent link",
                "OpenCode agent link not exported yet",
                "Profile-to-agent links need a finalized sub-agent mapping.",
            ));
        }
        agent_map.insert(name.clone(), Value::Object(agent));
    }
    if !agent_map.is_empty() {
        config.insert("agent".to_string(), Value::Object(agent_map));
    }

    if !native.permissions.is_empty() {
        config.insert(
            "permissions".to_string(),
            Value::Array(opencode_permission_values_from_djinn_permissions(
                &native.permissions,
            )),
        );
    }

    collect_native_export_unsupported(native, &mut unsupported);
    dedupe_config_findings(&mut unsupported);
    dedupe_config_findings(&mut secrets);
    if report.files.iter().all(|file| !file.readable) {
        warnings.push("export preview used built-in empty Djinn config defaults".to_string());
    }

    ConfigExportPreview {
        target: "opencode".to_string(),
        mode: "dry-run".to_string(),
        checked_paths: report.checked_paths,
        readable_files: report
            .files
            .iter()
            .filter(|file| file.readable && file.errors.is_empty())
            .map(|file| file.path.clone())
            .collect(),
        config: Value::Object(config),
        unsupported,
        secrets,
        warnings,
    }
}

fn opencode_permission_values_from_djinn_permissions(
    permissions: &[DjinnConfigPermission],
) -> Vec<Value> {
    permissions
        .iter()
        .map(|permission| {
            serde_json::json!({
                "action": opencode_export_permission_action(&permission.action),
                "resource": permission.resource,
                "effect": permission.effect,
            })
        })
        .collect()
}

fn opencode_export_permission_action(action: &str) -> String {
    match action.trim() {
        "shell" => "bash".to_string(),
        other if other.is_empty() => "*".to_string(),
        other => other.to_string(),
    }
}

fn collect_native_export_unsupported(
    native: &DjinnConfig,
    unsupported: &mut Vec<ConfigDoctorFinding>,
) {
    if !native.instructions.is_empty() {
        unsupported.push(config_finding(
            "/instructions",
            "Djinn instruction registry",
            "OpenCode instructions not exported yet",
            "Instruction export needs path precedence and workspace scoping decisions.",
        ));
    }
    if !native.commands.is_empty() {
        unsupported.push(config_finding(
            "/commands",
            "Djinn command templates",
            "OpenCode commands not exported yet",
            "Command-template export needs target-specific command schema mapping.",
        ));
    }
    if !native.tools.is_empty() {
        unsupported.push(config_finding(
            "/tools",
            "Djinn tool settings",
            "OpenCode tools not exported yet",
            "Tool export needs a target-specific tool/MCP mapping decision.",
        ));
    }
    if !native.agents.is_empty() {
        unsupported.push(config_finding(
            "/agents",
            "Djinn sub-agents",
            "OpenCode sub-agents not exported yet",
            "Sub-agent export needs the finalized Djinn agent model.",
        ));
    }
}

fn redact_secret_reference(value: &str) -> String {
    if value.starts_with("env:") || value == "auto" || value.starts_with("opencode:") {
        value.to_string()
    } else {
        "<redacted>".to_string()
    }
}

fn write_config_export_preview(
    preview: &ConfigExportPreview,
    output: &Path,
    force: bool,
) -> Result<ConfigExportWriteReport> {
    let label = match preview.target.as_str() {
        "copilot" => "Copilot",
        "opencode" => "OpenCode",
        _ => "target",
    };
    let overwritten = write_json_config_file(&preview.config, output, force, label)?;
    Ok(ConfigExportWriteReport {
        target: preview.target.clone(),
        mode: "write".to_string(),
        path: output.display().to_string(),
        overwritten,
        config: preview.config.clone(),
        unsupported: preview.unsupported.clone(),
        secrets: preview.secrets.clone(),
        warnings: preview.warnings.clone(),
    })
}

fn write_json_config_file(value: &Value, output: &Path, force: bool, label: &str) -> Result<bool> {
    let exists = output.exists();
    if exists && !force {
        bail!(
            "refusing to overwrite existing {label} config {}; pass --force to replace it or choose --output",
            output.display()
        );
    }
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {label} config directory {}", parent.display()))?;
    }
    let mut rendered = serde_json::to_string_pretty(value)?;
    rendered.push('\n');
    fs::write(output, rendered)
        .with_context(|| format!("writing {label} config {}", output.display()))?;
    Ok(exists)
}

fn write_config_import_preview(
    preview: &ConfigImportPreview,
    output: &Path,
    force: bool,
) -> Result<ConfigImportWriteReport> {
    if preview.readable_files.is_empty() {
        bail!(
            "no readable {} config files found; nothing to write",
            preview.source
        );
    }
    let imported = djinn_config_from_import_patch(&preview.patch);
    let (config, overwritten, merged, summary, warnings) = if output.exists() && !force {
        let existing = read_djinn_config_file(output)?;
        let warnings = preview.warnings.clone();
        let (config, summary) = merge_import_patch_into_djinn_config(existing, &preview.patch);
        let _ = write_djinn_config_file(&config, output, true)?;
        (config, false, true, summary, warnings)
    } else {
        let overwritten = write_djinn_config_file(&imported, output, force)?;
        let summary = import_write_summary_from_patch(&preview.patch);
        (
            imported,
            overwritten,
            false,
            summary,
            preview.warnings.clone(),
        )
    };
    Ok(ConfigImportWriteReport {
        source: preview.source.clone(),
        mode: "write".to_string(),
        path: output.display().to_string(),
        overwritten,
        merged,
        summary,
        config,
        unsupported: preview.unsupported.clone(),
        unknown: preview.unknown.clone(),
        secrets: preview.secrets.clone(),
        warnings,
    })
}

fn read_djinn_config_file(path: &Path) -> Result<DjinnConfig> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading existing Djinn config {}", path.display()))?;
    parse_djinn_config(&content)
        .with_context(|| format!("parsing existing Djinn config {}", path.display()))
}

fn write_djinn_config_file(config: &DjinnConfig, output: &Path, force: bool) -> Result<bool> {
    let value = serde_json::to_value(config)?;
    write_json_config_file(&value, output, force, "Djinn")
}

fn merge_import_patch_into_djinn_config(
    mut existing: DjinnConfig,
    patch: &DjinnConfigPatchPreview,
) -> (DjinnConfig, ConfigImportWriteSummary) {
    let mut summary = ConfigImportWriteSummary::default();

    if existing.default_profile.is_none() {
        existing.default_profile = patch.default_profile.clone();
        summary.applied_default_profile = patch.default_profile.clone();
    } else if patch.default_profile.is_some()
        && existing.default_profile.as_ref() != patch.default_profile.as_ref()
    {
        summary.preserved_default_profile = existing.default_profile.clone();
        summary.skipped_import_default_profile = patch.default_profile.clone();
    }

    for (name, provider) in &patch.providers {
        if djinn_provider_exists_with_alias(&existing.providers, name) {
            summary.skipped_providers.push(name.clone());
            continue;
        }
        existing.providers.insert(
            name.clone(),
            DjinnConfigProvider {
                provider_type: provider.provider_type.clone(),
                auth: provider.auth.clone(),
                endpoint: None,
            },
        );
        summary.added_providers.push(name.clone());
    }

    for (name, profile) in &patch.profiles {
        if existing.profiles.contains_key(name) {
            summary.skipped_profiles.push(name.clone());
            continue;
        }
        existing.profiles.insert(
            name.clone(),
            DjinnConfigProfile {
                model: profile.model.clone(),
                instructions: profile.instructions.clone(),
                permissions: profile
                    .permissions
                    .iter()
                    .map(djinn_config_permission_from_patch)
                    .collect(),
                tools: Vec::new(),
                agent: None,
            },
        );
        summary.added_profiles.push(name.clone());
    }

    for permission in &patch.permissions {
        let permission = djinn_config_permission_from_patch(permission);
        if existing.permissions.contains(&permission) {
            summary.skipped_shared_permissions += 1;
            continue;
        }
        existing.permissions.push(permission);
        summary.added_shared_permissions += 1;
    }

    (existing, summary)
}

fn import_write_summary_from_patch(patch: &DjinnConfigPatchPreview) -> ConfigImportWriteSummary {
    ConfigImportWriteSummary {
        applied_default_profile: patch.default_profile.clone(),
        added_providers: patch.providers.keys().cloned().collect(),
        added_profiles: patch.profiles.keys().cloned().collect(),
        added_shared_permissions: patch.permissions.len(),
        ..ConfigImportWriteSummary::default()
    }
}

fn djinn_provider_exists_with_alias(
    providers: &BTreeMap<String, DjinnConfigProvider>,
    imported_name: &str,
) -> bool {
    providers
        .keys()
        .any(|existing| djinn_provider_names_match(existing, imported_name))
}

fn djinn_provider_names_match(existing: &str, imported: &str) -> bool {
    existing == imported
        || (is_copilot_provider_name(existing) && is_copilot_provider_name(imported))
}

fn is_copilot_provider_name(name: &str) -> bool {
    matches!(name, "copilot" | "github-copilot")
}

fn djinn_config_from_import_patch(patch: &DjinnConfigPatchPreview) -> DjinnConfig {
    DjinnConfig {
        version: patch.version,
        default_profile: patch.default_profile.clone(),
        providers: patch
            .providers
            .iter()
            .map(|(name, provider)| {
                (
                    name.clone(),
                    DjinnConfigProvider {
                        provider_type: provider.provider_type.clone(),
                        auth: provider.auth.clone(),
                        endpoint: None,
                    },
                )
            })
            .collect(),
        profiles: patch
            .profiles
            .iter()
            .map(|(name, profile)| {
                (
                    name.clone(),
                    DjinnConfigProfile {
                        model: profile.model.clone(),
                        instructions: profile.instructions.clone(),
                        permissions: profile
                            .permissions
                            .iter()
                            .map(djinn_config_permission_from_patch)
                            .collect(),
                        tools: Vec::new(),
                        agent: None,
                    },
                )
            })
            .collect(),
        permissions: patch
            .permissions
            .iter()
            .map(djinn_config_permission_from_patch)
            .collect(),
        instructions: BTreeMap::new(),
        commands: BTreeMap::new(),
        tools: BTreeMap::new(),
        agents: BTreeMap::new(),
    }
}

fn djinn_config_permission_from_patch(
    permission: &DjinnPermissionPatchPreview,
) -> DjinnConfigPermission {
    DjinnConfigPermission {
        action: permission.action.clone(),
        resource: permission.resource.clone(),
        effect: permission.effect.clone(),
    }
}

fn opencode_config_import_preview_from_values(
    checked_paths: Vec<String>,
    sources: Vec<(PathBuf, Value)>,
    mut warnings: Vec<String>,
) -> ConfigImportPreview {
    let mut patch = DjinnConfigPatchPreview::default();
    let mut unsupported = Vec::new();
    let mut unknown = Vec::new();
    let mut secrets = Vec::new();
    let readable_files = sources
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();

    if sources.is_empty() {
        warnings.push("no readable OpenCode config files found".to_string());
    }

    for (path, value) in &sources {
        let doctor = opencode_config_doctor_from_value(path, value);
        unsupported.extend(doctor.unsupported);
        unknown.extend(doctor.unknown);
        secrets.extend(doctor.secrets);
        apply_opencode_config_to_patch(value, &mut patch);
    }

    dedupe_config_findings(&mut unsupported);
    dedupe_config_findings(&mut unknown);
    dedupe_config_findings(&mut secrets);

    ConfigImportPreview {
        source: "opencode".to_string(),
        mode: "dry-run".to_string(),
        checked_paths,
        readable_files,
        patch,
        unsupported,
        unknown,
        secrets,
        warnings,
    }
}

fn apply_opencode_config_to_patch(value: &Value, patch: &mut DjinnConfigPatchPreview) {
    let Some(object) = value.as_object() else {
        return;
    };

    let default_profile = object
        .get("default_agent")
        .or_else(|| object.get("defaultAgent"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(ToOwned::to_owned);
    if patch.default_profile.is_none() {
        patch.default_profile = default_profile.clone();
    }
    let fallback_profile = default_profile
        .clone()
        .or_else(|| patch.default_profile.clone())
        .unwrap_or_else(|| "default".to_string());

    if let Some(model) = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        let profile = patch.profiles.entry(fallback_profile.clone()).or_default();
        if profile.model.is_none() {
            profile.model = Some(model.to_string());
        }
        push_unique_string(&mut profile.source_pointers, "/model");
        add_provider_from_model(model, patch);
    }

    collect_import_permissions_from_value(value, "", &mut patch.permissions);
    collect_import_providers(value, patch);
    collect_import_enabled_providers(value, patch);
    collect_import_agents(value, patch);
}

fn collect_import_agents(value: &Value, patch: &mut DjinnConfigPatchPreview) {
    for container in ["agent", "agents"] {
        let Some(agents) = value.get(container).and_then(Value::as_object) else {
            continue;
        };
        for (name, agent) in agents {
            let profile_pointer = format!("/{}/{}", container, json_pointer_escape(name));
            let mut model_to_add_provider = None;
            {
                let profile = patch.profiles.entry(name.to_string()).or_default();
                push_unique_string(&mut profile.source_pointers, &profile_pointer);
                if let Some(model) = agent
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                {
                    profile.model = Some(model.to_string());
                    push_unique_string(
                        &mut profile.source_pointers,
                        &format!("{profile_pointer}/model"),
                    );
                    model_to_add_provider = Some(model.to_string());
                }
                collect_import_permissions_from_value(
                    agent,
                    &profile_pointer,
                    &mut profile.permissions,
                );
            }
            if let Some(model) = model_to_add_provider {
                add_provider_from_model(&model, patch);
            }
        }
    }
}

fn collect_import_providers(value: &Value, patch: &mut DjinnConfigPatchPreview) {
    let Some(providers) = value.get("providers").and_then(Value::as_object) else {
        return;
    };
    for (name, provider) in providers {
        let pointer = format!("/providers/{}", json_pointer_escape(name));
        let entry = patch.providers.entry(name.to_string()).or_default();
        if entry.provider_type.is_empty() {
            entry.provider_type = name.to_string();
        }
        push_unique_string(&mut entry.source_pointers, &pointer);
        if provider
            .get("apiKey")
            .or_else(|| provider.get("api_key"))
            .is_some()
        {
            entry.auth = Some(format!("opencode:{pointer}/apiKey"));
        }
    }
}

fn collect_import_enabled_providers(value: &Value, patch: &mut DjinnConfigPatchPreview) {
    let Some(providers) = value
        .get("enabled_providers")
        .or_else(|| value.get("enabledProviders"))
    else {
        return;
    };
    let values: Vec<String> = match providers {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Value::String(value) => value
            .split([',', ';', '\n'])
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    for provider in values {
        let entry = patch.providers.entry(provider.clone()).or_default();
        if entry.provider_type.is_empty() {
            entry.provider_type = provider;
        }
        push_unique_string(&mut entry.source_pointers, "/enabled_providers");
    }
}

fn add_provider_from_model(model: &str, patch: &mut DjinnConfigPatchPreview) {
    let Some((provider, _)) = model.split_once('/') else {
        return;
    };
    if provider.trim().is_empty() {
        return;
    }
    let entry = patch.providers.entry(provider.to_string()).or_default();
    if entry.provider_type.is_empty() {
        entry.provider_type = provider.to_string();
    }
    push_unique_string(&mut entry.source_pointers, "model-prefix");
}

fn collect_import_permissions_from_value(
    value: &Value,
    base_pointer: &str,
    out: &mut Vec<DjinnPermissionPatchPreview>,
) {
    if let Some(permission) = value.get("permission") {
        collect_import_v1_permissions(permission, &format_pointer(base_pointer, "permission"), out);
    }
    if let Some(permissions) = value.get("permissions") {
        collect_import_v2_permissions(
            permissions,
            &format_pointer(base_pointer, "permissions"),
            out,
        );
    }
}

fn collect_import_v1_permissions(
    permission: &Value,
    base_pointer: &str,
    out: &mut Vec<DjinnPermissionPatchPreview>,
) {
    let Some(permission) = permission.as_object() else {
        return;
    };
    for (action, value) in permission {
        let normalized_action = opencode_permission_action(action);
        let action_pointer = format!("{base_pointer}/{}", json_pointer_escape(action));
        if let Some(effect) = value.as_str().and_then(normalized_permission_effect_string) {
            out.push(DjinnPermissionPatchPreview {
                action: normalized_action,
                resource: "*".to_string(),
                effect,
                source_pointer: action_pointer,
            });
            continue;
        }
        let Some(patterns) = value.as_object() else {
            continue;
        };
        for (pattern, effect) in patterns {
            if let Some(effect) = effect
                .as_str()
                .and_then(normalized_permission_effect_string)
            {
                out.push(DjinnPermissionPatchPreview {
                    action: normalized_action.clone(),
                    resource: pattern.to_string(),
                    effect,
                    source_pointer: format!("{action_pointer}/{}", json_pointer_escape(pattern)),
                });
            }
        }
    }
}

fn collect_import_v2_permissions(
    permissions: &Value,
    base_pointer: &str,
    out: &mut Vec<DjinnPermissionPatchPreview>,
) {
    let Some(permissions) = permissions.as_array() else {
        return;
    };
    for (index, rule) in permissions.iter().enumerate() {
        let source_pointer = format!("{base_pointer}/{index}");
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .map(opencode_permission_action)
            .unwrap_or_else(|| "*".to_string());
        let Some(effect) = rule
            .get("effect")
            .and_then(Value::as_str)
            .and_then(normalized_permission_effect_string)
        else {
            continue;
        };
        let resource = rule
            .get("resource")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|resource| !resource.is_empty())
            .unwrap_or("*");
        out.push(DjinnPermissionPatchPreview {
            action,
            resource: resource.to_string(),
            effect,
            source_pointer,
        });
    }
}

fn normalized_permission_effect_string(effect: &str) -> Option<String> {
    match effect.trim() {
        "allow" => Some("allow".to_string()),
        "ask" => Some("ask".to_string()),
        "deny" => Some("deny".to_string()),
        _ => None,
    }
}

fn format_pointer(base: &str, child: &str) -> String {
    if base.is_empty() {
        format!("/{}", json_pointer_escape(child))
    } else {
        format!("{}/{}", base, json_pointer_escape(child))
    }
}

fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn opencode_config_doctor(path: Option<PathBuf>) -> Result<ConfigDoctorReport> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(|| opencode_model_config_paths(&cwd)),
    );
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut files = Vec::new();

    for path in paths {
        if !path.exists() {
            files.push(ConfigDoctorFileReport {
                path: path.display().to_string(),
                exists: false,
                readable: false,
                mapped: Vec::new(),
                unsupported: Vec::new(),
                unknown: Vec::new(),
                secrets: Vec::new(),
                errors: Vec::new(),
            });
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                files.push(ConfigDoctorFileReport {
                    path: path.display().to_string(),
                    exists: true,
                    readable: false,
                    mapped: Vec::new(),
                    unsupported: Vec::new(),
                    unknown: Vec::new(),
                    secrets: Vec::new(),
                    errors: vec![format!("read failed: {error}")],
                });
                continue;
            }
        };

        match serde_json::from_str::<Value>(&content) {
            Ok(value) => files.push(opencode_config_doctor_from_value(&path, &value)),
            Err(error) => files.push(ConfigDoctorFileReport {
                path: path.display().to_string(),
                exists: true,
                readable: true,
                mapped: Vec::new(),
                unsupported: Vec::new(),
                unknown: Vec::new(),
                secrets: Vec::new(),
                errors: vec![format!("parse failed: {error}")],
            }),
        }
    }

    Ok(ConfigDoctorReport {
        source: "opencode".to_string(),
        checked_paths,
        summary: config_doctor_summary(&files),
        files,
    })
}

fn config_doctor_summary(files: &[ConfigDoctorFileReport]) -> ConfigDoctorSummary {
    ConfigDoctorSummary {
        checked_path_count: files.len(),
        readable_file_count: files.iter().filter(|file| file.readable).count(),
        mapped_count: files.iter().map(|file| file.mapped.len()).sum(),
        unsupported_count: files.iter().map(|file| file.unsupported.len()).sum(),
        unknown_count: files.iter().map(|file| file.unknown.len()).sum(),
        secret_count: files.iter().map(|file| file.secrets.len()).sum(),
        error_count: files.iter().map(|file| file.errors.len()).sum(),
    }
}

fn opencode_config_doctor_from_value(path: &Path, value: &Value) -> ConfigDoctorFileReport {
    let mut file = ConfigDoctorFileReport {
        path: path.display().to_string(),
        exists: true,
        readable: true,
        mapped: Vec::new(),
        unsupported: Vec::new(),
        unknown: Vec::new(),
        secrets: Vec::new(),
        errors: Vec::new(),
    };

    collect_config_secrets(value, "", &mut file.secrets);

    let Some(object) = value.as_object() else {
        file.errors
            .push("OpenCode config root must be a JSON object".to_string());
        return file;
    };

    for (key, nested) in object {
        let pointer = format!("/{}", json_pointer_escape(key));
        match key.as_str() {
            "$schema" | "schema" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode schema metadata",
                "not imported",
                "Useful to OpenCode editors/validation, but not a Djinn runtime concept.",
            ),
            "model" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode default model",
                "Djinn default model fallback",
                "Used when no selected/default agent model is available.",
            ),
            "small_model" | "smallModel" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode small model",
                "Djinn model option only",
                "Discovered for model selection; secondary-model semantics are not canonical yet.",
            ),
            "default_agent" | "defaultAgent" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode default agent",
                "Djinn default profile/agent selector",
                "Used to select agent-scoped model and permissions.",
            ),
            "agent" | "agents" => {
                push_mapped(
                    &mut file,
                    &pointer,
                    "OpenCode agent map",
                    "Djinn profiles / future agents",
                    "Djinn reads agent model and permission fields where they map cleanly.",
                );
                collect_opencode_agent_findings(nested, &pointer, &mut file);
            }
            "providers" => {
                push_mapped(
                    &mut file,
                    &pointer,
                    "OpenCode providers",
                    "Djinn provider/auth discovery",
                    "Djinn currently reuses OpenAI API-key configuration and model/provider ids.",
                );
                collect_opencode_provider_findings(nested, &pointer, &mut file);
            }
            "provider" | "enabled_providers" | "enabledProviders" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode provider selection",
                "Djinn provider selection hint",
                "Recognized as provider-related config; canonical provider schema is still pending.",
            ),
            "permission" | "permissions" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode permission policy",
                "Djinn read/mutation/shell permission policy",
                "Mapped to allow/ask/deny policy where actions and resources are compatible.",
            ),
            "instructions" | "instruction" | "instructionFiles" | "instruction_files" => {
                push_unsupported(
                    &mut file,
                    &pointer,
                    "OpenCode instructions",
                    "future Djinn instruction/context sources",
                    "Recognized but not imported yet; needs precedence and workspace-scope rules.",
                )
            }
            "command" | "commands" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode custom commands",
                "future Djinn prompt templates / command palette entries",
                "Recognized but not imported yet; needs a Djinn-native command-template model.",
            ),
            "mcp" | "mcpServers" | "mcp_servers" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode MCP config",
                "future external tool bridge",
                "MCP is intentionally deferred until there is a concrete need.",
            ),
            "theme" | "themes" | "ui" | "layout" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode UI settings",
                "possible Djinn TUI preferences",
                "Low-priority unless the setting maps directly to Djinn UI behavior.",
            ),
            "experimental" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode experimental settings",
                "not imported",
                "Experimental harness-specific behavior is recognized but not mapped into Djinn config.",
            ),
            "plugin" | "plugins" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode plugin entries",
                "harness-specific extension points",
                "Djinn does not import or install OpenCode plugins; keep plugin config in OpenCode.",
            ),
            _ if is_secret_key(key) => push_secret(
                &mut file.secrets,
                &pointer,
                "Secret-like OpenCode field",
                "secret reference only",
                "Value intentionally redacted and not imported/exported raw.",
            ),
            _ => push_unknown(
                &mut file,
                &pointer,
                "Unknown OpenCode field",
                "no Djinn mapping yet",
                "Not recognized by the current OpenCode adapter.",
            ),
        }
    }

    dedupe_config_findings(&mut file.secrets);
    file
}

fn collect_opencode_agent_findings(
    value: &Value,
    base_pointer: &str,
    file: &mut ConfigDoctorFileReport,
) {
    let Some(agents) = value.as_object() else {
        return;
    };
    for (agent_name, agent) in agents {
        let agent_pointer = format!("{}/{}", base_pointer, json_pointer_escape(agent_name));
        push_mapped(
            file,
            &agent_pointer,
            "OpenCode agent profile",
            "Djinn profile / future agent",
            "Profile name can be selected by Djinn when resolving model and permissions.",
        );
        if agent.get("model").and_then(Value::as_str).is_some() {
            push_mapped(
                file,
                &format!("{agent_pointer}/model"),
                "OpenCode agent model",
                "Djinn profile model",
                "Used when the requested/default Djinn profile matches this agent.",
            );
        }
        if agent.get("permission").is_some() || agent.get("permissions").is_some() {
            push_mapped(
                file,
                &format!("{agent_pointer}/permissions"),
                "OpenCode agent permissions",
                "Djinn profile permission policy",
                "Mapped into read/mutation/shell policy where compatible.",
            );
        }
    }
}

fn collect_opencode_provider_findings(
    value: &Value,
    base_pointer: &str,
    file: &mut ConfigDoctorFileReport,
) {
    let Some(providers) = value.as_object() else {
        return;
    };
    for (provider_name, provider) in providers {
        let provider_pointer = format!("{}/{}", base_pointer, json_pointer_escape(provider_name));
        push_mapped(
            file,
            &provider_pointer,
            "OpenCode provider entry",
            "Djinn provider discovery",
            "Provider entry is recognized; only compatible auth/model fields are used today.",
        );
        if provider
            .get("apiKey")
            .or_else(|| provider.get("api_key"))
            .is_some()
        {
            push_secret(
                &mut file.secrets,
                &format!("{provider_pointer}/apiKey"),
                "Provider API key",
                "secret reference only",
                "Value intentionally redacted; Djinn may read it locally but should not export it raw.",
            );
        }
    }
}

fn collect_config_secrets(value: &Value, pointer: &str, out: &mut Vec<ConfigDoctorFinding>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let child = format!("{}/{}", pointer, json_pointer_escape(key));
                if is_secret_key(key) {
                    push_secret(
                        out,
                        &child,
                        "Secret-like config field",
                        "secret reference only",
                        "Value intentionally redacted and excluded from import/export previews.",
                    );
                }
                collect_config_secrets(value, &child, out);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_config_secrets(value, &format!("{pointer}/{index}"), out);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("apikey")
        || key.contains("api_key")
        || key.contains("token")
        || key.contains("secret")
        || key == "access"
        || key == "refresh"
        || key == "password"
}

fn push_mapped(
    file: &mut ConfigDoctorFileReport,
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) {
    file.mapped
        .push(config_finding(pointer, concept, djinn_mapping, detail));
}

fn push_unsupported(
    file: &mut ConfigDoctorFileReport,
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) {
    file.unsupported
        .push(config_finding(pointer, concept, djinn_mapping, detail));
}

fn push_unknown(
    file: &mut ConfigDoctorFileReport,
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) {
    file.unknown
        .push(config_finding(pointer, concept, djinn_mapping, detail));
}

fn push_secret(
    findings: &mut Vec<ConfigDoctorFinding>,
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) {
    findings.push(config_finding(pointer, concept, djinn_mapping, detail));
}

fn config_finding(
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) -> ConfigDoctorFinding {
    ConfigDoctorFinding {
        pointer: if pointer.is_empty() {
            "/".to_string()
        } else {
            pointer.to_string()
        },
        concept: concept.to_string(),
        djinn_mapping: djinn_mapping.to_string(),
        detail: detail.to_string(),
    }
}

fn dedupe_config_findings(findings: &mut Vec<ConfigDoctorFinding>) {
    let mut seen = HashSet::new();
    findings.retain(|finding| seen.insert(finding.pointer.clone()));
}

fn json_pointer_escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn format_config_doctor_report(
    report: &ConfigDoctorReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config doctor".to_string(),
        format!("Source: {}", report.source),
        format!(
            "Summary: {} readable file(s), {} mapped, {} unsupported, {} unknown, {} secret reference(s), {} error(s)",
            report.summary.readable_file_count,
            report.summary.mapped_count,
            report.summary.unsupported_count,
            report.summary.unknown_count,
            report.summary.secret_count,
            report.summary.error_count,
        ),
        String::new(),
        "Checked paths:".to_string(),
    ];
    for path in &report.checked_paths {
        lines.push(format!("  - {path}"));
    }

    if report.files.iter().all(|file| !file.readable) {
        lines.push(String::new());
        lines.push("No readable config files found.".to_string());
    }

    for file in &report.files {
        lines.push(String::new());
        lines.push(format!("File: {}", file.path));
        lines.push(format!("  exists: {}", file.exists));
        lines.push(format!("  readable: {}", file.readable));
        push_config_finding_lines(&mut lines, "mapped", &file.mapped);
        push_config_finding_lines(&mut lines, "unsupported", &file.unsupported);
        push_config_finding_lines(&mut lines, "unknown", &file.unknown);
        push_config_finding_lines(&mut lines, "secrets", &file.secrets);
        if !file.errors.is_empty() {
            lines.push("  errors:".to_string());
            for error in &file.errors {
                lines.push(format!("    - {error}"));
            }
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn format_djinn_config_load_report(
    report: &DjinnConfigLoadReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec!["Djinn native config".to_string(), String::new()];
    lines.push("Checked paths:".to_string());
    for path in &report.checked_paths {
        lines.push(format!("  - {path}"));
    }
    lines.push(String::new());
    lines.push("Files:".to_string());
    for file in &report.files {
        lines.push(format!(
            "  - {} · exists: {} · readable: {}",
            file.path, file.exists, file.readable
        ));
        for error in &file.errors {
            lines.push(format!("    error: {error}"));
        }
    }
    if !report.warnings.is_empty() {
        lines.push(String::new());
        lines.push("Warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }

    lines.push(String::new());
    lines.push("Effective config:".to_string());
    lines.push(format!("  version: {}", report.effective.version));
    if let Some(profile) = &report.effective.default_profile {
        lines.push(format!("  default_profile: {profile}"));
    }
    lines.push(format!("  providers: {}", report.effective.providers.len()));
    for (name, provider) in &report.effective.providers {
        lines.push(format!("    - {name} ({})", provider.provider_type));
        if let Some(auth) = &provider.auth {
            lines.push(format!("      auth: {auth}"));
        }
        if let Some(endpoint) = &provider.endpoint {
            lines.push(format!("      endpoint: {endpoint}"));
        }
    }
    lines.push(format!("  profiles: {}", report.effective.profiles.len()));
    for (name, profile) in &report.effective.profiles {
        lines.push(format!("    - {name}"));
        if let Some(model) = &profile.model {
            lines.push(format!("      model: {model}"));
        }
        if !profile.instructions.is_empty() {
            lines.push(format!(
                "      instructions: {}",
                profile.instructions.join(", ")
            ));
        }
        if !profile.permissions.is_empty() {
            lines.push("      permissions:".to_string());
            for permission in &profile.permissions {
                lines.push(format!(
                    "        - {} {} -> {}",
                    permission.action, permission.resource, permission.effect
                ));
            }
        }
    }
    if !report.effective.permissions.is_empty() {
        lines.push("  shared permissions:".to_string());
        for permission in &report.effective.permissions {
            lines.push(format!(
                "    - {} {} -> {}",
                permission.action, permission.resource, permission.effect
            ));
        }
    }
    lines.push(format!(
        "  instructions: {}",
        report.effective.instructions.len()
    ));
    lines.push(format!("  commands: {}", report.effective.commands.len()));
    lines.push(format!("  tools: {}", report.effective.tools.len()));
    lines.push(format!("  agents: {}", report.effective.agents.len()));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn format_config_import_preview(
    preview: &ConfigImportPreview,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(preview)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config import preview".to_string(),
        format!("Source: {}", preview.source),
        format!("Mode: {}", preview.mode),
        String::new(),
        "Checked paths:".to_string(),
    ];
    for path in &preview.checked_paths {
        lines.push(format!("  - {path}"));
    }

    lines.push(String::new());
    lines.push("Readable files:".to_string());
    if preview.readable_files.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for path in &preview.readable_files {
            lines.push(format!("  - {path}"));
        }
    }

    lines.push(String::new());
    lines.push("Djinn config patch:".to_string());
    lines.push(format!("  version: {}", preview.patch.version));
    if let Some(profile) = &preview.patch.default_profile {
        lines.push(format!("  default_profile: {profile}"));
    }

    lines.push("  providers:".to_string());
    if preview.patch.providers.is_empty() {
        lines.push("    - none".to_string());
    } else {
        for (name, provider) in &preview.patch.providers {
            lines.push(format!("    - {name} ({})", provider.provider_type));
            if let Some(auth) = &provider.auth {
                lines.push(format!("      auth: {auth}"));
            }
        }
    }

    lines.push("  profiles:".to_string());
    if preview.patch.profiles.is_empty() {
        lines.push("    - none".to_string());
    } else {
        for (name, profile) in &preview.patch.profiles {
            lines.push(format!("    - {name}"));
            if let Some(model) = &profile.model {
                lines.push(format!("      model: {model}"));
            }
            if !profile.permissions.is_empty() {
                lines.push("      permissions:".to_string());
                for permission in &profile.permissions {
                    lines.push(format!(
                        "        - {} {} -> {} ({})",
                        permission.action,
                        permission.resource,
                        permission.effect,
                        permission.source_pointer
                    ));
                }
            }
        }
    }

    if !preview.patch.permissions.is_empty() {
        lines.push("  global permissions:".to_string());
        for permission in &preview.patch.permissions {
            lines.push(format!(
                "    - {} {} -> {} ({})",
                permission.action,
                permission.resource,
                permission.effect,
                permission.source_pointer
            ));
        }
    }

    push_config_finding_lines(&mut lines, "unsupported", &preview.unsupported);
    push_config_finding_lines(&mut lines, "unknown", &preview.unknown);
    push_config_finding_lines(&mut lines, "secrets", &preview.secrets);
    if !preview.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &preview.warnings {
            lines.push(format!("  - {warning}"));
        }
    }

    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn format_config_import_write_report(
    report: &ConfigImportWriteReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config import write".to_string(),
        format!("Source: {}", report.source),
        format!("Wrote: {}", report.path),
        format!("Overwritten: {}", report.overwritten),
        format!("Merged: {}", report.merged),
        String::new(),
        "Import summary:".to_string(),
    ];
    push_import_write_summary_lines(&mut lines, &report.summary);
    lines.extend([
        String::new(),
        "Written config:".to_string(),
        format!("  version: {}", report.config.version),
    ]);
    if let Some(profile) = &report.config.default_profile {
        lines.push(format!("  default_profile: {profile}"));
    }
    lines.push(format!("  providers: {}", report.config.providers.len()));
    for (name, provider) in &report.config.providers {
        lines.push(format!("    - {name} ({})", provider.provider_type));
        if let Some(auth) = &provider.auth {
            lines.push(format!("      auth: {auth}"));
        }
    }
    lines.push(format!("  profiles: {}", report.config.profiles.len()));
    for (name, profile) in &report.config.profiles {
        lines.push(format!("    - {name}"));
        if let Some(model) = &profile.model {
            lines.push(format!("      model: {model}"));
        }
    }
    push_config_finding_lines(&mut lines, "unsupported", &report.unsupported);
    push_config_finding_lines(&mut lines, "unknown", &report.unknown);
    push_config_finding_lines(&mut lines, "secrets", &report.secrets);
    if !report.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn push_import_write_summary_lines(lines: &mut Vec<String>, summary: &ConfigImportWriteSummary) {
    if let Some(profile) = &summary.applied_default_profile {
        lines.push(format!("  default_profile: applied {profile}"));
    }
    if let Some(profile) = &summary.preserved_default_profile {
        if let Some(imported) = &summary.skipped_import_default_profile {
            lines.push(format!(
                "  default_profile: preserved {profile} (skipped imported {imported})"
            ));
        } else {
            lines.push(format!("  default_profile: preserved {profile}"));
        }
    }
    lines.push(format!(
        "  providers: added {}{}; skipped {}{}",
        summary.added_providers.len(),
        format_named_summary(&summary.added_providers),
        summary.skipped_providers.len(),
        format_named_summary(&summary.skipped_providers),
    ));
    lines.push(format!(
        "  profiles: added {}{}; skipped {}{}",
        summary.added_profiles.len(),
        format_named_summary(&summary.added_profiles),
        summary.skipped_profiles.len(),
        format_named_summary(&summary.skipped_profiles),
    ));
    lines.push(format!(
        "  shared permissions: added {}; skipped {}",
        summary.added_shared_permissions, summary.skipped_shared_permissions
    ));
}

fn format_named_summary(names: &[String]) -> String {
    if names.is_empty() {
        String::new()
    } else {
        format!(" ({})", names.join(", "))
    }
}

fn format_config_export_preview(
    preview: &ConfigExportPreview,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(preview)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config export preview".to_string(),
        format!("Target: {}", preview.target),
        format!("Mode: {}", preview.mode),
        String::new(),
        "Checked paths:".to_string(),
    ];
    for path in &preview.checked_paths {
        lines.push(format!("  - {path}"));
    }
    lines.push(String::new());
    lines.push("Readable files:".to_string());
    if preview.readable_files.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for path in &preview.readable_files {
            lines.push(format!("  - {path}"));
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "{} config preview:",
        config_target_display_name(&preview.target)
    ));
    let rendered_config = serde_json::to_string_pretty(&preview.config)?;
    for line in rendered_config.lines() {
        lines.push(format!("  {line}"));
    }

    push_config_finding_lines(&mut lines, "unsupported", &preview.unsupported);
    push_config_finding_lines(&mut lines, "secrets", &preview.secrets);
    if !preview.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &preview.warnings {
            lines.push(format!("  - {warning}"));
        }
    }

    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn format_config_export_write_report(
    report: &ConfigExportWriteReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config export write".to_string(),
        format!("Target: {}", report.target),
        format!("Wrote: {}", report.path),
        format!("Overwritten: {}", report.overwritten),
        String::new(),
        format!(
            "Written {} config:",
            config_target_display_name(&report.target)
        ),
    ];
    let rendered_config = serde_json::to_string_pretty(&report.config)?;
    for line in rendered_config.lines() {
        lines.push(format!("  {line}"));
    }
    push_config_finding_lines(&mut lines, "unsupported", &report.unsupported);
    push_config_finding_lines(&mut lines, "secrets", &report.secrets);
    if !report.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn config_target_display_name(target: &str) -> &str {
    match target {
        "copilot" => "Copilot",
        "opencode" => "OpenCode",
        _ => "target",
    }
}

fn push_config_finding_lines(
    lines: &mut Vec<String>,
    label: &str,
    findings: &[ConfigDoctorFinding],
) {
    if findings.is_empty() {
        return;
    }
    lines.push(format!("  {label}:"));
    for finding in findings {
        lines.push(format!(
            "    - {} · {} -> {}",
            finding.pointer, finding.concept, finding.djinn_mapping
        ));
        lines.push(format!("      {}", finding.detail));
    }
}

fn run_agent(args: AgentArgs) -> Result<()> {
    match args.command {
        AgentCommand::Config(args) => {
            warn_legacy_agent_command(
                "agent config",
                Some("prefer `djinn agents ...`, `djinn ask`, and `djinn session ...`"),
            );
            run_agent_config(args)
        }
        AgentCommand::Tools(args) => {
            warn_legacy_agent_command(
                "agent tools",
                Some("prefer top-level tool inspection commands"),
            );
            run_agent_tools(args)
        }
        AgentCommand::Policy(args) => {
            warn_legacy_agent_command("agent policy", Some("policy remains legacy-only for now"));
            run_agent_policy(args)
        }
        AgentCommand::FileHistory(args) => {
            warn_legacy_agent_command(
                "agent file-history",
                Some("file history remains legacy-only for now"),
            );
            run_agent_file_history(args)
        }
        AgentCommand::Ask(args) => legacy_agent_ask(args),
    }
}

fn warn_legacy_agent_command(command: &str, replacement: Option<&str>) {
    let replacement = replacement
        .map(|replacement| format!("; {replacement}"))
        .unwrap_or_default();
    eprintln!("warning: `djinn {command}` is deprecated compatibility surface{replacement}");
}

fn run_session(args: SessionArgs) -> Result<()> {
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
        None => run_tui_command(TuiArgs {
            view: TuiView::Sessions,
            roots: Vec::new(),
            editor: args.editor,
        }),
    }
}

fn run_folder_session_tui(dir: PathBuf, editor: Option<String>) -> Result<()> {
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

fn session_context(args: SessionContextArgs) -> Result<()> {
    match args.command {
        SessionContextCommand::Discover(args) => session_context_discover(args),
        SessionContextCommand::Ls(args) => session_context_ls(args),
        SessionContextCommand::Add(args) => session_context_add(args),
        SessionContextCommand::Rm(args) => session_context_rm(args),
    }
}

fn session_compact(args: SessionCompactArgs) -> Result<()> {
    let report = compact_folder_session(&args.session_dir, args.output.as_deref())?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Compacted {} turns", report.turn_count);
        println!("Output: {}", report.output_path);
    }
    Ok(())
}

fn session_decide(args: SessionDecisionArgs, action: SessionDecisionAction) -> Result<()> {
    let report = decide_promotion_session(&args, action)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let verb = if args.dry_run {
            "Would record"
        } else {
            "Recorded"
        };
        println!(
            "{verb} {} decision for promotion session: {}",
            session_decision_action_label(action),
            report.session_dir
        );
        println!("  type: {}", report.promotion_type);
        if let Some(candidate) = &report.candidate {
            println!("  candidate: {candidate}");
        } else {
            println!("  candidate: all");
        }
        println!("  decision: {}", report.decision_path);
        if report.writebacks.is_empty() {
            println!("  durable writeback: none");
        } else if report.dry_run {
            println!("  durable writeback: dry-run preview");
        } else {
            println!("  durable writeback: yes");
        }
        for writeback in &report.writebacks {
            if let Some(path) = &writeback.path {
                println!(
                    "    - {} {} -> {} ({path})",
                    writeback.candidate_type, writeback.candidate, writeback.destination
                );
            } else {
                println!(
                    "    - {} {} -> {} [{}]",
                    writeback.candidate_type,
                    writeback.candidate,
                    writeback.destination,
                    writeback.id
                );
            }
            if let Some(preview) = &writeback.preview {
                println!("      preview: {}", preview.replace('\n', "\\n"));
            }
        }
        for post in &report.post_writebacks {
            let label = if post.status == "pending" {
                "follow-up"
            } else {
                "post-writeback"
            };
            println!("  {label}: {} -> {}", post.name, post.status);
            println!("    command: {}", post.command);
        }
        println!("  note: {}", report.note);
    }
    Ok(())
}

fn session_validate_events(args: SessionValidateEventsArgs) -> Result<()> {
    let report = validate_folder_session_events(&args.dir)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_validate_events_report(&report));
    }
    Ok(())
}

fn session_transcript(args: SessionTranscriptArgs) -> Result<()> {
    session_transcript::run_session_transcript(SessionTranscriptOptions {
        dir: args.dir,
        format: args.format,
        json: args.json,
        output: args.output,
        open: args.open,
        editor: args.editor,
    })
}

fn session_events(args: SessionEventsArgs) -> Result<()> {
    if args.all {
        let report =
            event_health_report_for_cache_sessions(args.limit, args.health_filter.as_deref())?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", format_event_health_report(&report));
        }
        if args.strict {
            ensure_event_health_strict(&report)?;
        }
        return Ok(());
    }

    let dir = args.dir.as_ref().ok_or_else(|| {
        anyhow!("session name, path, or Buddy id is required unless --all is used")
    })?;
    if let Some(backup) = &args.restore {
        let report = restore_folder_session_event_backup(dir, backup, args.write)?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", format_session_restore_events_report(&report));
        }
        return Ok(());
    }

    let report = if args.write {
        rebuild_folder_session_from_events(dir)?
    } else {
        project_folder_session_events(dir)?
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_project_events_report(&report));
    }
    Ok(())
}

fn session_promote(args: SessionPromoteArgs) -> Result<()> {
    let report = create_promotion_session(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Created Djinn promotion session: {}",
            report.promotion_session_dir
        );
        println!(
            "  type: {}",
            session_promote_type_label(report.promotion_type)
        );
        println!("  sources: {}", report.session_count);
        println!("  source packet: {}", report.source_packet_path);
        println!("  source refs: {}", report.sources_path);
        println!("  request: {}", report.request_path);
        println!("  summary: {}", report.summary_path);
        println!("  run: djinn session run {}", report.promotion_session_dir);
    }
    Ok(())
}

pub(crate) fn session_promote_type_label(promotion_type: SessionPromoteType) -> &'static str {
    match promotion_type {
        SessionPromoteType::Memory => "memory",
        SessionPromoteType::Todo => "todo",
        SessionPromoteType::Skill => "skill",
        SessionPromoteType::Pattern => "pattern",
    }
}

pub(crate) fn session_promote_type_instructions(
    promotion_type: SessionPromoteType,
) -> &'static str {
    match promotion_type {
        SessionPromoteType::Memory => {
            "Identify durable, reusable memories: nuggets of wisdom worth returning to. Return reviewed `djinn add memory ... --evidence ...` commands or say `No durable memories recommended.`"
        }
        SessionPromoteType::Todo => {
            "Identify concrete follow-up todos the user can take action on soon. Return reviewed todo candidates with evidence links or say `No actionable todos recommended.`"
        }
        SessionPromoteType::Skill => {
            "Identify reusable workflow knowledge that should become or update a skill. Return a short skill proposal with evidence links."
        }
        SessionPromoteType::Pattern => {
            "Synthesize common threads, themes, suggestions, conventions, gotchas, and workflow decisions across the source sessions. Separate high-confidence patterns from one-off observations."
        }
    }
}

fn session_status(args: SessionStatusArgs) -> Result<()> {
    let session_ref = resolve_existing_folder_session_reference(&args.dir)?;
    let report = folder_session_status(&session_ref.session_dir)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_folder_session_status(&report));
    }
    Ok(())
}

fn session_context_discover(args: SessionContextDiscoverArgs) -> Result<()> {
    let report = discover_folder_session_context(&args.session, args.dry_run)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_folder_session_context_discover(&report));
    }
    Ok(())
}

fn session_context_ls(args: SessionContextLsArgs) -> Result<()> {
    let report = list_folder_session_context(&args.session)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_folder_session_context_ls(&report));
    }
    Ok(())
}

fn session_context_add(args: SessionContextAddArgs) -> Result<()> {
    let report = add_folder_session_context_entry(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Linked context: {} -> {}", report.path, report.target);
        if report.replaced {
            println!("Replaced existing context entry: {}", report.name);
        }
    }
    Ok(())
}

fn session_context_rm(args: SessionContextRmArgs) -> Result<()> {
    let report = remove_folder_session_context_entry(&args.session, &args.name)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Removed context entry: {}", report.path);
    }
    Ok(())
}

fn session_ls(args: SessionLsArgs) -> Result<()> {
    let report = list_cache_folder_sessions(args.limit)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_folder_session_ls(&report));
    }
    Ok(())
}

fn session_shorten_names(args: SessionShortenNamesArgs) -> Result<()> {
    let report = shorten_cache_folder_session_names(args.dry_run)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_shorten_names_report(&report));
    }
    Ok(())
}

fn session_rename(args: SessionRenameArgs) -> Result<()> {
    let root = default_folder_session_root();
    let report = rename_folder_session_in_root(&args.dir, &args.new_name, &root, args.dry_run)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_rename_report(&report));
    }
    Ok(())
}

fn session_open(args: SessionOpenArgs) -> Result<()> {
    let target = resolve_folder_session_open_target(&args.dir, args.target)?;
    open_editor_path(&target, args.editor)
}

fn session_chat(args: SessionChatArgs) -> Result<()> {
    if !args.capture_request && args.dry_run {
        bail!("--dry-run is only supported with --capture-request");
    }
    if !args.capture_request && args.json {
        bail!("--json is only supported with --capture-request");
    }

    if args.capture_request {
        let session_ref = resolve_existing_folder_session_reference(&args.dir)?;
        let report = run_session_buddy(&SessionBuddyRunArgs {
            dir: session_ref.session_dir,
            buddy_bin: args.buddy_bin.clone(),
            buddy_session: session_ref.buddy_session,
            buddy_args: args.buddy_args.clone(),
            dry_run: args.dry_run,
        })?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", format_session_buddy_report(&report));
        }
        return Ok(());
    }

    let (session_dir, resolved_buddy_session) = resolve_top_level_buddy_session_arg(args.dir)?;
    run_top_level_folder_buddy_session_with_options(
        &session_dir,
        resolved_buddy_session,
        args.buddy_bin,
        &args.buddy_args,
    )
}

fn run_top_level_buddy_mode(session: Option<PathBuf>) -> Result<()> {
    if let Some(session) = session {
        let (session_dir, buddy_session) = resolve_top_level_buddy_session_arg(session)?;
        return run_top_level_folder_buddy_session(&session_dir, buddy_session);
    }
    run_plain_buddy_mode()
}

fn resolve_top_level_buddy_session_arg(session: PathBuf) -> Result<(PathBuf, Option<String>)> {
    let root = default_folder_session_root();
    let session_dir = resolve_session_dir_in_root(&session, &root)?;
    if session_dir.exists() {
        return Ok((session_dir, None));
    }

    Ok(resolve_existing_folder_session_reference_in_root(&session, &root)?.map_buddy_for_launch())
}

fn buddy_command_doctor_report(session: Option<&Path>) -> Result<BuddyCommandDoctorReport> {
    let session_dir = session.map(resolve_session_dir).transpose()?;
    let runtime_path = session_dir
        .as_ref()
        .map(|session_dir| session_dir.join("runtime/buddy.json"));
    let runtime = runtime_path
        .as_ref()
        .map(|path| read_buddy_runtime_state(path))
        .transpose()?
        .flatten();
    let mut report = buddy_command_doctor_report_from(
        env::var(DJINN_BUDDY_BIN_ENV).ok(),
        runtime.as_ref().and_then(|state| state.command.clone()),
        Some(&djinn_source_workspace_root()),
        session_dir.as_deref(),
        runtime_path.as_deref(),
    );
    report.bridge = Some(probe_buddy_bridge_doctor(
        &report.command,
        report.exists && report.executable,
    ));
    Ok(report)
}

fn session_consolidate(args: SessionConsolidateArgs) -> Result<()> {
    let root = default_folder_session_root();
    let report = consolidate_sessions_in_root(&root, &args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_consolidate_report(&report));
    }
    Ok(())
}

fn run_agents(args: AgentsArgs) -> Result<()> {
    match args.command {
        AgentsCommand::List(args) => agents_list(args),
        AgentsCommand::Show(args) => agents_show(args),
    }
}

fn agents_list(args: AgentsListArgs) -> Result<()> {
    let config = effective_djinn_config()?;
    let roles = configured_agent_roles(&config);
    print!(
        "{}",
        format_agent_role_list(&roles, output_format(args.format, args.json))?
    );
    Ok(())
}

fn agents_show(args: AgentsShowArgs) -> Result<()> {
    let config = effective_djinn_config()?;
    let roles = configured_agent_roles(&config);
    let role = resolve_agent_role(&roles, &args.name)?;
    print!(
        "{}",
        format_agent_role(role, output_format(args.format, args.json))?
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AgentRoleView {
    name: String,
    description: Option<String>,
    profile: Option<String>,
    model: Option<String>,
    effective_model: Option<String>,
    instructions: Vec<String>,
    tools: Vec<String>,
}

fn configured_agent_roles(config: &DjinnConfig) -> Vec<AgentRoleView> {
    config
        .agents
        .iter()
        .map(|(name, agent)| {
            let profile = agent
                .profile
                .as_deref()
                .map(str::trim)
                .filter(|profile| !profile.is_empty())
                .map(ToOwned::to_owned);
            let model = agent
                .model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(ToOwned::to_owned);
            let effective_model = model.clone().or_else(|| {
                profile
                    .as_deref()
                    .and_then(|profile| profile_model_from_config(config, profile))
            });
            AgentRoleView {
                name: name.clone(),
                description: agent
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|description| !description.is_empty())
                    .map(ToOwned::to_owned),
                profile,
                model,
                effective_model,
                instructions: agent.instructions.clone(),
                tools: agent.tools.clone(),
            }
        })
        .collect()
}

fn resolve_agent_role<'a>(roles: &'a [AgentRoleView], name: &str) -> Result<&'a AgentRoleView> {
    let requested = name.trim();
    if let Some(role) = roles.iter().find(|role| role.name == requested) {
        return Ok(role);
    }
    if let Some(role) = roles
        .iter()
        .find(|role| role.name.eq_ignore_ascii_case(requested))
    {
        return Ok(role);
    }
    let needle = requested.to_lowercase();
    let matches = roles
        .iter()
        .filter(|role| {
            role.name.to_lowercase().contains(&needle)
                || role
                    .description
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&needle)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [role] => Ok(role),
        [] => bail!("no agent role named {requested:?} found"),
        many => {
            eprintln!("multiple agent roles match {requested:?}:");
            for role in many {
                eprintln!("  - {}", role.name);
            }
            bail!("agent role name is ambiguous")
        }
    }
}

fn format_agent_role_list(roles: &[AgentRoleView], format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(roles)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    if roles.is_empty() {
        return Ok("No configured Djinn agent roles.\n".to_string());
    }
    let mut lines = vec!["Djinn agent roles".to_string(), String::new()];
    for role in roles {
        lines.push(format!("  - {}", role.name));
        if let Some(description) = &role.description {
            lines.push(format!("    {description}"));
        }
        if let Some(profile) = &role.profile {
            lines.push(format!("    profile: {profile}"));
        }
        if let Some(model) = &role.effective_model {
            lines.push(format!("    model: {model}"));
        }
        if !role.tools.is_empty() {
            lines.push(format!("    tools: {}", role.tools.join(", ")));
        }
    }
    lines.push(String::new());
    lines.push(format!("Total: {} agent roles", roles.len()));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn format_agent_role(role: &AgentRoleView, format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(role)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let mut lines = vec![
        "Djinn agent role".to_string(),
        format!("Name: {}", role.name),
    ];
    if let Some(description) = &role.description {
        lines.push(format!("Description: {description}"));
    }
    if let Some(profile) = &role.profile {
        lines.push(format!("Profile: {profile}"));
    }
    if let Some(model) = &role.model {
        lines.push(format!("Model override: {model}"));
    }
    if let Some(model) = &role.effective_model {
        lines.push(format!("Effective model: {model}"));
    }
    lines.push("Instructions:".to_string());
    if role.instructions.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for instruction in &role.instructions {
            lines.push(format!("  - {instruction}"));
        }
    }
    lines.push("Tools:".to_string());
    if role.tools.is_empty() {
        lines.push("  - inherited/default".to_string());
    } else {
        for tool in &role.tools {
            lines.push(format!("  - {tool}"));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentRoleSelection {
    agent_name: Option<String>,
    profile: String,
    model: Option<String>,
    instructions: Vec<String>,
    tools: Vec<String>,
}

fn resolve_agent_role_selection(
    agent: Option<String>,
    requested_profile: &str,
    requested_model: Option<String>,
) -> Result<AgentRoleSelection> {
    let config = effective_djinn_config()?;
    resolve_agent_role_selection_from_config(&config, agent, requested_profile, requested_model)
}

fn resolve_agent_role_selection_from_config(
    config: &DjinnConfig,
    agent: Option<String>,
    requested_profile: &str,
    requested_model: Option<String>,
) -> Result<AgentRoleSelection> {
    let Some(agent_name) = agent
        .as_deref()
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    else {
        let profile = resolve_agent_profile_from_config(config, requested_profile);
        return Ok(AgentRoleSelection {
            agent_name: None,
            instructions: profile_instructions_from_config(config, &profile),
            profile,
            model: requested_model,
            tools: Vec::new(),
        });
    };

    let roles = configured_agent_roles(&config);
    let role = resolve_agent_role(&roles, agent_name)?;
    let profile = role
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| requested_profile.trim().to_string());
    let profile = resolve_agent_profile_from_config(config, &profile);
    let mut instructions = profile_instructions_from_config(config, &profile);
    for instruction in &role.instructions {
        push_unique_string(&mut instructions, instruction);
    }
    Ok(AgentRoleSelection {
        agent_name: Some(role.name.clone()),
        profile,
        model: requested_model.or_else(|| role.model.clone()),
        instructions,
        tools: role.tools.clone(),
    })
}

fn profile_instructions_from_config(config: &DjinnConfig, profile: &str) -> Vec<String> {
    config
        .profiles
        .get(profile)
        .map(|profile| profile.instructions.clone())
        .unwrap_or_default()
}

fn parent_session_id_from_arg(parent_session: Option<String>) -> Option<AgentSessionId> {
    parent_session
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .map(AgentSessionId::new)
}

fn run_agent_config(args: AgentConfigArgs) -> Result<()> {
    match args.command {
        AgentConfigCommand::List(args) => agent_config_list(args),
        AgentConfigCommand::Show(args) => agent_config_show(args),
    }
}

fn run_agent_tools(args: AgentToolsArgs) -> Result<()> {
    match args.command {
        AgentToolsCommand::List(args) => agent_tools_list(args),
        AgentToolsCommand::Show(args) => agent_tools_show(args),
    }
}

fn run_agent_policy(args: AgentPolicyArgs) -> Result<()> {
    match args.command {
        AgentPolicyCommand::List(args) => agent_policy_list(args),
        AgentPolicyCommand::Audit(args) => agent_policy_audit(args),
        AgentPolicyCommand::Revoke(args) => agent_policy_revoke(args),
    }
}

fn append_agent_session_lifecycle_event(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    state: AgentSessionLifecycleState,
    mode: AgentSessionExecutionMode,
    reason: impl Into<String>,
    note: Option<String>,
) -> Result<()> {
    store.append_event(
        id,
        AgentSessionEvent::new(AgentSessionEventKind::SessionLifecycleUpdated {
            state,
            mode: Some(mode),
            reason: Some(reason.into()),
            note,
        }),
    )
}

fn run_agent_file_history(args: AgentFileHistoryArgs) -> Result<()> {
    match args.command {
        AgentFileHistoryCommand::List(args) => agent_file_history_list(args),
        AgentFileHistoryCommand::Restore(args) => agent_file_history_restore(args),
    }
}

fn agent_config_list(args: AgentConfigListArgs) -> Result<()> {
    let current_profile = resolve_agent_profile(&args.profile)?;
    let current_model = resolve_agent_model(args.model, &current_profile)?;
    let profiles = agent_profile_options(&current_profile)?;
    let models = agent_model_options(&current_model)?;
    print!(
        "{}",
        format_agent_config_options(
            &current_profile,
            &current_model,
            &profiles,
            &models,
            output_format(args.format, args.json),
        )?
    );
    Ok(())
}

fn agent_config_show(args: AgentConfigShowArgs) -> Result<()> {
    let config =
        resolve_agent_effective_config(args.workspace, args.profile, args.agent, args.model)?;
    print!(
        "{}",
        format_agent_effective_config(&config, output_format(args.format, args.json))?
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AgentEffectiveConfig {
    workspace: String,
    agent_name: Option<String>,
    profile: String,
    model: String,
    agent_instructions: Vec<String>,
    agent_tools: Vec<String>,
    read_access: ReadAccessPolicy,
    permissions: PermissionPolicy,
    read_access_rules: Vec<AgentEffectivePolicyRule>,
    permission_rules: Vec<AgentEffectivePolicyRule>,
    guardrails: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AgentEffectivePolicyRule {
    source: String,
    action: String,
    resource: String,
    effect: String,
}

fn resolve_agent_effective_config(
    workspace: Option<PathBuf>,
    profile: String,
    agent: Option<String>,
    model: Option<String>,
) -> Result<AgentEffectiveConfig> {
    let selection = resolve_agent_role_selection(agent, &profile, model)?;
    let profile = selection.profile;
    let workspace = resolve_agent_workspace(workspace)?;
    let model = resolve_agent_model(selection.model, &profile)?;
    agent_effective_config_from_parts(
        workspace,
        profile,
        model,
        selection.agent_name,
        selection.instructions,
        selection.tools,
    )
}

fn agent_effective_config_from_parts(
    workspace: String,
    profile: String,
    model: String,
    agent_name: Option<String>,
    agent_instructions: Vec<String>,
    agent_tools: Vec<String>,
) -> Result<AgentEffectiveConfig> {
    let workspace_path = Path::new(&workspace);
    Ok(AgentEffectiveConfig {
        model,
        read_access: resolve_agent_read_access_policy(&profile, workspace_path)?,
        permissions: resolve_agent_permission_policy(&profile, workspace_path)?,
        read_access_rules: effective_read_access_rules_with_sources(&profile, workspace_path)?,
        permission_rules: effective_permission_rules_with_sources(&profile, workspace_path)?,
        guardrails: agent_policy_guardrails(),
        agent_name,
        agent_instructions,
        agent_tools,
        workspace,
        profile,
    })
}

fn agent_session_runtime_config(config: &AgentEffectiveConfig) -> AgentSessionRuntimeConfig {
    AgentSessionRuntimeConfig {
        model: config.model.clone(),
        agent_instructions: config.agent_instructions.clone(),
        agent_tools: config.agent_tools.clone(),
        read_access: AgentSessionPolicySnapshot {
            default_effect: if config.read_access.allow_roots.is_empty() {
                "allow".to_string()
            } else {
                "allow configured roots".to_string()
            },
            rules: config
                .read_access_rules
                .iter()
                .map(agent_session_policy_rule_from_effective)
                .collect(),
            guardrails: vec![
                "secret-read guardrails block known credential/token/key/auth paths".to_string(),
            ],
        },
        permissions: AgentSessionPolicySnapshot {
            default_effect: "allow with guardrails".to_string(),
            rules: config
                .permission_rules
                .iter()
                .map(agent_session_policy_rule_from_effective)
                .collect(),
            guardrails: config.guardrails.clone(),
        },
    }
}

fn agent_session_policy_rule_from_effective(
    rule: &AgentEffectivePolicyRule,
) -> AgentSessionPolicyRule {
    AgentSessionPolicyRule {
        source: rule.source.clone(),
        action: rule.action.clone(),
        resource: rule.resource.clone(),
        effect: rule.effect.clone(),
    }
}

fn agent_tools_list(args: AgentToolsListArgs) -> Result<()> {
    let selection = resolve_agent_role_selection(args.agent, &args.profile, None)?;
    let specs = agent_tool_specs(args.workspace, &selection.profile, &selection.tools)?;
    print!(
        "{}",
        format_agent_tool_specs(&specs, output_format(args.format, args.json))?
    );
    Ok(())
}

fn agent_tools_show(args: AgentToolsShowArgs) -> Result<()> {
    let selection = resolve_agent_role_selection(args.agent, &args.profile, None)?;
    let specs = agent_tool_specs(args.workspace, &selection.profile, &selection.tools)?;
    let spec = resolve_agent_tool_spec(&specs, &args.name)?;
    print!(
        "{}",
        format_agent_tool_spec(spec, output_format(args.format, args.json))?
    );
    Ok(())
}

fn agent_policy_list(args: AgentPolicyListArgs) -> Result<()> {
    let config =
        resolve_agent_effective_config(args.workspace, args.profile, args.agent, args.model)?;
    let report = agent_policy_report(&config);
    print!(
        "{}",
        format_agent_policy_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

fn agent_policy_audit(args: AgentPolicyAuditArgs) -> Result<()> {
    let config =
        resolve_agent_effective_config(args.workspace, args.profile, args.agent, args.model)?;
    let report = agent_policy_audit_report(&config);
    print!(
        "{}",
        format_agent_policy_audit_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

fn agent_policy_revoke(args: AgentPolicyRevokeArgs) -> Result<()> {
    let report = AgentPolicyRevokeReport {
        action: args.action,
        resource: args.resource,
        durable_approvals_found: 0,
        revoked: 0,
        message: "No durable approval store exists yet; session approvals are process-local and expire with the agent process.".to_string(),
    };
    print!(
        "{}",
        format_agent_policy_revoke_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AgentPolicyReport {
    workspace: String,
    agent_name: Option<String>,
    profile: String,
    model: String,
    policy_sources: Vec<String>,
    read_access_rules: Vec<AgentEffectivePolicyRule>,
    permission_rules: Vec<AgentEffectivePolicyRule>,
    guardrails: Vec<String>,
    session_approvals: String,
    durable_approvals: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AgentPolicyAuditReport {
    policy: AgentPolicyReport,
    findings: Vec<AgentPolicyAuditFinding>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AgentPolicyAuditFinding {
    severity: String,
    code: String,
    message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AgentPolicyRevokeReport {
    action: Option<String>,
    resource: Option<String>,
    durable_approvals_found: usize,
    revoked: usize,
    message: String,
}

fn agent_policy_report(config: &AgentEffectiveConfig) -> AgentPolicyReport {
    AgentPolicyReport {
        workspace: config.workspace.clone(),
        agent_name: config.agent_name.clone(),
        profile: config.profile.clone(),
        model: config.model.clone(),
        policy_sources: effective_policy_sources(config),
        read_access_rules: config.read_access_rules.clone(),
        permission_rules: config.permission_rules.clone(),
        guardrails: config.guardrails.clone(),
        session_approvals: "process-local action/workspace/resource grants".to_string(),
        durable_approvals: "not implemented; native config is the durable policy surface"
            .to_string(),
    }
}

fn agent_policy_audit_report(config: &AgentEffectiveConfig) -> AgentPolicyAuditReport {
    let policy = agent_policy_report(config);
    let mut findings = vec![
        AgentPolicyAuditFinding {
            severity: "info".to_string(),
            code: "hard_guardrails".to_string(),
            message: "Built-in secret-read, destructive shell/git, and sensitive mutation guardrails are active.".to_string(),
        },
        AgentPolicyAuditFinding {
            severity: "info".to_string(),
            code: "session_scoped_approvals".to_string(),
            message: "Interactive approvals are process-local and scoped by action, workspace, and resource/path.".to_string(),
        },
        AgentPolicyAuditFinding {
            severity: "info".to_string(),
            code: "no_durable_approval_store".to_string(),
            message: "No durable approval database exists; persistent policy changes must be reviewed native config edits.".to_string(),
        },
    ];
    if policy.permission_rules.is_empty() && policy.read_access_rules.is_empty() {
        findings.push(AgentPolicyAuditFinding {
            severity: "notice".to_string(),
            code: "no_config_policy_rules".to_string(),
            message: "No native config permission rules are active for this profile; built-in defaults and guardrails apply.".to_string(),
        });
    }
    AgentPolicyAuditReport { policy, findings }
}

fn format_agent_policy_report(report: &AgentPolicyReport, format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let mut lines = vec![
        "Agent effective policy".to_string(),
        format!("Workspace: {}", report.workspace),
        format!(
            "Agent: {}",
            report.agent_name.as_deref().unwrap_or("<none>")
        ),
        format!("Profile: {}", report.profile),
        format!("Model: {}", report.model),
        String::new(),
        "Policy sources:".to_string(),
    ];
    if report.policy_sources.is_empty() {
        lines.push("  - built-in defaults only".to_string());
    } else {
        for source in &report.policy_sources {
            lines.push(format!("  - {source}"));
        }
    }
    lines.push("Read access rules:".to_string());
    push_agent_policy_rule_lines(&mut lines, &report.read_access_rules);
    lines.push("Permission rules:".to_string());
    push_agent_policy_rule_lines(&mut lines, &report.permission_rules);
    lines.push("Guardrails:".to_string());
    for guardrail in &report.guardrails {
        lines.push(format!("  - {guardrail}"));
    }
    lines.push(format!("Session approvals: {}", report.session_approvals));
    lines.push(format!("Durable approvals: {}", report.durable_approvals));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn push_agent_policy_rule_lines(lines: &mut Vec<String>, rules: &[AgentEffectivePolicyRule]) {
    if rules.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for rule in rules {
            lines.push(format!(
                "  - {}: {} {} {}",
                rule.source, rule.effect, rule.action, rule.resource
            ));
        }
    }
}

fn format_agent_policy_audit_report(
    report: &AgentPolicyAuditReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let mut lines = vec![
        "Agent policy audit".to_string(),
        format!("Workspace: {}", report.policy.workspace),
        format!("Profile: {}", report.policy.profile),
        String::new(),
        "Findings:".to_string(),
    ];
    for finding in &report.findings {
        lines.push(format!(
            "  - [{}] {}: {}",
            finding.severity, finding.code, finding.message
        ));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn format_agent_policy_revoke_report(
    report: &AgentPolicyRevokeReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let mut lines = vec![
        "Agent policy revoke".to_string(),
        format!(
            "Durable approvals found: {}",
            report.durable_approvals_found
        ),
        format!("Revoked: {}", report.revoked),
        report.message.clone(),
    ];
    if let Some(action) = &report.action {
        lines.push(format!("Action selector: {action}"));
    }
    if let Some(resource) = &report.resource {
        lines.push(format!("Resource selector: {resource}"));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn agent_tool_specs(
    workspace: Option<PathBuf>,
    profile: &str,
    allowed_tools: &[String],
) -> Result<Vec<ToolSpec>> {
    let workspace = resolve_agent_workspace(workspace)?;
    let workspace_path = Path::new(&workspace);
    let read_access = resolve_agent_read_access_policy(profile, workspace_path)?;
    let permissions = resolve_agent_permission_policy(profile, workspace_path)?;
    let mut registry = tools_with_policies_file_history_and_gate(
        workspace_path,
        read_access,
        permissions,
        None,
        None,
    )?;
    registry.retain_names(allowed_tools)?;
    Ok(registry.specs())
}

fn agent_file_history_list(args: AgentFileHistoryListArgs) -> Result<()> {
    let entries = file_history_store().list_entries(FileHistoryFilter {
        patch_id: args.patch_id,
        workspace: args.workspace,
        limit: args.limit,
    })?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("File history is empty.");
    } else {
        for (idx, entry) in entries.iter().enumerate() {
            let target = entry
                .new_path
                .as_ref()
                .map(|new_path| format!("{} -> {new_path}", entry.path))
                .unwrap_or_else(|| entry.path.clone());
            println!(
                "  {}. [{}] {} {} — patch {} — {}",
                idx + 1,
                entry.id,
                entry.operation,
                target,
                entry.patch_id,
                entry.created_at
            );
        }
        println!("\nTotal: {} file-history entries", entries.len());
    }
    Ok(())
}

fn agent_file_history_restore(args: AgentFileHistoryRestoreArgs) -> Result<()> {
    let id = FileHistoryEntryId::new(args.id);
    let report = file_history_store().restore_entry(
        &id,
        FileHistoryRestoreOptions {
            force: args.force,
            remove_new_path: args.remove_new_path,
            dry_run: args.dry_run,
        },
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let prefix = if report.dry_run {
            "File history preview"
        } else {
            "File history restored"
        };
        println!(
            "{prefix} [{}]: {} {}",
            report.entry.id, report.action, report.restored_path
        );
        if report.force_required && report.dry_run && !args.force {
            println!("Force would be required for a real restore.");
        }
        if let Some(path) = report.removed_new_path {
            let verb = if report.dry_run {
                "Would remove"
            } else {
                "Removed"
            };
            println!("{verb} move destination: {path}");
        }
    }
    Ok(())
}

fn load_djinn_config_for_workspace(workspace: &str) -> Result<DjinnConfigLoadReport> {
    load_djinn_config_from_paths(clean_unique_paths(vec![
        default_djinn_config_path(),
        Path::new(workspace).join(".djinn.json"),
    ]))
}

fn nonempty_owned_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn top_level_ask(args: AgentAskArgs) -> Result<()> {
    agent_ask(args, true, AgentAskOutputMode::Ask)
}

fn legacy_agent_ask(args: AgentAskArgs) -> Result<()> {
    warn_legacy_agent_command("agent ask", Some("use top-level `djinn ask`"));
    agent_ask(args, true, AgentAskOutputMode::Ask)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentAskOutputMode {
    Ask,
    SessionRun { open: bool, background_worker: bool },
}

fn session_run(mut args: SessionRunArgs) -> Result<()> {
    if args.background_worker && args.foreground {
        bail!("--background-worker cannot be combined with --fg");
    }
    let session_ref = resolve_existing_folder_session_reference(&args.dir)?;
    let session_dir = session_ref.session_dir.clone();
    args.dir = session_dir.clone();
    if args.background_worker {
        touch_background_run_marker_from_env("worker_started");
    }
    let manifest = read_folder_session_manifest(&session_dir)?;
    if manifest
        .as_ref()
        .and_then(|manifest| manifest.kind.as_deref())
        == Some("promotion")
    {
        if args.dry_run && args.background_worker {
            bail!("--dry-run cannot be combined with --background-worker");
        }
        if !args.foreground && !args.background_worker && !args.dry_run {
            return session_run_background(args);
        }
        return session_run_promotion(args, session_dir, manifest.unwrap());
    }
    if args.dry_run {
        bail!("--dry-run is currently only supported for promotion sessions");
    }
    if !args.foreground && !args.background_worker {
        return session_run_background(args);
    }
    let open = args.open;
    let background_worker = args.background_worker;
    agent_ask(
        AgentAskArgs {
            prompt: None,
            session_id: None,
            session_dir: Some(args.dir),
            title: None,
            workspace: None,
            profile: args.profile,
            agent: args.agent,
            parent_session: None,
            model: args.model,
            api_key: args
                .api_key
                .or_else(|| env::var("DJINN_SESSION_RUN_API_KEY").ok()),
            base_url: args.base_url,
            max_tool_rounds: args.max_tool_rounds,
            json: args.json,
            print: args.print,
            open: false,
        },
        true,
        AgentAskOutputMode::SessionRun {
            open,
            background_worker,
        },
    )
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionRunBackgroundReport {
    status: String,
    session_dir: String,
    pid: u32,
    log_path: String,
    watch_command: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct PromotionCandidateGenerationReport {
    status: String,
    dry_run: bool,
    session_dir: String,
    promotion_type: String,
    model: Option<String>,
    source_packet_path: String,
    prompt_path: Option<String>,
    response_path: Option<String>,
    candidates_dir: String,
    candidate_index_path: Option<String>,
    candidate_count: usize,
    candidates: Vec<PromotionGeneratedCandidateReport>,
}

fn session_run_promotion(
    args: SessionRunArgs,
    session_dir: PathBuf,
    manifest: FolderSessionManifest,
) -> Result<()> {
    if args.print || args.open {
        bail!("--print and --open are not supported for promotion candidate generation");
    }
    let report = generate_promotion_candidates(&args, &session_dir, &manifest)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.dry_run {
        println!(
            "Promotion candidate generation dry run: {}",
            report.session_dir
        );
        println!("  type: {}", report.promotion_type);
        if let Some(model) = &report.model {
            println!("  model: {model}");
        }
        println!("  source packet: {}", report.source_packet_path);
        println!("  candidates dir: {}", report.candidates_dir);
        if let Some(prompt_path) = &report.prompt_path {
            println!("  prompt preview: {prompt_path}");
        }
    } else {
        println!("Generated promotion candidates: {}", report.session_dir);
        println!("  type: {}", report.promotion_type);
        if let Some(model) = &report.model {
            println!("  model: {model}");
        }
        println!(
            "  response: {}",
            report.response_path.as_deref().unwrap_or("none")
        );
        println!("  candidates: {}", report.candidate_count);
        for candidate in &report.candidates {
            println!(
                "    - {} {} -> {}",
                candidate.candidate_type, candidate.id, candidate.path
            );
        }
        println!(
            "  accept: djinn session accept {} --dry-run",
            report.session_dir
        );
    }
    Ok(())
}

fn generate_promotion_candidates(
    args: &SessionRunArgs,
    session_dir: &Path,
    manifest: &FolderSessionManifest,
) -> Result<PromotionCandidateGenerationReport> {
    let promotion_type = manifest
        .promotion_type
        .clone()
        .unwrap_or_else(|| "memory".to_string());
    let source_packet_path = session_dir.join("context").join("source-packet.md");
    let source_packet = fs::read_to_string(&source_packet_path)
        .with_context(|| format!("reading {}", source_packet_path.display()))?;
    let prompt = render_promotion_candidate_generation_prompt(&promotion_type, &source_packet);
    let outputs_dir = session_dir.join("outputs");
    let generation_dir = outputs_dir.join("generation");
    let candidates_dir = outputs_dir.join("candidates");
    let timestamp = chrono::Local::now()
        .timestamp_nanos_opt()
        .unwrap_or_default();
    fs::create_dir_all(&generation_dir)
        .with_context(|| format!("creating generation directory {}", generation_dir.display()))?;
    fs::create_dir_all(&candidates_dir)
        .with_context(|| format!("creating candidates directory {}", candidates_dir.display()))?;
    let prompt_path = generation_dir.join(format!("{timestamp}-prompt.md"));
    fs::write(&prompt_path, ensure_trailing_newline(&prompt))
        .with_context(|| format!("writing {}", prompt_path.display()))?;

    let (profile, model) = resolve_promotion_generation_profile_model(args, manifest)?;
    if args.dry_run {
        return Ok(PromotionCandidateGenerationReport {
            status: "dry_run".to_string(),
            dry_run: true,
            session_dir: session_dir.display().to_string(),
            promotion_type,
            model: Some(model),
            source_packet_path: source_packet_path.display().to_string(),
            prompt_path: Some(prompt_path.display().to_string()),
            response_path: None,
            candidates_dir: candidates_dir.display().to_string(),
            candidate_index_path: None,
            candidate_count: 0,
            candidates: Vec::new(),
        });
    }

    let response = complete_promotion_candidate_model(
        &prompt,
        model.clone(),
        args.api_key.clone(),
        args.base_url.clone(),
        &profile,
    )?;
    let response_path = generation_dir.join(format!("{timestamp}-response.md"));
    fs::write(
        &response_path,
        ensure_trailing_newline(&response.message.content),
    )
    .with_context(|| format!("writing {}", response_path.display()))?;
    let candidates = write_generated_promotion_candidates(
        session_dir,
        &promotion_type,
        &response.message.content,
        &candidates_dir,
    )?;
    let candidate_index_path = write_promotion_candidate_index(session_dir, &candidates)?;
    write_promotion_generation_summary(session_dir, &promotion_type, &candidates)?;

    Ok(PromotionCandidateGenerationReport {
        status: "generated".to_string(),
        dry_run: false,
        session_dir: session_dir.display().to_string(),
        promotion_type,
        model: Some(model),
        source_packet_path: source_packet_path.display().to_string(),
        prompt_path: Some(prompt_path.display().to_string()),
        response_path: Some(response_path.display().to_string()),
        candidates_dir: candidates_dir.display().to_string(),
        candidate_index_path: Some(candidate_index_path.display().to_string()),
        candidate_count: candidates.len(),
        candidates,
    })
}

fn resolve_promotion_generation_profile_model(
    args: &SessionRunArgs,
    manifest: &FolderSessionManifest,
) -> Result<(String, String)> {
    let workspace = session_manifest_workspace_path(Some(manifest))
        .unwrap_or(env::current_dir().context("resolving current workspace")?);
    let workspace = resolve_agent_workspace(Some(workspace))?;
    let config_report = load_djinn_config_for_workspace(&workspace)?;
    let requested_profile = nonempty_owned_string(args.profile.clone())
        .or_else(|| manifest.profile.clone())
        .unwrap_or_else(|| "default".to_string());
    let requested_agent = args.agent.clone().or_else(|| manifest.agent.clone());
    let requested_model = args.model.clone().or_else(|| manifest.model.clone());
    let selection = resolve_agent_role_selection_from_config(
        &config_report.effective,
        requested_agent,
        &requested_profile,
        requested_model,
    )?;
    let profile = selection.profile;
    let model =
        resolve_agent_model_from_config(selection.model, &config_report.effective, &profile);
    Ok((profile, model))
}

fn complete_promotion_candidate_model(
    prompt: &str,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    profile: &str,
) -> Result<djinn_agent::ModelResponse> {
    let messages = vec![
        ModelMessage {
            role: ModelRole::System,
            content: format!(
                "You generate Djinn promotion candidates for profile `{profile}`. Return only fenced TOML candidate blocks; do not write files or mutate durable stores."
            ),
            tool_call_id: None,
            tool_calls: Vec::new(),
        },
        ModelMessage {
            role: ModelRole::User,
            content: prompt.to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        },
    ];
    let client: Box<dyn ModelClient> = if is_copilot_model(&model) {
        let token = resolve_copilot_token(api_key)?;
        let endpoint = base_url
            .or_else(|| env::var("GITHUB_COPILOT_CHAT_COMPLETIONS_URL").ok())
            .unwrap_or_else(|| "https://api.githubcopilot.com/chat/completions".to_string());
        Box::new(CopilotClient::with_endpoint(token, endpoint))
    } else {
        Box::new(resolve_openai_client(api_key, base_url)?)
    };
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .with_context(|| "creating Tokio runtime for promotion candidate generation")?;
    tokio.block_on(client.complete(ModelRequest {
        model,
        messages,
        tools: Vec::new(),
    }))
}

fn session_run_background(args: SessionRunArgs) -> Result<()> {
    if args.print || args.open {
        bail!("--print and --open require --fg because background runs return before an answer exists");
    }
    let session_dir = resolve_existing_folder_session_dir(&args.dir)?;
    resolve_agent_request_prompt(None, Some(&session_dir))?;
    let report = spawn_background_session_run(&session_dir, &args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_run_background_started(&report));
    }
    Ok(())
}

fn spawn_background_session_run(
    session_dir: &Path,
    args: &SessionRunArgs,
) -> Result<SessionRunBackgroundReport> {
    let log_path = background_session_run_log_path(session_dir)?;
    let marker_path = log_path.with_extension("toml");
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening background run log {}", log_path.display()))?;
    let err_file = log_file
        .try_clone()
        .with_context(|| format!("cloning background run log {}", log_path.display()))?;
    let exe = env::current_exe().context("resolving current djinn executable")?;
    let command_hint = background_session_run_command_hint(&exe, session_dir, args);
    let native_session_id = read_folder_session_manifest(session_dir)?
        .and_then(|manifest| manifest.session_id.map(|id| id.to_string()));
    let mut command = ProcessCommand::new(exe);
    command
        .arg("session")
        .arg("run")
        .arg(session_dir)
        .arg("--background-worker")
        .env("DJINN_BACKGROUND_RUN_MARKER", &marker_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file));
    if let Some(profile) = &args.profile {
        command.arg("--profile").arg(profile);
    }
    if let Some(agent) = &args.agent {
        command.arg("--agent").arg(agent);
    }
    if let Some(model) = &args.model {
        command.arg("--model").arg(model);
    }
    if let Some(api_key) = &args.api_key {
        command.env("DJINN_SESSION_RUN_API_KEY", api_key);
    }
    if let Some(base_url) = &args.base_url {
        command.arg("--base-url").arg(base_url);
    }
    command
        .arg("--max-tool-rounds")
        .arg(args.max_tool_rounds.to_string());
    let child = command.spawn().with_context(|| {
        format!(
            "spawning background session run for {}",
            session_dir.display()
        )
    })?;
    let pid = child.id();
    write_background_session_run_marker(
        session_dir,
        &log_path,
        pid,
        &command_hint,
        native_session_id.as_deref(),
    )?;
    Ok(SessionRunBackgroundReport {
        status: "started".to_string(),
        session_dir: session_dir.display().to_string(),
        pid,
        log_path: log_path.display().to_string(),
        watch_command: format!("djinn session watch {}", session_dir.display()),
    })
}

fn background_session_run_command_hint(
    exe: &Path,
    session_dir: &Path,
    args: &SessionRunArgs,
) -> String {
    let mut parts = vec![
        shell_quote(&exe.display().to_string()),
        "session".to_string(),
        "run".to_string(),
        shell_quote(&session_dir.display().to_string()),
        "--background-worker".to_string(),
    ];
    if let Some(profile) = &args.profile {
        parts.push("--profile".to_string());
        parts.push(shell_quote(profile));
    }
    if let Some(agent) = &args.agent {
        parts.push("--agent".to_string());
        parts.push(shell_quote(agent));
    }
    if let Some(model) = &args.model {
        parts.push("--model".to_string());
        parts.push(shell_quote(model));
    }
    if let Some(base_url) = &args.base_url {
        parts.push("--base-url".to_string());
        parts.push(shell_quote(base_url));
    }
    parts.push("--max-tool-rounds".to_string());
    parts.push(args.max_tool_rounds.to_string());
    let command = parts.join(" ");
    if args.api_key.is_some() {
        format!("DJINN_SESSION_RUN_API_KEY=<redacted> {command}")
    } else {
        command
    }
}

fn touch_background_run_marker_from_env(phase: &str) {
    let Some(path) = env::var_os("DJINN_BACKGROUND_RUN_MARKER").map(PathBuf::from) else {
        return;
    };
    let _ = touch_background_run_marker(&path, phase);
}

fn background_progress_phase(event: &AgentProgressEvent) -> &'static str {
    match event {
        AgentProgressEvent::ModelRequestStarted { .. } => "model_request_started",
        AgentProgressEvent::ModelResponseCompleted { .. } => "model_response_completed",
        AgentProgressEvent::ToolCallStarted { .. } => "tool_call_started",
        AgentProgressEvent::ToolCallCompleted { .. } => "tool_call_completed",
    }
}

pub(crate) fn upsert_toml_root_string(content: &str, key: &str, value: &str) -> Result<String> {
    let rendered = format!("{key} = {}", toml_string(value)?);
    let mut replaced = false;
    let mut output = String::new();
    for line in content.lines() {
        if !replaced && line.trim_start().starts_with(&format!("{key} =")) {
            output.push_str(&rendered);
            output.push('\n');
            replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if !replaced {
        output.push_str(&rendered);
        output.push('\n');
    }
    Ok(output)
}

fn format_session_run_background_started(report: &SessionRunBackgroundReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Started Djinn session run: {}", report.session_dir));
    lines.push(format!("  pid: {}", report.pid));
    lines.push(format!("  log: {}", report.log_path));
    lines.push(format!("  watch: {}", report.watch_command));
    lines.push(String::new());
    lines.join("\n")
}

fn agent_ask(
    args: AgentAskArgs,
    auto_folder_session: bool,
    output_mode: AgentAskOutputMode,
) -> Result<()> {
    let session_dir = args
        .session_dir
        .as_deref()
        .map(resolve_session_dir)
        .transpose()?;
    let should_auto_folder_session = auto_folder_session
        && session_dir.is_none()
        && args
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .is_none();
    let folder_manifest = session_dir
        .as_deref()
        .map(read_folder_session_manifest)
        .transpose()?
        .flatten();
    let prompt = resolve_agent_request_prompt(args.prompt.clone(), session_dir.as_deref())?;
    let folder_context_instructions =
        resolve_folder_session_context_instructions(session_dir.as_deref())?;
    let mut store = agent_session_store();
    let requested_session_id = args
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| AgentSessionId::new(id.to_string()));
    let requested_session_id = match requested_session_id {
        Some(id) => Some(id),
        None => folder_manifest
            .as_ref()
            .and_then(|manifest| manifest.session_id.clone()),
    };

    if let (Some(session_dir), Some(id)) = (session_dir.as_deref(), requested_session_id.as_ref()) {
        store = agent_session_store_for_folder_session(session_dir, id);
        hydrate_folder_agent_session_from_events_jsonl(session_dir, id, folder_manifest.as_ref())?;
    }

    let (id, workspace, profile, model, system_instructions, allowed_tools) =
        if let Some(id) = requested_session_id {
            let session = store
                .load_session(&id)
                .with_context(|| format!("loading agent session {id}"))?;
            let workspace = if session.meta.workspace.trim().is_empty() {
                resolve_agent_workspace(None)?
            } else {
                session.meta.workspace.clone()
            };
            let workspace = resolve_agent_workspace(
                args.workspace
                    .clone()
                    .or_else(|| session_manifest_workspace_path(folder_manifest.as_ref()))
                    .or_else(|| Some(PathBuf::from(workspace))),
            )?;
            let requested_profile = nonempty_owned_string(args.profile.clone())
                .or_else(|| {
                    folder_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.profile.clone())
                })
                .or_else(|| nonempty_owned_string(Some(session.meta.profile.clone())))
                .unwrap_or_else(|| "default".to_string());
            let requested_agent = args
                .agent
                .clone()
                .or_else(|| {
                    folder_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.agent.clone())
                })
                .or_else(|| session.meta.agent_name.clone());
            let requested_model = args
                .model
                .clone()
                .or_else(|| {
                    folder_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.model.clone())
                })
                .or_else(|| latest_session_model(&session));
            let config_report = load_djinn_config_for_workspace(&workspace)?;
            let selection = resolve_agent_role_selection_from_config(
                &config_report.effective,
                requested_agent,
                &requested_profile,
                requested_model,
            )?;
            let profile = selection.profile;
            let model = resolve_agent_model_from_config(
                selection.model.clone(),
                &config_report.effective,
                &profile,
            );
            let mut system_instructions =
                resolve_agent_instruction_contents(&workspace, &selection.instructions)?;
            system_instructions.extend(folder_context_instructions.clone());
            (
                id,
                workspace,
                profile,
                model,
                system_instructions,
                selection.tools,
            )
        } else {
            let workspace = resolve_agent_workspace(
                args.workspace
                    .clone()
                    .or_else(|| session_manifest_workspace_path(folder_manifest.as_ref())),
            )?;
            let config_report = load_djinn_config_for_workspace(&workspace)?;
            let requested_profile = nonempty_owned_string(args.profile.clone())
                .or_else(|| {
                    folder_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.profile.clone())
                })
                .unwrap_or_else(|| "default".to_string());
            let requested_agent = args.agent.clone().or_else(|| {
                folder_manifest
                    .as_ref()
                    .and_then(|manifest| manifest.agent.clone())
            });
            let requested_model = args.model.clone().or_else(|| {
                folder_manifest
                    .as_ref()
                    .and_then(|manifest| manifest.model.clone())
            });
            let selection = resolve_agent_role_selection_from_config(
                &config_report.effective,
                requested_agent,
                &requested_profile,
                requested_model,
            )?;
            let profile = selection.profile;
            let model = resolve_agent_model_from_config(
                selection.model.clone(),
                &config_report.effective,
                &profile,
            );
            let parent_session_id = parent_session_id_from_arg(args.parent_session.clone());
            validate_agent_child_session_depth(&store, parent_session_id.as_ref())?;
            let title = args
                .title
                .clone()
                .unwrap_or_else(|| prompt_title(&prompt, "Djinn prompt"));
            let mut system_instructions =
                resolve_agent_instruction_contents(&workspace, &selection.instructions)?;
            system_instructions.extend(folder_context_instructions.clone());
            let effective_config = agent_effective_config_from_parts(
                workspace.clone(),
                profile.clone(),
                model.clone(),
                selection.agent_name.clone(),
                selection.instructions.clone(),
                selection.tools.clone(),
            )?;
            let meta = AgentSessionMeta {
                title,
                workspace: workspace.clone(),
                profile: profile.clone(),
                agent_name: selection.agent_name,
                parent_session_id,
                source: "djinn".to_string(),
                runtime_config: Some(agent_session_runtime_config(&effective_config)),
                ..AgentSessionMeta::default()
            };
            let id = store.create_session(meta)?;
            (
                id,
                workspace,
                profile,
                model,
                system_instructions,
                selection.tools,
            )
        };

    let projected_session_dir = session_dir
        .clone()
        .or_else(|| should_auto_folder_session.then(|| auto_folder_session_dir(&prompt, &id)));
    if args.open && !should_auto_folder_session {
        bail!("`djinn ask --open` opens only the auto-created folder for a new ask; use `djinn session <name-or-path> --open` for existing sessions");
    }
    if let Some(session_dir) = &projected_session_dir {
        store = relocate_agent_session_into_folder(&store, session_dir, &id)?;
        let session = store.load_session(&id)?;
        write_agent_session_toml(session_dir, &session)?;
        if should_auto_folder_session {
            let buddy_backend = BuddyBridgeBackend::resolved(None)?;
            ensure_folder_session_buddy_binding_for_ask(
                session_dir,
                &session,
                Path::new(&workspace),
                &buddy_backend,
            )?;
        }
    }

    let lifecycle_mode = match output_mode {
        AgentAskOutputMode::SessionRun {
            background_worker: true,
            ..
        } => AgentSessionExecutionMode::Background,
        _ => AgentSessionExecutionMode::Foreground,
    };
    let lifecycle_reason_prefix = match output_mode {
        AgentAskOutputMode::SessionRun { .. } => "djinn session run",
        AgentAskOutputMode::Ask => "djinn ask",
    };
    store.append_event(
        &id,
        AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
            content: prompt.clone(),
        }),
    )?;
    maybe_auto_title_agent_session(&store, &id, &prompt)?;
    append_agent_session_lifecycle_event(
        &store,
        &id,
        AgentSessionLifecycleState::Running,
        lifecycle_mode.clone(),
        format!("{lifecycle_reason_prefix} started"),
        None,
    )?;
    sync_folder_session_events_jsonl_from_store(projected_session_dir.as_deref(), &store, &id)?;
    let session_for_model = store.load_session(&id)?;
    let background_worker = matches!(
        output_mode,
        AgentAskOutputMode::SessionRun {
            background_worker: true,
            ..
        }
    );
    let response = match complete_openai_messages_with_progress(
        &store,
        &id,
        agent_model_messages(&session_for_model, &workspace, &system_instructions),
        model.clone(),
        args.api_key,
        args.base_url,
        args.max_tool_rounds,
        &profile,
        allowed_tools,
        !args.json,
        |event| {
            if background_worker {
                touch_background_run_marker_from_env(background_progress_phase(&event));
            }
            Ok(())
        },
    ) {
        Ok(response) => {
            append_agent_session_lifecycle_event(
                &store,
                &id,
                AgentSessionLifecycleState::Completed,
                lifecycle_mode.clone(),
                format!("{lifecycle_reason_prefix} completed"),
                None,
            )?;
            sync_folder_session_events_jsonl_from_store(
                projected_session_dir.as_deref(),
                &store,
                &id,
            )?;
            response
        }
        Err(error) => {
            let _ = append_agent_session_lifecycle_event(
                &store,
                &id,
                AgentSessionLifecycleState::Failed,
                lifecycle_mode,
                format!("{lifecycle_reason_prefix} failed"),
                Some(error.to_string()),
            );
            let _ = sync_folder_session_events_jsonl_from_store(
                projected_session_dir.as_deref(),
                &store,
                &id,
            );
            return Err(error);
        }
    };
    let session = store.load_session(&id)?;
    let projection = if let Some(session_dir) = &projected_session_dir {
        let projection = project_agent_session_dir(
            session_dir,
            &session,
            &prompt,
            response.message.content.trim_end(),
        )?;
        ensure_folder_session_readme(session_dir)?;
        Some(projection)
    } else {
        None
    };
    let folder_path_output = auto_folder_session && projected_session_dir.is_some();
    if let AgentAskOutputMode::SessionRun { .. } = output_mode {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "completed",
                    "session_id": id.to_string(),
                    "session_dir": projected_session_dir,
                    "model": model,
                    "summary_path": projection.as_ref().map(|projection| &projection.summary_path),
                    "request_path": projection.as_ref().map(|projection| &projection.request_path),
                    "turn_dir": projection.as_ref().and_then(|projection| projection.turn_dir.as_ref()),
                    "response_path": projection
                        .as_ref()
                        .and_then(|projection| projection.turn_dir.as_ref())
                        .map(|turn_dir| turn_dir.join("response.md")),
                }))?
            );
        } else {
            if args.print {
                println!("{}", response.message.content);
            }
            print!(
                "{}",
                format_session_run_completion(
                    &id,
                    projection.as_ref(),
                    projected_session_dir.as_deref()
                )
            );
        }
    } else if args.json && folder_path_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "session_dir": projected_session_dir,
            }))?
        );
    } else if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "completed",
                "provider": "openai",
                "model": model,
                "response": response,
                "session": session,
                "session_dir": projected_session_dir,
            }))?
        );
    } else if args.print {
        println!("{}", response.message.content);
    } else if folder_path_output {
        if let Some(session_dir) = &projected_session_dir {
            println!("{}", session_dir.display());
        }
    } else {
        println!("{}", response.message.content);
        println!("\nAgent session [{}]: {}", id, session.meta.title);
        println!("Path: {}", store.session_file_path(&id).display());
        if let Some(session_dir) = &projected_session_dir {
            println!("Session dir: {}", session_dir.display());
        }
    }
    if args.open {
        if let Some(projection) = &projection {
            open_editor_path(&projection.summary_path, None)?;
        }
    }
    if let AgentAskOutputMode::SessionRun { open: true, .. } = output_mode {
        if let Some(projection) = &projection {
            open_editor_path(&projection.summary_path, None)?;
        }
    }
    Ok(())
}

fn format_session_run_completion(
    id: &AgentSessionId,
    projection: Option<&AgentSessionDirProjection>,
    session_dir: Option<&Path>,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Completed Djinn session run: {id}"));
    if let Some(projection) = projection {
        lines.push(format!("  session: {}", projection.session_dir.display()));
        lines.push(format!("  summary: {}", projection.summary_path.display()));
        if let Some(turn_dir) = &projection.turn_dir {
            lines.push(format!(
                "  response: {}",
                turn_dir.join("response.md").display()
            ));
        } else {
            lines.push("  response: summary.md (turns/ projection not written)".to_string());
        }
        lines.push(format!("  request: {}", projection.request_path.display()));
    } else if let Some(session_dir) = session_dir {
        lines.push(format!("  session: {}", session_dir.display()));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn validate_agent_child_session_depth(
    store: &JsonlAgentSessionStore,
    parent_session_id: Option<&AgentSessionId>,
) -> Result<()> {
    let Some(parent_session_id) = parent_session_id else {
        return Ok(());
    };

    let parent_depth = agent_session_depth(store, parent_session_id)?;
    if parent_depth >= AGENT_CHILD_SESSION_MAX_DEPTH {
        bail!(
            "child session depth limit exceeded: parent session {parent_session_id} is at depth \
             {parent_depth}; maximum child-session depth is {AGENT_CHILD_SESSION_MAX_DEPTH} \
             levels below the root"
        );
    }

    Ok(())
}

fn agent_session_depth(
    store: &JsonlAgentSessionStore,
    session_id: &AgentSessionId,
) -> Result<usize> {
    let mut depth = 0;
    let mut current = session_id.clone();
    let mut seen = HashSet::new();

    loop {
        if !seen.insert(current.clone()) {
            bail!("cycle detected in agent session parent chain at {current}");
        }

        let session = store
            .load_session(&current)
            .with_context(|| format!("loading parent agent session {current}"))?;
        let Some(parent) = session.meta.parent_session_id else {
            return Ok(depth);
        };

        depth += 1;
        current = parent;
    }
}

fn maybe_auto_title_agent_session(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    prompt: &str,
) -> Result<()> {
    let session = store.load_session(id)?;
    if !should_auto_title_agent_session(&session) {
        return Ok(());
    }
    let title = infer_agent_session_title(prompt);
    if title.trim().is_empty() || title == session.meta.title {
        return Ok(());
    }
    store.append_event(
        id,
        AgentSessionEvent::new(AgentSessionEventKind::SessionTitleUpdated { title }),
    )
}

fn should_auto_title_agent_session(session: &AgentSession) -> bool {
    let title = session.meta.title.trim();
    let default_title =
        title.is_empty() || title == "Agent chat" || title == "Untitled agent session";
    default_title
        && session
            .events
            .iter()
            .filter(|event| matches!(event.kind, AgentSessionEventKind::UserMessage { .. }))
            .count()
            == 1
}

fn infer_agent_session_title(prompt: &str) -> String {
    let title = prompt_title(prompt, "Djinn session")
        .trim_matches(|ch: char| ch.is_ascii_punctuation() || ch.is_whitespace())
        .trim()
        .to_string();
    if title.is_empty() {
        "Djinn session".to_string()
    } else {
        title
    }
}

fn latest_session_model(session: &AgentSession) -> Option<String> {
    for event in session.events.iter().rev() {
        match &event.kind {
            AgentSessionEventKind::SessionModelUpdated { model } => {
                let model = model.trim();
                if !model.is_empty() {
                    return Some(model.to_string());
                }
            }
            AgentSessionEventKind::SessionProfileUpdated { .. } => return None,
            _ => {}
        }
    }
    None
}

fn prompt_auth_provider() -> AuthProvider {
    println!("┌  Add credential");
    println!("│");
    println!("◇  Select provider");
    println!("│  1) OpenAI");
    let choice = prompt_number("Provider", 1, 1).unwrap_or(1);
    match choice {
        _ => AuthProvider::Openai,
    }
}

fn prompt_openai_login_method() -> OpenAiLoginMethod {
    println!("│");
    println!("◆  Login method");
    println!("│  1) ChatGPT Pro/Plus (browser)");
    println!("│  2) ChatGPT Pro/Plus (headless)");
    println!("│  3) Manually enter API Key");
    match prompt_number("Login method", 1, 3).unwrap_or(1) {
        2 => OpenAiLoginMethod::Headless,
        3 => OpenAiLoginMethod::ApiKey,
        _ => OpenAiLoginMethod::Browser,
    }
}

fn prompt_number(prompt: &str, default: usize, max: usize) -> Result<usize> {
    eprint!("{prompt} [{default}]: ");
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(default);
    }
    let value = input
        .parse::<usize>()
        .with_context(|| format!("invalid {prompt} selection `{input}`"))?;
    if value == 0 || value > max {
        bail!("{prompt} selection must be between 1 and {max}");
    }
    Ok(value)
}

fn run_openai_login_method(method: OpenAiLoginMethod) -> Result<()> {
    match method {
        OpenAiLoginMethod::Browser => run_djinn_openai_browser_login(),
        OpenAiLoginMethod::Headless => run_djinn_openai_device_login(),
        OpenAiLoginMethod::ApiKey => run_djinn_openai_api_key_login(),
    }
}

fn run_djinn_openai_api_key_login() -> Result<()> {
    println!("Save an OpenAI API key for Djinn.");
    println!(
        "The key will be stored in {} with owner-only permissions.",
        djinn_auth_path().display()
    );
    let api_key = read_secret_line("OpenAI API key: ")?.trim().to_string();
    if api_key.is_empty() {
        bail!("OpenAI API key cannot be empty");
    }
    write_djinn_openai_api_key(&api_key)?;
    println!("OpenAI API key saved for Djinn.");
    Ok(())
}

#[derive(Debug, Clone)]
struct OpenAiPkce {
    verifier: String,
    challenge: String,
}

fn run_djinn_openai_browser_login() -> Result<()> {
    println!("Starting Djinn OpenAI browser login.");
    let pkce = generate_openai_pkce()?;
    let state = random_base64_url(32)?;
    let redirect_uri = format!(
        "http://localhost:{}/auth/callback",
        OPENCODE_OPENAI_OAUTH_PORT
    );
    let listener = TcpListener::bind(("127.0.0.1", OPENCODE_OPENAI_OAUTH_PORT))
        .with_context(|| format!("binding OAuth callback server on {redirect_uri}"))?;
    listener
        .set_nonblocking(true)
        .with_context(|| "setting OAuth callback listener nonblocking")?;
    let url = openai_authorize_url(&redirect_uri, &pkce, &state);
    println!("Opening browser for OpenAI authorization…");
    println!("If it does not open, visit: {url}");
    let _ = open_url_in_browser(&url);
    let code = wait_for_openai_browser_callback(listener, &state)?;
    let tokens = exchange_openai_browser_authorization_code(&code, &redirect_uri, &pkce)?;
    let account_id = extract_account_id_from_tokens(&tokens);
    let oauth = OpenCodeOpenAiOAuthCredential {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: current_time_millis() + tokens.expires_in.unwrap_or(3600) * 1000,
        account_id,
    };
    write_djinn_openai_oauth(&oauth)?;
    println!("Login successful. Saved OpenAI OAuth credentials for Djinn.");
    Ok(())
}

fn wait_for_openai_browser_callback(listener: TcpListener, expected_state: &str) -> Result<String> {
    let started = Instant::now();
    let timeout = Duration::from_secs(10 * 60);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if started.elapsed() > timeout {
                    bail!("OpenAI OAuth browser authorization timed out");
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error).with_context(|| "waiting for OpenAI OAuth callback"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .with_context(|| "setting OAuth callback read timeout")?;
    let mut buffer = [0_u8; 8192];
    let read = stream
        .read(&mut buffer)
        .with_context(|| "reading OpenAI OAuth callback")?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or_default();
    let target = first_line.split_whitespace().nth(1).unwrap_or_default();
    let params = parse_query_params(target);
    let error = params
        .get("error_description")
        .or_else(|| params.get("error"))
        .cloned();
    if let Some(error) = error {
        let _ = write_oauth_callback_response(&mut stream, false, &error);
        bail!("OpenAI OAuth failed: {error}");
    }
    let code = params
        .get("code")
        .cloned()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("OpenAI OAuth callback did not include a code"));
    let state = params.get("state").map(String::as_str).unwrap_or_default();
    if state != expected_state {
        let _ = write_oauth_callback_response(&mut stream, false, "Invalid OAuth state");
        bail!("OpenAI OAuth callback state did not match");
    }
    let code = code?;
    let _ = write_oauth_callback_response(
        &mut stream,
        true,
        "Authorization complete. Return to Djinn.",
    );
    Ok(code)
}

fn write_oauth_callback_response(
    stream: &mut std::net::TcpStream,
    success: bool,
    message: &str,
) -> Result<()> {
    let title = if success {
        "Djinn authorization complete"
    } else {
        "Djinn authorization failed"
    };
    let body = format!(
        "<html><body><h1>{}</h1><p>{}</p></body></html>",
        html_escape(title),
        html_escape(message)
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

fn exchange_openai_browser_authorization_code(
    code: &str,
    redirect_uri: &str,
    pkce: &OpenAiPkce,
) -> Result<OpenCodeOpenAiTokenResponse> {
    let response = reqwest::blocking::Client::new()
        .post(format!("{OPENCODE_OPENAI_OAUTH_ISSUER}/oauth/token"))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", OPENCODE_OPENAI_OAUTH_CLIENT_ID),
            ("code_verifier", pkce.verifier.as_str()),
        ])
        .send()
        .with_context(|| "exchanging OpenAI browser authorization code")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading OpenAI browser token response")?;
    if !status.is_success() {
        bail!("OpenAI browser token exchange failed ({status}): {text}");
    }
    serde_json::from_str(&text)
        .with_context(|| format!("parsing OpenAI browser token response: {text}"))
}

fn generate_openai_pkce() -> Result<OpenAiPkce> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let random = random_bytes(43)?;
    let verifier = random
        .into_iter()
        .map(|byte| CHARS[byte as usize % CHARS.len()] as char)
        .collect::<String>();
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Ok(OpenAiPkce {
        verifier,
        challenge,
    })
}

fn random_base64_url(bytes: usize) -> Result<String> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes(bytes)?))
}

fn random_bytes(bytes: usize) -> Result<Vec<u8>> {
    let mut data = vec![0_u8; bytes];
    #[cfg(unix)]
    {
        let mut file = fs::File::open("/dev/urandom").with_context(|| "opening /dev/urandom")?;
        file.read_exact(&mut data)
            .with_context(|| "reading random bytes")?;
        return Ok(data);
    }
    #[cfg(not(unix))]
    {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for (idx, byte) in data.iter_mut().enumerate() {
            *byte = ((seed >> ((idx % 16) * 8)) & 0xff) as u8;
        }
        Ok(data)
    }
}

fn openai_authorize_url(redirect_uri: &str, pkce: &OpenAiPkce, state: &str) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", OPENCODE_OPENAI_OAUTH_CLIENT_ID),
        ("redirect_uri", redirect_uri),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", "opencode"),
    ];
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{OPENCODE_OPENAI_OAUTH_ISSUER}/oauth/authorize?{query}")
}

fn open_url_in_browser(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let status = if cfg!(target_os = "windows") {
        ProcessCommand::new(opener)
            .args(["/C", "start", "", url])
            .status()
    } else {
        ProcessCommand::new(opener).arg(url).status()
    }
    .with_context(|| format!("opening browser with `{opener}`"))?;
    if !status.success() {
        bail!("browser opener `{opener}` exited with {status}");
    }
    Ok(())
}

fn parse_query_params(target: &str) -> BTreeMap<String, String> {
    let query = target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'+' => {
                out.push(b' ');
                idx += 1;
            }
            b'%' if idx + 2 < bytes.len() => {
                let hex = &value[idx + 1..idx + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    idx += 3;
                } else {
                    out.push(bytes[idx]);
                    idx += 1;
                }
            }
            byte => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Debug, Deserialize)]
struct OpenAiDeviceAuthUserCodeResponse {
    device_auth_id: String,
    user_code: String,
    interval: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiDeviceAuthTokenResponse {
    authorization_code: String,
    code_verifier: String,
}

fn run_djinn_openai_device_login() -> Result<()> {
    println!("Starting Djinn OpenAI login.");
    let device = request_openai_device_auth_user_code()?;
    println!();
    println!("Open: {OPENCODE_OPENAI_OAUTH_ISSUER}/codex/device");
    println!("Enter code: {}", device.user_code);
    println!();
    println!("Waiting for authorization…");

    let interval = device
        .interval
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(5);
    let auth_code = poll_openai_device_auth_token(&device, interval)?;
    let tokens = exchange_openai_device_authorization_code(&auth_code)?;
    let account_id = extract_account_id_from_tokens(&tokens);
    let oauth = OpenCodeOpenAiOAuthCredential {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: current_time_millis() + tokens.expires_in.unwrap_or(3600) * 1000,
        account_id,
    };
    write_djinn_openai_oauth(&oauth)?;
    println!("Login successful. Saved OpenAI OAuth credentials for Djinn.");
    Ok(())
}

fn request_openai_device_auth_user_code() -> Result<OpenAiDeviceAuthUserCodeResponse> {
    let response = reqwest::blocking::Client::new()
        .post(format!(
            "{OPENCODE_OPENAI_OAUTH_ISSUER}/api/accounts/deviceauth/usercode"
        ))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, oauth_user_agent())
        .body(format!(
            r#"{{"client_id":"{OPENCODE_OPENAI_OAUTH_CLIENT_ID}"}}"#
        ))
        .send()
        .with_context(|| "starting OpenAI device authorization")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading OpenAI device authorization response")?;
    if !status.is_success() {
        bail!("OpenAI device authorization failed ({status}): {text}");
    }
    serde_json::from_str(&text)
        .with_context(|| format!("parsing OpenAI device authorization response: {text}"))
}

fn poll_openai_device_auth_token(
    device: &OpenAiDeviceAuthUserCodeResponse,
    interval_seconds: u64,
) -> Result<OpenAiDeviceAuthTokenResponse> {
    let started = SystemTime::now();
    let timeout = Duration::from_secs(10 * 60);
    loop {
        if started.elapsed().unwrap_or_default() > timeout {
            bail!("OpenAI device authorization timed out");
        }
        let response = reqwest::blocking::Client::new()
            .post(format!(
                "{OPENCODE_OPENAI_OAUTH_ISSUER}/api/accounts/deviceauth/token"
            ))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::USER_AGENT, oauth_user_agent())
            .body(format!(
                r#"{{"device_auth_id":"{}","user_code":"{}"}}"#,
                device.device_auth_id, device.user_code
            ))
            .send()
            .with_context(|| "polling OpenAI device authorization")?;
        let status = response.status();
        let text = response
            .text()
            .with_context(|| "reading OpenAI device authorization poll response")?;
        if status.is_success() {
            return serde_json::from_str(&text).with_context(|| {
                format!("parsing OpenAI device authorization poll response: {text}")
            });
        }
        if status.as_u16() != 403 && status.as_u16() != 404 {
            bail!("OpenAI device authorization failed ({status}): {text}");
        }
        print!(".");
        let _ = io::stdout().flush();
        thread::sleep(Duration::from_secs(interval_seconds).saturating_add(Duration::from_secs(3)));
    }
}

fn exchange_openai_device_authorization_code(
    auth: &OpenAiDeviceAuthTokenResponse,
) -> Result<OpenCodeOpenAiTokenResponse> {
    let response = reqwest::blocking::Client::new()
        .post(format!("{OPENCODE_OPENAI_OAUTH_ISSUER}/oauth/token"))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", auth.authorization_code.as_str()),
            (
                "redirect_uri",
                "https://auth.openai.com/deviceauth/callback",
            ),
            ("client_id", OPENCODE_OPENAI_OAUTH_CLIENT_ID),
            ("code_verifier", auth.code_verifier.as_str()),
        ])
        .send()
        .with_context(|| "exchanging OpenAI device authorization code")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading OpenAI device token response")?;
    if !status.is_success() {
        bail!("OpenAI device token exchange failed ({status}): {text}");
    }
    serde_json::from_str(&text)
        .with_context(|| format!("parsing OpenAI device token response: {text}"))
}

fn write_djinn_openai_oauth(oauth: &OpenCodeOpenAiOAuthCredential) -> Result<()> {
    let mut openai = Map::new();
    openai.insert("type".to_string(), Value::String("oauth".to_string()));
    openai.insert("access".to_string(), Value::String(oauth.access.clone()));
    openai.insert("refresh".to_string(), Value::String(oauth.refresh.clone()));
    openai.insert(
        "expires".to_string(),
        Value::Number(serde_json::Number::from(oauth.expires)),
    );
    if let Some(account_id) = &oauth.account_id {
        openai.insert("accountId".to_string(), Value::String(account_id.clone()));
    }
    write_djinn_openai_auth_value(Value::Object(openai))
}

fn write_djinn_openai_api_key(api_key: &str) -> Result<()> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        bail!("OpenAI API key cannot be empty");
    }
    let mut openai = Map::new();
    openai.insert("type".to_string(), Value::String("api".to_string()));
    openai.insert("key".to_string(), Value::String(api_key.to_string()));
    write_djinn_openai_auth_value(Value::Object(openai))
}

fn write_djinn_openai_auth_value(openai: Value) -> Result<()> {
    let path = djinn_auth_path();
    let mut value = if path.exists() {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading Djinn auth file {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("parsing Djinn auth file {}", path.display()))?
    } else {
        Value::Object(Map::new())
    };
    let Some(root) = value.as_object_mut() else {
        bail!("Djinn auth file root must be a JSON object");
    };
    root.insert("openai".to_string(), openai);

    djinn_core::ensure_parent(&path)?;
    let rendered = format!("{}\n", serde_json::to_string_pretty(&value)?);
    fs::write(&path, rendered)
        .with_context(|| format!("writing Djinn auth file {}", path.display()))?;
    set_owner_only_permissions(&path)?;
    Ok(())
}

fn read_secret_line(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let echo_disabled = disable_terminal_echo();
    let mut value = String::new();
    let result = io::stdin().read_line(&mut value);
    if echo_disabled {
        let _ = enable_terminal_echo();
        eprintln!();
    }
    result?;
    Ok(value)
}

#[cfg(unix)]
fn disable_terminal_echo() -> bool {
    if !io::stdin().is_terminal() {
        return false;
    }
    ProcessCommand::new("stty")
        .arg("-echo")
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn enable_terminal_echo() -> bool {
    ProcessCommand::new("stty")
        .arg("echo")
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn disable_terminal_echo() -> bool {
    false
}

#[cfg(not(unix))]
fn enable_terminal_echo() -> bool {
    false
}

fn djinn_auth_path() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| djinn_core::home_dir().join(".local").join("share"))
        .join("djinn")
        .join("auth.json")
}

fn oauth_user_agent() -> String {
    format!("djinn/{}", env!("CARGO_PKG_VERSION"))
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("reading permissions for {}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("setting permissions for {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn same_agent_option(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

fn agent_profile_options(current: &str) -> Result<Vec<String>> {
    let mut profiles = vec!["default".to_string(), current.trim().to_string()];
    let config = effective_djinn_config()?;
    if let Some(default_profile) = config.default_profile {
        profiles.push(default_profile);
    }
    profiles.extend(config.profiles.keys().cloned());
    profiles.extend(config.agents.keys().cloned());
    Ok(clean_unique_options(profiles))
}

fn agent_model_options(current: &str) -> Result<Vec<String>> {
    let mut models = vec![
        current.trim().to_string(),
        "gpt-4o-mini".to_string(),
        "copilot/gpt-4.1".to_string(),
    ];
    if let Ok(model) = env::var("DJINN_OPENAI_MODEL") {
        models.push(model);
    }
    if let Ok(model) = env::var("DJINN_COPILOT_MODEL") {
        models.push(model);
    }
    models.extend(copilot_model_options()?);
    let config = effective_djinn_config()?;
    for profile in config.profiles.values() {
        if let Some(model) = &profile.model {
            models.push(model.clone());
        }
    }
    for agent in config.agents.values() {
        if let Some(model) = &agent.model {
            models.push(model.clone());
        }
    }
    Ok(clean_unique_options(models))
}

fn format_agent_config_options(
    current_profile: &str,
    current_model: &str,
    profiles: &[String],
    models: &[String],
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(&serde_json::json!({
            "current_profile": current_profile,
            "current_model": current_model,
            "profiles": profiles,
            "models": models,
        }))?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Agent config options".to_string(),
        format!("Current profile: {current_profile}"),
        format!("Current model: {current_model}"),
        String::new(),
        "Profiles:".to_string(),
    ];
    for profile in profiles {
        let marker = if same_agent_option(profile, current_profile) {
            "*"
        } else {
            " "
        };
        lines.push(format!("{marker} {profile}"));
    }
    lines.push(String::new());
    lines.push("Models:".to_string());
    for model in models {
        let marker = if same_agent_option(model, current_model) {
            "*"
        } else {
            " "
        };
        lines.push(format!("{marker} {model}"));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn format_agent_effective_config(
    config: &AgentEffectiveConfig,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(config)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Agent effective config".to_string(),
        format!("Workspace: {}", config.workspace),
        format!(
            "Agent: {}",
            config.agent_name.as_deref().unwrap_or("<none>")
        ),
        format!("Profile: {}", config.profile),
        format!("Model: {}", config.model),
        String::new(),
        "Role instructions:".to_string(),
    ];
    if config.agent_instructions.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for instruction in &config.agent_instructions {
            lines.push(format!("  - {instruction}"));
        }
    }
    lines.push("Role tool allowlist:".to_string());
    if config.agent_tools.is_empty() {
        lines.push("  - all runtime tools".to_string());
    } else {
        for tool in &config.agent_tools {
            lines.push(format!("  - {tool}"));
        }
    }
    lines.push("Policy sources:".to_string());
    lines.push("  - built-in guardrails".to_string());
    if config.agent_name.is_some() {
        lines.push("  - selected agent role context".to_string());
    }
    if config.read_access_rules.is_empty() && config.permission_rules.is_empty() {
        lines.push("  - no native config permission rules".to_string());
    } else {
        for source in effective_policy_sources(config) {
            lines.push(format!("  - {source}"));
        }
    }
    lines.extend([String::new(), "Read access:".to_string()]);
    if config.read_access.allow_roots.is_empty()
        && config.read_access.deny_roots.is_empty()
        && config.read_access.rules.is_empty()
    {
        lines.push("  allow by default".to_string());
    } else {
        for root in &config.read_access.allow_roots {
            lines.push(format!("  allow root: {}", root.display()));
        }
        for root in &config.read_access.deny_roots {
            lines.push(format!("  deny root: {}", root.display()));
        }
        for rule in &config.read_access.rules {
            lines.push(format!("  {:?}: {}", rule.effect, rule.pattern));
        }
    }
    if !config.read_access_rules.is_empty() {
        lines.push("  Sources:".to_string());
        for rule in &config.read_access_rules {
            lines.push(format!(
                "    {}: {} {} {}",
                rule.source, rule.effect, rule.action, rule.resource
            ));
        }
    }
    lines.push(String::new());
    lines.push("Permissions:".to_string());
    if config.permissions.rules.is_empty() {
        lines.push("  allow by default with destructive-action guardrails".to_string());
    } else {
        for rule in &config.permissions.rules {
            lines.push(format!(
                "  {:?}: {} {}",
                rule.effect, rule.action, rule.resource
            ));
        }
        lines.push("  destructive-action guardrails always apply".to_string());
    }
    if !config.permission_rules.is_empty() {
        lines.push("  Sources:".to_string());
        for rule in &config.permission_rules {
            lines.push(format!(
                "    {}: {} {} {}",
                rule.source, rule.effect, rule.action, rule.resource
            ));
        }
    }
    lines.push("Guardrails:".to_string());
    for guardrail in &config.guardrails {
        lines.push(format!("  - {guardrail}"));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn effective_policy_sources(config: &AgentEffectiveConfig) -> Vec<String> {
    let mut sources = Vec::new();
    for rule in config
        .read_access_rules
        .iter()
        .chain(config.permission_rules.iter())
    {
        push_unique_string(&mut sources, &rule.source);
    }
    sources
}

fn format_agent_tool_specs(specs: &[ToolSpec], format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(specs)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Agent runtime tools".to_string(),
        format!("{} tool{}", specs.len(), plural_suffix(specs.len())),
        String::new(),
    ];
    for spec in specs {
        lines.push(format!("- {}", spec.name));
        let summary = spec
            .description
            .split('.')
            .next()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .unwrap_or(&spec.description);
        if !summary.trim().is_empty() {
            lines.push(format!("  {summary}."));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn resolve_agent_tool_spec<'a>(specs: &'a [ToolSpec], name: &str) -> Result<&'a ToolSpec> {
    let name = name.trim();
    if name.is_empty() {
        bail!("agent tool name cannot be empty");
    }
    if let Some(spec) = specs
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(name))
    {
        return Ok(spec);
    }
    let lowered = name.to_ascii_lowercase();
    let matches = specs
        .iter()
        .filter(|spec| spec.name.to_ascii_lowercase().contains(&lowered))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [spec] => Ok(spec),
        [] => bail!("unknown agent tool `{name}`"),
        _ => bail!(
            "ambiguous agent tool `{name}`; matches: {}",
            matches
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn format_agent_tool_spec(spec: &ToolSpec, format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(spec)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let schema = serde_json::to_string_pretty(&spec.input_schema)?;
    let mut lines = vec![
        spec.name.clone(),
        String::new(),
        spec.description.clone(),
        String::new(),
        "Input schema:".to_string(),
        schema,
    ];
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn clean_unique_options(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for value in values.into_iter().map(|value| value.trim().to_string()) {
        if value.is_empty() || !seen.insert(value.clone()) {
            continue;
        }
        out.push(value);
    }
    out
}

fn copilot_model_options() -> Result<Vec<String>> {
    let mut models = Vec::new();
    for name in [
        "DJINN_COPILOT_MODEL",
        "GITHUB_COPILOT_MODEL",
        "COPILOT_MODEL",
    ] {
        if let Ok(model) = env::var(name) {
            if let Some(model) = copilot_model_option_from_str(&model) {
                models.push(model);
            }
        }
    }
    for name in [
        "DJINN_COPILOT_MODELS",
        "GITHUB_COPILOT_MODELS",
        "COPILOT_MODELS",
    ] {
        if let Ok(value) = env::var(name) {
            models.extend(copilot_model_options_from_list(&value));
        }
    }
    models.extend(copilot_model_options_from_local_config()?);
    Ok(clean_unique_options(models))
}

fn copilot_model_options_from_local_config() -> Result<Vec<String>> {
    let mut models = Vec::new();
    for path in copilot_model_config_paths() {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading GitHub Copilot config {}", path.display()))?;
        models.extend(
            copilot_model_options_from_content(&content)
                .with_context(|| format!("parsing GitHub Copilot config {}", path.display()))?,
        );
    }
    Ok(clean_unique_options(models))
}

fn copilot_model_config_paths() -> Vec<PathBuf> {
    let mut paths = copilot_auth_paths();
    for root in copilot_config_roots() {
        paths.push(root.join("models.json"));
        paths.push(root.join("config.json"));
    }
    clean_unique_paths(paths)
}

fn default_copilot_config_path() -> PathBuf {
    copilot_config_roots()
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            djinn_core::home_dir()
                .join(".config")
                .join("github-copilot")
        })
        .join("config.json")
}

fn copilot_config_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(xdg_config) = env::var_os("XDG_CONFIG_HOME") {
        roots.push(PathBuf::from(xdg_config).join("github-copilot"));
    }
    roots.push(
        djinn_core::home_dir()
            .join(".config")
            .join("github-copilot"),
    );
    clean_unique_paths(roots)
}

fn clean_unique_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in paths {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

fn copilot_model_options_from_content(content: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(content)?;
    Ok(copilot_model_options_from_value(&value))
}

fn copilot_model_options_from_value(value: &Value) -> Vec<String> {
    let mut models = Vec::new();
    collect_copilot_model_options(value, false, &mut models);
    clean_unique_options(models)
}

fn copilot_model_options_from_list(value: &str) -> Vec<String> {
    clean_unique_options(
        value
            .split([',', ';', '\n'])
            .filter_map(copilot_model_option_from_str)
            .collect(),
    )
}

fn collect_copilot_model_options(value: &Value, model_context: bool, out: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for key in [
                "model",
                "model_id",
                "modelId",
                "selected_model",
                "selectedModel",
                "default_model",
                "defaultModel",
            ] {
                if let Some(model) = object
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(copilot_model_option_from_str)
                {
                    out.push(model);
                }
            }

            for key in [
                "models",
                "available_models",
                "availableModels",
                "chat_models",
                "chatModels",
                "model_choices",
                "modelChoices",
                "custom_models",
                "customModels",
            ] {
                if let Some(value) = object.get(key) {
                    collect_copilot_model_options(value, true, out);
                }
            }

            if model_context {
                for key in ["id", "name", "slug"] {
                    if let Some(model) = object
                        .get(key)
                        .and_then(Value::as_str)
                        .and_then(copilot_model_option_from_str)
                    {
                        out.push(model);
                    }
                }
                for (key, value) in object {
                    if let Some(model) = copilot_model_option_from_str(key) {
                        out.push(model);
                    }
                    collect_copilot_model_options(value, true, out);
                }
            } else {
                for value in object.values() {
                    collect_copilot_model_options(value, false, out);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_copilot_model_options(value, model_context, out);
            }
        }
        Value::String(value) if model_context => {
            if let Some(model) = copilot_model_option_from_str(value) {
                out.push(model);
            }
        }
        _ => {}
    }
}

fn copilot_model_option_from_str(model: &str) -> Option<String> {
    let model = model.trim().trim_matches('"').trim_matches('\'').trim();
    if !looks_like_copilot_model_id(model) {
        return None;
    }
    if is_copilot_model(model) {
        Some(model.to_string())
    } else {
        Some(format!("copilot/{model}"))
    }
}

fn looks_like_copilot_model_id(model: &str) -> bool {
    if model.is_empty() || model.len() > 120 {
        return false;
    }
    let lower = model.to_ascii_lowercase();
    if lower.contains("gemini")
        || lower.contains("token")
        || lower.starts_with("gho_")
        || lower.starts_with("ghu_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("sk-")
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.contains('@')
        || lower.chars().any(char::is_whitespace)
    {
        return false;
    }
    lower.contains("gpt")
        || lower.contains("claude")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.starts_with("o5")
        || lower.contains("/o1")
        || lower.contains("/o3")
        || lower.contains("/o4")
        || lower.contains("/o5")
}

fn complete_openai_messages_with_progress<F>(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    messages: Vec<ModelMessage>,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    max_tool_rounds: usize,
    profile: &str,
    allowed_tools: Vec<String>,
    interactive_permissions: bool,
    on_progress: F,
) -> Result<djinn_agent::ModelResponse>
where
    F: FnMut(AgentProgressEvent) -> Result<()>,
{
    if is_copilot_model(&model) {
        let token = resolve_copilot_token(api_key)?;
        let endpoint = base_url
            .or_else(|| env::var("GITHUB_COPILOT_CHAT_COMPLETIONS_URL").ok())
            .unwrap_or_else(|| "https://api.githubcopilot.com/chat/completions".to_string());
        let client = CopilotClient::with_endpoint(token, endpoint);
        return complete_messages_with_client(
            store,
            id,
            messages,
            model,
            max_tool_rounds,
            profile,
            allowed_tools,
            interactive_permissions,
            client,
            on_progress,
        );
    }

    let client = resolve_openai_client(api_key, base_url)?;
    complete_messages_with_client(
        store,
        id,
        messages,
        model,
        max_tool_rounds,
        profile,
        allowed_tools,
        interactive_permissions,
        client,
        on_progress,
    )
}

fn complete_messages_with_client<M, F>(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    messages: Vec<ModelMessage>,
    model: String,
    max_tool_rounds: usize,
    profile: &str,
    allowed_tools: Vec<String>,
    interactive_permissions: bool,
    client: M,
    mut on_progress: F,
) -> Result<djinn_agent::ModelResponse>
where
    M: ModelClient + 'static,
    F: FnMut(AgentProgressEvent) -> Result<()>,
{
    let workspace = store.load_session(id)?.meta.workspace;
    let read_access = resolve_agent_read_access_policy(profile, Path::new(&workspace))?;
    let permissions = resolve_agent_permission_policy(profile, Path::new(&workspace))?;
    let file_history = Arc::new(JsonlFileHistoryStore::default_in(
        &djinn_core::default_data_dir(),
    ));
    let permission_gate: Option<Arc<dyn PermissionGate>> = if interactive_permissions
        && io::stdin().is_terminal()
        && (io::stdout().is_terminal() || io::stderr().is_terminal())
    {
        Some(Arc::new(TerminalPermissionGate::new()))
    } else {
        None
    };
    let mut registry = tools_with_policies_file_history_and_gate(
        workspace.clone(),
        read_access,
        permissions,
        Some(file_history),
        permission_gate,
    )?;
    registry.retain_names(&allowed_tools)?;
    let runtime = AgentRuntime::new(client, store.clone(), registry);
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .with_context(|| "creating Tokio runtime for agent request")?;
    tokio.block_on(runtime.complete_with_tools_and_progress(
        id,
        ModelRequest {
            model,
            messages,
            tools: Vec::new(),
        },
        max_tool_rounds,
        |event| on_progress(event),
    ))
}

fn resolve_openai_client(
    api_key: Option<String>,
    base_url: Option<String>,
) -> Result<OpenAiClient> {
    let auth = resolve_openai_auth(api_key)?;
    Ok(match auth {
        OpenAiAuth::ApiKey(api_key) => {
            let base_url = base_url
                .or_else(|| env::var("OPENAI_BASE_URL").ok())
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            OpenAiClient::with_base_url(api_key, base_url)
        }
        OpenAiAuth::OAuth(oauth) => OpenAiClient::with_oauth(oauth),
    })
}

fn is_copilot_model(model: &str) -> bool {
    let model = model.trim();
    model.starts_with("copilot/") || model.starts_with("github-copilot/")
}

#[derive(Debug, Deserialize)]
struct CopilotInternalTokenResponse {
    token: String,
}

fn resolve_copilot_token(explicit: Option<String>) -> Result<String> {
    if let Some(token) = explicit
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
    {
        return Ok(token);
    }
    for name in [
        "DJINN_COPILOT_TOKEN",
        "GITHUB_COPILOT_TOKEN",
        "COPILOT_TOKEN",
    ] {
        if let Ok(token) = env::var(name) {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
    }
    for name in ["DJINN_COPILOT_OAUTH_TOKEN", "GITHUB_COPILOT_OAUTH_TOKEN"] {
        if let Ok(token) = env::var(name) {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return exchange_copilot_oauth_token(&token);
            }
        }
    }
    if let Some(oauth_token) = copilot_oauth_token_from_local_config()? {
        return exchange_copilot_oauth_token(&oauth_token);
    }
    if let Some(oauth_token) = github_cli_auth_token()? {
        return exchange_copilot_oauth_token(&oauth_token);
    }
    Err(anyhow::anyhow!(
        "GitHub Copilot auth is required for copilot/* models; pass --api-key with a Copilot API token, set GITHUB_COPILOT_TOKEN, connect GitHub Copilot so ~/.config/github-copilot/hosts.json or apps.json contains an OAuth token, or authenticate the GitHub CLI so `gh auth token` works"
    ))
}

fn exchange_copilot_oauth_token(oauth_token: &str) -> Result<String> {
    let url = env::var("GITHUB_COPILOT_TOKEN_URL")
        .unwrap_or_else(|_| "https://api.github.com/copilot_internal/v2/token".to_string());
    let response = reqwest::blocking::Client::new()
        .get(url)
        .bearer_auth(oauth_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, "djinn-agent")
        .send()
        .with_context(|| "exchanging GitHub Copilot OAuth token")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading GitHub Copilot token response")?;
    if !status.is_success() {
        bail!("GitHub Copilot token exchange failed ({status})");
    }
    let token: CopilotInternalTokenResponse =
        serde_json::from_str(&text).with_context(|| "parsing GitHub Copilot token response")?;
    let token = token.token.trim().to_string();
    if token.is_empty() {
        bail!("GitHub Copilot token response did not include a token");
    }
    Ok(token)
}

fn copilot_oauth_token_from_local_config() -> Result<Option<String>> {
    for path in copilot_auth_paths() {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading GitHub Copilot auth file {}", path.display()))?;
        if let Some(token) = copilot_oauth_token_from_content(&content)
            .with_context(|| format!("parsing GitHub Copilot auth file {}", path.display()))?
        {
            return Ok(Some(token));
        }
    }
    Ok(None)
}

fn github_cli_auth_token() -> Result<Option<String>> {
    let gh = env::var_os("DJINN_GH_BIN").unwrap_or_else(|| "gh".into());
    let output = match ProcessCommand::new(gh)
        .arg("auth")
        .arg("token")
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| "running `gh auth token`"),
    };
    if !output.status.success() {
        return Ok(None);
    }
    Ok(github_cli_auth_token_from_stdout(&output.stdout))
}

fn github_cli_auth_token_from_stdout(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn copilot_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in copilot_config_roots() {
        paths.push(root.join("hosts.json"));
        paths.push(root.join("apps.json"));
    }
    clean_unique_paths(paths)
}

fn copilot_oauth_token_from_content(content: &str) -> Result<Option<String>> {
    let value: Value = serde_json::from_str(content)?;
    Ok(find_json_string_by_keys(
        &value,
        &["oauth_token", "oauthToken"],
    ))
}

fn find_json_string_by_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(value) = object
                    .get(*key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    return Some(value.to_string());
                }
            }
            object
                .values()
                .find_map(|value| find_json_string_by_keys(value, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_json_string_by_keys(value, keys)),
        _ => None,
    }
}

#[allow(dead_code)]
const OPENCODE_OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
#[allow(dead_code)]
const OPENCODE_OPENAI_OAUTH_ISSUER: &str = "https://auth.openai.com";
#[allow(dead_code)]
const OPENCODE_OPENAI_CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENCODE_OPENAI_OAUTH_PORT: u16 = 1455;

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenCodeOpenAiOAuthCredential {
    access: String,
    refresh: String,
    expires: i64,
    account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenCodeOpenAiAuthCredential {
    ApiKey(String),
    OAuth(OpenCodeOpenAiOAuthCredential),
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenCodeOpenAiTokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    refresh_token: String,
    expires_in: Option<i64>,
}

fn resolve_openai_auth(explicit: Option<String>) -> Result<OpenAiAuth> {
    if let Some(api_key) = explicit
        .map(|api_key| api_key.trim().to_string())
        .filter(|api_key| !api_key.is_empty())
    {
        return Ok(OpenAiAuth::ApiKey(api_key));
    }
    if let Ok(api_key) = env::var("OPENAI_API_KEY") {
        let api_key = api_key.trim().to_string();
        if !api_key.is_empty() {
            return Ok(OpenAiAuth::ApiKey(api_key));
        }
    }
    if let Some(auth) = djinn_config_openai_auth()? {
        return Ok(auth);
    }
    if let Some(auth) = djinn_auth_openai_auth()? {
        return Ok(auth);
    }
    if let Some(auth) = opencode_auth_openai_auth()? {
        return Ok(auth);
    }
    Err(anyhow::anyhow!(
        "OpenAI auth is required; use / then `Add credential…`, run `djinn auth login`, pass --api-key, set OPENAI_API_KEY, or configure providers.openai.auth in Djinn config"
    ))
}

fn djinn_config_openai_auth() -> Result<Option<OpenAiAuth>> {
    let config = effective_djinn_config()?;
    let Some(provider) = config.providers.get("openai") else {
        return Ok(None);
    };
    let Some(auth) = provider
        .auth
        .as_deref()
        .map(str::trim)
        .filter(|auth| !auth.is_empty())
    else {
        return Ok(None);
    };
    if let Some(name) = auth.strip_prefix("env:") {
        let name = name.trim();
        if !name.is_empty() {
            if let Ok(api_key) = env::var(name) {
                let api_key = api_key.trim().to_string();
                if !api_key.is_empty() {
                    return Ok(Some(OpenAiAuth::ApiKey(api_key)));
                }
            }
        }
        return Ok(None);
    }
    if auth == "auto" {
        return Ok(None);
    }
    if auth.starts_with("opencode:") {
        bail!(
            "providers.openai.auth references OpenCode config; run import to migrate to a Djinn-owned env: reference instead"
        );
    }
    Ok(Some(OpenAiAuth::ApiKey(auth.to_string())))
}

fn djinn_auth_openai_auth() -> Result<Option<OpenAiAuth>> {
    let path = djinn_auth_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading Djinn auth file {}", path.display()))?;
    let Some(auth) = opencode_auth_openai_auth_from_content(&content)
        .with_context(|| format!("parsing Djinn auth file {}", path.display()))?
    else {
        return Ok(None);
    };
    opencode_auth_credential_to_openai_auth(auth, Some((&path, &content))).map(Some)
}

#[allow(dead_code)]
fn opencode_openai_api_key() -> Result<Option<String>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    opencode_openai_api_key_from_paths(&opencode_model_config_paths(&cwd))
}

fn opencode_openai_api_key_from_paths(paths: &[PathBuf]) -> Result<Option<String>> {
    for path in paths {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        if let Some(api_key) = opencode_openai_api_key_from_content(&content)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?
        {
            return Ok(Some(api_key));
        }
    }
    Ok(None)
}

fn opencode_openai_api_key_from_content(content: &str) -> Result<Option<String>> {
    let value: Value = serde_json::from_str(content)?;
    Ok(value
        .pointer("/providers/openai/apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())
        .map(ToOwned::to_owned))
}

#[allow(dead_code)]
fn opencode_auth_openai_auth() -> Result<Option<OpenAiAuth>> {
    if let Ok(content) = env::var("OPENCODE_AUTH_CONTENT") {
        if let Some(auth) = opencode_auth_openai_auth_from_content(&content)
            .with_context(|| "parsing OPENCODE_AUTH_CONTENT")?
        {
            return opencode_auth_credential_to_openai_auth(auth, None).map(Some);
        }
    }

    let path = opencode_auth_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading OpenCode auth file {}", path.display()))?;
    let Some(auth) = opencode_auth_openai_auth_from_content(&content)
        .with_context(|| format!("parsing OpenCode auth file {}", path.display()))?
    else {
        return Ok(None);
    };
    opencode_auth_credential_to_openai_auth(auth, Some((&path, &content))).map(Some)
}

#[allow(dead_code)]
fn opencode_auth_path() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| djinn_core::home_dir().join(".local").join("share"))
        .join("opencode")
        .join("auth.json")
}

#[cfg(test)]
fn opencode_auth_openai_api_key_from_content(content: &str) -> Result<Option<String>> {
    Ok(match opencode_auth_openai_auth_from_content(content)? {
        Some(OpenCodeOpenAiAuthCredential::ApiKey(api_key)) => Some(api_key),
        Some(OpenCodeOpenAiAuthCredential::OAuth(_)) | None => None,
    })
}

fn opencode_auth_openai_auth_from_content(
    content: &str,
) -> Result<Option<OpenCodeOpenAiAuthCredential>> {
    let value: Value = serde_json::from_str(content)?;
    let Some(openai) = value.pointer("/openai").and_then(Value::as_object) else {
        return Ok(None);
    };
    match openai.get("type").and_then(Value::as_str) {
        Some("api") => Ok(openai
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|api_key| !api_key.is_empty())
            .map(ToOwned::to_owned)
            .map(OpenCodeOpenAiAuthCredential::ApiKey)),
        Some("oauth") => {
            let access = openai
                .get("access")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let refresh = openai
                .get("refresh")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if access.is_empty() && refresh.is_empty() {
                bail!("OpenCode OpenAI OAuth credential is missing both access and refresh tokens");
            }
            let expires = openai
                .get("expires")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let account_id = openai
                .get("accountId")
                .or_else(|| openai.get("account_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|account_id| !account_id.is_empty())
                .map(ToOwned::to_owned);
            Ok(Some(OpenCodeOpenAiAuthCredential::OAuth(
                OpenCodeOpenAiOAuthCredential {
                    access,
                    refresh,
                    expires,
                    account_id,
                },
            )))
        }
        Some(other) => Err(anyhow::anyhow!(
            "unsupported OpenCode OpenAI auth type `{other}`; expected `api` or `oauth`"
        )),
        None => Ok(None),
    }
}

#[allow(dead_code)]
fn opencode_auth_credential_to_openai_auth(
    auth: OpenCodeOpenAiAuthCredential,
    source: Option<(&Path, &str)>,
) -> Result<OpenAiAuth> {
    match auth {
        OpenCodeOpenAiAuthCredential::ApiKey(api_key) => Ok(OpenAiAuth::ApiKey(api_key)),
        OpenCodeOpenAiAuthCredential::OAuth(oauth) => {
            let oauth = if oauth_access_token_is_current(&oauth) {
                oauth
            } else {
                let (path, content) = source.ok_or_else(|| {
                    anyhow::anyhow!(
                        "OpenCode OpenAI OAuth access token is expired and cannot be refreshed from OPENCODE_AUTH_CONTENT; use the auth file or pass --api-key"
                    )
                })?;
                refresh_opencode_openai_oauth(path, content, &oauth)?
            };
            Ok(OpenAiAuth::OAuth(OpenAiOAuth {
                access: oauth.access,
                account_id: oauth.account_id,
                codex_api_endpoint: OPENCODE_OPENAI_CODEX_API_ENDPOINT.to_string(),
            }))
        }
    }
}

#[allow(dead_code)]
fn oauth_access_token_is_current(oauth: &OpenCodeOpenAiOAuthCredential) -> bool {
    !oauth.access.is_empty() && oauth.expires > current_time_millis()
}

#[allow(dead_code)]
fn refresh_opencode_openai_oauth(
    path: &Path,
    content: &str,
    current: &OpenCodeOpenAiOAuthCredential,
) -> Result<OpenCodeOpenAiOAuthCredential> {
    if current.refresh.is_empty() {
        bail!("OpenCode OpenAI OAuth access token is expired and no refresh token is available");
    }

    let tokens = refresh_openai_oauth_token(&current.refresh)?;
    let account_id = extract_account_id_from_tokens(&tokens).or_else(|| current.account_id.clone());
    let refreshed = OpenCodeOpenAiOAuthCredential {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: current_time_millis() + tokens.expires_in.unwrap_or(3600) * 1000,
        account_id,
    };
    write_refreshed_opencode_openai_oauth(path, content, &refreshed)?;
    Ok(refreshed)
}

#[allow(dead_code)]
fn refresh_openai_oauth_token(refresh_token: &str) -> Result<OpenCodeOpenAiTokenResponse> {
    let response = reqwest::blocking::Client::new()
        .post(format!("{OPENCODE_OPENAI_OAUTH_ISSUER}/oauth/token"))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", OPENCODE_OPENAI_OAUTH_CLIENT_ID),
        ])
        .send()
        .with_context(|| "refreshing OpenCode OpenAI OAuth token")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading OpenCode OpenAI OAuth refresh response")?;
    if !status.is_success() {
        bail!("OpenCode OpenAI OAuth token refresh failed ({status}): {text}");
    }
    serde_json::from_str(&text)
        .with_context(|| format!("parsing OpenCode OpenAI OAuth refresh response: {text}"))
}

fn write_refreshed_opencode_openai_oauth(
    path: &Path,
    content: &str,
    refreshed: &OpenCodeOpenAiOAuthCredential,
) -> Result<()> {
    let mut value: Value = serde_json::from_str(content)?;
    let Some(root) = value.as_object_mut() else {
        bail!("OpenCode auth file root must be a JSON object");
    };
    let openai = root
        .entry("openai".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(openai) = openai.as_object_mut() else {
        bail!("OpenCode auth file openai entry must be a JSON object");
    };
    openai.insert("type".to_string(), Value::String("oauth".to_string()));
    openai.insert(
        "access".to_string(),
        Value::String(refreshed.access.clone()),
    );
    openai.insert(
        "refresh".to_string(),
        Value::String(refreshed.refresh.clone()),
    );
    openai.insert(
        "expires".to_string(),
        Value::Number(serde_json::Number::from(refreshed.expires)),
    );
    if let Some(account_id) = &refreshed.account_id {
        openai.insert("accountId".to_string(), Value::String(account_id.clone()));
    }

    let rendered = format!("{}\n", serde_json::to_string_pretty(&value)?);
    fs::write(path, rendered)
        .with_context(|| format!("writing OpenCode auth file {}", path.display()))
}

#[allow(dead_code)]
fn extract_account_id_from_tokens(tokens: &OpenCodeOpenAiTokenResponse) -> Option<String> {
    tokens
        .id_token
        .as_deref()
        .and_then(extract_account_id_from_jwt)
        .or_else(|| extract_account_id_from_jwt(&tokens.access_token))
}

fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("chatgpt_account_id")
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|auth| auth.get("chatgpt_account_id"))
        })
        .or_else(|| claims.get("organizations")?.as_array()?.first()?.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
        .map(ToOwned::to_owned)
}

fn current_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn resolve_agent_model(explicit: Option<String>, profile: &str) -> Result<String> {
    let config = effective_djinn_config()?;
    Ok(resolve_agent_model_from_config(explicit, &config, profile))
}

fn resolve_agent_model_from_config(
    explicit: Option<String>,
    config: &DjinnConfig,
    profile: &str,
) -> String {
    if let Some(model) = explicit
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
    {
        return model;
    }
    if let Some(model) = profile_model_from_config(config, profile) {
        return model;
    }
    for name in [
        "DJINN_AGENT_MODEL",
        "DJINN_COPILOT_MODEL",
        "DJINN_OPENAI_MODEL",
    ] {
        let Ok(model) = env::var(name) else {
            continue;
        };
        let model = model.trim().to_string();
        if !model.is_empty() {
            return model;
        }
    }
    "gpt-4o-mini".to_string()
}

fn resolve_agent_profile(requested: &str) -> Result<String> {
    let requested = requested.trim();
    if !requested.is_empty() && requested != "default" {
        return Ok(requested.to_string());
    }
    let config = effective_djinn_config()?;
    Ok(resolve_agent_profile_from_config(&config, requested))
}

fn resolve_agent_profile_from_config(config: &DjinnConfig, requested: &str) -> String {
    let requested = requested.trim();
    if !requested.is_empty() && requested != "default" {
        return requested.to_string();
    }
    config
        .default_profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .unwrap_or(if requested.is_empty() {
            "default"
        } else {
            requested
        })
        .to_string()
}

fn profile_model_from_config(config: &DjinnConfig, profile: &str) -> Option<String> {
    config
        .profiles
        .get(profile)
        .and_then(|profile| profile.model.as_deref())
        .or_else(|| {
            config
                .agents
                .get(profile)
                .and_then(|agent| agent.model.as_deref())
        })
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

#[allow(dead_code)]
fn opencode_default_model(profile: &str) -> Result<Option<String>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    opencode_default_model_from_paths(&opencode_model_config_paths(&cwd), profile)
}

fn opencode_model_config_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.push(cwd.join(".opencode.json"));
    paths.push(default_opencode_config_path());
    paths.push(
        djinn_core::home_dir()
            .join(".config")
            .join("opencode")
            .join(".opencode.json"),
    );
    if let Some(xdg_config) = env::var_os("XDG_CONFIG_HOME") {
        paths.push(
            PathBuf::from(xdg_config)
                .join("opencode")
                .join(".opencode.json"),
        );
    }
    paths.push(djinn_core::home_dir().join(".opencode.json"));
    paths
}

fn opencode_default_model_from_paths(paths: &[PathBuf], profile: &str) -> Result<Option<String>> {
    for path in paths {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        if let Some(model) = opencode_default_model_from_content(&content, profile)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?
        {
            return Ok(Some(model));
        }
    }
    Ok(None)
}

fn opencode_default_model_from_content(content: &str, profile: &str) -> Result<Option<String>> {
    let value: Value = serde_json::from_str(content)?;

    let profile = profile.trim();
    if !profile.is_empty() && profile != "default" {
        if let Some(model) = opencode_agent_model(&value, profile) {
            return Ok(Some(model));
        }
    }

    if let Some(default_agent) = value
        .get("default_agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    {
        if let Some(model) = opencode_agent_model(&value, default_agent) {
            return Ok(Some(model));
        }
    }

    for agent in ["coder", "default"] {
        if let Some(model) = opencode_agent_model(&value, agent) {
            return Ok(Some(model));
        }
    }

    for pointer in ["/agent/model", "/model"] {
        if let Some(model) = json_string_pointer(&value, pointer) {
            return Ok(Some(model));
        }
    }
    Ok(None)
}

fn opencode_agent_model(value: &Value, agent: &str) -> Option<String> {
    ["agent", "agents"].into_iter().find_map(|container| {
        value
            .get(container)
            .and_then(Value::as_object)
            .and_then(|agents| agents.get(agent))
            .and_then(|agent| agent.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn json_string_pointer(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_agent_read_access_policy(profile: &str, workspace: &Path) -> Result<ReadAccessPolicy> {
    let mut policy = ReadAccessPolicy::lax(workspace);
    policy
        .rules
        .extend(djinn_config_read_access_rules(profile, workspace)?);
    Ok(policy)
}

fn resolve_agent_permission_policy(profile: &str, workspace: &Path) -> Result<PermissionPolicy> {
    let mut policy = PermissionPolicy::allow_by_default();
    policy
        .rules
        .extend(djinn_config_permission_rules(profile, workspace)?);
    Ok(policy)
}

fn effective_read_access_rules_with_sources(
    profile: &str,
    workspace: &Path,
) -> Result<Vec<AgentEffectivePolicyRule>> {
    let config = effective_djinn_config()?;
    let mut rules = Vec::new();
    extend_effective_policy_rules(
        "shared permissions",
        &config.permissions,
        workspace,
        &mut rules,
        true,
    );
    if let Some(profile_config) = config.profiles.get(profile) {
        extend_effective_policy_rules(
            &format!("profile:{profile}"),
            &profile_config.permissions,
            workspace,
            &mut rules,
            true,
        );
    }
    Ok(rules)
}

fn effective_permission_rules_with_sources(
    profile: &str,
    workspace: &Path,
) -> Result<Vec<AgentEffectivePolicyRule>> {
    let config = effective_djinn_config()?;
    let mut rules = Vec::new();
    extend_effective_policy_rules(
        "shared permissions",
        &config.permissions,
        workspace,
        &mut rules,
        false,
    );
    if let Some(profile_config) = config.profiles.get(profile) {
        extend_effective_policy_rules(
            &format!("profile:{profile}"),
            &profile_config.permissions,
            workspace,
            &mut rules,
            false,
        );
    }
    Ok(rules)
}

fn extend_effective_policy_rules(
    source: &str,
    permissions: &[DjinnConfigPermission],
    workspace: &Path,
    out: &mut Vec<AgentEffectivePolicyRule>,
    read_access_only: bool,
) {
    for permission in permissions {
        let action = permission.action.trim();
        let is_read_access = action == "read" || action == "*" || action == "external_directory";
        if read_access_only != is_read_access {
            continue;
        }
        out.push(AgentEffectivePolicyRule {
            source: source.to_string(),
            action: permission.action.trim().to_string(),
            resource: config_permission_pattern(&permission.resource, workspace),
            effect: permission.effect.trim().to_string(),
        });
    }
}

fn agent_policy_guardrails() -> Vec<String> {
    vec![
        "secret-read guardrails block known credential/token/key/auth paths".to_string(),
        "destructive shell/git guardrails always apply before policy rules".to_string(),
        "sensitive/system path mutation guardrails always apply".to_string(),
        "session approvals are action-, workspace-, and resource/path-scoped".to_string(),
    ]
}

fn djinn_config_read_access_rules(profile: &str, workspace: &Path) -> Result<Vec<ReadAccessRule>> {
    let config = effective_djinn_config()?;
    let mut rules = Vec::new();
    extend_read_access_rules_from_permissions(&config.permissions, workspace, &mut rules);
    if let Some(profile) = config.profiles.get(profile) {
        extend_read_access_rules_from_permissions(&profile.permissions, workspace, &mut rules);
    }
    Ok(rules)
}

fn djinn_config_permission_rules(profile: &str, workspace: &Path) -> Result<Vec<PermissionRule>> {
    let config = effective_djinn_config()?;
    let mut rules = Vec::new();
    extend_permission_rules_from_config(&config.permissions, workspace, &mut rules);
    if let Some(profile) = config.profiles.get(profile) {
        extend_permission_rules_from_config(&profile.permissions, workspace, &mut rules);
    }
    Ok(rules)
}

fn extend_read_access_rules_from_permissions(
    permissions: &[DjinnConfigPermission],
    workspace: &Path,
    out: &mut Vec<ReadAccessRule>,
) {
    for permission in permissions {
        let action = permission.action.trim();
        if action != "read" && action != "*" && action != "external_directory" {
            continue;
        }
        if let Some(effect) = djinn_config_read_access_effect(&permission.effect) {
            out.push(ReadAccessRule {
                pattern: config_permission_pattern(&permission.resource, workspace),
                effect,
            });
        }
    }
}

fn extend_permission_rules_from_config(
    permissions: &[DjinnConfigPermission],
    workspace: &Path,
    out: &mut Vec<PermissionRule>,
) {
    for permission in permissions {
        if let Some(effect) = djinn_config_permission_effect(&permission.effect) {
            out.push(PermissionRule {
                action: config_permission_action(&permission.action),
                resource: config_permission_pattern(&permission.resource, workspace),
                effect,
            });
        }
    }
}

fn djinn_config_read_access_effect(effect: &str) -> Option<ReadAccessEffect> {
    match effect.trim() {
        "allow" => Some(ReadAccessEffect::Allow),
        "ask" => Some(ReadAccessEffect::Ask),
        "deny" => Some(ReadAccessEffect::Deny),
        _ => None,
    }
}

fn djinn_config_permission_effect(effect: &str) -> Option<PermissionEffect> {
    match effect.trim() {
        "allow" => Some(PermissionEffect::Allow),
        "ask" => Some(PermissionEffect::Ask),
        "deny" => Some(PermissionEffect::Deny),
        _ => None,
    }
}

fn config_permission_action(action: &str) -> String {
    match action.trim() {
        "bash" => "shell".to_string(),
        other if other.is_empty() => "*".to_string(),
        other => other.to_string(),
    }
}

fn config_permission_pattern(pattern: &str, workspace: &Path) -> String {
    let pattern = pattern.trim();
    if pattern == "*" || pattern.is_empty() {
        return "*".to_string();
    }
    let home = djinn_core::home_dir();
    let expanded = if pattern == "~" {
        home.to_string_lossy().to_string()
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest).to_string_lossy().to_string()
    } else if pattern == "$HOME" {
        home.to_string_lossy().to_string()
    } else if let Some(rest) = pattern.strip_prefix("$HOME/") {
        home.join(rest).to_string_lossy().to_string()
    } else {
        pattern.to_string()
    };

    if expanded.starts_with('/') || !expanded.contains('/') {
        expanded
    } else {
        workspace.join(expanded).to_string_lossy().to_string()
    }
}

#[allow(dead_code)]
fn opencode_permission_policy_rules(
    profile: &str,
    workspace: &Path,
) -> Result<Option<Vec<PermissionRule>>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for path in opencode_model_config_paths(&cwd) {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        let rules = opencode_permission_policy_rules_from_content(&content, profile, workspace)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?;
        if !rules.is_empty() {
            return Ok(Some(rules));
        }
    }
    Ok(None)
}

fn opencode_permission_policy_rules_from_content(
    content: &str,
    profile: &str,
    workspace: &Path,
) -> Result<Vec<PermissionRule>> {
    let value: Value = serde_json::from_str(content)?;
    let mut rules = Vec::new();

    collect_opencode_general_permission_rules(&value, workspace, &mut rules);
    if let Some(agent) = opencode_selected_agent_config(&value, profile) {
        collect_opencode_general_permission_rules(agent, workspace, &mut rules);
    }

    Ok(rules)
}

#[allow(dead_code)]
fn opencode_read_access_rules(
    profile: &str,
    workspace: &Path,
) -> Result<Option<Vec<ReadAccessRule>>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for path in opencode_model_config_paths(&cwd) {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        let rules = opencode_read_access_rules_from_content(&content, profile, workspace)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?;
        if !rules.is_empty() {
            return Ok(Some(rules));
        }
    }
    Ok(None)
}

fn opencode_read_access_rules_from_content(
    content: &str,
    profile: &str,
    workspace: &Path,
) -> Result<Vec<ReadAccessRule>> {
    let value: Value = serde_json::from_str(content)?;
    let mut rules = Vec::new();

    collect_opencode_permission_rules(&value, workspace, &mut rules);
    if let Some(agent) = opencode_selected_agent_config(&value, profile) {
        collect_opencode_permission_rules(agent, workspace, &mut rules);
    }

    Ok(rules)
}

fn opencode_selected_agent_config<'a>(value: &'a Value, profile: &str) -> Option<&'a Value> {
    let profile = profile.trim();
    if !profile.is_empty() && profile != "default" {
        if let Some(agent) = opencode_agent_config(value, profile) {
            return Some(agent);
        }
    }
    if let Some(default_agent) = value
        .get("default_agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    {
        if let Some(agent) = opencode_agent_config(value, default_agent) {
            return Some(agent);
        }
    }
    opencode_agent_config(value, "coder").or_else(|| opencode_agent_config(value, "default"))
}

fn opencode_agent_config<'a>(value: &'a Value, agent: &str) -> Option<&'a Value> {
    ["agent", "agents"].into_iter().find_map(|container| {
        value
            .get(container)
            .and_then(Value::as_object)
            .and_then(|agents| agents.get(agent))
    })
}

fn collect_opencode_permission_rules(
    value: &Value,
    workspace: &Path,
    out: &mut Vec<ReadAccessRule>,
) {
    if let Some(permission) = value.get("permission") {
        collect_opencode_v1_permission_rules(permission, workspace, out);
    }
    if let Some(permissions) = value.get("permissions") {
        collect_opencode_v2_permission_rules(permissions, workspace, out);
    }
}

fn collect_opencode_general_permission_rules(
    value: &Value,
    workspace: &Path,
    out: &mut Vec<PermissionRule>,
) {
    if let Some(permission) = value.get("permission") {
        collect_opencode_v1_general_permission_rules(permission, workspace, out);
    }
    if let Some(permissions) = value.get("permissions") {
        collect_opencode_v2_general_permission_rules(permissions, workspace, out);
    }
}

fn collect_opencode_v1_general_permission_rules(
    permission: &Value,
    workspace: &Path,
    out: &mut Vec<PermissionRule>,
) {
    let Some(permission) = permission.as_object() else {
        return;
    };
    for (action, value) in permission {
        let action = opencode_permission_action(action);
        if let Some(effect) = value.as_str().and_then(opencode_permission_effect) {
            out.push(PermissionRule {
                action,
                resource: "*".to_string(),
                effect,
            });
            continue;
        }
        let Some(patterns) = value.as_object() else {
            continue;
        };
        for (pattern, effect) in patterns {
            if let Some(effect) = effect.as_str().and_then(opencode_permission_effect) {
                out.push(PermissionRule {
                    action: action.clone(),
                    resource: opencode_permission_pattern(pattern, workspace),
                    effect,
                });
            }
        }
    }
}

fn collect_opencode_v2_general_permission_rules(
    permissions: &Value,
    workspace: &Path,
    out: &mut Vec<PermissionRule>,
) {
    let Some(permissions) = permissions.as_array() else {
        return;
    };
    for rule in permissions {
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .map(opencode_permission_action)
            .unwrap_or_else(|| "*".to_string());
        let Some(effect) = rule
            .get("effect")
            .and_then(Value::as_str)
            .and_then(opencode_permission_effect)
        else {
            continue;
        };
        let resource = rule.get("resource").and_then(Value::as_str).unwrap_or("*");
        out.push(PermissionRule {
            action,
            resource: opencode_permission_pattern(resource, workspace),
            effect,
        });
    }
}

fn collect_opencode_v1_permission_rules(
    permission: &Value,
    workspace: &Path,
    out: &mut Vec<ReadAccessRule>,
) {
    let Some(permission) = permission.as_object() else {
        return;
    };
    for key in ["*", "read"] {
        let Some(value) = permission.get(key) else {
            continue;
        };
        if let Some(effect) = value.as_str().and_then(opencode_read_access_effect) {
            out.push(ReadAccessRule {
                pattern: "*".to_string(),
                effect,
            });
            continue;
        }
        let Some(patterns) = value.as_object() else {
            continue;
        };
        for (pattern, action) in patterns {
            if let Some(effect) = action.as_str().and_then(opencode_read_access_effect) {
                out.push(ReadAccessRule {
                    pattern: opencode_permission_pattern(pattern, workspace),
                    effect,
                });
            }
        }
    }
}

fn collect_opencode_v2_permission_rules(
    permissions: &Value,
    workspace: &Path,
    out: &mut Vec<ReadAccessRule>,
) {
    let Some(permissions) = permissions.as_array() else {
        return;
    };
    for rule in permissions {
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if action != "read" && action != "*" && action != "external_directory" {
            continue;
        }
        let Some(effect) = rule
            .get("effect")
            .or_else(|| rule.get("action"))
            .and_then(Value::as_str)
            .and_then(opencode_read_access_effect)
        else {
            continue;
        };
        let pattern = rule.get("resource").and_then(Value::as_str).unwrap_or("*");
        out.push(ReadAccessRule {
            pattern: opencode_permission_pattern(pattern, workspace),
            effect,
        });
    }
}

fn opencode_read_access_effect(effect: &str) -> Option<ReadAccessEffect> {
    match effect.trim() {
        "allow" => Some(ReadAccessEffect::Allow),
        "ask" => Some(ReadAccessEffect::Ask),
        "deny" => Some(ReadAccessEffect::Deny),
        _ => None,
    }
}

fn opencode_permission_action(action: &str) -> String {
    match action.trim() {
        "bash" => "shell".to_string(),
        other if other.is_empty() => "*".to_string(),
        other => other.to_string(),
    }
}

fn opencode_permission_effect(effect: &str) -> Option<PermissionEffect> {
    match effect.trim() {
        "allow" => Some(PermissionEffect::Allow),
        "ask" => Some(PermissionEffect::Ask),
        "deny" => Some(PermissionEffect::Deny),
        _ => None,
    }
}

fn opencode_permission_pattern(pattern: &str, workspace: &Path) -> String {
    let pattern = pattern.trim();
    if pattern == "*" || pattern.is_empty() {
        return "*".to_string();
    }
    let home = djinn_core::home_dir();
    let expanded = if pattern == "~" {
        home.to_string_lossy().to_string()
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest).to_string_lossy().to_string()
    } else if pattern == "$HOME" {
        home.to_string_lossy().to_string()
    } else if let Some(rest) = pattern.strip_prefix("$HOME/") {
        home.join(rest).to_string_lossy().to_string()
    } else {
        pattern.to_string()
    };

    if expanded.starts_with('/') || !expanded.contains('/') {
        expanded
    } else {
        workspace.join(expanded).to_string_lossy().to_string()
    }
}

fn resolve_agent_workspace(path: Option<PathBuf>) -> Result<String> {
    let path = path.unwrap_or(env::current_dir().with_context(|| "reading current directory")?);
    Ok(path
        .canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .to_string())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ResolvedAgentInstruction {
    pub(crate) source: String,
    pub(crate) content: String,
}

fn resolve_agent_instruction_contents(
    workspace: &str,
    references: &[String],
) -> Result<Vec<ResolvedAgentInstruction>> {
    if references.is_empty() {
        return Ok(Vec::new());
    }
    let config = effective_djinn_config()?;
    let workspace_path = Path::new(workspace);
    let mut resolved = Vec::new();
    for reference in references {
        let reference = reference.trim();
        if reference.is_empty() {
            continue;
        }
        if let Some(instruction) = config.instructions.get(reference) {
            if let Some(text) = instruction
                .text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                resolved.push(ResolvedAgentInstruction {
                    source: reference.to_string(),
                    content: truncate(text, 20_000),
                });
            }
            if let Some(path) = instruction
                .path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
            {
                if let Some(resolved_instruction) =
                    read_agent_instruction_file(workspace_path, path)?
                {
                    resolved.push(ResolvedAgentInstruction {
                        source: format!("{reference}:{path}"),
                        content: resolved_instruction.content,
                    });
                }
            }
            continue;
        }
        if let Some(instruction) = read_agent_instruction_file(workspace_path, reference)? {
            resolved.push(instruction);
        }
    }
    Ok(resolved)
}

fn read_agent_instruction_file(
    workspace: &Path,
    reference: &str,
) -> Result<Option<ResolvedAgentInstruction>> {
    let path = resolve_agent_instruction_path(workspace, reference);
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading agent instruction file {}", path.display()))?;
    Ok(Some(ResolvedAgentInstruction {
        source: path.display().to_string(),
        content: truncate(content.trim(), 20_000),
    }))
}

fn resolve_agent_instruction_path(workspace: &Path, reference: &str) -> PathBuf {
    let reference = reference.trim();
    if let Some(rest) = reference.strip_prefix("~/") {
        return djinn_core::home_dir().join(rest);
    }
    let path = PathBuf::from(reference);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}

fn agent_system_message(
    workspace: &str,
    instructions: &[ResolvedAgentInstruction],
) -> ModelMessage {
    let mut content = format!(
        "You are running in workspace `{workspace}`. Read-only filesystem tools may also access other paths such as the user's home directory when the configured access policy allows it. Use absolute paths, `~`, or `$HOME` for non-workspace locations."
    );
    if !instructions.is_empty() {
        content.push_str("\n\nAdditional configured instructions:");
        for instruction in instructions {
            content.push_str(&format!(
                "\n\n--- {} ---\n{}",
                instruction.source, instruction.content
            ));
        }
    }
    ModelMessage {
        role: ModelRole::System,
        content,
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

fn agent_model_messages(
    session: &AgentSession,
    workspace: &str,
    instructions: &[ResolvedAgentInstruction],
) -> Vec<ModelMessage> {
    let mut messages = vec![agent_system_message(workspace, instructions)];
    for event in &session.events {
        match &event.kind {
            AgentSessionEventKind::UserMessage { content } => messages.push(ModelMessage {
                role: ModelRole::User,
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
            }),
            AgentSessionEventKind::AssistantMessage { content } if !content.trim().is_empty() => {
                messages.push(ModelMessage {
                    role: ModelRole::Assistant,
                    content: content.clone(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                });
            }
            AgentSessionEventKind::Summary { content } if !content.trim().is_empty() => {
                messages.push(ModelMessage {
                    role: ModelRole::Assistant,
                    content: format!("Previous session summary: {content}"),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                });
            }
            _ => {}
        }
    }
    messages
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TuiRunOutcome {
    Exit,
    Action(djinn_tui::TuiAction),
}

fn run_tui(args: TuiArgs) -> Result<()> {
    let initial_tab = dashboard_tab(args.view);
    let mut tui = djinn_tui::TuiSession::enter()?;
    let outcome = run_tui_in_session(&mut tui, &args, initial_tab)?;
    tui.finish()?;
    match outcome {
        TuiRunOutcome::Exit => Ok(()),
        TuiRunOutcome::Action(action) => {
            handle_tui_action(action, args.editor)?;
            Ok(())
        }
    }
}

fn run_tui_in_session(
    tui: &mut djinn_tui::TuiSession,
    args: &TuiArgs,
    initial_tab: djinn_tui::DashboardTab,
) -> Result<TuiRunOutcome> {
    let roots = tool_roots(args.roots.clone());
    let tools = scan_tools(&roots)?;
    let sessions = session_records_for_dashboard()?;
    let memories = memory_store().list()?;
    let suggestions = suggestion_store().list()?;
    let skills = skill_records()?;
    let active_context = context_store().active()?;
    let Some(action) = tui.run_dashboard_with_handler(
        tools,
        sessions,
        memories,
        suggestions,
        skills,
        active_context,
        initial_tab,
        |action| match action {
            djinn_tui::TuiAction::DeleteMemories(ids) => remove_memories_silent(&ids).map(|_| ()),
            djinn_tui::TuiAction::DeleteSuggestions(ids) => remove_suggestions(&ids).map(|_| ()),
            djinn_tui::TuiAction::OpenSession(_)
            | djinn_tui::TuiAction::PromoteSessions { .. }
            | djinn_tui::TuiAction::OpenTool(_)
            | djinn_tui::TuiAction::OpenSkill(_)
            | djinn_tui::TuiAction::ReviewMemory(_) => Ok(()),
        },
    )?
    else {
        return Ok(TuiRunOutcome::Exit);
    };

    Ok(TuiRunOutcome::Action(action))
}

fn handle_tui_action(action: djinn_tui::TuiAction, editor: Option<String>) -> Result<bool> {
    match action {
        djinn_tui::TuiAction::OpenSession(session) => {
            run_folder_session_tui(PathBuf::from(session.path), editor).map(|_| false)
        }
        djinn_tui::TuiAction::PromoteSessions {
            promotion_type,
            sessions,
        } => promote_tui_sessions(promotion_type, sessions, editor).map(|_| false),
        djinn_tui::TuiAction::OpenTool(entry) => open_tool_entry(&entry, editor).map(|_| false),
        djinn_tui::TuiAction::OpenSkill(entry) => open_skill_entry(&entry, editor).map(|_| false),
        djinn_tui::TuiAction::ReviewMemory(id) => accept_memory(AcceptMemoryArgs {
            id,
            agent: None,
            title: "djinn memory suggestion review".to_string(),
            opencode_bin: "opencode".to_string(),
            dry_run: false,
        })
        .map(|_| false),
        djinn_tui::TuiAction::DeleteMemories(ids) => remove_memories_silent(&ids).map(|_| false),
        djinn_tui::TuiAction::DeleteSuggestions(ids) => remove_suggestions(&ids).map(|_| false),
    }
}

fn promote_tui_sessions(
    promotion_type: djinn_tui::DashboardPromotionType,
    sessions: Vec<djinn_tui::SessionRecord>,
    editor: Option<String>,
) -> Result<()> {
    if sessions.is_empty() {
        bail!("select at least one session to promote");
    }
    let args = SessionPromoteArgs {
        dirs: sessions
            .iter()
            .map(|session| PathBuf::from(&session.path))
            .collect(),
        promotion_type: session_promote_type_from_dashboard(promotion_type),
        promotion_session_dir: None,
        max_chars_per_artifact: 1200,
        force: false,
        json: false,
    };
    let report = create_promotion_session(&args)?;
    println!(
        "Created {} promotion session from {} selected session{}: {}",
        session_promote_type_label(args.promotion_type),
        report.session_count,
        plural_suffix(report.session_count),
        report.promotion_session_dir
    );
    run_folder_session_tui(PathBuf::from(report.promotion_session_dir), editor)
}

fn session_promote_type_from_dashboard(
    promotion_type: djinn_tui::DashboardPromotionType,
) -> SessionPromoteType {
    match promotion_type {
        djinn_tui::DashboardPromotionType::Memory => SessionPromoteType::Memory,
        djinn_tui::DashboardPromotionType::Todo => SessionPromoteType::Todo,
        djinn_tui::DashboardPromotionType::Skill => SessionPromoteType::Skill,
        djinn_tui::DashboardPromotionType::Pattern => SessionPromoteType::Pattern,
    }
}

fn session_records_for_dashboard() -> Result<Vec<djinn_tui::SessionRecord>> {
    let report = list_cache_folder_sessions(None)?;
    Ok(report
        .sessions
        .into_iter()
        .map(|session| djinn_tui::SessionRecord {
            name: session.display_name,
            reference_name: session.reference_name,
            path: session.path,
            state: session.lifecycle.state,
            mode: session.lifecycle.mode,
            updated_at: session.updated_at.or(session.modified_at),
            repo_path: session.repo_path.or(session.workspace),
            summary_preview: session.summary_preview,
            turn_count: session.turn_count,
            event_health: folder_session_event_health_label(&session.event_health),
            candidate_status: session
                .candidates
                .as_ref()
                .map(format_session_candidate_status),
            candidate_details: session
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
            candidate_entries: session
                .candidates
                .as_ref()
                .map(|candidates| candidates.entries.iter().map(tui_candidate_row).collect())
                .unwrap_or_default(),
            next_action: session.next_action,
        })
        .collect())
}

fn dashboard_tab(view: TuiView) -> djinn_tui::DashboardTab {
    match view {
        TuiView::Tools => djinn_tui::DashboardTab::Tools,
        TuiView::Sessions => djinn_tui::DashboardTab::Sessions,
        TuiView::Memories => djinn_tui::DashboardTab::Memories,
        TuiView::Suggestions => djinn_tui::DashboardTab::Suggestions,
        TuiView::Skills => djinn_tui::DashboardTab::Skills,
    }
}

fn default_dashboard_tui_args() -> TuiArgs {
    TuiArgs {
        view: TuiView::Sessions,
        roots: Vec::new(),
        editor: None,
    }
}

fn list_tools(scope: ToolsScope) -> Result<()> {
    let roots = tool_roots(scope.roots);
    let entries = scan_tools(&roots)?;
    if entries.is_empty() {
        println!("Djinn found 0 tools under {}", format_roots(&roots));
        return Ok(());
    }
    if output_format(scope.format, scope.json) == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for entry in entries {
            println!(
                "{}\t{}:{}\t{}",
                entry.name,
                entry.path.display(),
                entry.line,
                entry.description
            );
        }
    }
    Ok(())
}

fn list_memories() -> Result<()> {
    let records = memory_store().list()?;
    if records.is_empty() {
        println!("Memories are empty.");
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!(
                "  {}. [{}] {}{}",
                idx + 1,
                record.id,
                record.text,
                format_memory_suffix(record)
            );
        }
        println!("\nTotal: {} memories", records.len());
    }
    Ok(())
}

fn list_ideas() -> Result<()> {
    let records = idea_store().list()?;
    if records.is_empty() {
        println!("Ideas are empty.");
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!(
                "  {}. [{}] {}{}",
                idx + 1,
                record.id,
                record.text,
                format_idea_suffix(record)
            );
        }
        println!("\nTotal: {} ideas", records.len());
    }
    Ok(())
}

fn list_actions() -> Result<()> {
    let records = action_store().list()?;
    if records.is_empty() {
        println!("Actions are empty.");
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!(
                "  {}. [{}] {}{}",
                idx + 1,
                record.id,
                record.text,
                format_action_suffix(record)
            );
        }
        println!("\nTotal: {} actions", records.len());
    }
    Ok(())
}

fn list_suggestions() -> Result<()> {
    let records = suggestion_store().list()?;
    if records.is_empty() {
        println!("Suggestions are empty.");
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!(
                "  {}. [{}] {}{}",
                idx + 1,
                record.id,
                record.text,
                format_suggestion_suffix(record)
            );
        }
        println!("\nTotal: {} suggestions", records.len());
    }
    Ok(())
}

fn list_skills(args: ListSkillsArgs) -> Result<()> {
    let records = skill_records()?;
    if output_format(args.format, args.json) == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else if records.is_empty() {
        println!("No skills found.");
        println!(
            "Djinn-managed skills live under {}",
            skill_store().managed_root().display()
        );
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!(
                "  {}. [{}] {}{}",
                idx + 1,
                record.name,
                if record.description.is_empty() {
                    "No description".to_string()
                } else {
                    record.description.clone()
                },
                format_skill_suffix(record)
            );
        }
        println!("\nTotal: {} skills", records.len());
    }
    Ok(())
}

fn show_skill(args: ShowSkillArgs) -> Result<()> {
    let records = skill_records()?;
    let record = resolve_skill(&records, &args.name)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(record)?);
        return Ok(());
    }
    println!("# {}\n", record.name);
    if !record.description.is_empty() {
        println!("{}\n", record.description);
    }
    println!("Source: {}", record.source);
    println!("Managed: {}", if record.managed { "yes" } else { "no" });
    println!("Path: {}", record.path.display());
    println!("Root: {}", record.root.display());
    println!("\n## SKILL.md\n");
    println!("{}", read_skill_content(record)?);
    Ok(())
}

fn add_skill(args: AddSkillArgs) -> Result<()> {
    let record = skill_store().add(&args.name, args.description.as_deref(), args.force)?;
    println!("Skill added [{}]: {}", record.name, record.path.display());
    Ok(())
}

fn rm_skill(args: RmSkillArgs) -> Result<()> {
    let store = skill_store();
    let records = store.list()?;
    let removed = store.remove(&records, &args.name)?;
    println!(
        "Skill removed [{}]: {}",
        removed.name,
        removed.path.display()
    );
    Ok(())
}

fn list_contexts(args: ListCtxArgs) -> Result<()> {
    let store = context_store();
    let records = store.list()?;
    let active = store.active_name()?.unwrap_or_default();
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "active": active,
                "contexts": records,
            }))?
        );
    } else if records.is_empty() {
        println!("No contexts configured.");
        println!("Add one with `djinn add ctx <name> --root <path>`.");
    } else {
        for record in &records {
            let marker = if record.name.eq_ignore_ascii_case(&active) {
                "*"
            } else {
                " "
            };
            println!(
                "{marker} [{}] {}{}",
                record.name,
                if record.description.is_empty() {
                    "No description".to_string()
                } else {
                    record.description.clone()
                },
                format_context_suffix(record)
            );
        }
        println!("\nTotal: {} contexts", records.len());
    }
    Ok(())
}

fn show_context(args: ShowCtxArgs) -> Result<()> {
    let store = context_store();
    let records = store.list()?;
    let active = store.active_name()?.unwrap_or_default();
    let record = if let Some(name) = args.name.as_deref() {
        resolve_context(&records, name)?.clone()
    } else {
        store.active()?.ok_or_else(|| {
            anyhow::anyhow!("no active context; add one with `djinn add ctx <name> --root <path>`")
        })?
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "active": record.name.eq_ignore_ascii_case(&active),
                "context": record,
            }))?
        );
        return Ok(());
    }
    println!("# {}\n", record.name);
    if !record.description.is_empty() {
        println!("{}\n", record.description);
    }
    println!(
        "Active: {}",
        if record.name.eq_ignore_ascii_case(&active) {
            "yes"
        } else {
            "no"
        }
    );
    if !record.memory_scope.is_empty() {
        println!("Memory scope: {}", record.memory_scope);
    }
    println!("\nTool roots:");
    if record.roots.is_empty() {
        println!("  - (none configured; Djinn falls back to default roots)");
    } else {
        for root in &record.roots {
            println!("  - {}", root.display());
        }
    }
    println!("\nSkill roots:");
    if record.skill_roots.is_empty() {
        println!("  - (none configured; Djinn uses default skill roots)");
    } else {
        for root in &record.skill_roots {
            println!("  - {}", root.display());
        }
    }
    Ok(())
}

fn add_context(args: AddCtxArgs) -> Result<()> {
    let record = context_store().add_or_update(
        ContextInput {
            name: args.name,
            description: args.description,
            roots: args.roots,
            skill_roots: args.skill_roots,
            memory_scope: args.memory_scope,
        },
        args.switch,
    )?;
    println!(
        "Context saved [{}]{}",
        record.name,
        format_context_suffix(&record)
    );
    Ok(())
}

fn switch_context(name: &str) -> Result<()> {
    let record = context_store().switch(name)?;
    println!("Active context: {}", record.name);
    Ok(())
}

fn add_memory(args: AddMemoryArgs) -> Result<MemoryRecord> {
    memory_store().add_input(memory_input_from_args(args)?)
}

fn add_idea(args: AddMemoryArgs) -> Result<IdeaRecord> {
    idea_store().add_input(memory_input_from_args(args)?)
}

fn add_action(args: AddMemoryArgs) -> Result<ActionRecord> {
    action_store().add_input(memory_input_from_args(args)?)
}

fn add_suggestion(args: AddSuggestionArgs) -> Result<()> {
    let sources = if args.source_memories.is_empty() {
        Vec::new()
    } else {
        let memories = memory_store().list()?;
        args.source_memories
            .iter()
            .map(|id| {
                let memory = resolve_memory(&memories, id)?;
                Ok(MemorySource {
                    source_type: "memory".to_string(),
                    source: "djinn".to_string(),
                    source_id: memory.id.clone(),
                    chat_id: String::new(),
                    title: memory.text.clone(),
                    captured_at: memory.created_at.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?
    };
    let record = suggestion_store().add_input(SuggestionInput {
        text: args.text,
        target: args.target,
        rationale: args.rationale,
        draft: args.draft,
        evidence: args.evidence,
        sources,
    })?;
    println!("Suggestion saved [{}]: {}", record.id, record.text);
    Ok(())
}

fn memory_input_from_args(args: AddMemoryArgs) -> Result<MemoryInput> {
    Ok(MemoryInput {
        text: args.text,
        scope: args.scope,
        kind: args.kind,
        confidence: args.confidence,
        not_before: args.not_before,
        evidence: args.evidence,
        sources: Vec::new(),
    })
}

fn default_opencode_config_path() -> PathBuf {
    djinn_core::home_dir()
        .join(".config")
        .join("opencode")
        .join("opencode.json")
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn clear_memories(no_backup: bool) -> Result<()> {
    if !io::stdin().is_terminal() {
        bail!("refusing to clear memories from a non-interactive shell");
    }
    print!("Clear Djinn memories? Type 'clear' to confirm: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if answer.trim() != "clear" {
        println!("Aborted.");
        return Ok(());
    }
    let backup = memory_store().clear_with_backup(!no_backup)?;
    if let Some(info) = backup {
        println!(
            "Memories cleared ({} records). Backup written to {} and metadata to {}",
            info.record_count,
            info.path.display(),
            info.metadata_path.display()
        );
    } else {
        println!("Memories cleared.");
    }
    Ok(())
}

fn rm_memory(keyword: &str) -> Result<()> {
    let removed = memory_store().remove_matching(keyword)?;
    if removed.is_empty() {
        println!("No memories matched {keyword:?}.");
    } else {
        println!("Removed {} memories:", removed.len());
        for record in removed {
            println!("  - [{}] {}", record.id, record.text);
        }
    }
    Ok(())
}

fn ingest_memories(args: IngestMemoriesArgs) -> Result<()> {
    let memories = memory_store().list()?;
    let resolved_ids = resolve_memory_ids(&memories, &args.ids)?;
    let selected = resolved_ids
        .iter()
        .map(|id| resolve_memory(&memories, id).cloned())
        .collect::<Result<Vec<_>>>()?;
    let mut outputs = Vec::new();
    for memory in &selected {
        let target = if args.target == IngestTarget::Auto {
            infer_ingest_target(memory)
        } else {
            args.target
        };
        outputs.push(ingest_memory_as(memory, target, args.force)?);
    }
    if !args.keep {
        memory_store().remove_ids(&resolved_ids)?;
    }

    println!("Ingested {} memories:", outputs.len());
    for output in outputs {
        println!("  - {output}");
    }
    Ok(())
}

fn ingest_memory_as(
    memory: &MemoryRecord,
    target: IngestTarget,
    force_skill: bool,
) -> Result<String> {
    let input = memory_input_from_memory(memory);
    match target {
        IngestTarget::Auto => unreachable!("auto target must be resolved before ingestion"),
        IngestTarget::Memory => {
            let record = memory_store().add_input(input)?;
            Ok(format!("memory [{}]: {}", record.id, record.text))
        }
        IngestTarget::Suggestion => {
            let suggestion = suggestion_store().add_input(SuggestionInput {
                text: memory.text.clone(),
                target: non_empty_option(&memory.kind),
                rationale: Some("Created from an active memory.".to_string()),
                draft: None,
                evidence: memory.evidence.clone(),
                sources: memory.sources.clone(),
            })?;
            Ok(format!(
                "suggestion [{}]: {}",
                suggestion.id, suggestion.text
            ))
        }
        IngestTarget::Skill => {
            let name = skill_name_from_memory(memory);
            let content = skill_content_from_memory(memory);
            let skill =
                skill_store().add_with_content(&name, &memory.text, content, force_skill)?;
            Ok(format!("skill [{}]: {}", skill.name, skill.path.display()))
        }
        IngestTarget::Idea => {
            let idea = idea_store().add_input(input)?;
            Ok(format!("idea [{}]: {}", idea.id, idea.text))
        }
        IngestTarget::Action => {
            let action = action_store().add_input(input)?;
            Ok(format!("action [{}]: {}", action.id, action.text))
        }
    }
}

fn infer_ingest_target(memory: &MemoryRecord) -> IngestTarget {
    let haystack = format!("{} {}", memory.kind, memory.text).to_lowercase();
    if haystack.contains("skill") {
        IngestTarget::Skill
    } else if haystack.contains("preference") || haystack.contains("instruction") {
        IngestTarget::Suggestion
    } else if haystack.contains("action") || haystack.contains("todo") || haystack.contains("task")
    {
        IngestTarget::Action
    } else if haystack.contains("idea")
        || haystack.contains("improvement")
        || haystack.contains("consider")
    {
        IngestTarget::Idea
    } else {
        IngestTarget::Memory
    }
}

fn memory_input_from_memory(memory: &MemoryRecord) -> MemoryInput {
    MemoryInput {
        text: memory.text.clone(),
        scope: non_empty_option(&memory.scope),
        kind: non_empty_option(&memory.kind),
        confidence: non_empty_option(&memory.confidence),
        not_before: non_empty_option(&memory.not_before),
        evidence: memory.evidence.clone(),
        sources: memory.sources.clone(),
    }
}

fn skill_name_from_memory(memory: &MemoryRecord) -> String {
    memory
        .id
        .split('-')
        .filter(|part| !part.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-")
}

fn skill_content_from_memory(memory: &MemoryRecord) -> String {
    let name = skill_name_from_memory(memory);
    let mut out = format!(
        "# Skill: {name}\n\n{}\n\n## When to use\n\n- Use when this remembered workflow applies to the current task.\n\n## Workflow\n\n1. Apply the remembered guidance below.\n\n## Ingested guidance\n\n{}\n",
        memory.text,
        memory.text
    );
    if !memory.evidence.is_empty() {
        out.push_str("\n## Evidence\n\n");
        for evidence in &memory.evidence {
            out.push_str(&format!("- {evidence}\n"));
        }
    }
    out
}

fn accept_memory(args: AcceptMemoryArgs) -> Result<()> {
    review_memories(ReviewMemoriesArgs {
        ids: vec![args.id],
        limit: 1,
        all: false,
        query: None,
        agent: args.agent,
        title: args.title,
        opencode_bin: args.opencode_bin,
        dry_run: args.dry_run,
    })
}

fn reject_memories(ids: &[String]) -> Result<()> {
    let removed = remove_memories_silent(ids)?;
    if removed.is_empty() {
        println!("No memories were rejected.");
    } else {
        println!("Rejected and removed {} memories:", removed.len());
        for memory in removed {
            println!("  - [{}] {}", memory.id, memory.text);
        }
    }
    Ok(())
}

fn remove_memories_silent(ids: &[String]) -> Result<Vec<MemoryRecord>> {
    let memories = memory_store().list()?;
    let resolved = resolve_memory_ids(&memories, ids)?;
    memory_store().remove_ids(&resolved)
}

fn complete_suggestions(ids: &[String]) -> Result<()> {
    let removed = remove_suggestions(ids)?;
    if removed.is_empty() {
        println!("No suggestions were completed.");
    } else {
        println!("Completed and removed {} suggestions:", removed.len());
        for suggestion in removed {
            println!("  - [{}] {}", suggestion.id, suggestion.text);
        }
        println!("Starting an agent session for completed suggestions will be added later.");
    }
    Ok(())
}

fn reject_suggestions(ids: &[String]) -> Result<()> {
    let removed = remove_suggestions(ids)?;
    if removed.is_empty() {
        println!("No suggestions were rejected.");
    } else {
        println!("Rejected and removed {} suggestions:", removed.len());
        for suggestion in removed {
            println!("  - [{}] {}", suggestion.id, suggestion.text);
        }
    }
    Ok(())
}

fn remove_suggestions(ids: &[String]) -> Result<Vec<SuggestionRecord>> {
    let suggestions = suggestion_store().list()?;
    let resolved = resolve_suggestion_ids(&suggestions, ids)?;
    suggestion_store().remove_ids(&resolved)
}

fn non_empty_option(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn show_memory(id: &str) -> Result<()> {
    let memories = memory_store().list()?;
    let record = resolve_memory(&memories, id)?;

    println!("# {}\n", record.id);
    println!("{}\n", record.text);
    println!("Created: {}", record.created_at);
    if !record.scope.trim().is_empty() {
        println!("Scope: {}", record.scope);
    }
    if !record.kind.trim().is_empty() {
        println!("Kind: {}", record.kind);
    }
    if !record.confidence.trim().is_empty() {
        println!("Confidence: {}", record.confidence);
    }
    if !record.not_before.trim().is_empty() {
        println!("Not before: {}", record.not_before);
    }
    if !record.evidence.is_empty() {
        println!("\n## Evidence\n");
        for (idx, evidence) in record.evidence.iter().enumerate() {
            println!("{}. {}", idx + 1, evidence);
        }
    }

    if !record.sources.is_empty() {
        println!("\n## Sources\n");
        for source in &record.sources {
            println!("- {}", format_memory_source(source));
        }
    }

    Ok(())
}

fn show_idea(id: &str) -> Result<()> {
    let ideas = idea_store().list()?;
    let record = resolve_idea(&ideas, id)?;
    println!("# {}\n", record.id);
    println!("{}\n", record.text);
    println!("Created: {}", record.created_at);
    println!("Status: {}", record.status);
    if !record.scope.trim().is_empty() {
        println!("Scope: {}", record.scope);
    }
    if !record.kind.trim().is_empty() {
        println!("Kind: {}", record.kind);
    }
    if !record.confidence.trim().is_empty() {
        println!("Confidence: {}", record.confidence);
    }
    if !record.evidence.is_empty() {
        println!("\n## Evidence\n");
        for (idx, evidence) in record.evidence.iter().enumerate() {
            println!("{}. {}", idx + 1, evidence);
        }
    }
    Ok(())
}

fn show_action(id: &str) -> Result<()> {
    let actions = action_store().list()?;
    let record = resolve_action(&actions, id)?;
    println!("# {}\n", record.id);
    println!("{}\n", record.text);
    println!("Created: {}", record.created_at);
    println!("Status: {}", record.status);
    if !record.scope.trim().is_empty() {
        println!("Scope: {}", record.scope);
    }
    if !record.kind.trim().is_empty() {
        println!("Kind: {}", record.kind);
    }
    if !record.priority.trim().is_empty() {
        println!("Priority: {}", record.priority);
    }
    if !record.evidence.is_empty() {
        println!("\n## Evidence\n");
        for (idx, evidence) in record.evidence.iter().enumerate() {
            println!("{}. {}", idx + 1, evidence);
        }
    }
    Ok(())
}

fn show_suggestion(id: &str) -> Result<()> {
    let suggestions = suggestion_store().list()?;
    let record = resolve_suggestion(&suggestions, id)?;
    println!("# {}\n", record.id);
    println!("{}\n", record.text);
    println!("Created: {}", record.created_at);
    println!("Status: {}", record.status);
    if !record.target.trim().is_empty() {
        println!("Target: {}", record.target);
    }
    if !record.rationale.trim().is_empty() {
        println!("\n## Rationale\n\n{}", record.rationale);
    }
    if !record.draft.trim().is_empty() {
        println!("\n## Draft\n\n{}", record.draft);
    }
    if !record.evidence.is_empty() {
        println!("\n## Evidence\n");
        for (idx, evidence) in record.evidence.iter().enumerate() {
            println!("{}. {}", idx + 1, evidence);
        }
    }
    if !record.sources.is_empty() {
        println!("\n## Sources\n");
        for source in &record.sources {
            let label = if !source.title.trim().is_empty() {
                source.title.as_str()
            } else {
                source.source_id.as_str()
            };
            println!("- [{}] {}", source.source_type, label);
        }
    }
    Ok(())
}

fn show_tool(args: ToolLookupArgs) -> Result<()> {
    let roots = tool_roots(args.roots);
    let entries = scan_tools(&roots)?;
    let entry = resolve_tool(&entries, &args.name)?;
    if output_format(args.format, args.json) == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(entry)?);
    } else {
        println!("# {}\n", entry.name);
        println!("{}\n", entry.description);
        println!("Source: {}:{}\n", entry.path.display(), entry.line);
        println!("## Preview\n");
        println!("```text\n{}\n```", entry.preview);
    }
    Ok(())
}

fn search_tools(args: SearchToolsArgs) -> Result<()> {
    let query = args.query.to_lowercase();
    let roots = tool_roots(args.roots);
    let matches = scan_tools(&roots)?
        .into_iter()
        .filter(|entry| {
            entry.name.to_lowercase().contains(&query)
                || entry.description.to_lowercase().contains(&query)
                || entry.preview.to_lowercase().contains(&query)
        })
        .collect::<Vec<_>>();
    if output_format(args.format, args.json) == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&matches)?);
    } else {
        for entry in &matches {
            println!(
                "{}\t{}:{}\t{}",
                entry.name,
                entry.path.display(),
                entry.line,
                entry.description
            );
        }
        println!("\nTotal: {} matching tools", matches.len());
    }
    Ok(())
}

fn search_memories(query: &str) -> Result<()> {
    let query = query.to_lowercase();
    let matches = memory_store()
        .list()?
        .into_iter()
        .filter(|record| memory_matches(record, &query))
        .collect::<Vec<_>>();
    for (idx, record) in matches.iter().enumerate() {
        println!(
            "  {}. [{}] {}{}",
            idx + 1,
            record.id,
            record.text,
            format_memory_suffix(record)
        );
    }
    println!("\nTotal: {} matching memories", matches.len());
    Ok(())
}

fn select_memories_for_review(
    records: &[MemoryRecord],
    args: &ReviewMemoriesArgs,
) -> Result<Vec<MemoryRecord>> {
    if !args.ids.is_empty() {
        let mut seen = HashSet::new();
        let mut selected = Vec::new();
        for id in &args.ids {
            let record = resolve_memory(records, id)?;
            if seen.insert(record.id.clone()) {
                selected.push(record.clone());
            }
        }
        return Ok(selected);
    }
    let query = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(str::to_lowercase);
    let matches = records
        .iter()
        .filter(|record| {
            query
                .as_deref()
                .map(|query| memory_matches(record, query))
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();

    let selected = if args.all {
        matches
    } else {
        let mut latest = matches
            .into_iter()
            .rev()
            .take(args.limit)
            .collect::<Vec<_>>();
        latest.reverse();
        latest
    };

    if selected.is_empty() {
        bail!("no memories matched the review selection");
    }
    Ok(selected)
}

fn search_suggestions(query: &str) -> Result<()> {
    let query = query.to_lowercase();
    let matches = suggestion_store()
        .list()?
        .into_iter()
        .filter(|record| suggestion_matches(record, &query))
        .collect::<Vec<_>>();
    for (idx, record) in matches.iter().enumerate() {
        println!(
            "  {}. [{}] {}{}",
            idx + 1,
            record.id,
            record.text,
            format_suggestion_suffix(record)
        );
    }
    println!("\nTotal: {} matching suggestions", matches.len());
    Ok(())
}

fn review_memories(args: ReviewMemoriesArgs) -> Result<()> {
    let memories = memory_store().list()?;
    let selected = select_memories_for_review(&memories, &args)?;
    let suggestions = suggestion_store().list()?;
    let prompt = format_memory_review_prompt(&selected, &suggestions, &args);

    if args.dry_run {
        println!("{prompt}");
        return Ok(());
    }

    let output = spawn_background_opencode_review(
        &args.opencode_bin,
        &args.title,
        args.agent.as_deref(),
        &prompt,
    )?;
    println!("Memory review started in the background.");
    println!("Output: {}", output.output_path.display());
    println!("Prompt: {}", output.prompt_path.display());
    println!("Djinn will send a notification when the review completes if osascript is available.");
    Ok(())
}

#[derive(Debug, Clone)]
struct BackgroundReviewOutput {
    output_path: PathBuf,
    prompt_path: PathBuf,
}

fn spawn_background_opencode_review(
    opencode_bin: &str,
    title: &str,
    agent: Option<&str>,
    prompt: &str,
) -> Result<BackgroundReviewOutput> {
    let reviews_dir = djinn_core::default_cache_dir().join("reviews");
    fs::create_dir_all(&reviews_dir)
        .with_context(|| format!("creating {}", reviews_dir.display()))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let output_path = reviews_dir.join(format!("memory-review-{stamp}.md"));
    let prompt_path = reviews_dir.join(format!("memory-review-{stamp}.prompt.md"));
    fs::write(&prompt_path, prompt)
        .with_context(|| format!("writing review prompt {}", prompt_path.display()))?;

    let script = background_review_script(opencode_bin, title, agent, &prompt_path, &output_path);
    ProcessCommand::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| "spawning background memory review")?;

    Ok(BackgroundReviewOutput {
        output_path,
        prompt_path,
    })
}

fn background_review_script(
    opencode_bin: &str,
    title: &str,
    agent: Option<&str>,
    prompt_path: &Path,
    output_path: &Path,
) -> String {
    let agent = agent
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("");
    format!(
        r#"PROMPT_FILE={prompt_file}
OUT_FILE={out_file}
OPENCODE_BIN={opencode_bin}
TITLE={title}
AGENT={agent}
export DJINN_REVIEWER=1
export DJINN_OPENCODE_PLUGIN_CHILD=1
{{
  printf '# Djinn memory curation review\n\n'
  printf 'Started: %s\n' "$(date)"
  printf 'Prompt file: %s\n\n' "$PROMPT_FILE"
  if [ -n "$AGENT" ]; then
    "$OPENCODE_BIN" run "$(cat "$PROMPT_FILE")" --title "$TITLE" --agent "$AGENT"
  else
    "$OPENCODE_BIN" run "$(cat "$PROMPT_FILE")" --title "$TITLE"
  fi
  REVIEW_STATUS=$?
  printf '\n---\nFinished: %s\nExit status: %s\n' "$(date)" "$REVIEW_STATUS"
}} > "$OUT_FILE" 2>&1
if command -v osascript >/dev/null 2>&1; then
  if [ "$REVIEW_STATUS" -eq 0 ]; then
    osascript -e 'display notification "Review output is ready under ~/.cache/djinn/reviews." with title "Djinn memory review complete"' >/dev/null 2>&1 || true
  else
    osascript -e 'display notification "Review failed; see output under ~/.cache/djinn/reviews." with title "Djinn memory review failed"' >/dev/null 2>&1 || true
  fi
fi
exit "$REVIEW_STATUS"
"#,
        prompt_file = shell_quote(&prompt_path.display().to_string()),
        out_file = shell_quote(&output_path.display().to_string()),
        opencode_bin = shell_quote(opencode_bin),
        title = shell_quote(title),
        agent = shell_quote(agent),
    )
}

fn open_tool(args: OpenToolArgs) -> Result<()> {
    let roots = tool_roots(args.roots);
    let entries = scan_tools(&roots)?;
    let entry = resolve_tool(&entries, &args.name)?;
    open_tool_entry(entry, args.editor)
}

fn open_tool_entry(entry: &ToolEntry, editor: Option<String>) -> Result<()> {
    open_editor_at(&entry.path, entry.line, editor)
}

fn open_skill_entry(entry: &SkillRecord, editor: Option<String>) -> Result<()> {
    open_editor_at(&entry.path, 1, editor)
}

fn format_memory_review_prompt(
    memories: &[MemoryRecord],
    suggestions: &[SuggestionRecord],
    args: &ReviewMemoriesArgs,
) -> String {
    let mut out = String::from("# Djinn Memory Suggestion Review\n\n");
    out.push_str(
        "You are reviewing one or more Djinn memories. A memory is source evidence, not a target artifact. Do not copy memory text into a durable artifact. Instead, propose useful next steps as suggestions. You may create suggestions by running `djinn add suggestion ...` commands.\n\n",
    );
    out.push_str("## Review goals\n\n");
    out.push_str("- Decide whether these memories imply a skill, action, idea, config change, code/docs change, or other next step.\n");
    out.push_str("- Attach evidence from the reviewed memories.\n");
    out.push_str("- Prefer one clear suggestion over duplicating the memory text.\n");
    out.push_str("- If there is no useful next step, say so and do not create a suggestion.\n\n");

    out.push_str("## Selection\n\n");
    out.push_str(&format!("- Memories included: {}\n", memories.len()));
    if let Some(query) = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        out.push_str(&format!("- Query filter: `{query}`\n"));
    }
    if !args.all {
        out.push_str(&format!(
            "- Limit: latest {} matching memories\n",
            args.limit
        ));
    }

    out.push_str("\n## Existing suggestions\n\n```text\n");
    if suggestions.is_empty() {
        out.push_str("No open suggestions recorded.\n");
    } else {
        for suggestion in suggestions.iter().take(100) {
            out.push_str(&format!(
                "- [{}] {}{}\n",
                suggestion.id,
                suggestion.text,
                format_suggestion_suffix(suggestion)
            ));
        }
        if suggestions.len() > 100 {
            out.push_str(&format!(
                "... {} more suggestions omitted ...\n",
                suggestions.len() - 100
            ));
        }
    }
    out.push_str("```\n\n## Memories to review\n\n");
    for memory in memories {
        out.push_str(&format!("### [{}] {}\n\n", memory.id, memory.text));
        let mut details = Vec::new();
        if !memory.scope.trim().is_empty() {
            details.push(format!("scope: {}", memory.scope));
        }
        if !memory.kind.trim().is_empty() {
            details.push(format!("kind: {}", memory.kind));
        }
        if !memory.confidence.trim().is_empty() {
            details.push(format!("confidence: {}", memory.confidence));
        }
        if !memory.not_before.trim().is_empty() {
            details.push(format!("not-before: {}", memory.not_before));
        }
        if !details.is_empty() {
            out.push_str(&format!("Metadata: {}\n\n", details.join(", ")));
        }
        if !memory.evidence.is_empty() {
            out.push_str("Evidence:\n");
            for evidence in &memory.evidence {
                out.push_str(&format!("- {}\n", evidence));
            }
            out.push('\n');
        }
        if !memory.sources.is_empty() {
            out.push_str(&format!("Sources: {} pointer(s)\n\n", memory.sources.len()));
        }
    }

    out.push_str(
        "## Required output format\n\nIf useful, create one or more suggestions with commands like:\n\n```bash\ndjinn add suggestion \"Create a skill to ...\" --target skill --rationale \"Based on memories X and Y ...\" --evidence \"...\" --source-memory MEMORY_ID\n```\n\nTargets may include: skill, action, idea, config, code, docs, cleanup, or other. If no suggestion is warranted, say `No suggestion warranted.`\n",
    );
    out
}

fn tool_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    if !roots.is_empty() {
        return roots;
    }
    if let Ok(raw) = env::var("DJINN_TOOL_ROOTS") {
        let parsed = env::split_paths(&raw).collect::<Vec<_>>();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    if let Ok(Some(ctx)) = context_store().active() {
        if !ctx.roots.is_empty() {
            return ctx.roots;
        }
    }
    vec![djinn_core::default_dotfiles_root()]
}

fn scan_tools(roots: &[PathBuf]) -> Result<Vec<ToolEntry>> {
    let mut all = Vec::new();
    for root in roots {
        all.extend(djinn_tools::scan(root, &djinn_tools::default_extensions())?);
    }
    all.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.path.cmp(&right.path))
            .then(left.line.cmp(&right.line))
    });
    Ok(all)
}

fn resolve_tool<'a>(entries: &'a [ToolEntry], name: &str) -> Result<&'a ToolEntry> {
    if let Some(entry) = entries.iter().find(|entry| entry.name == name) {
        return Ok(entry);
    }
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(name))
    {
        return Ok(entry);
    }
    let needle = name.to_lowercase();
    let matches = entries
        .iter()
        .filter(|entry| entry.name.to_lowercase().contains(&needle))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [entry] => Ok(entry),
        [] => bail!("no tool named {name:?} found"),
        many => {
            eprintln!("multiple tools match {name:?}:");
            for entry in many {
                eprintln!("  - {} ({})", entry.name, entry.path.display());
            }
            bail!("tool name is ambiguous")
        }
    }
}

fn resolve_memory<'a>(records: &'a [MemoryRecord], id: &str) -> Result<&'a MemoryRecord> {
    if let Some(record) = records.iter().find(|record| record.id == id) {
        return Ok(record);
    }
    if let Some(record) = records
        .iter()
        .find(|record| record.id.eq_ignore_ascii_case(id))
    {
        return Ok(record);
    }
    let needle = id.to_lowercase();
    let matches = records
        .iter()
        .filter(|record| {
            record.id.to_lowercase().contains(&needle)
                || record.text.to_lowercase().contains(&needle)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => bail!("no memory named {id:?} found"),
        many => {
            eprintln!("multiple memories match {id:?}:");
            for record in many {
                eprintln!("  - [{}] {}", record.id, record.text);
            }
            bail!("memory id is ambiguous")
        }
    }
}

fn resolve_memory_ids(records: &[MemoryRecord], ids: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();
    for id in ids {
        let record = resolve_memory(records, id)?;
        if seen.insert(record.id.clone()) {
            resolved.push(record.id.clone());
        }
    }
    Ok(resolved)
}

fn resolve_idea<'a>(records: &'a [IdeaRecord], id: &str) -> Result<&'a IdeaRecord> {
    if let Some(record) = records.iter().find(|record| record.id == id) {
        return Ok(record);
    }
    if let Some(record) = records
        .iter()
        .find(|record| record.id.eq_ignore_ascii_case(id))
    {
        return Ok(record);
    }
    let needle = id.to_lowercase();
    let matches = records
        .iter()
        .filter(|record| {
            record.id.to_lowercase().contains(&needle)
                || record.text.to_lowercase().contains(&needle)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => bail!("no idea named {id:?} found"),
        many => {
            eprintln!("multiple ideas match {id:?}:");
            for record in many {
                eprintln!("  - [{}] {}", record.id, record.text);
            }
            bail!("idea id is ambiguous")
        }
    }
}

fn resolve_action<'a>(records: &'a [ActionRecord], id: &str) -> Result<&'a ActionRecord> {
    if let Some(record) = records.iter().find(|record| record.id == id) {
        return Ok(record);
    }
    if let Some(record) = records
        .iter()
        .find(|record| record.id.eq_ignore_ascii_case(id))
    {
        return Ok(record);
    }
    let needle = id.to_lowercase();
    let matches = records
        .iter()
        .filter(|record| {
            record.id.to_lowercase().contains(&needle)
                || record.text.to_lowercase().contains(&needle)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => bail!("no action named {id:?} found"),
        many => {
            eprintln!("multiple actions match {id:?}:");
            for record in many {
                eprintln!("  - [{}] {}", record.id, record.text);
            }
            bail!("action id is ambiguous")
        }
    }
}

fn resolve_suggestion<'a>(
    records: &'a [SuggestionRecord],
    id: &str,
) -> Result<&'a SuggestionRecord> {
    if let Some(record) = records.iter().find(|record| record.id == id) {
        return Ok(record);
    }
    if let Some(record) = records
        .iter()
        .find(|record| record.id.eq_ignore_ascii_case(id))
    {
        return Ok(record);
    }
    let needle = id.to_lowercase();
    let matches = records
        .iter()
        .filter(|record| {
            record.id.to_lowercase().contains(&needle)
                || record.text.to_lowercase().contains(&needle)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => bail!("no suggestion named {id:?} found"),
        many => {
            eprintln!("multiple suggestions match {id:?}:");
            for record in many {
                eprintln!("  - [{}] {}", record.id, record.text);
            }
            bail!("suggestion id is ambiguous")
        }
    }
}

fn resolve_suggestion_ids(records: &[SuggestionRecord], ids: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();
    for id in ids {
        let record = resolve_suggestion(records, id)?;
        if seen.insert(record.id.clone()) {
            resolved.push(record.id.clone());
        }
    }
    Ok(resolved)
}

fn memory_matches(record: &MemoryRecord, query: &str) -> bool {
    record.id.to_lowercase().contains(query)
        || record.text.to_lowercase().contains(query)
        || record.scope.to_lowercase().contains(query)
        || record.kind.to_lowercase().contains(query)
        || record.confidence.to_lowercase().contains(query)
        || record.not_before.to_lowercase().contains(query)
        || record
            .evidence
            .iter()
            .any(|evidence| evidence.to_lowercase().contains(query))
}

fn suggestion_matches(record: &SuggestionRecord, query: &str) -> bool {
    record.id.to_lowercase().contains(query)
        || record.text.to_lowercase().contains(query)
        || record.status.to_lowercase().contains(query)
        || record.target.to_lowercase().contains(query)
        || record.rationale.to_lowercase().contains(query)
        || record.draft.to_lowercase().contains(query)
        || record
            .evidence
            .iter()
            .any(|evidence| evidence.to_lowercase().contains(query))
}

fn format_memory_source(source: &MemorySource) -> String {
    let label = if !source.title.trim().is_empty() {
        source.title.as_str()
    } else if !source.chat_id.trim().is_empty() {
        source.chat_id.as_str()
    } else if !source.source_id.trim().is_empty() {
        source.source_id.as_str()
    } else {
        "unknown source"
    };

    let availability = if source.source_type == "chat" || !source.chat_id.is_empty() {
        "legacy chat reference"
    } else {
        "external"
    };

    let mut parts = vec![format!("{label} — {availability}")];
    if !source.source_type.trim().is_empty() {
        parts.push(format!("type: {}", source.source_type));
    }
    if !source.source.trim().is_empty() {
        parts.push(format!("source: {}", source.source));
    }
    if !source.source_id.trim().is_empty() {
        parts.push(format!("source-id: {}", source.source_id));
    }
    if !source.chat_id.trim().is_empty() {
        parts.push(format!("chat-id: {}", source.chat_id));
    }
    if !source.captured_at.trim().is_empty() {
        parts.push(format!("captured: {}", source.captured_at));
    }
    parts.join("; ")
}

fn format_memory_suffix(record: &MemoryRecord) -> String {
    let mut parts = Vec::new();
    if !record.scope.trim().is_empty() {
        parts.push(record.scope.as_str());
    }
    if !record.kind.trim().is_empty() {
        parts.push(record.kind.as_str());
    }
    if !record.confidence.trim().is_empty() {
        parts.push(record.confidence.as_str());
    }
    if !record.not_before.trim().is_empty() {
        parts.push(record.not_before.as_str());
    }
    if !record.sources.is_empty() {
        parts.push("sourced");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

fn format_idea_suffix(record: &IdeaRecord) -> String {
    let mut parts = Vec::new();
    if !record.scope.trim().is_empty() {
        parts.push(record.scope.as_str());
    }
    if !record.kind.trim().is_empty() {
        parts.push(record.kind.as_str());
    }
    if !record.confidence.trim().is_empty() {
        parts.push(record.confidence.as_str());
    }
    if !record.sources.is_empty() {
        parts.push("sourced");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

fn format_action_suffix(record: &ActionRecord) -> String {
    let mut parts = Vec::new();
    if !record.status.trim().is_empty() {
        parts.push(record.status.as_str());
    }
    if !record.scope.trim().is_empty() {
        parts.push(record.scope.as_str());
    }
    if !record.priority.trim().is_empty() {
        parts.push(record.priority.as_str());
    }
    if !record.sources.is_empty() {
        parts.push("sourced");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

fn format_suggestion_suffix(record: &SuggestionRecord) -> String {
    let mut parts = Vec::new();
    if !record.status.trim().is_empty() {
        parts.push(record.status.as_str());
    }
    if !record.target.trim().is_empty() {
        parts.push(record.target.as_str());
    }
    if !record.sources.is_empty() {
        parts.push("sourced");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

fn format_skill_suffix(record: &SkillRecord) -> String {
    let mut parts = vec![record.source.as_str()];
    if record.managed {
        parts.push("managed");
    }
    format!(" ({})", parts.join(", "))
}

fn format_context_suffix(record: &ContextRecord) -> String {
    let mut parts = Vec::new();
    if !record.memory_scope.trim().is_empty() {
        parts.push(format!("scope: {}", record.memory_scope));
    }
    if !record.roots.is_empty() {
        parts.push(format!("roots: {}", record.roots.len()));
    }
    if !record.skill_roots.is_empty() {
        parts.push(format!("skill-roots: {}", record.skill_roots.len()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

fn output_format(format: OutputFormat, json: bool) -> OutputFormat {
    if json {
        OutputFormat::Json
    } else {
        format
    }
}

fn format_roots(roots: &[PathBuf]) -> String {
    roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn write_tools_index(roots: &[PathBuf], entries: &[ToolEntry], index_path: &Path) -> Result<bool> {
    let index_entries = entries
        .iter()
        .map(|entry| djinn_core::IndexEntry {
            name: entry.name.clone(),
            description: entry.description.clone(),
            path: entry.path.to_string_lossy().replace('\\', "/"),
            line: entry.line,
        })
        .collect::<Vec<_>>();
    let payload = djinn_core::IndexPayload {
        schema_version: 1,
        source: "djinn-rust-tool-scan".to_string(),
        root: format_roots(roots),
        count: index_entries.len(),
        entries: index_entries,
    };
    let mut rendered = serde_json::to_vec_pretty(&payload)?;
    rendered.push(b'\n');
    djinn_core::write_if_changed(index_path, &rendered)
}

pub(crate) fn memory_store() -> djinn_memory::MemoryStore {
    djinn_memory::MemoryStore::default_in(&djinn_core::default_data_dir())
}

fn idea_store() -> IdeaStore {
    IdeaStore::default_in(&djinn_core::default_data_dir())
}

pub(crate) fn action_store() -> ActionStore {
    ActionStore::default_in(&djinn_core::default_data_dir())
}

fn suggestion_store() -> SuggestionStore {
    SuggestionStore::default_in(&djinn_core::default_data_dir())
}

pub(crate) fn skill_store() -> SkillStore {
    SkillStore::default_in(&djinn_core::default_data_dir())
}

fn context_store() -> ContextStore {
    ContextStore::default_in(&djinn_core::default_data_dir())
}

fn agent_session_store() -> JsonlAgentSessionStore {
    JsonlAgentSessionStore::default_in(&djinn_core::default_data_dir())
}

fn file_history_store() -> JsonlFileHistoryStore {
    JsonlFileHistoryStore::default_in(&djinn_core::default_data_dir())
}

fn skill_records() -> Result<Vec<SkillRecord>> {
    let store = skill_store();
    let mut roots = store.default_roots();
    if let Some(ctx) = context_store().active()? {
        for root in ctx.skill_roots {
            if !roots.iter().any(|existing| existing.path == root) {
                roots.push(SkillRoot {
                    path: root,
                    source: format!("ctx:{}", ctx.name),
                    managed: false,
                });
            }
        }
    }
    Ok(discover_skills(&roots)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use djinn_memory::AgentSessionTokenUsage;

    fn temp_agent_store(name: &str) -> JsonlAgentSessionStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-cli-agent-chat-{name}-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        JsonlAgentSessionStore::default_in(&dir)
    }

    fn test_memory(kind: &str, text: &str) -> MemoryRecord {
        MemoryRecord {
            id: "memory".to_string(),
            text: text.to_string(),
            created_at: "2026-07-09".to_string(),
            status: "active".to_string(),
            scope: "project:djinn".to_string(),
            kind: kind.to_string(),
            confidence: "medium".to_string(),
            not_before: String::new(),
            evidence: Vec::new(),
            sources: Vec::new(),
        }
    }

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
    fn configured_agent_roles_render_effective_model_and_resolve_names() {
        let config = parse_djinn_config(
            r#"{
              "version": 1,
              "profiles": {
                "review": {"model": "openai/gpt-5.5"}
              },
              "agents": {
                "reviewer": {
                  "description": "Review code diffs",
                  "profile": "review",
                  "instructions": ["docs/review.md"],
                  "tools": ["read_file", "search_files"]
                },
                "planner": {
                  "model": "copilot/gpt-4.1"
                }
              }
            }"#,
        )
        .unwrap();

        let roles = configured_agent_roles(&config);
        assert_eq!(roles.len(), 2);
        let reviewer = resolve_agent_role(&roles, "review").unwrap();
        assert_eq!(reviewer.name, "reviewer");
        assert_eq!(reviewer.effective_model.as_deref(), Some("openai/gpt-5.5"));
        assert_eq!(reviewer.tools, vec!["read_file", "search_files"]);
        let rendered = format_agent_role_list(&roles, OutputFormat::Text).unwrap();
        assert!(rendered.contains("Djinn agent roles"));
        assert!(rendered.contains("reviewer"));
        assert!(rendered.contains("model: openai/gpt-5.5"));
        let show = format_agent_role(reviewer, OutputFormat::Text).unwrap();
        assert!(show.contains("Name: reviewer"));
        assert!(show.contains("Effective model: openai/gpt-5.5"));
        assert!(show.contains("docs/review.md"));

        let json = format_agent_role(reviewer, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "reviewer");
        assert_eq!(value["effective_model"], "openai/gpt-5.5");
    }

    #[test]
    fn native_djinn_config_parses_merges_and_renders_without_raw_secrets() {
        let base = parse_djinn_config(
            r#"{
              "version": 1,
              "default_profile": "default",
              "providers": {
                "openai": {"type": "openai", "auth": "env:OPENAI_API_KEY"}
              },
              "profiles": {
                "default": {"model": "openai/gpt-4.1-mini"}
              }
            }"#,
        )
        .unwrap();
        let project = parse_djinn_config(
            r#"{
              "version": 1,
              "default_profile": "work",
              "providers": {
                "copilot": {"type": "copilot", "auth": "auto"}
              },
              "profiles": {
                "work": {
                  "model": "copilot/gpt-4.1",
                  "instructions": ["AGENTS.md"],
                  "permissions": [{"action": "shell", "resource": "cargo test", "effect": "ask"}]
                }
              },
              "permissions": [{"action": "read", "resource": "src/**", "effect": "allow"}]
            }"#,
        )
        .unwrap();

        let effective = merge_djinn_configs(vec![base, project]);

        assert_eq!(effective.default_profile.as_deref(), Some("work"));
        assert!(effective.providers.contains_key("openai"));
        assert!(effective.providers.contains_key("copilot"));
        assert_eq!(
            effective
                .profiles
                .get("work")
                .and_then(|profile| profile.model.as_deref()),
            Some("copilot/gpt-4.1")
        );

        let rendered = format_djinn_config_load_report(
            &DjinnConfigLoadReport {
                checked_paths: vec![
                    "/tmp/config.json".to_string(),
                    "/tmp/.djinn.json".to_string(),
                ],
                files: Vec::new(),
                effective,
                warnings: Vec::new(),
            },
            OutputFormat::Text,
        )
        .unwrap();
        assert!(rendered.contains("default_profile: work"));
        assert!(rendered.contains("copilot/gpt-4.1"));
        assert!(!rendered.contains("sk-"));
    }

    #[test]
    fn native_djinn_config_doctor_classifies_unknown_and_secret_like_fields() {
        let value: Value = serde_json::from_str(
            r#"{
              "version": 1,
              "profiles": {},
              "api_key": "sk-secret",
              "surprise": true
            }"#,
        )
        .unwrap();

        let report = djinn_config_doctor_from_value(Path::new("/tmp/config.json"), &value);

        assert!(report
            .mapped
            .iter()
            .any(|finding| finding.pointer == "/version"));
        assert!(report
            .mapped
            .iter()
            .any(|finding| finding.pointer == "/profiles"));
        assert!(report
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/api_key"));
        assert!(report
            .unknown
            .iter()
            .any(|finding| finding.pointer == "/surprise"));
    }

    #[test]
    fn native_djinn_config_supplies_profile_model_and_permission_rules() {
        let config = parse_djinn_config(
            r#"{
              "version": 1,
              "default_profile": "work",
              "profiles": {
                "work": {
                  "model": "copilot/gpt-4.1",
                  "permissions": [
                    {"action": "shell", "resource": "cargo test", "effect": "ask"}
                  ]
                }
              },
              "permissions": [
                {"action": "read", "resource": "src/**", "effect": "allow"}
              ]
            }"#,
        )
        .unwrap();
        let workspace = PathBuf::from("/tmp/djinn-native-config-test");
        let mut read_rules = Vec::new();
        let mut permission_rules = Vec::new();

        assert_eq!(
            profile_model_from_config(&config, "work").as_deref(),
            Some("copilot/gpt-4.1")
        );
        extend_read_access_rules_from_permissions(&config.permissions, &workspace, &mut read_rules);
        extend_permission_rules_from_config(
            &config.profiles["work"].permissions,
            &workspace,
            &mut permission_rules,
        );

        assert_eq!(read_rules.len(), 1);
        assert_eq!(
            read_rules[0].pattern,
            "/tmp/djinn-native-config-test/src/**"
        );
        assert_eq!(read_rules[0].effect, ReadAccessEffect::Allow);
        assert_eq!(permission_rules.len(), 1);
        assert_eq!(permission_rules[0].action, "shell");
        assert_eq!(permission_rules[0].resource, "cargo test");
        assert_eq!(permission_rules[0].effect, PermissionEffect::Ask);
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
    fn opencode_config_doctor_classifies_mapped_unsupported_unknown_and_secrets() {
        let value: Value = serde_json::from_str(
            r#"{
              "model": "openai/gpt-4.1",
              "default_agent": "coder",
              "agent": {
                "coder": {
                  "model": "copilot/gpt-4.1",
                  "permissions": [{"action": "read", "resource": "src/**", "effect": "allow"}]
                }
              },
              "providers": {
                "openai": {"apiKey": "sk-secret"}
              },
              "commands": {"test": "cargo test"},
              "mcpServers": {},
              "surprise": true
            }"#,
        )
        .unwrap();

        let report = opencode_config_doctor_from_value(Path::new("/tmp/opencode.json"), &value);

        assert!(report
            .mapped
            .iter()
            .any(|finding| finding.pointer == "/model"));
        assert!(report
            .mapped
            .iter()
            .any(|finding| finding.pointer == "/agent/coder/model"));
        assert!(report
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/commands"));
        assert!(report
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/mcpServers"));
        assert!(report
            .unknown
            .iter()
            .any(|finding| finding.pointer == "/surprise"));
        assert!(report
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/providers/openai/apiKey"));

        let rendered = format_config_doctor_report(
            &ConfigDoctorReport {
                source: "opencode".to_string(),
                checked_paths: vec!["/tmp/opencode.json".to_string()],
                summary: config_doctor_summary(&[report.clone()]),
                files: vec![report],
            },
            OutputFormat::Text,
        )
        .unwrap();
        assert!(rendered.contains("/providers/openai/apiKey"));
        assert!(!rendered.contains("sk-secret"));
    }

    #[test]
    fn opencode_config_import_preview_maps_patch_without_secret_values() {
        let value: Value = serde_json::from_str(
            r#"{
              "model": "openai/gpt-4.1-mini",
              "default_agent": "coder",
              "enabled_providers": ["openai", "github-copilot"],
              "agent": {
                "coder": {
                  "model": "copilot/gpt-4.1",
                  "permissions": [
                    {"action": "bash", "resource": "cargo test", "effect": "ask"}
                  ]
                }
              },
              "permission": {
                "read": {"src/**": "allow"}
              },
              "providers": {
                "openai": {"apiKey": "sk-secret"}
              },
              "commands": {"test": "cargo test"}
            }"#,
        )
        .unwrap();

        let preview = opencode_config_import_preview_from_values(
            vec!["/tmp/opencode.json".to_string()],
            vec![(PathBuf::from("/tmp/opencode.json"), value)],
            Vec::new(),
        );

        assert_eq!(preview.patch.default_profile.as_deref(), Some("coder"));
        assert_eq!(
            preview
                .patch
                .profiles
                .get("coder")
                .and_then(|profile| profile.model.as_deref()),
            Some("copilot/gpt-4.1")
        );
        assert!(preview.patch.providers.contains_key("openai"));
        assert!(preview.patch.providers.contains_key("github-copilot"));
        assert!(preview.patch.providers.contains_key("copilot"));
        assert_eq!(
            preview
                .patch
                .providers
                .get("openai")
                .and_then(|provider| provider.auth.as_deref()),
            Some("opencode:/providers/openai/apiKey")
        );
        assert!(preview
            .patch
            .permissions
            .iter()
            .any(|permission| permission.action == "read"
                && permission.resource == "src/**"
                && permission.effect == "allow"));
        assert!(preview
            .patch
            .profiles
            .get("coder")
            .unwrap()
            .permissions
            .iter()
            .any(|permission| permission.action == "shell"
                && permission.resource == "cargo test"
                && permission.effect == "ask"));
        assert!(preview
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/commands"));
        assert!(preview
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/providers/openai/apiKey"));

        let rendered = format_config_import_preview(&preview, OutputFormat::Text).unwrap();
        assert!(rendered.contains("default_profile: coder"));
        assert!(rendered.contains("model: copilot/gpt-4.1"));
        assert!(rendered.contains("opencode:/providers/openai/apiKey"));
        assert!(!rendered.contains("sk-secret"));
    }

    #[test]
    fn config_import_write_refuses_overwrite_without_force() {
        let mut config = DjinnConfig::default();
        config.default_profile = Some("default".to_string());
        let path = std::env::temp_dir().join(format!(
            "djinn-config-overwrite-test-{}.json",
            current_time_millis()
        ));
        fs::write(&path, "existing\n").unwrap();

        let error = write_djinn_config_file(&config, &path, false).unwrap_err();
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing\n");

        let overwritten = write_djinn_config_file(&config, &path, true).unwrap();
        assert!(overwritten);
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("\"version\": 1"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_import_write_report_serializes_native_config_without_secret_values() {
        let value: Value = serde_json::from_str(
            r#"{
              "default_agent": "coder",
              "agent": {"coder": {"model": "copilot/gpt-4.1"}},
              "providers": {"openai": {"apiKey": "sk-secret"}}
            }"#,
        )
        .unwrap();
        let preview = opencode_config_import_preview_from_values(
            vec!["/tmp/opencode.json".to_string()],
            vec![(PathBuf::from("/tmp/opencode.json"), value)],
            Vec::new(),
        );
        let path = std::env::temp_dir().join(format!(
            "djinn-config-write-report-test-{}.json",
            current_time_millis()
        ));

        let report = write_config_import_preview(&preview, &path, false).unwrap();
        let rendered = format_config_import_write_report(&report, OutputFormat::Json).unwrap();

        assert!(rendered.contains("copilot/gpt-4.1"));
        assert!(rendered.contains("opencode:/providers/openai/apiKey"));
        assert!(!rendered.contains("sk-secret"));
        assert!(path.exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_import_write_merges_existing_config_without_overwriting_same_name_profiles() {
        let value: Value = serde_json::from_str(
            r#"{
              "oauth_token": "ghu-secret-token",
              "models": ["gpt-4.1"]
            }"#,
        )
        .unwrap();
        let mut preview = copilot_config_import_preview_from_values(
            vec!["/tmp/copilot.json".to_string()],
            vec![(PathBuf::from("/tmp/copilot.json"), value)],
            Vec::new(),
        );
        preview.patch.profiles.insert(
            "sonnet".to_string(),
            DjinnProfilePatchPreview {
                model: Some("copilot/claude-sonnet-4".to_string()),
                instructions: Vec::new(),
                permissions: Vec::new(),
                source_pointers: vec!["/tmp/copilot.json".to_string()],
            },
        );
        let path = std::env::temp_dir().join(format!(
            "djinn-config-merge-import-test-{}.json",
            current_time_millis()
        ));
        fs::write(
            &path,
            r#"{
              "version": 1,
              "default_profile": "🧠",
              "providers": {
                "github-copilot": {"type": "github-copilot", "auth": "auto"},
                "openai": {"type": "openai", "auth": "env:OPENAI_API_KEY"}
              },
              "profiles": {
                "🧠": {"model": "openai/gpt-5.5"},
                "default": {"model": "openai/gpt-4.1"}
              }
            }
            "#,
        )
        .unwrap();

        let report = write_config_import_preview(&preview, &path, false).unwrap();
        let written = parse_djinn_config(&fs::read_to_string(&path).unwrap()).unwrap();

        assert!(report.merged);
        assert!(!report.overwritten);
        assert_eq!(written.default_profile.as_deref(), Some("🧠"));
        assert_eq!(
            written
                .profiles
                .get("default")
                .and_then(|profile| profile.model.as_deref()),
            Some("openai/gpt-4.1")
        );
        assert_eq!(
            written
                .profiles
                .get("sonnet")
                .and_then(|profile| profile.model.as_deref()),
            Some("copilot/claude-sonnet-4")
        );
        assert!(written.providers.contains_key("openai"));
        assert!(written.providers.contains_key("github-copilot"));
        assert!(!written.providers.contains_key("copilot"));
        assert_eq!(report.summary.added_providers, Vec::<String>::new());
        assert_eq!(report.summary.skipped_providers, vec!["copilot"]);
        assert_eq!(report.summary.added_profiles, vec!["sonnet"]);
        assert_eq!(report.summary.skipped_profiles, vec!["default"]);
        assert_eq!(
            report.summary.preserved_default_profile.as_deref(),
            Some("🧠")
        );
        assert_eq!(
            report.summary.skipped_import_default_profile.as_deref(),
            Some("default")
        );
        let rendered = format_config_import_write_report(&report, OutputFormat::Text).unwrap();
        assert!(rendered.contains("providers: added 0; skipped 1 (copilot)"));
        assert!(rendered.contains("profiles: added 1 (sonnet); skipped 1 (default)"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn opencode_config_export_preview_projects_native_config_without_secret_values() {
        let mut config = DjinnConfig::default();
        config.default_profile = Some("coder".to_string());
        config.providers.insert(
            "openai".to_string(),
            DjinnConfigProvider {
                provider_type: "openai".to_string(),
                auth: Some("sk-secret".to_string()),
                endpoint: None,
            },
        );
        config.profiles.insert(
            "coder".to_string(),
            DjinnConfigProfile {
                model: Some("openai/gpt-4.1".to_string()),
                instructions: vec!["AGENTS.md".to_string()],
                permissions: vec![DjinnConfigPermission {
                    action: "shell".to_string(),
                    resource: "cargo test".to_string(),
                    effect: "ask".to_string(),
                }],
                tools: Vec::new(),
                agent: None,
            },
        );
        config.permissions.push(DjinnConfigPermission {
            action: "read".to_string(),
            resource: "src/**".to_string(),
            effect: "allow".to_string(),
        });
        config.commands.insert(
            "test".to_string(),
            DjinnConfigCommandTemplate {
                prompt: "Run tests".to_string(),
                description: None,
            },
        );

        let preview = opencode_config_export_preview_from_load_report(DjinnConfigLoadReport {
            checked_paths: vec!["/tmp/djinn.json".to_string()],
            files: vec![DjinnConfigFileReport {
                path: "/tmp/djinn.json".to_string(),
                exists: true,
                readable: true,
                errors: Vec::new(),
            }],
            effective: config,
            warnings: Vec::new(),
        });

        assert_eq!(preview.target, "opencode");
        assert_eq!(preview.config["default_agent"], "coder");
        assert_eq!(preview.config["model"], "openai/gpt-4.1");
        assert_eq!(preview.config["agent"]["coder"]["model"], "openai/gpt-4.1");
        assert_eq!(
            preview.config["agent"]["coder"]["permissions"][0]["action"],
            "bash"
        );
        assert_eq!(preview.config["permissions"][0]["action"], "read");
        assert!(preview
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/commands"));
        assert!(preview
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/profiles/coder/instructions"));
        assert!(preview
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/providers/openai/auth"));

        let rendered = format_config_export_preview(&preview, OutputFormat::Json).unwrap();
        assert!(rendered.contains("openai/gpt-4.1"));
        assert!(!rendered.contains("sk-secret"));
    }

    #[test]
    fn copilot_config_doctor_and_import_preview_map_models_without_tokens() {
        let value: Value = serde_json::from_str(
            r#"{
              "oauth_token": "ghu-secret-token",
              "models": ["gpt-4.1", {"id": "claude-sonnet-4"}],
              "unknownFlag": true
            }"#,
        )
        .unwrap();

        let doctor = copilot_config_doctor_from_value(Path::new("/tmp/copilot.json"), &value);
        assert!(doctor.mapped.iter().any(|finding| finding.pointer == "/"));
        assert!(doctor
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/oauth_token"));
        assert!(doctor
            .unknown
            .iter()
            .any(|finding| finding.pointer == "/unknownFlag"));

        let preview = copilot_config_import_preview_from_values(
            vec!["/tmp/copilot.json".to_string()],
            vec![(PathBuf::from("/tmp/copilot.json"), value)],
            Vec::new(),
        );
        assert_eq!(preview.source, "copilot");
        assert_eq!(
            preview
                .patch
                .providers
                .get("copilot")
                .and_then(|provider| provider.auth.as_deref()),
            Some("auto")
        );
        assert_eq!(
            preview
                .patch
                .profiles
                .get("default")
                .and_then(|profile| profile.model.as_deref()),
            Some("copilot/gpt-4.1")
        );
        let rendered = format_config_import_preview(&preview, OutputFormat::Json).unwrap();
        assert!(rendered.contains("copilot/gpt-4.1"));
        assert!(!rendered.contains("ghu-secret-token"));
    }

    #[test]
    fn copilot_config_import_preview_without_sources_does_not_invent_provider() {
        let preview = copilot_config_import_preview_from_values(Vec::new(), Vec::new(), Vec::new());

        assert!(preview.readable_files.is_empty());
        assert!(preview.patch.providers.is_empty());
        assert!(preview.patch.profiles.is_empty());
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning == "no readable Copilot config files found"));
    }

    #[test]
    fn copilot_config_export_preview_projects_native_config_without_secret_values() {
        let mut config = DjinnConfig::default();
        config.default_profile = Some("default".to_string());
        config.providers.insert(
            "copilot".to_string(),
            DjinnConfigProvider {
                provider_type: "copilot".to_string(),
                auth: Some("ghu-secret-token".to_string()),
                endpoint: None,
            },
        );
        config.profiles.insert(
            "default".to_string(),
            DjinnConfigProfile {
                model: Some("copilot/gpt-4.1".to_string()),
                instructions: Vec::new(),
                permissions: vec![DjinnConfigPermission {
                    action: "shell".to_string(),
                    resource: "*".to_string(),
                    effect: "ask".to_string(),
                }],
                tools: Vec::new(),
                agent: None,
            },
        );

        let preview = copilot_config_export_preview_from_load_report(DjinnConfigLoadReport {
            checked_paths: vec!["/tmp/djinn.json".to_string()],
            files: vec![DjinnConfigFileReport {
                path: "/tmp/djinn.json".to_string(),
                exists: true,
                readable: true,
                errors: Vec::new(),
            }],
            effective: config,
            warnings: Vec::new(),
        });

        assert_eq!(preview.target, "copilot");
        assert_eq!(preview.config["provider"], "github-copilot");
        assert_eq!(preview.config["model"], "gpt-4.1");
        assert_eq!(preview.config["models"][0], "gpt-4.1");
        assert!(preview
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/profiles"));
        assert!(preview
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/providers/copilot/auth"));

        let rendered = format_config_export_preview(&preview, OutputFormat::Json).unwrap();
        assert!(rendered.contains("gpt-4.1"));
        assert!(!rendered.contains("ghu-secret-token"));
    }

    #[test]
    fn config_export_write_refuses_overwrite_without_force() {
        let value = serde_json::json!({"model": "openai/gpt-4.1"});
        let path = std::env::temp_dir().join(format!(
            "opencode-config-overwrite-test-{}.json",
            current_time_millis()
        ));
        fs::write(&path, "existing\n").unwrap();

        let error = write_json_config_file(&value, &path, false, "OpenCode").unwrap_err();
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing\n");

        let overwritten = write_json_config_file(&value, &path, true, "OpenCode").unwrap();
        assert!(overwritten);
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("openai/gpt-4.1"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_export_write_report_serializes_opencode_config_without_secret_values() {
        let preview = ConfigExportPreview {
            target: "opencode".to_string(),
            mode: "dry-run".to_string(),
            checked_paths: vec!["/tmp/djinn.json".to_string()],
            readable_files: vec!["/tmp/djinn.json".to_string()],
            config: serde_json::json!({"model": "openai/gpt-4.1"}),
            unsupported: Vec::new(),
            secrets: vec![config_finding(
                "/providers/openai/auth",
                "Djinn provider auth reference",
                "not exported raw",
                "OpenCode export omits provider auth reference `<redacted>`.",
            )],
            warnings: Vec::new(),
        };
        let path = std::env::temp_dir().join(format!(
            "opencode-config-write-report-test-{}.json",
            current_time_millis()
        ));

        let report = write_config_export_preview(&preview, &path, false).unwrap();
        let rendered = format_config_export_write_report(&report, OutputFormat::Json).unwrap();

        assert!(rendered.contains("openai/gpt-4.1"));
        assert!(rendered.contains("/providers/openai/auth"));
        assert!(!rendered.contains("sk-secret"));
        assert!(path.exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn copilot_model_prefixes_route_to_copilot_provider() {
        assert!(is_copilot_model("copilot/gpt-4.1"));
        assert!(is_copilot_model("github-copilot/claude-sonnet-4"));
        assert!(!is_copilot_model("openai/gpt-4o-mini"));
        assert!(!is_copilot_model("gpt-4o-mini"));
    }

    #[test]
    fn copilot_oauth_token_reads_hosts_and_apps_json_shapes() {
        let hosts = r#"{
          "github.com": {
            "oauth_token": "ghu-host-token",
            "user": "octo"
          }
        }"#;
        let apps = r#"{
          "apps": [
            {"github": {"oauthToken": "ghu-app-token"}}
          ]
        }"#;

        assert_eq!(
            copilot_oauth_token_from_content(hosts).unwrap().as_deref(),
            Some("ghu-host-token")
        );
        assert_eq!(
            copilot_oauth_token_from_content(apps).unwrap().as_deref(),
            Some("ghu-app-token")
        );
    }

    #[test]
    fn copilot_model_options_read_models_without_leaking_auth_strings() {
        let content = r#"{
          "github.com": {
            "oauth_token": "ghu-host-token",
            "user": "octo"
          },
          "defaultModel": "gpt-4.1",
          "availableModels": [
            { "id": "gpt-4.1", "name": "GPT 4.1" },
            { "modelId": "claude-sonnet-4" },
            "o4-mini",
            "gemini-2.5-pro"
          ],
          "models": {
            "gpt-4o": { "label": "GPT 4o" },
            "not-a-model": { "label": "ignored" }
          }
        }"#;

        let models = copilot_model_options_from_content(content).unwrap();

        assert_eq!(
            models,
            vec![
                "copilot/gpt-4.1",
                "copilot/gpt-4o",
                "copilot/claude-sonnet-4",
                "copilot/o4-mini"
            ]
        );
        assert!(!models.iter().any(|model| model.contains("ghu-host-token")));
        assert!(!models.iter().any(|model| model.contains("gemini")));
    }

    #[test]
    fn copilot_model_list_parser_normalizes_and_deduplicates() {
        let models = copilot_model_options_from_list(
            "gpt-4.1, copilot/gpt-4.1;github-copilot/claude-sonnet-4\n sk-secret",
        );

        assert_eq!(
            models,
            vec!["copilot/gpt-4.1", "github-copilot/claude-sonnet-4"]
        );
    }

    #[test]
    fn github_cli_auth_token_parser_reads_first_nonempty_line() {
        assert_eq!(
            github_cli_auth_token_from_stdout(b"\n  gho-cli-token  \nignored\n").as_deref(),
            Some("gho-cli-token")
        );
        assert_eq!(github_cli_auth_token_from_stdout(b"\n  \n"), None);
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
    fn folder_backed_session_projection_writes_events_and_context_without_duplicate_logs() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-folder-session-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("summary.md"), "old summary\n").unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("agt_folder"),
            meta: AgentSessionMeta {
                title: "Folder session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "new request".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "new summary".to_string(),
                }),
            ],
        };

        let projection =
            project_agent_session_dir(&dir, &session, "new request", "new summary").unwrap();

        assert_eq!(fs::read_to_string(dir.join("request.md")).unwrap(), "");
        assert_eq!(
            fs::read_to_string(dir.join("summary.md")).unwrap(),
            "new summary\n"
        );
        assert!(projection.context_dir.exists());
        assert!(projection.turn_dir.is_none());
        assert!(!dir.join("turns").exists());
        assert!(dir.join("djinn.toml").exists());
        let events_jsonl = fs::read_to_string(dir.join("events.jsonl")).unwrap();
        assert_eq!(events_jsonl.lines().count(), 2);
        assert!(events_jsonl.contains("\"type\":\"user_message\""));
        assert!(events_jsonl.contains("\"type\":\"assistant_message\""));
        write_folder_session_events_jsonl(&dir, &session).unwrap();
        let events_jsonl_after_second_shadow =
            fs::read_to_string(dir.join("events.jsonl")).unwrap();
        assert_eq!(events_jsonl_after_second_shadow.lines().count(), 2);
        assert!(!dir.join("logs/summary-history.md").exists());
        assert!(!dir.join("logs/events.jsonl").exists());
        assert!(!dir.join("logs/transcript.md").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_transcript_renders_markdown_from_events_jsonl() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-session-transcript-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("transcript-session"),
            meta: AgentSessionMeta {
                title: "Transcript Session".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![
                AgentSessionEvent::with_session(
                    AgentSessionId::new("transcript-session"),
                    AgentSessionEventKind::UserMessage {
                        content: "What is structured programming?".to_string(),
                    },
                ),
                AgentSessionEvent::with_session(
                    AgentSessionId::new("transcript-session"),
                    AgentSessionEventKind::AssistantMessage {
                        content: "It emphasizes clear control flow.".to_string(),
                    },
                ),
            ],
        };
        write_folder_session_events_jsonl(&dir, &session).unwrap();

        let report = build_session_transcript(&dir, SessionTranscriptFormat::Markdown).unwrap();
        let rendered = render_session_transcript_markdown(&report);

        assert_eq!(report.turn_count, 1);
        assert_eq!(report.turns[0].request_line, 1);
        assert_eq!(report.turns[0].response_line, 2);
        assert!(rendered.contains("# Session Transcript"));
        assert!(rendered.contains("## Turn 1"));
        assert!(rendered.contains("### User"));
        assert!(rendered.contains("What is structured programming?"));
        assert!(rendered.contains("### Assistant"));
        assert!(rendered.contains("It emphasizes clear control flow."));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_session_events_jsonl_hydrates_native_history_for_continuation() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-events-first-session-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let id = AgentSessionId::new("agt_events_first");
        fs::write(
            dir.join("djinn.toml"),
            "session_id = \"agt_events_first\"\ntitle = \"Events First\"\nworkspace = \"/tmp/workspace\"\nprofile = \"default\"\n",
        )
        .unwrap();
        let events = vec![
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "event request".to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::AssistantMessage {
                    content: "event response".to_string(),
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
        fs::write(dir.join("events.jsonl"), events_jsonl).unwrap();

        let stale = AgentSession {
            id: id.clone(),
            meta: AgentSessionMeta {
                title: "stale".to_string(),
                source: "djinn".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "stale native request".to_string(),
                },
            )],
        };
        write_agent_session_native_jsonl(&dir, &stale).unwrap();

        let manifest = read_folder_session_manifest(&dir).unwrap();
        assert!(
            hydrate_folder_agent_session_from_events_jsonl(&dir, &id, manifest.as_ref()).unwrap()
        );
        let loaded = folder_agent_session_store(&dir).load_session(&id).unwrap();
        let messages = agent_model_messages(&loaded, "/tmp/workspace", &[]);

        assert_eq!(loaded.events.len(), 2);
        assert_eq!(loaded.meta.workspace, "/tmp/workspace");
        assert!(messages
            .iter()
            .any(|message| message.content == "event request"));
        assert!(messages
            .iter()
            .any(|message| message.content == "event response"));
        assert!(!messages
            .iter()
            .any(|message| message.content.contains("stale native")));

        let _ = fs::remove_dir_all(&dir);
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
    #[cfg(unix)]
    fn buddy_bridge_backend_uses_hidden_json_protocol_for_list_and_create() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-bridge-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let request_log = root.join("bridge-requests.jsonl");
        let buddy_bin = root.join("buddy-bridge.sh");
        let script = r#"#!/bin/sh
if [ "$1" = "djinn-bridge" ]; then
  request=$(cat)
  printf '%s\n' "$request" >> '__REQUEST_LOG__'
  case "$request" in
    *list_sessions*)
      cat <<'JSON'
{"type":"sessions","sessions":[{"id":"bud_bridge","title":"Bridge Session","updated":0,"created":0,"projectId":"project-bridge","directory":"/tmp/bridge"}]}
JSON
      exit 0
      ;;
    *get_session*)
      cat <<'JSON'
{"type":"session","session":{"id":"bud_bridge","title":"Bridge Session","updated":0,"created":0,"projectId":"project-bridge","directory":"/tmp/bridge"}}
JSON
      exit 0
      ;;
    *create_session*)
      cat <<'JSON'
{"type":"created_session","session":{"id":"bud_created_bridge","title":"Created Through Bridge","repo_path":"/tmp/created","created_at":"2026-08-01T12:00:00Z"}}
JSON
      exit 0
      ;;
    *delete_session*)
      cat <<'JSON'
{"type":"deleted_session","session_id":"bud_created_bridge"}
JSON
      exit 0
      ;;
  esac
fi
printf 'legacy fallback unexpectedly used: %s\n' "$*" >&2
exit 2
"#
        .replace("__REQUEST_LOG__", &request_log.display().to_string());
        fs::write(&buddy_bin, script).unwrap();
        let mut permissions = fs::metadata(&buddy_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&buddy_bin, permissions).unwrap();

        let backend = BuddyBridgeBackend::explicit(buddy_bin.display().to_string());
        let sessions = backend.list_sessions().unwrap();
        let fetched = backend.get_session("bud_bridge").unwrap();
        let created = backend
            .create_session("Created Through Bridge", "/tmp/created")
            .unwrap();
        backend.delete_session("bud_created_bridge").unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "bud_bridge");
        assert_eq!(sessions[0].repo_path, "/tmp/bridge");
        assert_eq!(sessions[0].created_at, "1970-01-01T00:00:00+00:00");
        assert_eq!(fetched.id, "bud_bridge");
        assert_eq!(fetched.title, "Bridge Session");
        assert_eq!(created.id, "bud_created_bridge");
        assert_eq!(created.repo_path, "/tmp/created");

        let requests = fs::read_to_string(&request_log).unwrap();
        assert!(requests.contains(r#""type":"list_sessions""#));
        assert!(requests.contains(r#""type":"get_session""#));
        assert!(requests.contains(r#""type":"create_session""#));
        assert!(requests.contains(r#""type":"delete_session""#));
        assert!(requests.contains(r#""title":"Created Through Bridge""#));
        assert!(requests.contains(r#""repo_path":"/tmp/created""#));
        assert!(requests.contains(r#""session_id":"bud_created_bridge""#));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn buddy_bridge_backend_falls_back_to_legacy_cli_when_bridge_fails() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-bridge-fallback-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let fallback_log = root.join("fallback-log.txt");
        let buddy_bin = root.join("buddy-fallback.sh");
        let script = r#"#!/bin/sh
if [ "$1" = "djinn-bridge" ]; then
  echo bridge unavailable >&2
  exit 77
fi
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--format" ] && [ "$4" = "json" ]; then
  printf 'legacy-list\n' >> '__FALLBACK_LOG__'
  cat <<'JSON'
[{"id":"bud_legacy","title":"Legacy Session","updated":0,"created":0,"projectId":"project-legacy","directory":"/tmp/legacy"}]
JSON
  exit 0
fi
if [ "$1" = "session" ] && [ "$2" = "create" ]; then
  printf 'legacy-create:%s:%s\n' "$6" "$8" >> '__FALLBACK_LOG__'
  printf '{"id":"bud_legacy_created","title":"%s","repo_path":"%s","created_at":"2026-08-01T12:00:00Z"}\n' "$6" "$8"
  exit 0
fi
if [ "$1" = "session" ] && [ "$2" = "delete" ]; then
  printf 'legacy-delete:%s\n' "$3" >> '__FALLBACK_LOG__'
  exit 0
fi
echo unexpected buddy args: "$@" >&2
exit 2
"#
        .replace("__FALLBACK_LOG__", &fallback_log.display().to_string());
        fs::write(&buddy_bin, script).unwrap();
        let mut permissions = fs::metadata(&buddy_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&buddy_bin, permissions).unwrap();

        let backend = BuddyBridgeBackend::explicit(buddy_bin.display().to_string());
        let sessions = backend.list_sessions().unwrap();
        let fetched = backend.get_session("bud_legacy").unwrap();
        let created = backend
            .create_session("Fallback Title", "/tmp/fallback")
            .unwrap();
        backend.delete_session("bud_legacy_created").unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "bud_legacy");
        assert_eq!(sessions[0].repo_path, "/tmp/legacy");
        assert_eq!(fetched.id, "bud_legacy");
        assert_eq!(fetched.title, "Legacy Session");
        assert_eq!(created.id, "bud_legacy_created");
        assert_eq!(created.title, "Fallback Title");
        assert_eq!(created.repo_path, "/tmp/fallback");
        assert_eq!(
            fs::read_to_string(&fallback_log).unwrap(),
            "legacy-list\nlegacy-list\nlegacy-create:Fallback Title:/tmp/fallback\nlegacy-delete:bud_legacy_created\n"
        );

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

    #[test]
    fn buddy_command_resolver_uses_env_runtime_in_tree_then_unavailable() {
        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-command-resolver-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(root.join("tools/buddy/bin")).unwrap();
        let in_tree = root.join(IN_TREE_BUDDY_COMMAND);
        fs::write(&in_tree, "#!/bin/sh\n").unwrap();
        let runtime = Some("runtime-buddy --flag".to_string());

        assert_eq!(
            resolve_buddy_command_from(
                Some("env-buddy --debug".to_string()),
                runtime.clone(),
                Some(&root),
            ),
            Some("env-buddy --debug".to_string())
        );
        assert_eq!(
            resolve_buddy_command_from(Some("  ".to_string()), runtime.clone(), Some(&root)),
            Some("runtime-buddy --flag".to_string())
        );
        assert_eq!(
            resolve_buddy_command_from(None, Some("  ".to_string()), Some(&root)),
            Some(in_tree.display().to_string())
        );
        let in_tree_resolution = BuddyCommandResolution {
            command: in_tree.display().to_string(),
            source: IN_TREE_BUDDY_COMMAND.to_string(),
        };
        assert_eq!(in_tree_resolution.runtime_command_override(), None);
        let explicit_resolution = BuddyCommandResolution {
            command: "env-buddy --debug".to_string(),
            source: DJINN_BUDDY_BIN_ENV.to_string(),
        };
        assert_eq!(
            explicit_resolution.runtime_command_override().as_deref(),
            Some("env-buddy --debug")
        );
        assert_eq!(
            resolve_buddy_command_from(None, None, Some(&root.join("missing-root"))),
            None
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn buddy_doctor_report_explains_selected_source() {
        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-doctor-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(root.join("tools/buddy/bin")).unwrap();
        let in_tree = root.join(IN_TREE_BUDDY_COMMAND);
        fs::write(&in_tree, "#!/bin/sh\n").unwrap();

        let in_tree_report = buddy_command_doctor_report_from(None, None, Some(&root), None, None);
        assert_eq!(in_tree_report.command, in_tree.display().to_string());
        assert_eq!(in_tree_report.source, IN_TREE_BUDDY_COMMAND);
        assert!(in_tree_report.exists);
        assert!(!in_tree_report.executable);
        assert!(
            format_buddy_command_doctor_report(&in_tree_report, OutputFormat::Text)
                .unwrap()
                .contains("source: tools/buddy/bin/buddy")
        );
        assert!(!in_tree_report
            .candidates
            .iter()
            .any(|candidate| candidate.source == "buddy"));
        assert!(in_tree_report
            .note
            .contains("does not fall back to external Buddy"));

        let unavailable_report = buddy_command_doctor_report_from(
            None,
            None,
            Some(&root.join("missing-root")),
            None,
            None,
        );
        assert_eq!(unavailable_report.command, "<unavailable>");
        assert_eq!(unavailable_report.source, UNAVAILABLE_BUDDY_COMMAND_SOURCE);
        assert!(!unavailable_report.exists);
        assert!(!unavailable_report.executable);
        assert!(unavailable_report
            .note
            .contains("No Buddy command is configured"));

        let runtime_report = buddy_command_doctor_report_from(
            None,
            Some("/old/buddy --dev".to_string()),
            Some(&root),
            Some(Path::new("/tmp/session")),
            Some(Path::new("/tmp/session/runtime/buddy.json")),
        );
        assert_eq!(runtime_report.command, "/old/buddy --dev");
        assert_eq!(runtime_report.source, "runtime/buddy.json.command");
        assert_eq!(runtime_report.session_dir.as_deref(), Some("/tmp/session"));
        assert_eq!(
            runtime_report.runtime_path.as_deref(),
            Some("/tmp/session/runtime/buddy.json")
        );
        assert!(runtime_report.note.contains("runtime command overrides"));

        let json = format_buddy_command_doctor_report(&runtime_report, OutputFormat::Json).unwrap();
        assert!(json.contains("\"source\": \"runtime/buddy.json.command\""));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn buddy_doctor_reports_bridge_health() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-doctor-bridge-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let buddy_bin = root.join("buddy-bridge-ok.sh");
        fs::write(
            &buddy_bin,
            r#"#!/bin/sh
if [ "$1" = "djinn-bridge" ]; then
  cat >/dev/null
  printf '{"type":"sessions","sessions":[]}\n'
  exit 0
fi
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--format" ] && [ "$4" = "json" ]; then
  printf '[]\n'
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&buddy_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&buddy_bin, permissions).unwrap();

        let mut report = buddy_command_doctor_report_from(
            Some(buddy_bin.display().to_string()),
            None,
            None,
            None,
            None,
        );
        report.bridge = Some(probe_buddy_bridge_doctor(
            &report.command,
            report.exists && report.executable,
        ));

        let bridge = report.bridge.as_ref().unwrap();
        assert!(bridge.bridge_available);
        assert!(bridge.bridge_list_sessions_ok);
        assert!(bridge.fallback_available);
        assert!(bridge.fallback_list_sessions_ok);
        let text = format_buddy_command_doctor_report(&report, OutputFormat::Text).unwrap();
        assert!(text.contains("bridge:"));
        assert!(text.contains("status: ok"));
        let json = format_buddy_command_doctor_report(&report, OutputFormat::Json).unwrap();
        assert!(json.contains("\"bridge_available\": true"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn buddy_doctor_reports_bridge_failure_with_legacy_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-doctor-bridge-fallback-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let buddy_bin = root.join("buddy-bridge-fallback.sh");
        fs::write(
            &buddy_bin,
            r#"#!/bin/sh
if [ "$1" = "djinn-bridge" ]; then
  echo bridge unavailable >&2
  exit 77
fi
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--format" ] && [ "$4" = "json" ]; then
  printf '[]\n'
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&buddy_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&buddy_bin, permissions).unwrap();

        let mut report = buddy_command_doctor_report_from(
            Some(buddy_bin.display().to_string()),
            None,
            None,
            None,
            None,
        );
        report.bridge = Some(probe_buddy_bridge_doctor(
            &report.command,
            report.exists && report.executable,
        ));

        let bridge = report.bridge.as_ref().unwrap();
        assert!(!bridge.bridge_available);
        assert!(!bridge.bridge_list_sessions_ok);
        assert!(bridge.bridge_error.as_deref().unwrap().contains("status"));
        assert!(bridge.fallback_available);
        assert!(bridge.fallback_list_sessions_ok);
        let text = format_buddy_command_doctor_report(&report, OutputFormat::Text).unwrap();
        assert!(text.contains("status: unavailable; legacy CLI fallback will be used"));
        let json = format_buddy_command_doctor_report(&report, OutputFormat::Json).unwrap();
        assert!(json.contains("\"bridge_available\": false"));
        assert!(json.contains("\"fallback_available\": true"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn buddy_runtime_omits_command_when_no_override_is_recorded() {
        let root = std::env::temp_dir().join(format!(
            "djinn-buddy-runtime-command-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let runtime_path = root.join("runtime/buddy.json");
        write_buddy_runtime_state(
            &runtime_path,
            &BuddyRuntimeState {
                buddy_session: Some("ses_default_in_tree".to_string()),
                stale_buddy_sessions: Vec::new(),
                command: None,
                args: Vec::new(),
                last_run_at: None,
                last_prompt_chars: 0,
                last_response_chars: 0,
            },
        )
        .unwrap();

        let raw = fs::read_to_string(&runtime_path).unwrap();
        assert!(raw.contains("ses_default_in_tree"));
        assert!(!raw.contains("command"));

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
    fn session_validate_events_reports_event_turn_summary_agreement() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-validate-events-ok-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("agt_validate_events_ok"),
            meta: AgentSessionMeta {
                title: "Validate events".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "question".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "answer".to_string(),
                }),
            ],
        };
        project_agent_session_dir(&dir, &session, "question", "answer").unwrap();

        let report = validate_folder_session_events(&dir).unwrap();
        let text = format_session_validate_events_report(&report);

        assert!(report.all_valid);
        assert_eq!(report.event_count, 2);
        assert_eq!(report.event_turn_count, 1);
        assert_eq!(report.turn_count, 0);
        assert_eq!(report.root_summary_matches_latest_turn, Some(true));
        assert!(text.contains("status: valid"));
        assert!(text.contains("issues: none"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_validate_events_reports_mismatched_turn_and_summary() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-validate-events-mismatch-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let turn_dir = dir.join("turns/turn-1");
        fs::create_dir_all(&turn_dir).unwrap();
        fs::write(turn_dir.join("request.md"), "question\n").unwrap();
        fs::write(turn_dir.join("response.md"), "different answer\n").unwrap();
        fs::write(dir.join("summary.md"), "stale summary\n").unwrap();
        fs::write(
            dir.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"question\"}\n{\"type\":\"assistant_message\",\"content\":\"answer\"}\n",
        )
        .unwrap();

        let report = validate_folder_session_events(&dir).unwrap();
        let codes = report
            .issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<Vec<_>>();

        assert!(!report.all_valid);
        assert_eq!(report.event_turn_count, 1);
        assert_eq!(report.turn_count, 1);
        assert_eq!(report.root_summary_matches_latest_turn, Some(false));
        assert!(codes.contains(&"turn_response_mismatch"));
        assert!(codes.contains(&"root_summary_mismatch"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_validate_events_reports_duplicate_event_ids() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-validate-events-duplicate-id-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("summary.md"), "answer\n").unwrap();
        fs::write(
            dir.join("events.jsonl"),
            concat!(
                "{\"event_id\":\"buddy:user_message:msg_1\",\"type\":\"user_message\",\"content\":\"question\"}\n",
                "{\"event_id\":\"buddy:assistant_message:msg_2\",\"type\":\"assistant_message\",\"content\":\"answer\"}\n",
                "{\"event_id\":\"buddy:user_message:msg_1\",\"type\":\"checkpoint\",\"label\":\"duplicate envelope\"}\n",
            ),
        )
        .unwrap();

        let report = validate_folder_session_events(&dir).unwrap();

        assert!(!report.all_valid);
        assert_eq!(report.event_turn_count, 1);
        assert_eq!(report.root_summary_matches_latest_turn, Some(true));
        let duplicate = report
            .issues
            .iter()
            .find(|issue| issue.code == "duplicate_event_id")
            .expect("expected duplicate_event_id issue");
        assert_eq!(duplicate.line, Some(3));
        assert!(duplicate.message.contains("duplicates line 1"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_events_health_filters_duplicate_event_ids() {
        let root = std::env::temp_dir().join(format!(
            "djinn-events-health-duplicate-id-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let ok = root.join("ok-session");
        let duplicate = root.join("duplicate-session");
        fs::create_dir_all(&ok).unwrap();
        fs::create_dir_all(&duplicate).unwrap();
        fs::write(ok.join("summary.md"), "answer\n").unwrap();
        fs::write(
            ok.join("events.jsonl"),
            concat!(
                "{\"event_id\":\"buddy:user_message:msg_ok_1\",\"type\":\"user_message\",\"content\":\"question\"}\n",
                "{\"event_id\":\"buddy:assistant_message:msg_ok_2\",\"type\":\"assistant_message\",\"content\":\"answer\"}\n",
            ),
        )
        .unwrap();
        fs::write(duplicate.join("summary.md"), "answer\n").unwrap();
        fs::write(
            duplicate.join("events.jsonl"),
            concat!(
                "{\"event_id\":\"buddy:user_message:msg_dup\",\"type\":\"user_message\",\"content\":\"question\"}\n",
                "{\"event_id\":\"buddy:assistant_message:msg_dup_reply\",\"type\":\"assistant_message\",\"content\":\"answer\"}\n",
                "{\"event_id\":\"buddy:user_message:msg_dup\",\"type\":\"checkpoint\",\"label\":\"duplicate\"}\n",
            ),
        )
        .unwrap();

        let report =
            event_health_report_for_folder_session_root(&root, None, Some("duplicate_event_id"))
                .unwrap();
        let text = format_event_health_report(&report);

        assert_eq!(report.total, 1);
        assert_eq!(report.not_ready, 1);
        assert_eq!(report.sessions[0].name, "duplicate-session");
        assert!(report.sessions[0]
            .issue_codes
            .contains(&"duplicate_event_id".to_string()));
        assert!(text.contains("filter: duplicate_event_id"));
        assert!(text.contains("duplicate_event_id"));
        assert!(!text.contains("ok-session"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_project_events_renders_dry_run_turn_tree() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-project-events-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let turn_dir = dir.join("turns/existing-turn");
        fs::create_dir_all(&turn_dir).unwrap();
        fs::write(turn_dir.join("request.md"), "question\n").unwrap();
        fs::write(turn_dir.join("response.md"), "old answer\n").unwrap();
        fs::write(dir.join("summary.md"), "old answer\n").unwrap();
        fs::write(
            dir.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"question\"}\n{\"type\":\"assistant_message\",\"content\":\"new answer\"}\n{\"type\":\"user_message\",\"content\":\"follow-up\"}\n{\"type\":\"assistant_message\",\"content\":\"follow-up answer\"}\n",
        )
        .unwrap();

        let report = project_folder_session_events(&dir).unwrap();
        let text = format_session_project_events_report(&report);

        assert!(!report.writes);
        assert_eq!(report.projected_turn_count, 2);
        assert_eq!(report.existing_turn_count, 1);
        assert_eq!(report.turns[0].id, "existing-turn");
        assert_eq!(report.turns[0].request_state, "matches");
        assert_eq!(report.turns[0].response_state, "would_update");
        assert_eq!(report.turns[1].id, "event-turn-0002");
        assert_eq!(report.turns[1].request_state, "would_create");
        assert_eq!(report.summary.as_ref().unwrap().state, "would_update");
        assert!(text.contains("writes: no"));
        assert!(text.contains("event-turn-0002"));
        assert!(text.contains("summary.md:"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_events_write_rebuilds_turns_and_preserves_backup() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-events-write-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let turn_dir = dir.join("turns/existing-turn");
        fs::create_dir_all(&turn_dir).unwrap();
        fs::write(turn_dir.join("request.md"), "old question\n").unwrap();
        fs::write(turn_dir.join("response.md"), "old answer\n").unwrap();
        fs::write(dir.join("summary.md"), "old answer\n").unwrap();
        fs::write(
            dir.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"new question\"}\n{\"type\":\"assistant_message\",\"content\":\"new answer\"}\n",
        )
        .unwrap();

        let report = rebuild_folder_session_from_events(&dir).unwrap();
        let backup_dir = PathBuf::from(report.backup_dir.as_ref().unwrap());

        assert!(report.writes);
        assert_eq!(report.projected_turn_count, 1);
        assert_eq!(report.turns[0].request_state, "matches");
        assert_eq!(report.turns[0].response_state, "matches");
        assert_eq!(
            fs::read_to_string(dir.join("turns/existing-turn/request.md")).unwrap(),
            "new question\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("turns/existing-turn/response.md")).unwrap(),
            "new answer\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("summary.md")).unwrap(),
            "new answer\n"
        );
        assert_eq!(
            fs::read_to_string(backup_dir.join("turns/existing-turn/response.md")).unwrap(),
            "old answer\n"
        );
        assert_eq!(
            fs::read_to_string(backup_dir.join("summary.md")).unwrap(),
            "old answer\n"
        );
        assert!(backup_dir.join("backup.toml").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_events_restore_rebuild_backup_round_trips_previous_state() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-events-restore-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let turn_dir = dir.join("turns/original-turn");
        fs::create_dir_all(&turn_dir).unwrap();
        fs::write(turn_dir.join("request.md"), "original question\n").unwrap();
        fs::write(turn_dir.join("response.md"), "original answer\n").unwrap();
        fs::write(dir.join("summary.md"), "original answer\n").unwrap();
        fs::write(
            dir.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"new question\"}\n{\"type\":\"assistant_message\",\"content\":\"new answer\"}\n",
        )
        .unwrap();

        let rebuild = rebuild_folder_session_from_events(&dir).unwrap();
        let backup_dir = PathBuf::from(rebuild.backup_dir.unwrap());
        let backup_name = backup_dir.file_name().unwrap().to_os_string();

        let preview = restore_folder_session_event_backup(&dir, Path::new(&backup_name), false)
            .expect("preview restore backup");
        assert!(!preview.writes);
        assert_eq!(preview.restored_turn_count, 1);
        assert!(preview.safety_backup_dir.is_none());
        assert_eq!(
            fs::read_to_string(dir.join("summary.md")).unwrap(),
            "new answer\n"
        );

        let restored = restore_folder_session_event_backup(&dir, Path::new(&backup_name), true)
            .expect("restore backup");
        assert!(restored.writes);
        assert!(restored.safety_backup_dir.is_some());
        assert_eq!(
            fs::read_to_string(dir.join("turns/original-turn/request.md")).unwrap(),
            "original question\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("turns/original-turn/response.md")).unwrap(),
            "original answer\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("summary.md")).unwrap(),
            "original answer\n"
        );
        let safety_backup = PathBuf::from(restored.safety_backup_dir.unwrap());
        assert_eq!(
            fs::read_to_string(safety_backup.join("turns/original-turn/response.md")).unwrap(),
            "new answer\n"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_events_all_reports_cache_health() {
        let root = std::env::temp_dir().join(format!(
            "djinn-events-health-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let ready = root.join("ready-session");
        let ready_turn = ready.join("turns/turn-1");
        fs::create_dir_all(&ready_turn).unwrap();
        fs::write(ready_turn.join("request.md"), "question\n").unwrap();
        fs::write(ready_turn.join("response.md"), "answer\n").unwrap();
        fs::write(ready.join("summary.md"), "answer\n").unwrap();
        fs::write(
            ready.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"question\"}\n{\"type\":\"assistant_message\",\"content\":\"answer\"}\n",
        )
        .unwrap();

        let not_ready = root.join("not-ready-session");
        fs::create_dir_all(&not_ready).unwrap();
        fs::write(not_ready.join("summary.md"), "orphan summary\n").unwrap();

        let report = event_health_report_for_folder_session_root(&root, None, None).unwrap();
        let text = format_event_health_report(&report);

        assert_eq!(report.total, 2);
        assert_eq!(report.ready, 1);
        assert_eq!(report.not_ready, 1);
        assert!(report.sessions.iter().any(|session| {
            session.name == "ready-session" && session.ready && session.event_turn_count == 1
        }));
        assert!(report.sessions.iter().any(|session| {
            session.name == "not-ready-session"
                && !session.ready
                && session
                    .issue_codes
                    .contains(&"missing_events_jsonl".to_string())
        }));
        assert!(text.contains("Event ledger health"));
        assert!(text.contains("ready: 1"));
        assert!(text.contains("not ready: 1"));
        assert!(ensure_event_health_strict(&report)
            .unwrap_err()
            .to_string()
            .contains("strict check failed"));

        let strict_ok = SessionEventsHealthReport {
            root: root.display().to_string(),
            filter: None,
            total: 1,
            ready: 1,
            not_ready: 0,
            sessions: Vec::new(),
            note: "ok".to_string(),
        };
        ensure_event_health_strict(&strict_ok).unwrap();
        let not_ready =
            event_health_report_for_folder_session_root(&root, None, Some("not-ready")).unwrap();
        assert_eq!(not_ready.filter.as_deref(), Some("not-ready"));
        assert_eq!(not_ready.total, 1);
        assert_eq!(not_ready.not_ready, 1);
        assert_eq!(not_ready.sessions[0].name, "not-ready-session");
        let missing =
            event_health_report_for_folder_session_root(&root, None, Some("missing")).unwrap();
        assert_eq!(missing.total, 1);
        assert_eq!(missing.sessions[0].name, "not-ready-session");
        let ready_only =
            event_health_report_for_folder_session_root(&root, None, Some("ready")).unwrap();
        assert_eq!(ready_only.total, 1);
        assert_eq!(ready_only.sessions[0].name, "ready-session");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_backed_session_projection_preserves_context_manifest_sections() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-folder-manifest-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("djinn.toml"),
            "version = 1\nprofile = \"work\"\n\n[context]\npath = \"context\"\n\n[context.repo]\npath = \"/tmp/repo\"\nlink = \"context/repo\"\n",
        )
        .unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("agt_manifest"),
            meta: AgentSessionMeta {
                title: "Folder session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "work".to_string(),
                source: "djinn".to_string(),
                ..AgentSessionMeta::default()
            },
            events: Vec::new(),
        };

        project_agent_session_dir(&dir, &session, "request", "summary").unwrap();
        let manifest = fs::read_to_string(dir.join("djinn.toml")).unwrap();

        assert!(manifest.contains("session_id = \"agt_manifest\""));
        assert!(manifest.contains("[context]\npath = \"context\""));
        assert!(manifest.contains("[context.repo]\npath = \"/tmp/repo\""));
        assert_eq!(
            session_id_from_session_dir(&dir).unwrap(),
            Some(AgentSessionId::new("agt_manifest"))
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_session_manifest_parses_defaults_and_repo_path() {
        let manifest = r#"version = 1
session_id = "agt_manifest"
profile = "work"
agent = "architect"
model = "repo-model"
workspace = "/tmp/workspace"

[context]
path = "context"

[context.repo]
path = "/tmp/repo"
link = "context/repo"
"#;

        let parsed = parse_folder_session_manifest(manifest);

        assert_eq!(parsed.session_id, Some(AgentSessionId::new("agt_manifest")));
        assert_eq!(parsed.profile.as_deref(), Some("work"));
        assert_eq!(parsed.agent.as_deref(), Some("architect"));
        assert_eq!(parsed.model.as_deref(), Some("repo-model"));
        assert_eq!(parsed.workspace.as_deref(), Some("/tmp/workspace"));
        assert_eq!(parsed.repo_path.as_deref(), Some("/tmp/repo"));
        assert_eq!(parsed.repo_link.as_deref(), Some("context/repo"));
        assert_eq!(
            session_manifest_workspace_path(Some(&parsed)),
            Some(PathBuf::from("/tmp/workspace"))
        );
    }

    #[test]
    fn folder_session_status_reports_manifest_files_turns_and_context_skips() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-status-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let context = dir.join("context");
        let repo = root.join("repo");
        fs::create_dir_all(&context).unwrap();
        fs::create_dir_all(dir.join("turns/turn-1")).unwrap();
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            dir.join("djinn.toml"),
            format!(
                "session_id = \"agt_missing\"\nprofile = \"work\"\nmodel = \"repo-model\"\nworkspace = \"{}\"\n\n[context.repo]\npath = \"{}\"\nlink = \"context/repo\"\n",
                repo.display(),
                repo.display()
            ),
        )
        .unwrap();
        fs::write(dir.join("request.md"), "request\n").unwrap();
        fs::write(dir.join("summary.md"), "summary\n").unwrap();
        fs::write(context.join("notes.md"), "note\n").unwrap();
        fs::write(context.join("data.bin"), "binary-ish\n").unwrap();
        fs::write(dir.join("turns/turn-1/request.md"), "turn request\n").unwrap();
        fs::create_dir_all(dir.join("outputs/candidates")).unwrap();
        fs::write(
            dir.join("outputs/candidates/memory-001.toml"),
            "type = \"memory\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("outputs/candidates/memory-002.toml"),
            "type = \"memory\"\n",
        )
        .unwrap();
        fs::write(
            dir.join("outputs/candidate-index.toml"),
            "candidate_count = 2\n",
        )
        .unwrap();
        fs::write(
            dir.join("outputs/candidate-status.toml"),
            "[[events]]\ncandidate = \"memory-001\"\nstatus = \"accepted\"\n",
        )
        .unwrap();
        create_dir_symlink(&repo, &context.join("repo")).unwrap();

        let report = folder_session_status(&dir).unwrap();
        let text = format_folder_session_status(&report);

        assert!(report.manifest_exists);
        assert_eq!(report.session_id.as_deref(), Some("agt_missing"));
        assert!(!report.native_session_exists);
        assert_eq!(report.profile.as_deref(), Some("work"));
        assert_eq!(report.model.as_deref(), Some("repo-model"));
        assert!(report.files.request_md);
        assert!(report.files.summary_md);
        assert!(report.files.context_dir);
        assert!(report.files.turns_dir);
        assert!(!report.files.events_jsonl);
        assert_eq!(report.turn_count, 1);
        assert_eq!(report.event_count, 0);
        assert_eq!(report.lifecycle.state, "not_started");
        assert_eq!(report.latest_turn.as_ref().unwrap().id, "turn-1");
        assert!(!report.latest_turn.as_ref().unwrap().has_response);
        assert_eq!(report.candidates.as_ref().unwrap().candidate_count, 2);
        assert_eq!(report.candidates.as_ref().unwrap().accepted_count, 1);
        assert_eq!(report.candidates.as_ref().unwrap().pending_count, 1);
        assert_eq!(report.candidates.as_ref().unwrap().entries.len(), 2);
        assert_eq!(
            report.candidates.as_ref().unwrap().entries[0].id,
            "memory-001"
        );
        assert_eq!(
            report.candidates.as_ref().unwrap().entries[0].status,
            "accepted"
        );
        assert_eq!(report.context_ingestible_count, 1);
        let repo_status = report.repo.as_ref().unwrap();
        assert!(repo_status.link_exists);
        assert!(repo_status.link_is_symlink);
        assert!(!repo_status.link_broken);
        assert!(text.contains("Skipped context:"));
        assert!(text.contains("State: not_started"));
        assert!(text.contains("events.jsonl: no"));
        assert!(text.contains("Events: 0"));
        assert!(text.contains("Latest turn:"));
        assert!(text.contains("Candidates:"));
        assert!(text.contains("2 total, 1 accepted, 0 denied, 1 pending"));
        assert!(text.contains("memory-001 [memory] accepted"));
        assert!(text.contains("memory-002 [memory] pending"));
        assert!(text.contains("context/data.bin: unsupported file type"));
        assert!(text.contains("context/repo: symlink directory not ingested"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_status_reports_lifecycle_and_latest_response() {
        let store = temp_agent_store("folder-status-lifecycle");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Lifecycle session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        append_agent_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Completed,
            AgentSessionExecutionMode::Foreground,
            "test completed",
            Some("all done".to_string()),
        )
        .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "request".to_string(),
                }),
            )
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "response".to_string(),
                }),
            )
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::ToolCall {
                    id: "tool-1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"path": "summary.md"}),
                }),
            )
            .unwrap();
        let root = std::env::temp_dir().join(format!(
            "djinn-session-status-lifecycle-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");
        let session = store.load_session(&id).unwrap();
        project_agent_session_dir(&session_dir, &session, "request", "response").unwrap();
        relocate_agent_session_into_folder(&store, &session_dir, &id).unwrap();

        let report = folder_session_status(&session_dir).unwrap();
        let text = format_folder_session_status(&report);

        assert!(report.native_session_exists);
        assert_eq!(report.lifecycle.state, "completed");
        assert_eq!(report.lifecycle.mode.as_deref(), Some("foreground"));
        assert_eq!(report.lifecycle.reason.as_deref(), Some("test completed"));
        assert_eq!(report.lifecycle.note.as_deref(), Some("all done"));
        assert!(report.files.events_jsonl);
        assert_eq!(report.event_count, 4);
        let latest = report.latest_turn.as_ref().unwrap();
        assert!(latest.has_response);
        assert!(latest
            .response_path
            .as_deref()
            .unwrap()
            .ends_with("events.jsonl"));
        assert!(report
            .next_action
            .as_deref()
            .unwrap()
            .contains("open latest summary"));
        assert!(text.contains("State: completed"));
        assert!(text.contains("Mode: foreground"));
        assert!(text.contains("State note: all done"));
        assert!(text.contains("events.jsonl: yes"));
        assert!(text.contains("Events: 4"));
        assert!(text.contains("summary.md"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_status_and_list_prefer_events_jsonl_without_turns() {
        let root = std::env::temp_dir().join(format!(
            "djinn-event-native-status-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("event-native");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            "title = \"Event Native\"\ncreated_at = \"2026-08-01T12:00:00Z\"\n",
        )
        .unwrap();
        fs::write(session_dir.join("summary.md"), "stale summary\n").unwrap();
        fs::write(
            session_dir.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"first question\"}\n{\"type\":\"assistant_message\",\"content\":\"first answer\"}\n{\"type\":\"user_message\",\"content\":\"latest question\"}\n{\"type\":\"assistant_message\",\"content\":\"latest event answer\"}\n",
        )
        .unwrap();

        let status = folder_session_status(&session_dir).unwrap();
        assert!(status.files.events_jsonl);
        assert!(!status.files.turns_dir);
        assert_eq!(status.event_count, 4);
        assert_eq!(status.turn_count, 2);
        let latest = status.latest_turn.as_ref().unwrap();
        assert_eq!(latest.id, "event-turn-0002");
        assert!(latest
            .request_path
            .as_deref()
            .unwrap()
            .ends_with("events.jsonl"));
        assert!(latest
            .response_path
            .as_deref()
            .unwrap()
            .ends_with("events.jsonl"));

        let list = list_folder_sessions_in_root(&root, None).unwrap();
        assert_eq!(list.sessions.len(), 1);
        assert_eq!(list.sessions[0].turn_count, 2);
        assert_eq!(
            list.sessions[0].summary_preview.as_deref(),
            Some("latest event answer")
        );
        assert_eq!(
            list.sessions[0].latest_turn.as_ref().unwrap().id,
            "event-turn-0002"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_status_marks_dead_background_worker_as_failed() {
        let store = temp_agent_store("folder-status-stale-background");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Stale background session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        append_agent_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Running,
            AgentSessionExecutionMode::Background,
            "djinn session run started",
            None,
        )
        .unwrap();
        let root = std::env::temp_dir().join(format!(
            "djinn-session-status-stale-background-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");
        let session = store.load_session(&id).unwrap();
        project_agent_session_dir(&session_dir, &session, "request", "in progress").unwrap();
        relocate_agent_session_into_folder(&store, &session_dir, &id).unwrap();
        let log_path = session_dir.join(".djinn/runs/session-run-stale.log");
        fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        fs::write(&log_path, "worker started\n").unwrap();
        write_background_session_run_marker(
            &session_dir,
            &log_path,
            4_294_967_295,
            "djinn session run /tmp/session --background-worker",
            Some(id.as_str()),
        )
        .unwrap();

        let report = folder_session_status(&session_dir).unwrap();
        let rendered = format_session_watch_snapshot(&report);
        let run = latest_background_session_run_status(&session_dir).unwrap();

        assert_eq!(run.run_id, "session-run-stale");
        assert_eq!(run.native_session_id.as_deref(), Some(id.as_str()));
        assert_eq!(
            run.command.as_deref(),
            Some("djinn session run /tmp/session --background-worker")
        );
        assert_eq!(report.lifecycle.state, "failed");
        assert_eq!(report.lifecycle.mode.as_deref(), Some("background"));
        assert_eq!(
            report.lifecycle.reason.as_deref(),
            Some("background_worker_stale")
        );
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("no live process found"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("worker started"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("session-run-stale"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains(id.as_str()));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("Last transcript event"));
        assert!(format_agent_session_event_summary(&AgentSessionEvent::new(
            AgentSessionEventKind::ToolCall {
                id: "tool-1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"path": "summary.md"}),
            }
        ))
        .contains("tool_call id=tool-1 name=read"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("lifecycle state=running"));
        let marker = fs::read_to_string(log_path.with_extension("toml")).unwrap();
        assert!(marker.contains("recovery_reason = \"background_worker_stale\""));
        assert!(marker.contains("recovery_observed_at ="));
        assert!(marker.contains("last_observed_event ="));
        assert!(marker.contains("lifecycle state=running"));
        assert!(report
            .next_action
            .as_deref()
            .unwrap()
            .contains("djinn session run"));
        assert!(report.next_action.as_deref().unwrap().contains("--fg"));
        assert!(rendered.contains("State: failed (background)"));
        assert!(rendered.contains("Reason: background_worker_stale"));
        assert!(rendered.contains("Next:"));
        session_watch(SessionWatchArgs {
            dir: session_dir.clone(),
            interval_ms: 1,
            timeout_seconds: Some(1),
            json: false,
        })
        .unwrap();

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_status_marks_stale_heartbeat_worker_as_unresponsive() {
        let store = temp_agent_store("folder-status-unresponsive-background");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Unresponsive background session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        append_agent_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Running,
            AgentSessionExecutionMode::Background,
            "djinn session run started",
            None,
        )
        .unwrap();
        let root = std::env::temp_dir().join(format!(
            "djinn-session-status-unresponsive-background-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");
        let session = store.load_session(&id).unwrap();
        project_agent_session_dir(&session_dir, &session, "request", "in progress").unwrap();
        relocate_agent_session_into_folder(&store, &session_dir, &id).unwrap();
        let log_path = session_dir.join(".djinn/runs/session-run-unresponsive.log");
        fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        fs::write(&log_path, "waiting for model\n").unwrap();
        write_background_session_run_marker(
            &session_dir,
            &log_path,
            std::process::id(),
            "djinn session run /tmp/session --background-worker",
            Some(id.as_str()),
        )
        .unwrap();
        let marker_path = log_path.with_extension("toml");
        let marker = fs::read_to_string(&marker_path).unwrap();
        let marker =
            upsert_toml_root_string(&marker, "heartbeat_at", "2000-01-01T00:00:00Z").unwrap();
        let marker = upsert_toml_root_string(&marker, "heartbeat_phase", "model_call").unwrap();
        fs::write(&marker_path, marker).unwrap();

        let report = folder_session_status(&session_dir).unwrap();
        let run = latest_background_session_run_status(&session_dir).unwrap();

        assert!(run.alive);
        assert!(run.heartbeat_age_seconds.unwrap() >= BACKGROUND_RUN_UNRESPONSIVE_SECONDS);
        assert_eq!(report.lifecycle.state, "failed");
        assert_eq!(
            report.lifecycle.reason.as_deref(),
            Some("background_worker_unresponsive")
        );
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("still alive but appears unresponsive"));
        assert!(report
            .lifecycle
            .note
            .as_deref()
            .unwrap()
            .contains("Phase: model_call"));
        assert!(report.next_action.as_deref().unwrap().contains("--fg"));
        let marker = fs::read_to_string(marker_path).unwrap();
        assert!(marker.contains("recovery_reason = \"background_worker_unresponsive\""));
        assert!(marker.contains("recovery_observed_at ="));
        assert!(marker.contains("last_observed_event ="));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn background_progress_updates_heartbeat_marker_phase() {
        let root = std::env::temp_dir().join(format!(
            "djinn-background-progress-marker-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let marker_path = root.join("session-run-test.toml");
        fs::write(
            &marker_path,
            "version = 1\nrun_id = \"session-run-test\"\nheartbeat_at = \"2000-01-01T00:00:00Z\"\nheartbeat_phase = \"spawned\"\n",
        )
        .unwrap();

        let started = AgentProgressEvent::ModelRequestStarted { round: 2 };
        assert_eq!(background_progress_phase(&started), "model_request_started");
        touch_background_run_marker(&marker_path, background_progress_phase(&started)).unwrap();
        let marker = fs::read_to_string(&marker_path).unwrap();
        assert!(marker.contains("heartbeat_phase = \"model_request_started\""));
        assert!(!marker.contains("2000-01-01T00:00:00Z"));

        let tool_call = djinn_agent::ModelToolCall {
            id: "call-1".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"path": "summary.md"}),
        };
        let tool_started = AgentProgressEvent::ToolCallStarted {
            round: 2,
            call: tool_call.clone(),
        };
        assert_eq!(
            background_progress_phase(&tool_started),
            "tool_call_started"
        );
        touch_background_run_marker(&marker_path, background_progress_phase(&tool_started))
            .unwrap();
        let marker = fs::read_to_string(&marker_path).unwrap();
        assert!(marker.contains("heartbeat_phase = \"tool_call_started\""));

        let tool_completed = AgentProgressEvent::ToolCallCompleted {
            round: 2,
            call: tool_call,
            result: djinn_agent::ToolResult {
                output: serde_json::json!({"ok": true}),
                success: true,
            },
            elapsed_ms: 42,
        };
        assert_eq!(
            background_progress_phase(&tool_completed),
            "tool_call_completed"
        );

        let _ = fs::remove_dir_all(&root);
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

    #[test]
    fn session_watch_snapshot_renders_status_changes() {
        let report = SessionStatusReport {
            session_dir: "/tmp/session".to_string(),
            manifest_exists: true,
            session_id: Some("agt_watch".to_string()),
            native_session_exists: true,
            profile: Some("default".to_string()),
            agent: None,
            model: Some("openai/gpt-5.5".to_string()),
            workspace: Some("/tmp/workspace".to_string()),
            repo: None,
            lifecycle: SessionStatusLifecycleReport {
                state: "running".to_string(),
                mode: Some("background".to_string()),
                updated_at: Some("2026-07-28T12:00:00Z".to_string()),
                reason: Some("started".to_string()),
                note: None,
            },
            files: SessionStatusFileReport {
                request_md: true,
                summary_md: true,
                context_dir: true,
                compacted_md: false,
                turns_dir: true,
                events_jsonl: true,
            },
            turn_count: 1,
            event_count: 3,
            latest_turn: Some(SessionStatusTurnReport {
                id: "turn-1".to_string(),
                request_path: Some("/tmp/session/turns/turn-1/request.md".to_string()),
                response_path: None,
                has_response: false,
            }),
            candidates: None,
            context_ingestible_count: 0,
            context_skipped: Vec::new(),
            next_action: Some("check again: djinn session status /tmp/session".to_string()),
        };

        let rendered = format_session_watch_snapshot(&report);
        let key = session_watch_snapshot_key(&report).unwrap();

        assert!(rendered.contains("Session: /tmp/session"));
        assert!(rendered.contains("State: running (background)"));
        assert!(rendered.contains("Latest turn: turn-1"));
        assert!(rendered.contains("Request: /tmp/session/turns/turn-1/request.md"));
        assert!(rendered.contains("Next: check again"));
        assert!(key.contains("running"));
        assert!(key.contains("turn-1"));
    }

    #[test]
    fn relocates_native_jsonl_into_folder_session() {
        let store = temp_agent_store("folder-native-relocate");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Move me".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let source_path = store.session_file_path(&id);
        let root = std::env::temp_dir().join(format!(
            "djinn-folder-native-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");

        let folder_store = relocate_agent_session_into_folder(&store, &session_dir, &id).unwrap();
        let target_path = folder_store.session_file_path(&id);

        assert!(!source_path.exists());
        assert_eq!(
            target_path,
            session_dir.join(".djinn").join(format!("{id}.jsonl"))
        );
        assert!(target_path.exists());
        assert_eq!(
            folder_store.load_session(&id).unwrap().meta.title,
            "Move me"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_ls_scans_cache_root_without_external_index() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-ls-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        let gamma = root.join("gamma");
        let delta = root.join("delta");
        let long = root.join("session-agt_1785201896467199000_123_0");
        fs::create_dir_all(alpha.join("turns/turn-a")).unwrap();
        fs::create_dir_all(beta.join("turns")).unwrap();
        fs::create_dir_all(gamma.join("turns/turn-g")).unwrap();
        fs::create_dir_all(delta.join("turns/turn-d")).unwrap();
        fs::create_dir_all(long.join("turns/turn-long")).unwrap();
        fs::write(
            alpha.join("djinn.toml"),
            "session_id = \"agt_alpha\"\ncreated_at = \"2026-07-27T12:34:56.123-04:00\"\nworkspace = \"/tmp/workspace\"\n\n[context.repo]\npath = \"/tmp/repo-b\"\n",
        )
        .unwrap();
        fs::write(alpha.join("request.md"), "request\n").unwrap();
        fs::write(alpha.join("summary.md"), "summary\n").unwrap();
        fs::write(alpha.join("turns/turn-a/response.md"), "response\n").unwrap();
        fs::create_dir_all(alpha.join("runtime")).unwrap();
        fs::write(
            alpha.join("runtime/buddy.json"),
            r#"{
  "buddy_session": "bud_alpha",
  "command": "buddy-dev",
  "last_run_at": "2026-08-01T12:00:00Z",
  "last_prompt_chars": 7,
  "last_response_chars": 8
}
"#,
        )
        .unwrap();
        let alpha_store = folder_agent_session_store(&alpha);
        let alpha_id = alpha_store
            .create_session(AgentSessionMeta {
                title: "Alpha".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "test".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        fs::write(
            alpha.join("djinn.toml"),
            format!(
                "session_id = \"{}\"\ncreated_at = \"2026-07-27T12:34:56.123-04:00\"\nworkspace = \"/tmp/workspace\"\n\n[context.repo]\npath = \"/tmp/repo-b\"\n",
                alpha_id
            ),
        )
        .unwrap();
        append_agent_session_lifecycle_event(
            &alpha_store,
            &alpha_id,
            AgentSessionLifecycleState::Running,
            AgentSessionExecutionMode::Background,
            "test running",
            None,
        )
        .unwrap();
        fs::write(
            gamma.join("djinn.toml"),
            "created_at = \"2026-07-28T12:34:56.123-04:00\"\n\n[context.repo]\npath = \"/tmp/repo-a\"\n",
        )
        .unwrap();
        fs::write(gamma.join("summary.md"), "newer repo-a summary\n").unwrap();
        fs::write(gamma.join("turns/turn-g/request.md"), "gamma request\n").unwrap();
        fs::write(
            gamma.join("turns/turn-g/response.md"),
            "newer repo-a summary\n",
        )
        .unwrap();
        fs::write(
            gamma.join("events.jsonl"),
            "{\"type\":\"user_message\",\"content\":\"gamma request\"}\n{\"type\":\"assistant_message\",\"content\":\"newer repo-a summary\"}\n",
        )
        .unwrap();
        fs::write(
            delta.join("djinn.toml"),
            "created_at = \"2026-07-27T12:34:56.123-04:00\"\n\n[context.repo]\npath = \"/tmp/repo-a\"\n",
        )
        .unwrap();
        fs::write(delta.join("summary.md"), "older repo-a summary\n").unwrap();
        fs::write(
            long.join("djinn.toml"),
            "created_at = \"2026-07-27T11:34:56.123-04:00\"\n\n[context.repo]\npath = \"/tmp/repo-a\"\n",
        )
        .unwrap();
        fs::write(long.join("summary.md"), "long folder summary\n").unwrap();

        let report = list_folder_sessions_in_root(&root, None).unwrap();
        let text = format_folder_session_ls(&report);

        assert_eq!(report.sessions.len(), 5);
        assert_eq!(
            report
                .sessions
                .iter()
                .map(|session| session.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "gamma",
                "delta",
                "session-agt_1785201896467199000_123_0",
                "alpha",
                "beta"
            ]
        );
        assert_eq!(report.sessions[2].display_name, "session");
        assert_eq!(
            report.sessions[2].reference_name,
            folder_session_reference_name("session-agt_1785201896467199000_123_0")
        );
        assert_eq!(
            report.sessions[3].session_id.as_deref(),
            Some(alpha_id.as_str())
        );
        assert!(report.sessions[3].native_session_exists);
        assert_eq!(report.sessions[3].lifecycle.state, "running");
        assert_eq!(
            report.sessions[3].lifecycle.mode.as_deref(),
            Some("background")
        );
        assert_eq!(
            report.sessions[3].latest_turn.as_ref().unwrap().id,
            "turn-a"
        );
        assert_eq!(
            report.sessions[3].created_at.as_deref(),
            non_empty_string(&alpha_store.load_session(&alpha_id).unwrap().meta.created_at)
                .as_deref()
        );
        assert_eq!(report.sessions[3].turn_count, 1);
        assert_eq!(
            report.sessions[3]
                .buddy
                .as_ref()
                .and_then(|buddy| buddy.buddy_session.as_deref()),
            Some("bud_alpha")
        );
        assert_eq!(
            report.sessions[3]
                .buddy
                .as_ref()
                .and_then(|buddy| buddy.command.as_deref()),
            Some("buddy-dev")
        );
        assert!(report.sessions[0].event_health.ready);
        assert_eq!(report.sessions[0].event_health.event_turn_count, 1);
        assert!(!report.sessions[3].event_health.ready);
        assert!(report.sessions[3].request_md);
        assert!(report.sessions[3].summary_md);
        assert_eq!(
            report.sessions[0].summary_preview.as_deref(),
            Some("newer repo-a summary")
        );
        assert_eq!(report.groups.len(), 3);
        assert_eq!(report.groups[0].repo, "repo-a");
        assert_eq!(
            report.groups[0]
                .sessions
                .iter()
                .map(|session| session.name.as_str())
                .collect::<Vec<_>>(),
            vec!["gamma", "delta", "session-agt_1785201896467199000_123_0"]
        );
        assert_eq!(report.groups[1].repo, "repo-b");
        assert_eq!(report.groups[2].repo, "-");
        assert!(!report.sessions[4].manifest_exists);
        assert!(text.contains("Cache folder sessions:"));
        assert!(text.contains("Repo: repo-a"));
        assert!(text.contains("Repo: repo-b"));
        assert!(text.contains("Repo: -"));
        assert!(text.contains("UPDATED"));
        assert!(text.contains("STATE"));
        assert!(text.contains("BUDDY"));
        assert!(!text.contains("TURNS"));
        assert!(!text.contains("EVENTS"));
        assert!(text.contains("bud_alpha"));
        assert!(!text.contains("ready:1/2"));
        assert!(text.contains("running/bac…"));
        assert!(text.contains("alpha"));
        assert!(text.contains("2026-07-27T11:34:56…"));
        assert!(text.contains(&folder_session_reference_name(
            "session-agt_1785201896467199000_123_0"
        )));
        assert!(text.contains("long folder summary"));
        assert!(!text.contains("session-agt_1785201896467199000"));
        assert!(text.contains("2026-07-27T12:34:56…"));
        assert!(text.contains("beta (no manifest)"));
        assert!(text.contains("newer repo-a summary"));
        assert!(!text.contains("native: agt_alpha"));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["sessions"][3]["lifecycle"]["state"], "running");
        assert_eq!(json["sessions"][0]["event_health"]["event_turn_count"], 1);
        assert_eq!(json["sessions"][3]["turn_count"], 1);
        assert_eq!(json["sessions"][3]["buddy"]["buddy_session"], "bud_alpha");
        assert_eq!(json["sessions"][3]["buddy"]["command"], "buddy-dev");
        assert_eq!(json["sessions"][3]["latest_turn"]["id"], "turn-a");
        assert_eq!(json["groups"][0]["repo"], "repo-a");
        assert_eq!(json["groups"][0]["sessions"][0]["name"], "gamma");
        assert_eq!(
            json["groups"][1]["sessions"][0]["lifecycle"]["state"],
            "running"
        );
        assert_eq!(
            json["groups"][1]["sessions"][0]["lifecycle"]["mode"],
            "background"
        );
        assert_eq!(
            json["groups"][1]["sessions"][0]["buddy"]["buddy_session"],
            "bud_alpha"
        );
        assert_eq!(json["groups"][0]["sessions"][2]["display_name"], "session");
        assert_eq!(
            json["groups"][0]["sessions"][2]["reference_name"],
            folder_session_reference_name("session-agt_1785201896467199000_123_0")
        );

        let limited = list_folder_sessions_in_root(&root, Some(1)).unwrap();
        assert_eq!(limited.sessions.len(), 1);
        assert_eq!(limited.sessions[0].name, "gamma");
        assert_eq!(limited.groups.len(), 1);
        assert_eq!(limited.groups[0].repo, "repo-a");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_list_datetime_compaction_removes_fractional_seconds() {
        assert_eq!(
            compact_session_list_datetime("2026-07-27T12:34:56.123-04:00"),
            "2026-07-27T12:34:56-04:00"
        );
        assert_eq!(
            compact_session_list_datetime("2026-07-27T16:34:56.123Z"),
            "2026-07-27T16:34:56Z"
        );
        assert_eq!(
            parse_session_list_datetime_ms("2026-07-27T16:34:56.123Z"),
            Some(1_785_170_096_123)
        );
    }

    #[test]
    fn folder_session_display_name_hides_native_id_suffix() {
        assert_eq!(
            folder_session_display_name("write-plan-agt_1785201896467199000_123_0"),
            "write-plan"
        );
        assert_eq!(
            folder_session_reference_name("write-plan-agt_1785201896467199000_123_0"),
            format!(
                "write-plan-{}",
                short_agent_session_suffix_from_str("agt_1785201896467199000_123_0")
            )
        );
        assert_eq!(
            folder_session_display_name("session-agt_1785201896467199000_123_0"),
            "session"
        );
        assert_eq!(
            folder_session_reference_name("session-agt_1785201896467199000_123_0"),
            format!(
                "session-{}",
                short_agent_session_suffix_from_str("agt_1785201896467199000_123_0")
            )
        );
        assert_eq!(folder_session_display_name("manual-notes"), "manual-notes");
        assert_eq!(
            folder_session_reference_name("manual-notes"),
            "manual-notes"
        );
    }

    #[test]
    fn folder_session_reference_name_resolves_to_full_cache_folder() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-ref-name-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session = root.join("agent-chat-agt_1785201849270486000_123_0");
        fs::create_dir_all(&session).unwrap();

        assert_eq!(
            resolve_folder_session_reference_name(
                &root,
                Path::new(&folder_session_reference_name(
                    "agent-chat-agt_1785201849270486000_123_0"
                ))
            )
            .unwrap(),
            Some(session.clone())
        );
        assert_eq!(
            resolve_folder_session_reference_name(&root, Path::new("missing")).unwrap(),
            None
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_rename_moves_cache_folder_and_preserves_buddy_runtime() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-rename-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let from = root.join("old-session");
        fs::create_dir_all(from.join("runtime")).unwrap();
        fs::write(from.join("summary.md"), "summary\n").unwrap();
        fs::write(
            from.join("runtime/buddy.json"),
            serde_json::json!({
                "buddy_session": "ses_renameBuddy123",
                "stale_buddy_sessions": []
            })
            .to_string(),
        )
        .unwrap();

        let dry = rename_folder_session_in_root(
            Path::new("ses_renameBuddy123"),
            "new-session",
            &root,
            true,
        )
        .unwrap();
        assert!(dry.dry_run);
        assert!(dry.renamed);
        assert!(from.exists());
        assert!(!root.join("new-session").exists());

        let report = rename_folder_session_in_root(
            Path::new("ses_renameBuddy123"),
            "new-session",
            &root,
            false,
        )
        .unwrap();

        let to = root.join("new-session");
        assert!(report.renamed);
        assert!(!from.exists());
        assert!(to.join("summary.md").exists());
        assert!(to.join("runtime/buddy.json").exists());
        assert!(fs::read_to_string(to.join("runtime/buddy.json"))
            .unwrap()
            .contains("ses_renameBuddy123"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_rename_rejects_path_target_and_existing_destination() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-rename-guard-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(root.join("old-session")).unwrap();
        fs::create_dir_all(root.join("existing-session")).unwrap();

        assert!(rename_folder_session_in_root(
            Path::new("old-session"),
            "nested/new-session",
            &root,
            false,
        )
        .is_err());
        assert!(rename_folder_session_in_root(
            Path::new("old-session"),
            "existing-session",
            &root,
            false,
        )
        .is_err());
        assert!(root.join("old-session").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn shorten_folder_session_names_renames_legacy_long_cache_folders() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-shorten-names-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let legacy_name = "agent-chat-agt_1785201849270486000_123_0";
        let short_name = folder_session_reference_name(legacy_name);
        let legacy = root.join(legacy_name);
        fs::create_dir_all(&legacy).unwrap();

        let dry = shorten_folder_session_names_in_root(&root, true).unwrap();
        assert!(legacy.exists());
        assert_eq!(dry.renamed.len(), 1);
        assert_eq!(
            Path::new(&dry.renamed[0].to)
                .file_name()
                .and_then(|name| name.to_str()),
            Some(short_name.as_str())
        );

        let report = shorten_folder_session_names_in_root(&root, false).unwrap();
        assert_eq!(report.renamed.len(), 1);
        assert!(!legacy.exists());
        assert!(root.join(&short_name).exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_open_resolves_targets_and_repo() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-open-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(dir.join("context")).unwrap();
        fs::create_dir_all(dir.join("turns")).unwrap();
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            dir.join("djinn.toml"),
            format!(
                "profile = \"default\"\n\n[context.repo]\npath = \"{}\"\nlink = \"context/repo\"\n",
                repo.display()
            ),
        )
        .unwrap();
        create_dir_symlink(&repo, &dir.join("context/repo")).unwrap();

        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Summary).unwrap(),
            dir.join("summary.md")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Request).unwrap(),
            dir.join("request.md")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Context).unwrap(),
            dir.join("context")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Compacted).unwrap(),
            dir.join("context/compacted.md")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Turns).unwrap(),
            dir.join("turns")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Manifest).unwrap(),
            dir.join("djinn.toml")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Repo).unwrap(),
            PathBuf::from(repo.display().to_string())
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_open_resolves_buddy_session_id() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-open-buddy-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        fs::create_dir_all(dir.join("runtime")).unwrap();
        fs::write(dir.join("summary.md"), "summary\n").unwrap();
        fs::write(
            dir.join("runtime/buddy.json"),
            r#"{
  "buddy_session": "ses_openBuddy123",
  "stale_buddy_sessions": []
}
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_folder_session_open_target_in_root(
                Path::new("ses_openBuddy123"),
                SessionOpenTarget::Summary,
                &root,
            )
            .unwrap(),
            dir.join("summary.md")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_open_errors_when_session_does_not_exist() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-open-missing-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();

        let err = resolve_folder_session_open_target_in_root(
            Path::new("missing-session"),
            SessionOpenTarget::Summary,
            &root,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("folder session does not exist"));
        assert!(err.contains("missing-session"));
        assert!(err.contains("run: djinn session init missing-session"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_context_ingestion_is_shallow_bounded_and_textual() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-context-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let context = dir.join("context");
        let repo = root.join("repo");
        fs::create_dir_all(&context).unwrap();
        fs::create_dir_all(&repo).unwrap();
        fs::write(dir.join("request.md"), "current request\n").unwrap();
        fs::write(dir.join("summary.md"), "previous summary\n").unwrap();
        fs::write(context.join("notes.md"), "curated note\n").unwrap();
        fs::write(context.join("large.md"), "x".repeat(40 * 1024)).unwrap();
        fs::write(context.join("data.bin"), "not text\n").unwrap();
        fs::write(
            repo.join("secret.md"),
            "do not ingest through repo symlink\n",
        )
        .unwrap();
        create_dir_symlink(&repo, &context.join("repo")).unwrap();

        let instructions = resolve_folder_session_context_instructions(Some(&dir)).unwrap();
        let rendered = instructions
            .iter()
            .map(|instruction| format!("{}\n{}", instruction.source, instruction.content))
            .collect::<Vec<_>>()
            .join("\n---\n");

        assert!(rendered.contains("session-context:request.md"));
        assert!(rendered.contains("current request"));
        assert!(rendered.contains("session-context:summary.md"));
        assert!(rendered.contains("previous summary"));
        assert!(rendered.contains("session-context:context/notes.md"));
        assert!(rendered.contains("curated note"));
        assert!(!rendered.contains("do not ingest through repo symlink"));
        assert!(rendered.contains("context/repo: symlink directory not ingested"));
        assert!(rendered.contains("context/large.md: 40960 bytes exceeds 32768 byte limit"));
        assert!(rendered.contains("context/data.bin: unsupported file type"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_compact_writes_deterministic_turn_digest_with_evidence_links() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-compact-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let turn = dir.join("turns/20260727T120000-1");
        fs::create_dir_all(&turn).unwrap();
        fs::write(turn.join("request.md"), "Decide storage shape\n\nDetails").unwrap();
        fs::write(
            turn.join("response.md"),
            "Use context for durable notes and turns for evidence.\n",
        )
        .unwrap();

        let report = compact_folder_session(&dir, None).unwrap();
        let compacted = fs::read_to_string(dir.join("context/compacted.md")).unwrap();

        assert_eq!(report.turn_count, 1);
        assert_eq!(report.turns[0].id, "20260727T120000-1");
        assert!(compacted.contains("# Compacted session context"));
        assert!(compacted.contains("## User notes"));
        assert!(compacted.contains(FOLDER_SESSION_COMPACT_START_MARKER));
        assert!(compacted.contains(FOLDER_SESSION_COMPACT_END_MARKER));
        assert!(compacted.contains("### 20260727T120000-1"));
        assert!(compacted.contains("> Decide storage shape"));
        assert!(compacted.contains("> Use context for durable notes"));
        assert!(compacted.contains("[request](../turns/20260727T120000-1/request.md)"));
        assert!(compacted.contains("[response](../turns/20260727T120000-1/response.md)"));
        assert!(!dir.join("logs/transcript.md").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_compact_reads_event_turns_when_turn_projection_is_absent() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-compact-events-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let session = AgentSession {
            id: AgentSessionId::new("agt_compact_events"),
            meta: AgentSessionMeta {
                title: "Compact events".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn".to_string(),
                ..AgentSessionMeta::default()
            },
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "Use events for history".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "Keep turns as projection only".to_string(),
                }),
            ],
        };
        project_agent_session_dir(
            &dir,
            &session,
            "Use events for history",
            "Keep turns as projection only",
        )
        .unwrap();

        let report = compact_folder_session(&dir, None).unwrap();
        let compacted = fs::read_to_string(dir.join("context/compacted.md")).unwrap();

        assert_eq!(report.turn_count, 1);
        assert_eq!(report.turns[0].id, "event-turn-0001");
        assert!(compacted.contains("### event-turn-0001"));
        assert!(compacted.contains("> Use events for history"));
        assert!(compacted.contains("> Keep turns as projection only"));
        assert!(compacted.contains("[request](../events.jsonl)"));
        assert!(!dir.join("turns").exists());

        let _ = fs::remove_dir_all(&root);
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
    fn session_compact_replaces_generated_block_and_preserves_user_notes() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-compact-preserve-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let turn = dir.join("turns/20260727T120000-1");
        let context = dir.join("context");
        fs::create_dir_all(&turn).unwrap();
        fs::create_dir_all(&context).unwrap();
        fs::write(turn.join("request.md"), "Initial request\n").unwrap();
        fs::write(turn.join("response.md"), "Fresh response\n").unwrap();
        fs::write(
            context.join("compacted.md"),
            format!(
                "# Compacted session context\n\n## User notes\n\nKeep this decision.\n\n## Generated digest\n\n{FOLDER_SESSION_COMPACT_START_MARKER}\nOld generated response\n{FOLDER_SESSION_COMPACT_END_MARKER}\n\n## User appendix\n\nKeep appendix.\n"
            ),
        )
        .unwrap();

        compact_folder_session(&dir, None).unwrap();
        let compacted = fs::read_to_string(context.join("compacted.md")).unwrap();

        assert!(compacted.contains("Keep this decision."));
        assert!(compacted.contains("Keep appendix."));
        assert!(compacted.contains("> Fresh response"));
        assert!(!compacted.contains("Old generated response"));
        assert_eq!(
            compacted
                .matches(FOLDER_SESSION_COMPACT_START_MARKER)
                .count(),
            1
        );
        assert_eq!(
            compacted.matches(FOLDER_SESSION_COMPACT_END_MARKER).count(),
            1
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_context_commands_link_list_and_remove_entries() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-context-cmd-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session = root.join("session");
        let source = root.join("source");
        let repo = root.join("repo");
        fs::create_dir_all(&session).unwrap();
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&repo).unwrap();
        let notes = source.join("notes.md");
        let updated_notes = source.join("updated-notes.md");
        fs::write(&notes, "durable note\n").unwrap();
        fs::write(&updated_notes, "updated durable note\n").unwrap();
        fs::write(source.join("data.bin"), "binary-ish\n").unwrap();

        let add = add_folder_session_context_entry(&SessionContextAddArgs {
            session: session.clone(),
            path: notes.clone(),
            name: Some("notes.md".to_string()),
            force: false,
            json: false,
        })
        .unwrap();

        assert_eq!(add.name, "notes.md");
        assert!(!add.replaced);
        assert_eq!(
            fs::read_link(session.join("context/notes.md")).unwrap(),
            notes.canonicalize().unwrap()
        );
        assert!(add_folder_session_context_entry(&SessionContextAddArgs {
            session: session.clone(),
            path: source.join("data.bin"),
            name: Some("notes.md".to_string()),
            force: false,
            json: false,
        })
        .is_err());

        let replaced = add_folder_session_context_entry(&SessionContextAddArgs {
            session: session.clone(),
            path: repo.clone(),
            name: Some("repo".to_string()),
            force: true,
            json: false,
        })
        .unwrap();
        assert!(!replaced.replaced);
        let replaced_notes = add_folder_session_context_entry(&SessionContextAddArgs {
            session: session.clone(),
            path: updated_notes,
            name: Some("notes.md".to_string()),
            force: true,
            json: false,
        })
        .unwrap();
        assert!(replaced_notes.replaced);
        let binary = add_folder_session_context_entry(&SessionContextAddArgs {
            session: session.clone(),
            path: source.join("data.bin"),
            name: Some("data.bin".to_string()),
            force: false,
            json: false,
        })
        .unwrap();
        assert_eq!(binary.name, "data.bin");

        let report = list_folder_session_context(&session).unwrap();
        let rendered = format_folder_session_context_ls(&report);
        let notes_entry = report
            .entries
            .iter()
            .find(|entry| entry.name == "notes.md")
            .unwrap();
        assert_eq!(notes_entry.kind, "symlink_file");
        assert!(notes_entry.ingestible);
        let binary_entry = report
            .entries
            .iter()
            .find(|entry| entry.name == "data.bin")
            .unwrap();
        assert_eq!(binary_entry.kind, "symlink_file");
        assert!(!binary_entry.ingestible);
        assert!(binary_entry
            .skip_reason
            .as_deref()
            .unwrap()
            .contains("unsupported file type"));
        let repo_entry = report
            .entries
            .iter()
            .find(|entry| entry.name == "repo")
            .unwrap();
        assert_eq!(repo_entry.kind, "symlink_dir");
        assert!(!repo_entry.ingestible);
        assert!(rendered.contains("Session context:"));
        assert!(rendered.contains("notes.md"));
        assert!(rendered.contains("data.bin"));
        assert!(rendered.contains("repo"));

        assert!(validate_context_entry_name("nested/name").is_err());
        assert!(remove_folder_session_context_entry(&session, "../notes.md").is_err());
        let removed = remove_folder_session_context_entry(&session, "notes.md").unwrap();
        assert!(removed.removed);
        assert!(!session.join("context/notes.md").exists());
        assert!(repo.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_context_discover_links_high_signal_files_and_indexes_docs() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-context-discover-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(repo.join(".github/instructions")).unwrap();
        fs::create_dir_all(repo.join(".opencode/commands")).unwrap();
        fs::create_dir_all(repo.join(".opencode/skills/demo")).unwrap();
        fs::create_dir_all(repo.join("docs/node_modules/pkg")).unwrap();
        fs::create_dir_all(&session).unwrap();
        fs::write(
            session.join("djinn.toml"),
            format!(
                "session_id = \"test\"\n[context.repo]\npath = \"{}\"\n",
                repo.display()
            ),
        )
        .unwrap();
        fs::write(repo.join("README.md"), "# Demo Repo\n").unwrap();
        fs::write(repo.join("AGENTS.md"), "# Agents\n").unwrap();
        fs::write(
            repo.join("opencode.json"),
            r#"{"instructions":["./docs/opencode.md"],"skills":{"paths":[".opencode/skills"]}}"#,
        )
        .unwrap();
        fs::write(repo.join("docs/opencode.md"), "# OpenCode Notes\n").unwrap();
        fs::write(repo.join("docs/guide.md"), "# Guide\n").unwrap();
        fs::write(repo.join("docs/node_modules/pkg/ignored.md"), "# Ignored\n").unwrap();
        fs::write(
            repo.join(".github/instructions/go.md"),
            "# Go Instructions\n",
        )
        .unwrap();
        fs::write(repo.join(".opencode/commands/build.md"), "# Build\n").unwrap();
        fs::write(
            repo.join(".opencode/skills/demo/SKILL.md"),
            "# Demo Skill\n",
        )
        .unwrap();

        let dry_run = discover_folder_session_context(&session, true).unwrap();
        assert!(dry_run.dry_run);
        assert!(!dry_run.repo_index_written);
        assert!(!session.join("context").exists());
        assert!(dry_run
            .links
            .iter()
            .any(|link| link.name == "opencode-command-build.md"));

        let report = discover_folder_session_context(&session, false).unwrap();
        assert!(report.repo_index_written);
        assert!(session.join("context/repo-index.md").is_file());
        for name in [
            "AGENTS.md",
            "README.md",
            "opencode.md",
            "opencode-command-build.md",
            "opencode-skill-demo.md",
            "copilot-instruction-go.md",
        ] {
            let path = session.join("context").join(name);
            assert!(path.exists(), "expected discovered context link {name}");
            assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        }
        let context = list_folder_session_context(&session).unwrap();
        assert!(context
            .entries
            .iter()
            .any(|entry| { entry.name == "opencode-command-build.md" && entry.ingestible }));
        let repo_index = fs::read_to_string(session.join("context/repo-index.md")).unwrap();
        assert!(repo_index.contains("docs/guide.md"));
        assert!(repo_index.contains("docs/opencode.md"));
        assert!(report
            .ignored
            .iter()
            .any(|path| path.contains("node_modules")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_dir_resolution_uses_cache_root_for_bare_names_only() {
        assert_eq!(
            resolve_session_dir(Path::new("small-question")).unwrap(),
            default_folder_session_root().join("small-question")
        );
        assert_eq!(
            resolve_session_dir(Path::new("./small-question")).unwrap(),
            PathBuf::from("./small-question")
        );
        assert_eq!(
            resolve_session_dir(Path::new("nested/small-question")).unwrap(),
            PathBuf::from("nested/small-question")
        );
    }

    #[test]
    fn auto_folder_session_dir_uses_prompt_slug_and_session_id_under_cache_root() {
        let id = AgentSessionId::new("agt_auto_123");
        assert_eq!(
            auto_folder_session_dir("Small question: explain Rust?", &id),
            default_folder_session_root().join(format!(
                "small-question-explain-rust-{}",
                short_agent_session_suffix(&id)
            ))
        );
        assert_eq!(folder_session_slug("🧠"), "session");
    }

    #[test]
    fn agent_request_prompt_can_read_session_dir_request_md() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-request-md-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("request.md"), "from request file\n").unwrap();

        let prompt = resolve_agent_request_prompt(None, Some(&dir)).unwrap();

        assert_eq!(prompt, "from request file");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_session_run_completion_reports_output_paths() {
        let session_dir = PathBuf::from("/tmp/djinn/session");
        let projection = AgentSessionDirProjection {
            session_dir: session_dir.clone(),
            turn_dir: Some(session_dir.join("turns/20260728T120000-1")),
            context_dir: session_dir.join("context"),
            summary_path: session_dir.join("summary.md"),
            request_path: session_dir.join("request.md"),
        };

        let rendered = format_session_run_completion(
            &AgentSessionId::new("agt_run_test"),
            Some(&projection),
            Some(&session_dir),
        );

        assert!(rendered.contains("Completed Djinn session run: agt_run_test"));
        assert!(rendered.contains("summary.md"));
        assert!(rendered.contains("turns/20260728T120000-1/response.md"));
        assert!(rendered.contains("request.md"));
    }

    #[test]
    fn format_session_run_background_started_reports_watch_and_log() {
        let report = SessionRunBackgroundReport {
            status: "started".to_string(),
            session_dir: "/tmp/djinn/session".to_string(),
            pid: 4242,
            log_path: "/tmp/djinn/session/.djinn/runs/session-run.log".to_string(),
            watch_command: "djinn session watch /tmp/djinn/session".to_string(),
        };

        let rendered = format_session_run_background_started(&report);

        assert!(rendered.contains("Started Djinn session run: /tmp/djinn/session"));
        assert!(rendered.contains("pid: 4242"));
        assert!(rendered.contains("log: /tmp/djinn/session/.djinn/runs/session-run.log"));
        assert!(rendered.contains("watch: djinn session watch /tmp/djinn/session"));
    }

    #[test]
    fn session_init_scaffolds_folder_and_links_repo_without_duplicate_logs() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-init-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();

        let args = SessionInitArgs {
            dir: dir.clone(),
            link_repo: Some(repo.clone()),
            no_discover_context: false,
            profile: "default".to_string(),
            agent: None,
            model: None,
            force: false,
            json: false,
        };
        let report = initialize_folder_session(&args).unwrap();

        assert!(dir.join("djinn.toml").exists());
        assert!(dir.join("request.md").exists());
        assert!(dir.join("summary.md").exists());
        assert!(dir.join("context/djinn-context.md").exists());
        assert!(dir.join("context/repo-index.md").exists());
        assert!(!dir.join("turns").exists());
        assert!(!dir.join("logs/summary-history.md").exists());
        assert!(!dir.join("logs/events.jsonl").exists());
        assert!(!dir.join("logs/transcript.md").exists());

        let link = dir.join("context/repo");
        assert_eq!(fs::read_link(&link).unwrap(), repo.canonicalize().unwrap());
        assert_eq!(
            report.repo_link.as_ref().unwrap().path,
            link.display().to_string()
        );
        assert!(report.discovered_context.is_some());
        assert_eq!(fs::read_to_string(dir.join("request.md")).unwrap(), "");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_init_can_create_buddy_binding() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-init-buddy-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let creates = Arc::new(Mutex::new(Vec::new()));
        let backend = TestBuddyBackend {
            command: "in-tree-buddy".to_string(),
            runtime_command_override: None,
            create_id: "ses_init_bound".to_string(),
            creates: creates.clone(),
        };

        let args = SessionInitArgs {
            dir: dir.clone(),
            link_repo: Some(repo.clone()),
            no_discover_context: true,
            profile: "default".to_string(),
            agent: None,
            model: None,
            force: false,
            json: false,
        };
        let report = initialize_folder_session_with_buddy(&args, Some(&backend)).unwrap();

        let runtime_path = dir.join("runtime/buddy.json");
        assert!(runtime_path.exists());
        assert_eq!(
            report.buddy,
            Some(SessionInitBuddyReport {
                buddy_session: "ses_init_bound".to_string(),
                repo_path: repo.canonicalize().unwrap().display().to_string(),
                runtime_path: runtime_path.display().to_string(),
            })
        );
        assert_eq!(
            creates.lock().unwrap().as_slice(),
            &[(
                "Session".to_string(),
                repo.canonicalize().unwrap().display().to_string()
            )]
        );
        let runtime = fs::read_to_string(runtime_path).unwrap();
        assert!(runtime.contains("ses_init_bound"));
        assert!(!runtime.contains("command"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_init_is_idempotent_for_same_identity_but_rejects_conflicts() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-init-identity-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();

        let args = SessionInitArgs {
            dir: dir.clone(),
            link_repo: Some(repo.clone()),
            no_discover_context: false,
            profile: "default".to_string(),
            agent: None,
            model: Some("same-model".to_string()),
            force: false,
            json: false,
        };
        initialize_folder_session(&args).unwrap();
        initialize_folder_session(&args).unwrap();

        let conflicting = SessionInitArgs {
            model: Some("different-model".to_string()),
            ..args
        };
        let error = initialize_folder_session(&conflicting).unwrap_err();
        assert!(error
            .to_string()
            .contains("session folder already exists with different identity"));
        assert!(error
            .to_string()
            .contains("model existing=same-model requested=different-model"));

        let forced = SessionInitArgs {
            force: true,
            ..conflicting
        };
        initialize_folder_session(&forced).unwrap();
        assert!(fs::read_to_string(dir.join("djinn.toml"))
            .unwrap()
            .contains("model = \"different-model\""));

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

    #[test]
    fn format_agent_config_options_marks_current_choices() {
        let rendered = format_agent_config_options(
            "architect",
            "openai/gpt-5.5",
            &["default".to_string(), "architect".to_string()],
            &["gpt-4o-mini".to_string(), "openai/gpt-5.5".to_string()],
            OutputFormat::Text,
        )
        .unwrap();

        assert!(rendered.contains("Agent config options"));
        assert!(rendered.contains("Current profile: architect"));
        assert!(rendered.contains("* architect"));
        assert!(rendered.contains("  default"));
        assert!(rendered.contains("Current model: openai/gpt-5.5"));
        assert!(rendered.contains("* openai/gpt-5.5"));
    }

    #[test]
    fn format_agent_config_options_outputs_json() {
        let rendered = format_agent_config_options(
            "default",
            "gpt-4o-mini",
            &["default".to_string()],
            &["gpt-4o-mini".to_string()],
            OutputFormat::Json,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["current_profile"], "default");
        assert_eq!(value["current_model"], "gpt-4o-mini");
        assert_eq!(value["profiles"][0], "default");
        assert_eq!(value["models"][0], "gpt-4o-mini");
    }

    #[test]
    fn format_agent_effective_config_renders_text_summary() {
        let config = AgentEffectiveConfig {
            workspace: "/tmp/project".to_string(),
            agent_name: Some("reviewer".to_string()),
            profile: "architect".to_string(),
            model: "openai/gpt-5.5".to_string(),
            agent_instructions: vec!["docs/review.md".to_string()],
            agent_tools: vec!["read_file".to_string()],
            read_access: ReadAccessPolicy {
                allow_roots: vec![PathBuf::from("/tmp/project")],
                deny_roots: vec![PathBuf::from("/tmp/project/secrets")],
                rules: vec![ReadAccessRule {
                    pattern: "*/docs/*".to_string(),
                    effect: ReadAccessEffect::Allow,
                }],
            },
            permissions: PermissionPolicy {
                rules: vec![PermissionRule {
                    action: "write".to_string(),
                    resource: "*.rs".to_string(),
                    effect: PermissionEffect::Ask,
                }],
            },
            read_access_rules: vec![AgentEffectivePolicyRule {
                source: "profile:architect".to_string(),
                action: "read".to_string(),
                resource: "*/docs/*".to_string(),
                effect: "allow".to_string(),
            }],
            permission_rules: vec![AgentEffectivePolicyRule {
                source: "profile:architect".to_string(),
                action: "write".to_string(),
                resource: "*.rs".to_string(),
                effect: "ask".to_string(),
            }],
            guardrails: agent_policy_guardrails(),
        };

        let rendered = format_agent_effective_config(&config, OutputFormat::Text).unwrap();

        assert!(rendered.contains("Agent effective config"));
        assert!(rendered.contains("Workspace: /tmp/project"));
        assert!(rendered.contains("Agent: reviewer"));
        assert!(rendered.contains("Profile: architect"));
        assert!(rendered.contains("Model: openai/gpt-5.5"));
        assert!(rendered.contains("docs/review.md"));
        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("allow root: /tmp/project"));
        assert!(rendered.contains("deny root: /tmp/project/secrets"));
        assert!(rendered.contains("Allow: */docs/*"));
        assert!(rendered.contains("Ask: write *.rs"));
        assert!(rendered.contains("profile:architect"));
        assert!(rendered.contains("destructive-action guardrails always apply"));
        assert!(rendered.contains("secret-read guardrails"));
    }

    #[test]
    fn format_agent_effective_config_outputs_json() {
        let config = AgentEffectiveConfig {
            workspace: "/tmp/project".to_string(),
            agent_name: None,
            profile: "default".to_string(),
            model: "gpt-4o-mini".to_string(),
            agent_instructions: Vec::new(),
            agent_tools: Vec::new(),
            read_access: ReadAccessPolicy::allow_by_default(),
            permissions: PermissionPolicy::allow_by_default(),
            read_access_rules: Vec::new(),
            permission_rules: Vec::new(),
            guardrails: agent_policy_guardrails(),
        };

        let rendered = format_agent_effective_config(&config, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["workspace"], "/tmp/project");
        assert_eq!(value["agent_name"], Value::Null);
        assert_eq!(value["profile"], "default");
        assert_eq!(value["model"], "gpt-4o-mini");
        assert_eq!(value["permissions"]["rules"].as_array().unwrap().len(), 0);
        assert!(value["guardrails"].as_array().unwrap().len() >= 3);
    }

    #[test]
    fn format_agent_policy_surfaces_list_audit_and_revoke() {
        let config = AgentEffectiveConfig {
            workspace: "/tmp/project".to_string(),
            agent_name: Some("reviewer".to_string()),
            profile: "architect".to_string(),
            model: "openai/gpt-5.5".to_string(),
            agent_instructions: Vec::new(),
            agent_tools: Vec::new(),
            read_access: ReadAccessPolicy::allow_by_default(),
            permissions: PermissionPolicy::allow_by_default(),
            read_access_rules: vec![AgentEffectivePolicyRule {
                source: "shared permissions".to_string(),
                action: "read".to_string(),
                resource: "*".to_string(),
                effect: "allow".to_string(),
            }],
            permission_rules: vec![AgentEffectivePolicyRule {
                source: "profile:architect".to_string(),
                action: "shell".to_string(),
                resource: "*".to_string(),
                effect: "ask".to_string(),
            }],
            guardrails: agent_policy_guardrails(),
        };

        let report = agent_policy_report(&config);
        let rendered = format_agent_policy_report(&report, OutputFormat::Text).unwrap();
        assert!(rendered.contains("Agent effective policy"));
        assert!(rendered.contains("shared permissions"));
        assert!(rendered.contains("profile:architect: ask shell *"));
        assert!(rendered.contains("Durable approvals: not implemented"));

        let audit = agent_policy_audit_report(&config);
        let rendered = format_agent_policy_audit_report(&audit, OutputFormat::Text).unwrap();
        assert!(rendered.contains("Agent policy audit"));
        assert!(rendered.contains("hard_guardrails"));
        assert!(rendered.contains("no_durable_approval_store"));

        let revoke = AgentPolicyRevokeReport {
            action: Some("shell".to_string()),
            resource: Some("printf hello".to_string()),
            durable_approvals_found: 0,
            revoked: 0,
            message: "No durable approval store exists yet".to_string(),
        };
        let rendered = format_agent_policy_revoke_report(&revoke, OutputFormat::Text).unwrap();
        assert!(rendered.contains("Agent policy revoke"));
        assert!(rendered.contains("Revoked: 0"));
        assert!(rendered.contains("Action selector: shell"));
    }

    #[test]
    fn format_agent_tool_specs_lists_tool_names_and_summaries() {
        let specs = vec![ToolSpec {
            name: "edit_file".to_string(),
            description: "Replace one exact text block. Extra detail.".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];

        let rendered = format_agent_tool_specs(&specs, OutputFormat::Text).unwrap();

        assert!(rendered.contains("Agent runtime tools"));
        assert!(rendered.contains("1 tool"));
        assert!(rendered.contains("- edit_file"));
        assert!(rendered.contains("Replace one exact text block."));
        assert!(!rendered.contains("Extra detail"));
    }

    #[test]
    fn format_agent_tool_specs_outputs_json_schemas() {
        let specs = vec![ToolSpec {
            name: "write_file".to_string(),
            description: "Create or replace a file.".to_string(),
            input_schema: serde_json::json!({"type": "object", "required": ["path"]}),
        }];

        let rendered = format_agent_tool_specs(&specs, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value[0]["name"], "write_file");
        assert_eq!(value[0]["input_schema"]["required"][0], "path");
    }

    #[test]
    fn agent_tool_specs_apply_role_allowlist() {
        let workspace = std::env::temp_dir();
        let specs = agent_tool_specs(
            Some(workspace),
            "default",
            &["read_file".to_string(), "search_files".to_string()],
        )
        .unwrap();
        let names = specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["read_file", "search_files"]);
    }

    #[test]
    fn resolve_agent_tool_spec_matches_exact_and_unique_substrings() {
        let specs = vec![
            ToolSpec {
                name: "read_file".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
            ToolSpec {
                name: "write_file".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
        ];

        assert_eq!(
            resolve_agent_tool_spec(&specs, "READ_FILE").unwrap().name,
            "read_file"
        );
        assert_eq!(
            resolve_agent_tool_spec(&specs, "write").unwrap().name,
            "write_file"
        );
    }

    #[test]
    fn resolve_agent_tool_spec_rejects_unknown_and_ambiguous_names() {
        let specs = vec![
            ToolSpec {
                name: "read_file".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
            ToolSpec {
                name: "write_file".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
        ];

        assert!(resolve_agent_tool_spec(&specs, "missing")
            .unwrap_err()
            .to_string()
            .contains("unknown"));
        assert!(resolve_agent_tool_spec(&specs, "file")
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
    }

    #[test]
    fn format_agent_tool_spec_shows_schema_in_text_and_json() {
        let spec = ToolSpec {
            name: "write_file".to_string(),
            description: "Create or replace a file.".to_string(),
            input_schema: serde_json::json!({"type": "object", "required": ["path", "content"]}),
        };

        let text = format_agent_tool_spec(&spec, OutputFormat::Text).unwrap();
        assert!(text.contains("write_file"));
        assert!(text.contains("Create or replace a file."));
        assert!(text.contains("Input schema:"));
        assert!(text.contains("\"required\""));

        let json = format_agent_tool_spec(&spec, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "write_file");
        assert_eq!(value["input_schema"]["required"][1], "content");
    }

    #[test]
    fn agent_model_messages_keep_conversation_turns() {
        let session = AgentSession {
            id: AgentSessionId::new("agt_test"),
            meta: AgentSessionMeta::default(),
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "hello".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "hi".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ModelResponseMetadata {
                    model: "openai/gpt-test".to_string(),
                    provider: Some("openai".to_string()),
                    round: Some(0),
                    elapsed_ms: 10,
                    tool_calls: 0,
                    has_message: true,
                    request_chars: Some(5),
                    response_chars: Some(2),
                    retry_attempts: None,
                    usage: Some(AgentSessionTokenUsage {
                        input_tokens: Some(1),
                        output_tokens: Some(2),
                        total_tokens: Some(3),
                    }),
                    estimated_cost: None,
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                    id: "call-1".to_string(),
                    output: serde_json::json!({"stdout": "ignored"}),
                    success: true,
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolExecutionMetadata {
                    id: "call-1".to_string(),
                    name: "shell".to_string(),
                    round: Some(0),
                    elapsed_ms: 10,
                    success: true,
                    input_bytes: Some(10),
                    output_bytes: Some(20),
                    approval_required: Some(false),
                    approval_scope: None,
                    skipped_operations: Some(0),
                }),
            ],
        };

        let messages = agent_model_messages(&session, "/tmp/project", &[]);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ModelRole::System);
        assert_eq!(messages[1].role, ModelRole::User);
        assert_eq!(messages[1].content, "hello");
        assert_eq!(messages[2].role, ModelRole::Assistant);
        assert_eq!(messages[2].content, "hi");
    }

    #[test]
    fn agent_system_message_includes_resolved_instructions() {
        let instructions = vec![ResolvedAgentInstruction {
            source: "docs/review.md".to_string(),
            content: "Review for correctness and regressions.".to_string(),
        }];

        let message = agent_system_message("/tmp/project", &instructions);

        assert_eq!(message.role, ModelRole::System);
        assert!(message.content.contains("workspace `/tmp/project`"));
        assert!(message
            .content
            .contains("Additional configured instructions"));
        assert!(message.content.contains("--- docs/review.md ---"));
        assert!(message
            .content
            .contains("Review for correctness and regressions."));
    }

    #[test]
    fn read_agent_instruction_file_reads_workspace_relative_file() {
        let workspace =
            std::env::temp_dir().join(format!("djinn-instruction-test-{}", current_time_millis()));
        fs::create_dir_all(&workspace).unwrap();
        let path = workspace.join("AGENTS.md");
        fs::write(&path, "Use project conventions.\n").unwrap();

        let instruction = read_agent_instruction_file(&workspace, "AGENTS.md")
            .unwrap()
            .unwrap();

        assert_eq!(instruction.source, path.display().to_string());
        assert_eq!(instruction.content, "Use project conventions.");
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(workspace);
    }

    #[test]
    fn latest_session_model_uses_latest_model_until_profile_changes() {
        let mut session = AgentSession {
            id: AgentSessionId::new("agt_model"),
            meta: AgentSessionMeta::default(),
            events: vec![AgentSessionEvent::new(
                AgentSessionEventKind::SessionModelUpdated {
                    model: "openai/gpt-5.5".to_string(),
                },
            )],
        };

        assert_eq!(
            latest_session_model(&session).as_deref(),
            Some("openai/gpt-5.5")
        );

        session.events.push(AgentSessionEvent::new(
            AgentSessionEventKind::SessionProfileUpdated {
                profile: "architect".to_string(),
            },
        ));

        assert_eq!(latest_session_model(&session), None);

        session.events.push(AgentSessionEvent::new(
            AgentSessionEventKind::SessionModelUpdated {
                model: "openai/gpt-5.4-mini".to_string(),
            },
        ));

        assert_eq!(
            latest_session_model(&session).as_deref(),
            Some("openai/gpt-5.4-mini")
        );
    }

    #[test]
    fn child_session_depth_limit_allows_three_levels_below_root() {
        let store = temp_agent_store("child-depth-allow");
        let root = store
            .create_session(AgentSessionMeta {
                title: "root".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let child = store
            .create_session(AgentSessionMeta {
                title: "child".to_string(),
                parent_session_id: Some(root),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let grandchild = store
            .create_session(AgentSessionMeta {
                title: "grandchild".to_string(),
                parent_session_id: Some(child),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        validate_agent_child_session_depth(&store, Some(&grandchild)).unwrap();
    }

    #[test]
    fn child_session_depth_limit_rejects_fourth_level_below_root() {
        let store = temp_agent_store("child-depth-reject");
        let root = store
            .create_session(AgentSessionMeta {
                title: "root".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let child = store
            .create_session(AgentSessionMeta {
                title: "child".to_string(),
                parent_session_id: Some(root),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let grandchild = store
            .create_session(AgentSessionMeta {
                title: "grandchild".to_string(),
                parent_session_id: Some(child),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let great_grandchild = store
            .create_session(AgentSessionMeta {
                title: "great grandchild".to_string(),
                parent_session_id: Some(grandchild),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        let err = validate_agent_child_session_depth(&store, Some(&great_grandchild)).unwrap_err();

        assert!(err
            .to_string()
            .contains("child session depth limit exceeded"));
        assert!(err.to_string().contains("maximum child-session depth is 3"));
    }

    #[test]
    fn maybe_auto_title_agent_session_titles_first_default_session_prompt() {
        let store = temp_agent_store("auto-title");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Agent chat".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "Implement session auto title\nwith extra details".to_string(),
                }),
            )
            .unwrap();

        maybe_auto_title_agent_session(
            &store,
            &id,
            "Implement session auto title\nwith extra details",
        )
        .unwrap();

        let loaded = store.load_session(&id).unwrap();
        assert_eq!(loaded.meta.title, "Implement session auto title");
        assert!(loaded.events.iter().any(|event| matches!(
            &event.kind,
            AgentSessionEventKind::SessionTitleUpdated { title } if title == "Implement session auto title"
        )));
    }

    #[test]
    fn maybe_auto_title_agent_session_preserves_explicit_title() {
        let store = temp_agent_store("auto-title-explicit");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Explicit title".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "Different first prompt".to_string(),
                }),
            )
            .unwrap();

        maybe_auto_title_agent_session(&store, &id, "Different first prompt").unwrap();

        let loaded = store.load_session(&id).unwrap();
        assert_eq!(loaded.meta.title, "Explicit title");
    }

    #[test]
    fn opencode_default_model_reads_coder_agent_model() {
        let model = opencode_default_model_from_content(
            r#"{
              "agents": {
                "coder": { "model": "gpt-4.1" },
                "task": { "model": "gpt-4.1-mini" }
              }
            }"#,
            "default",
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("gpt-4.1"));
    }

    #[test]
    fn opencode_default_model_reads_new_agent_map_default_agent() {
        let model = opencode_default_model_from_content(
            r##"{
              "default_agent": "🧠",
              "model": "openai/gpt-5.4-mini",
              "agent": {
                "🧠": { "model": "openai/gpt-5.5" },
                "review": { "model": "openai/gpt-5.4" }
              }
            }"##,
            "default",
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("openai/gpt-5.5"));
    }

    #[test]
    fn opencode_default_model_reads_requested_profile_agent() {
        let model = opencode_default_model_from_content(
            r##"{
              "default_agent": "🧠",
              "model": "openai/gpt-5.4-mini",
              "agent": {
                "🧠": { "model": "openai/gpt-5.5" },
                "review": { "model": "openai/gpt-5.4" }
              }
            }"##,
            "review",
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("openai/gpt-5.4"));
    }

    #[test]
    fn opencode_default_model_falls_back_to_top_level_model() {
        let model = opencode_default_model_from_content(
            r#"{
              "model": "openai/gpt-5.4-mini"
            }"#,
            "default",
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("openai/gpt-5.4-mini"));
    }

    #[test]
    fn opencode_read_access_rules_reads_new_agent_permissions() {
        let workspace = PathBuf::from("/tmp/djinn-workspace");
        let rules = opencode_read_access_rules_from_content(
            r#"{
              "default_agent": "architect",
              "permissions": [
                { "action": "read", "resource": "*.env", "effect": "ask" }
              ],
              "agent": {
                "architect": {
                  "permissions": [
                    { "action": "read", "resource": "~/public/*", "effect": "allow" },
                    { "action": "read", "resource": "~/.ssh/*", "effect": "deny" }
                  ]
                }
              }
            }"#,
            "default",
            &workspace,
        )
        .unwrap();

        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].pattern, "*.env");
        assert_eq!(rules[0].effect, ReadAccessEffect::Ask);
        assert!(rules[1].pattern.ends_with("/public/*"));
        assert_eq!(rules[1].effect, ReadAccessEffect::Allow);
        assert!(rules[2].pattern.ends_with("/.ssh/*"));
        assert_eq!(rules[2].effect, ReadAccessEffect::Deny);
    }

    #[test]
    fn opencode_read_access_rules_reads_old_permission_object_for_profile() {
        let workspace = PathBuf::from("/tmp/djinn-workspace");
        let rules = opencode_read_access_rules_from_content(
            r#"{
              "agents": {
                "coder": {
                  "permission": {
                    "read": {
                      "docs/*": "allow",
                      "secrets/*": "deny"
                    }
                  }
                }
              }
            }"#,
            "coder",
            &workspace,
        )
        .unwrap();

        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].pattern, "/tmp/djinn-workspace/docs/*");
        assert_eq!(rules[0].effect, ReadAccessEffect::Allow);
        assert_eq!(rules[1].pattern, "/tmp/djinn-workspace/secrets/*");
        assert_eq!(rules[1].effect, ReadAccessEffect::Deny);
    }

    #[test]
    fn opencode_permission_policy_rules_map_bash_to_shell() {
        let workspace = PathBuf::from("/tmp/djinn-workspace");
        let rules = opencode_permission_policy_rules_from_content(
            r#"{
              "default_agent": "architect",
              "agent": {
                "architect": {
                  "permissions": [
                    { "action": "bash", "resource": "git reset*", "effect": "deny" },
                    { "action": "shell", "resource": "cargo test*", "effect": "allow" }
                  ]
                }
              }
            }"#,
            "default",
            &workspace,
        )
        .unwrap();

        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].action, "shell");
        assert_eq!(rules[0].resource, "git reset*");
        assert_eq!(rules[0].effect, PermissionEffect::Deny);
        assert_eq!(rules[1].action, "shell");
        assert_eq!(rules[1].resource, "cargo test*");
        assert_eq!(rules[1].effect, PermissionEffect::Allow);
    }

    #[test]
    fn opencode_permission_policy_rules_read_old_permission_object() {
        let workspace = PathBuf::from("/tmp/djinn-workspace");
        let rules = opencode_permission_policy_rules_from_content(
            r#"{
              "agents": {
                "coder": {
                  "permission": {
                    "shell": {
                      "npm publish*": "deny"
                    },
                    "edit": "allow"
                  }
                }
              }
            }"#,
            "coder",
            &workspace,
        )
        .unwrap();

        assert_eq!(rules.len(), 2);
        assert!(rules.iter().any(|rule| {
            rule.action == "shell"
                && rule.resource == "npm publish*"
                && rule.effect == PermissionEffect::Deny
        }));
        assert!(rules.iter().any(|rule| {
            rule.action == "edit" && rule.resource == "*" && rule.effect == PermissionEffect::Allow
        }));
    }

    #[test]
    fn opencode_default_model_uses_first_existing_path() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-opencode-model-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.json");
        let first = dir.join("first.json");
        let second = dir.join("second.json");
        fs::write(&first, r#"{"agents":{"coder":{"model":"gpt-4.1"}}}"#).unwrap();
        fs::write(&second, r#"{"agents":{"coder":{"model":"gpt-5"}}}"#).unwrap();

        let model =
            opencode_default_model_from_paths(&[missing, first, second], "default").unwrap();
        assert_eq!(model.as_deref(), Some("gpt-4.1"));
    }

    #[test]
    fn opencode_openai_api_key_reads_provider_key() {
        let api_key = opencode_openai_api_key_from_content(
            r#"{
              "providers": {
                "openai": { "apiKey": "sk-test" }
              }
            }"#,
        )
        .unwrap();
        assert_eq!(api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn opencode_openai_api_key_uses_first_existing_path() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-opencode-key-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.json");
        let first = dir.join("first.json");
        let second = dir.join("second.json");
        fs::write(&first, r#"{"providers":{"openai":{"apiKey":"sk-first"}}}"#).unwrap();
        fs::write(
            &second,
            r#"{"providers":{"openai":{"apiKey":"sk-second"}}}"#,
        )
        .unwrap();

        let api_key = opencode_openai_api_key_from_paths(&[missing, first, second]).unwrap();
        assert_eq!(api_key.as_deref(), Some("sk-first"));
    }

    #[test]
    fn opencode_auth_openai_api_key_reads_api_auth() {
        let api_key = opencode_auth_openai_api_key_from_content(
            r#"{
              "openai": { "type": "api", "key": "sk-auth" }
            }"#,
        )
        .unwrap();
        assert_eq!(api_key.as_deref(), Some("sk-auth"));
    }

    #[test]
    fn opencode_auth_openai_oauth_reads_access_refresh_and_account() {
        let auth = opencode_auth_openai_auth_from_content(
            r#"{
              "openai": {
                "type": "oauth",
                "access": "access-token",
                "refresh": "refresh-token",
                "expires": 9999999999999,
                "accountId": "account-123"
              }
            }"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            auth,
            OpenCodeOpenAiAuthCredential::OAuth(OpenCodeOpenAiOAuthCredential {
                access: "access-token".to_string(),
                refresh: "refresh-token".to_string(),
                expires: 9999999999999,
                account_id: Some("account-123".to_string()),
            })
        );
    }

    #[test]
    fn opencode_auth_openai_api_key_helper_ignores_oauth() {
        let api_key = opencode_auth_openai_api_key_from_content(
            r#"{
              "openai": {
                "type": "oauth",
                "access": "access-token",
                "refresh": "refresh-token",
                "expires": 9999999999999
              }
            }"#,
        )
        .unwrap();
        assert_eq!(api_key, None);
    }

    #[test]
    fn extract_account_id_from_jwt_reads_nested_openai_claim() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1"}}"#);
        let token = format!("header.{payload}.signature");
        assert_eq!(
            extract_account_id_from_jwt(&token).as_deref(),
            Some("acct-1")
        );
    }

    #[test]
    fn write_refreshed_opencode_oauth_preserves_other_providers() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-opencode-oauth-write-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        let content = r#"{
          "google": { "type": "api", "key": "google-key" },
          "openai": { "type": "oauth", "access": "old", "refresh": "old", "expires": 1 }
        }"#;
        fs::write(&path, content).unwrap();

        write_refreshed_opencode_openai_oauth(
            &path,
            content,
            &OpenCodeOpenAiOAuthCredential {
                access: "new-access".to_string(),
                refresh: "new-refresh".to_string(),
                expires: 42,
                account_id: Some("acct-2".to_string()),
            },
        )
        .unwrap();

        let rendered = fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            parsed["google"]["key"],
            Value::String("google-key".to_string())
        );
        assert_eq!(
            parsed["openai"]["access"],
            Value::String("new-access".to_string())
        );
        assert_eq!(
            parsed["openai"]["accountId"],
            Value::String("acct-2".to_string())
        );
    }

    #[test]
    fn infer_ingest_target_routes_memory_kinds() {
        assert_eq!(
            infer_ingest_target(&test_memory("instruction", "Use uv")),
            IngestTarget::Suggestion
        );
        assert_eq!(
            infer_ingest_target(&test_memory("skill-proposal", "Reusable workflow")),
            IngestTarget::Skill
        );
        assert_eq!(
            infer_ingest_target(&test_memory("idea", "Consider better search")),
            IngestTarget::Idea
        );
        assert_eq!(
            infer_ingest_target(&test_memory("action", "TODO: review docs")),
            IngestTarget::Action
        );
        assert_eq!(
            infer_ingest_target(&test_memory("preference", "Prefer concise output")),
            IngestTarget::Suggestion
        );
    }

    #[test]
    fn format_memory_review_prompt_creates_suggestions_from_memories() {
        let memories = vec![MemoryRecord {
            id: "djinn-session-note".to_string(),
            text: "Djinn implementation session detail".to_string(),
            created_at: "2026-07-09".to_string(),
            status: "active".to_string(),
            scope: "project:djinn".to_string(),
            kind: "implementation-note".to_string(),
            confidence: "medium".to_string(),
            not_before: String::new(),
            evidence: vec!["Captured during a Djinn session.".to_string()],
            sources: Vec::new(),
        }];
        let suggestions = vec![SuggestionRecord {
            id: "suggestion".to_string(),
            text: "Create a skill for recurring validation.".to_string(),
            created_at: "2026-07-09".to_string(),
            status: "open".to_string(),
            target: "skill".to_string(),
            rationale: "Repeated validation friction.".to_string(),
            draft: String::new(),
            evidence: Vec::new(),
            sources: Vec::new(),
        }];
        let args = ReviewMemoriesArgs {
            ids: Vec::new(),
            limit: 100,
            all: false,
            query: Some("djinn".to_string()),
            agent: None,
            title: "review".to_string(),
            opencode_bin: "opencode".to_string(),
            dry_run: true,
        };

        let prompt = format_memory_review_prompt(&memories, &suggestions, &args);
        assert!(prompt.contains("Memory Suggestion Review"));
        assert!(prompt.contains("djinn add suggestion"));
        assert!(prompt.contains("djinn-session-note"));
        assert!(prompt.contains("Create a skill for recurring validation."));
    }

    #[test]
    fn background_review_script_uses_prompt_file_and_notification() {
        let script = background_review_script(
            "opencode",
            "memory review",
            Some("reviewer"),
            Path::new("/tmp/prompt's.md"),
            Path::new("/tmp/out.md"),
        );
        assert!(script.contains("PROMPT_FILE='/tmp/prompt'\\''s.md'"));
        assert!(script.contains("DJINN_REVIEWER=1"));
        assert!(script.contains("osascript"));
        assert!(script.contains("--agent \"$AGENT\""));
        assert!(script.contains("> \"$OUT_FILE\" 2>&1"));
    }

    #[test]
    fn memory_source_format_tolerates_legacy_chat_reference() {
        let source = MemorySource {
            source_type: "chat".to_string(),
            source: "opencode".to_string(),
            source_id: "ses_missing".to_string(),
            chat_id: "missing-chat".to_string(),
            title: "Deleted OpenCode session".to_string(),
            captured_at: "2026-07-09".to_string(),
        };
        let rendered = format_memory_source(&source);
        assert!(rendered.contains("legacy chat reference"));
        assert!(rendered.contains("Deleted OpenCode session"));
    }
}
