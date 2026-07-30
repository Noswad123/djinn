use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::net::TcpListener;
use std::path::{Component, Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use djinn_agent::{
    tools_with_policies_file_history_and_gate, AgentProgressEvent, AgentRuntime, CopilotClient,
    ModelClient, ModelMessage, ModelRequest, ModelRole, OpenAiAuth, OpenAiClient, OpenAiOAuth,
    PermissionDecision, PermissionEffect, PermissionGate, PermissionPolicy, PermissionRequest,
    PermissionRule, ReadAccessEffect, ReadAccessPolicy, ReadAccessRule, ToolSpec,
};
use djinn_contexts::{resolve_context, ContextInput, ContextRecord, ContextStore};
use djinn_memory::{
    lifecycle_for, ActionRecord, ActionStore, AgentSession, AgentSessionEvent,
    AgentSessionEventKind, AgentSessionExecutionMode, AgentSessionId, AgentSessionLifecycleState,
    AgentSessionMeta, AgentSessionPolicyRule, AgentSessionPolicySnapshot,
    AgentSessionRuntimeConfig, AgentSessionStore, FileHistoryEntryId, FileHistoryFilter,
    FileHistoryRestoreOptions, IdeaRecord, IdeaStore, JsonlAgentSessionStore,
    JsonlFileHistoryStore, MemoryInput, MemoryRecord, MemorySource, SuggestionInput,
    SuggestionRecord, SuggestionStore,
};
use djinn_skills::{
    list_skills as discover_skills, read_skill_content, resolve_skill, SkillRecord, SkillRoot,
    SkillStore,
};
use djinn_tools::ToolEntry;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const AGENT_CHILD_SESSION_MAX_DEPTH: usize = 3;
const DEFAULT_AGENT_MAX_TOOL_ROUNDS: usize = 128;
const FOLDER_SESSION_CONTEXT_MAX_FILE_BYTES: u64 = 32 * 1024;
const FOLDER_SESSION_CONTEXT_MAX_TOTAL_BYTES: usize = 96 * 1024;
const FOLDER_SESSION_CONTEXT_MAX_FILES: usize = 16;
const FOLDER_SESSION_COMPACT_SNIPPET_CHARS: usize = 1_200;
const FOLDER_SESSION_COMPACT_START_MARKER: &str = "<!-- djinn:generated:start -->";
const FOLDER_SESSION_COMPACT_END_MARKER: &str = "<!-- djinn:generated:end -->";
const FOLDER_NATIVE_SESSION_DIR: &str = ".djinn";

#[derive(Debug, Parser)]
#[command(name = "djinn")]
#[command(about = "Local-first companion for OpenCode and other AI coding agents")]
struct Cli {
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
    /// Folder-backed session name or directory for convenience actions.
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
    /// Rename legacy long cache folder names to short copy-pasteable names.
    ShortenNames(SessionShortenNamesArgs),
    /// Remove a folder-backed session and its linked native session when present.
    Rm(SessionRmArgs),
}

#[derive(Debug, Args)]
struct SessionWatchArgs {
    /// Folder-backed session name or directory to watch.
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
    /// Folder-backed session name or directory to run.
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
    /// Folder-backed session name or directory containing turns/ and context/.
    #[arg(long = "session-dir")]
    session_dir: PathBuf,
    /// Output path. Defaults to <session-dir>/context/compacted.md.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionPromoteArgs {
    /// Folder-backed session names or directories to promote from.
    #[arg(required = true)]
    dirs: Vec<PathBuf>,
    /// Promotion type to prepare for.
    #[arg(long = "type", alias = "target", value_enum, default_value_t = SessionPromoteType::Memory)]
    promotion_type: SessionPromoteType,
    /// Promotion session folder to create. Bare names live under Djinn's cache session root.
    #[arg(long = "session-dir", alias = "output-dir")]
    promotion_session_dir: Option<PathBuf>,
    /// Maximum characters to include from each artifact excerpt.
    #[arg(long = "max-chars-per-artifact", default_value_t = 1200)]
    max_chars_per_artifact: usize,
    /// Replace generated promotion-session files if they already exist.
    #[arg(long)]
    force: bool,
    /// Output JSON instead of a text summary.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionDecisionArgs {
    /// Promotion session name or directory.
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
    /// Promotion session name or directory whose source sessions should be removed.
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
    /// Pattern promotion session name or directory.
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
    /// Promotion session name or directory.
    dir: PathBuf,
    /// Optional candidate id/path within the promotion session. Defaults to all candidates.
    candidate: Option<String>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum SessionPromoteType {
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
    /// Folder-backed session name or directory to update.
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
    /// Folder-backed session name or directory to inspect.
    session: PathBuf,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionContextAddArgs {
    /// Folder-backed session name or directory to update.
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
    /// Folder-backed session name or directory to update.
    session: PathBuf,
    /// Context entry name to remove.
    name: String,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionStatusArgs {
    /// Folder-backed session name or directory to inspect.
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
struct SessionOpenArgs {
    /// Folder-backed session name or directory to open.
    dir: PathBuf,
    /// Session artifact to open. Defaults to summary.md.
    #[arg(value_enum, default_value_t = SessionOpenTarget::Summary)]
    target: SessionOpenTarget,
    /// Editor command. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    editor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SessionOpenTarget {
    Summary,
    Request,
    Context,
    Compacted,
    Turns,
    Manifest,
    Repo,
}

#[derive(Debug, Args)]
struct SessionRmArgs {
    /// Folder-backed session name or directory to remove.
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

#[derive(Debug, Default)]
struct TerminalPermissionGate {
    session_scopes: Mutex<Vec<TerminalApprovalScope>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalApprovalScope {
    action: String,
    workspace: String,
    resources: HashSet<String>,
}

impl TerminalPermissionGate {
    fn new() -> Self {
        Self {
            session_scopes: Mutex::new(Vec::new()),
        }
    }

    fn cached_decision(&self, request: &PermissionRequest) -> Option<PermissionDecision> {
        let request_resources = approval_resources_from_metadata(&request.metadata);
        if request_resources.is_empty() {
            return None;
        }
        let workspace = request
            .metadata
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let scopes = self.session_scopes.lock().ok()?;
        let mut approved = Vec::new();
        for resource in &request_resources {
            let covered = scopes.iter().any(|scope| {
                scope.action == request.action
                    && scope.workspace == workspace
                    && scope.resources.contains(resource)
            });
            if !covered {
                return None;
            }
            approved.push(resource.clone());
        }
        if request
            .metadata
            .get("preview")
            .and_then(Value::as_array)
            .is_some()
        {
            Some(PermissionDecision::AllowPaths { paths: approved })
        } else {
            Some(PermissionDecision::AllowResources {
                resources: approved,
            })
        }
    }

    fn remember_resources_for_session(&self, request: &PermissionRequest, resources: Vec<String>) {
        let resources = resources
            .into_iter()
            .map(|resource| resource.trim().to_string())
            .filter(|resource| !resource.is_empty())
            .collect::<HashSet<_>>();
        if resources.is_empty() {
            return;
        }
        let workspace = request
            .metadata
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Ok(mut scopes) = self.session_scopes.lock() else {
            return;
        };
        if let Some(existing) = scopes
            .iter_mut()
            .find(|scope| scope.action == request.action && scope.workspace == workspace)
        {
            existing.resources.extend(resources);
        } else {
            scopes.push(TerminalApprovalScope {
                action: request.action.clone(),
                workspace,
                resources,
            });
        }
    }

    fn report_permission_blocked(&self, _request: &PermissionRequest) {}

    fn report_permission_resolved(&self) {}
}

#[async_trait]
impl PermissionGate for TerminalPermissionGate {
    async fn approve(&self, request: PermissionRequest) -> Result<PermissionDecision> {
        if let Some(decision) = self.cached_decision(&request) {
            return Ok(decision);
        }
        self.report_permission_blocked(&request);
        if request
            .metadata
            .get("preview")
            .and_then(Value::as_array)
            .is_some()
            && io::stdin().is_terminal()
            && io::stdout().is_terminal()
        {
            let decision = match djinn_tui::run_approval_dialog(request.metadata.clone())? {
                djinn_tui::ApprovalDecision::ApproveAll => PermissionDecision::Allow,
                djinn_tui::ApprovalDecision::ApprovePaths(paths) => {
                    PermissionDecision::AllowPaths { paths }
                }
                djinn_tui::ApprovalDecision::ApproveAllForSession(paths)
                | djinn_tui::ApprovalDecision::ApprovePathsForSession(paths) => {
                    self.remember_resources_for_session(&request, paths.clone());
                    PermissionDecision::AllowPaths { paths }
                }
                djinn_tui::ApprovalDecision::Deny => PermissionDecision::Deny,
            };
            self.report_permission_resolved();
            return Ok(decision);
        }
        eprintln!("\nPermission approval required: {}", request.description);
        eprint!("{}", format_permission_preview(&request.metadata)?);
        eprint!("Approve this request? [y]es once, [s]ession, [N]o: ");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        let decision = if answer == "y" || answer == "yes" {
            PermissionDecision::Allow
        } else if answer == "s" || answer == "session" {
            let resources = approval_resources_from_metadata(&request.metadata);
            self.remember_resources_for_session(&request, resources.clone());
            if request
                .metadata
                .get("preview")
                .and_then(Value::as_array)
                .is_some()
            {
                PermissionDecision::AllowPaths { paths: resources }
            } else {
                PermissionDecision::AllowResources { resources }
            }
        } else {
            PermissionDecision::Deny
        };
        self.report_permission_resolved();
        Ok(decision)
    }
}

fn approval_resources_from_metadata(metadata: &Value) -> Vec<String> {
    let mut resources = Vec::new();
    if let Some(preview) = metadata.get("preview").and_then(Value::as_array) {
        for item in preview {
            if let Some(path) = item
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.trim().is_empty())
            {
                push_unique_string(&mut resources, path);
            }
            if let Some(path) = item
                .get("new_path")
                .and_then(Value::as_str)
                .filter(|path| !path.trim().is_empty())
            {
                push_unique_string(&mut resources, path);
            }
        }
    }
    if let Some(values) = metadata.get("resources").and_then(Value::as_array) {
        for value in values {
            if let Some(resource) = value
                .as_str()
                .filter(|resource| !resource.trim().is_empty())
            {
                push_unique_string(&mut resources, resource);
            }
        }
    }
    if let Some(resource) = metadata
        .get("resource")
        .and_then(Value::as_str)
        .filter(|resource| !resource.trim().is_empty())
    {
        push_unique_string(&mut resources, resource);
    }
    resources
}

fn format_permission_preview(metadata: &Value) -> Result<String> {
    let Some(preview) = metadata.get("preview").and_then(Value::as_array) else {
        return Ok(format!("{}\n", serde_json::to_string_pretty(metadata)?));
    };
    let mut output = String::new();
    for item in preview {
        let operation = item["operation"].as_str().unwrap_or("operation");
        let path = item["relative_path"]
            .as_str()
            .or_else(|| item["path"].as_str())
            .unwrap_or("<unknown>");
        let added = item["lines_added"].as_u64().unwrap_or_default();
        let removed = item["lines_removed"].as_u64().unwrap_or_default();
        output.push_str(&format!("- {operation} {path} (+{added}/-{removed})\n"));
        if let Some(new_path) = item["relative_new_path"]
            .as_str()
            .or_else(|| item["new_path"].as_str())
        {
            output.push_str(&format!("  -> {new_path}\n"));
        }
        if let Some(hunks) = item["hunks"].as_array() {
            for (index, hunk) in hunks.iter().enumerate() {
                output.push_str(&format!("  @@ hunk {}\n", index + 1));
                if let Some(lines) = hunk["lines"].as_array() {
                    for line in lines {
                        let kind = line["kind"].as_str().unwrap_or("context");
                        let content = line["content"].as_str().unwrap_or_default();
                        let prefix = match kind {
                            "add" => '+',
                            "remove" => '-',
                            _ => ' ',
                        };
                        output.push_str(&format!("  {prefix} {content}\n"));
                    }
                }
            }
        }
    }
    Ok(output)
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
                .ok_or_else(|| anyhow!("session name or directory is required for --open"))?;
            session_open(SessionOpenArgs {
                dir,
                target: SessionOpenTarget::Summary,
                editor: args.editor,
            })
        }
        None if args.dir.is_some() => run_folder_session_tui(args.dir.unwrap()),
        None => run_tui_command(TuiArgs {
            view: TuiView::Sessions,
            roots: Vec::new(),
            editor: args.editor,
        }),
    }
}

fn run_folder_session_tui(dir: PathBuf) -> Result<()> {
    let session_dir = resolve_session_dir(&dir)?;
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
        let action_message = folder_session_action_message(&action, &session_dir);
        tui.suspend()?;
        let action_result = handle_folder_session_tui_action(action, session_dir.clone());
        tui.resume()?;
        message = Some(match action_result {
            Ok(()) => action_message,
            Err(err) => format!("Error: {err:#}"),
        });
    }
}

fn folder_session_action_message(
    action: &djinn_tui::FolderSessionAction,
    session_dir: &Path,
) -> String {
    match action {
        djinn_tui::FolderSessionAction::Run => {
            if folder_session_is_promotion(session_dir).unwrap_or(false) {
                "Started promotion generation in background".to_string()
            } else {
                "Started session run".to_string()
            }
        }
        djinn_tui::FolderSessionAction::Watch => "Watched session status".to_string(),
        djinn_tui::FolderSessionAction::OpenSummary => "Opened summary.md".to_string(),
        djinn_tui::FolderSessionAction::EditRequest => "Opened request.md".to_string(),
        djinn_tui::FolderSessionAction::OpenContext => "Opened context".to_string(),
        djinn_tui::FolderSessionAction::DiscoverContext => "Discovered session context".to_string(),
        djinn_tui::FolderSessionAction::ValidateCandidates => {
            "Validated promotion candidates".to_string()
        }
        djinn_tui::FolderSessionAction::ValidateCandidate(candidate) => {
            format!("Validated candidate {candidate}")
        }
        djinn_tui::FolderSessionAction::AcceptCandidate(candidate) => {
            format!("Accepted candidate {candidate}")
        }
        djinn_tui::FolderSessionAction::AcceptCandidateAndSyncMindweaver(candidate) => {
            format!("Accepted candidate {candidate} and ran MindWeaver sync handoff")
        }
        djinn_tui::FolderSessionAction::DenyCandidate(candidate) => {
            format!("Denied candidate {candidate}")
        }
        djinn_tui::FolderSessionAction::OpenCandidate(_) => "Opened candidate file".to_string(),
        djinn_tui::FolderSessionAction::OpenPath(path) => format!("Opened {path}"),
    }
}

fn handle_folder_session_tui_action(
    action: djinn_tui::FolderSessionAction,
    session_dir: PathBuf,
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
        djinn_tui::FolderSessionAction::Watch => session_watch(SessionWatchArgs {
            dir: session_dir,
            interval_ms: 1000,
            timeout_seconds: None,
            json: false,
        }),
        djinn_tui::FolderSessionAction::OpenSummary => session_open(SessionOpenArgs {
            dir: session_dir,
            target: SessionOpenTarget::Summary,
            editor: None,
        }),
        djinn_tui::FolderSessionAction::EditRequest => session_open(SessionOpenArgs {
            dir: session_dir,
            target: SessionOpenTarget::Request,
            editor: None,
        }),
        djinn_tui::FolderSessionAction::OpenContext => session_open(SessionOpenArgs {
            dir: session_dir,
            target: SessionOpenTarget::Context,
            editor: None,
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
            open_editor_path(Path::new(&path), None)
        }
        djinn_tui::FolderSessionAction::OpenPath(path) => open_editor_path(Path::new(&path), None),
    }
}

fn folder_session_is_promotion(session_dir: &Path) -> Result<bool> {
    Ok(read_folder_session_manifest(session_dir)?
        .and_then(|manifest| manifest.kind)
        .as_deref()
        == Some("promotion"))
}

fn folder_session_status_tui_view(
    session_dir: &Path,
) -> Result<djinn_tui::FolderSessionStatusView> {
    let report = folder_session_status(session_dir)?;
    let session_path = PathBuf::from(&report.session_dir);
    let title = session_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(folder_session_display_name)
        .unwrap_or_else(|| report.session_dir.clone());
    Ok(djinn_tui::FolderSessionStatusView {
        title,
        state: report.lifecycle.state.clone(),
        mode: report.lifecycle.mode.clone(),
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

fn run_session_command(command: SessionCommand) -> Result<()> {
    match command {
        SessionCommand::Init(args) => session_init(args),
        SessionCommand::Run(args) => session_run(args),
        SessionCommand::Watch(args) => session_watch(args),
        SessionCommand::Compact(args) => session_compact(args),
        SessionCommand::Promote(args) => session_promote(args),
        SessionCommand::Accept(args) => session_decide(args, SessionDecisionAction::Accept),
        SessionCommand::Deny(args) => session_decide(args, SessionDecisionAction::Deny),
        SessionCommand::ExportPattern(args) => session_export_pattern(args),
        SessionCommand::ValidateCandidates(args) => session_validate_candidates(args),
        SessionCommand::Cleanup(args) => session_cleanup(args),
        SessionCommand::Context(args) => session_context(args),
        SessionCommand::Status(args) => session_status(args),
        SessionCommand::Ls(args) => session_ls(args),
        SessionCommand::Open(args) => session_open(args),
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionCompactReport {
    session_dir: String,
    output_path: String,
    turn_count: usize,
    turns: Vec<CompactedTurnReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct CompactedTurnReport {
    id: String,
    request_path: Option<String>,
    response_path: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SessionDecisionAction {
    Accept,
    Deny,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionDecisionReport {
    action: SessionDecisionAction,
    dry_run: bool,
    session_dir: String,
    promotion_type: String,
    candidate: Option<String>,
    candidate_count: usize,
    decision_path: String,
    candidate_status_path: String,
    wrote_decision: bool,
    durable_writeback: bool,
    writebacks: Vec<SessionCandidateWritebackReport>,
    post_writebacks: Vec<SessionPostWritebackReport>,
    note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionPostWritebackReport {
    name: String,
    command: String,
    status: String,
    dry_run: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionCandidateWritebackReport {
    candidate: String,
    candidate_type: String,
    destination: String,
    id: String,
    path: Option<String>,
    preview: Option<String>,
    dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromotionCandidate {
    id: String,
    candidate_type: String,
    path: PathBuf,
    text: String,
    scope: Option<String>,
    kind: Option<String>,
    confidence: Option<String>,
    target: Option<String>,
    todo_adapter: Option<String>,
    area: Option<String>,
    priority: Option<String>,
    energy: Option<String>,
    due: Option<String>,
    start: Option<String>,
    estimate: Option<String>,
    rationale: Option<String>,
    draft: Option<String>,
    name: Option<String>,
    description: Option<String>,
    body: Option<String>,
    evidence: Vec<String>,
}

#[derive(Debug, Clone)]
struct PromotionWritebackStores {
    memory: djinn_memory::MemoryStore,
    action: ActionStore,
    skill: SkillStore,
    mindweaver_inbox: Option<PathBuf>,
    mindweaver_sync_command: Option<Vec<String>>,
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

fn decide_promotion_session(
    args: &SessionDecisionArgs,
    action: SessionDecisionAction,
) -> Result<SessionDecisionReport> {
    decide_promotion_session_with_stores(args, action, PromotionWritebackStores::default())
}

fn decide_promotion_session_with_stores(
    args: &SessionDecisionArgs,
    action: SessionDecisionAction,
    stores: PromotionWritebackStores,
) -> Result<SessionDecisionReport> {
    if action != SessionDecisionAction::Accept && args.sync_mindweaver {
        bail!("--sync-mindweaver only applies to `djinn session accept`");
    }
    let session_dir = resolve_session_dir(&args.dir)?;
    let manifest = read_folder_session_manifest(&session_dir)?.with_context(|| {
        format!(
            "missing promotion session manifest: {}",
            session_dir.display()
        )
    })?;
    if manifest.kind.as_deref() != Some("promotion") {
        bail!(
            "session {} is not a promotion session; `djinn session {}` only applies to kind = \"promotion\"",
            session_dir.display(),
            session_decision_action_label(action)
        );
    }
    let promotion_type = manifest
        .promotion_type
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let decisions_dir = session_dir.join("outputs").join("decisions");
    let candidate_status_path = session_dir.join("outputs").join("candidate-status.toml");
    let decision_path = decisions_dir.join(format!(
        "{}-{}.toml",
        chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default(),
        session_decision_action_label(action)
    ));
    let candidates = resolve_promotion_candidates(&session_dir, args.candidate.as_deref())?;
    let writebacks = if action == SessionDecisionAction::Accept {
        writeback_promotion_candidates(&session_dir, &candidates, args.dry_run, &stores)?
    } else {
        Vec::new()
    };
    let post_writebacks = if action == SessionDecisionAction::Accept && args.sync_mindweaver {
        sync_mindweaver_after_writeback(&writebacks, args.dry_run, &stores)?
    } else if action == SessionDecisionAction::Accept {
        pending_mindweaver_sync_handoff(&writebacks, args.dry_run, &stores)
    } else {
        Vec::new()
    };
    let durable_writeback = !writebacks.is_empty() && !args.dry_run;
    let note = if candidates.is_empty() {
        "Decision recorded; no stable promotion candidate files were found, so no durable writeback was attempted."
    } else if args.dry_run {
        "Dry run: candidate writeback was validated but no durable store or decision files were written."
    } else if post_writebacks.iter().any(|post| post.status == "completed") {
        "Decision recorded, accepted candidate(s) were written, and requested post-writeback handoff ran."
    } else if post_writebacks.iter().any(|post| post.status == "pending") {
        "Decision recorded and accepted MindWeaver todo candidate(s) were appended; run the listed follow-up command when ready to sync MindWeaver todos."
    } else if durable_writeback {
        "Decision recorded and accepted candidate(s) were written to durable stores/artifacts."
    } else {
        "Decision recorded; no durable writeback was performed."
    }
    .to_string();

    if !args.dry_run {
        fs::create_dir_all(&decisions_dir).with_context(|| {
            format!(
                "creating promotion decisions directory {}",
                decisions_dir.display()
            )
        })?;
        fs::write(
            &decision_path,
            render_session_decision_record(
                action,
                &session_dir,
                &promotion_type,
                args.candidate.as_deref(),
                &writebacks,
                &post_writebacks,
                &note,
            )?,
        )
        .with_context(|| format!("writing {}", decision_path.display()))?;
        append_promotion_candidate_status_events(
            &candidate_status_path,
            action,
            &candidates,
            &writebacks,
        )?;
    }

    Ok(SessionDecisionReport {
        action,
        dry_run: args.dry_run,
        session_dir: session_dir.display().to_string(),
        promotion_type,
        candidate: args.candidate.clone(),
        candidate_count: candidates.len(),
        decision_path: decision_path.display().to_string(),
        candidate_status_path: candidate_status_path.display().to_string(),
        wrote_decision: !args.dry_run,
        durable_writeback,
        writebacks,
        post_writebacks,
        note,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionValidateCandidatesReport {
    session_dir: String,
    promotion_type: String,
    candidate: Option<String>,
    candidate_count: usize,
    valid_count: usize,
    invalid_count: usize,
    all_valid: bool,
    candidates: Vec<SessionValidateCandidateEntry>,
    note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionValidateCandidateEntry {
    id: String,
    candidate_type: Option<String>,
    path: String,
    valid: bool,
    error: Option<String>,
}

fn session_validate_candidates(args: SessionValidateCandidatesArgs) -> Result<()> {
    let report = validate_promotion_session_candidates(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Validated promotion candidates: {}", report.session_dir);
        println!("  type: {}", report.promotion_type);
        if let Some(candidate) = &report.candidate {
            println!("  candidate: {candidate}");
        } else {
            println!("  candidate: all");
        }
        println!(
            "  result: {} valid, {} invalid",
            report.valid_count, report.invalid_count
        );
        for candidate in &report.candidates {
            let status = if candidate.valid { "valid" } else { "invalid" };
            let candidate_type = candidate.candidate_type.as_deref().unwrap_or("unknown");
            println!("    - {} ({candidate_type}): {status}", candidate.id);
            println!("      path: {}", candidate.path);
            if let Some(error) = &candidate.error {
                println!("      error: {error}");
            }
        }
        println!("  note: {}", report.note);
    }
    Ok(())
}

fn validate_promotion_session_candidates(
    args: &SessionValidateCandidatesArgs,
) -> Result<SessionValidateCandidatesReport> {
    let session_dir = resolve_session_dir(&args.dir)?;
    let manifest = read_folder_session_manifest(&session_dir)?.with_context(|| {
        format!(
            "missing promotion session manifest: {}",
            session_dir.display()
        )
    })?;
    if manifest.kind.as_deref() != Some("promotion") {
        bail!(
            "session {} is not a promotion session; `djinn session validate-candidates` only applies to kind = \"promotion\"",
            session_dir.display()
        );
    }
    let promotion_type = manifest
        .promotion_type
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let paths = promotion_candidate_paths(&session_dir, args.candidate.as_deref())?;
    let candidates = paths
        .iter()
        .map(|path| validate_promotion_candidate_path(&session_dir, path))
        .collect::<Vec<_>>();
    let valid_count = candidates
        .iter()
        .filter(|candidate| candidate.valid)
        .count();
    let invalid_count = candidates.len().saturating_sub(valid_count);
    let all_valid = invalid_count == 0;
    let note = if candidates.is_empty() {
        "No promotion candidate TOML files were found. Run `djinn session run <promotion-session>` or add candidate files under outputs/candidates/."
    } else if all_valid {
        "All checked promotion candidates are structurally valid. You can accept, deny, export, or continue editing them."
    } else {
        "One or more promotion candidates need repair. Edit the listed TOML files, then run validation again."
    }
    .to_string();

    Ok(SessionValidateCandidatesReport {
        session_dir: session_dir.display().to_string(),
        promotion_type,
        candidate: args.candidate.clone(),
        candidate_count: candidates.len(),
        valid_count,
        invalid_count,
        all_valid,
        candidates,
        note,
    })
}

fn validate_promotion_candidate_path(
    session_dir: &Path,
    path: &Path,
) -> SessionValidateCandidateEntry {
    let (id, candidate_type) = promotion_candidate_metadata(path);
    match read_promotion_candidate(session_dir, path) {
        Ok(candidate) => SessionValidateCandidateEntry {
            id: candidate.id,
            candidate_type: Some(candidate.candidate_type),
            path: candidate.path.display().to_string(),
            valid: true,
            error: None,
        },
        Err(err) => SessionValidateCandidateEntry {
            id,
            candidate_type,
            path: path.display().to_string(),
            valid: false,
            error: Some(format!("{err:#}")),
        },
    }
}

fn promotion_candidate_metadata(path: &Path) -> (String, Option<String>) {
    let fallback_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("candidate")
        .to_string();
    let Ok(content) = fs::read_to_string(path) else {
        return (fallback_id, None);
    };
    let id = candidate_string_value(&content, "id").unwrap_or(fallback_id);
    let candidate_type = candidate_string_value(&content, "type")
        .or_else(|| candidate_string_value(&content, "candidate_type"))
        .filter(|value| !value.trim().is_empty());
    (id, candidate_type)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionCleanupReport {
    dry_run: bool,
    session_dir: String,
    delete_sources: bool,
    source_count: usize,
    sources: Vec<SessionCleanupSourceReport>,
    note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionCleanupSourceReport {
    session_dir: String,
    exists: bool,
    removed: bool,
    removed_native_session: bool,
    status: String,
}

fn session_cleanup(args: SessionCleanupArgs) -> Result<()> {
    let report = cleanup_promotion_session(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let verb = if report.dry_run {
            "Would clean"
        } else {
            "Cleaned"
        };
        println!("{verb} promotion session sources: {}", report.session_dir);
        for source in &report.sources {
            println!(
                "  - {}: {}",
                source.session_dir,
                if source.removed {
                    "removed"
                } else {
                    source.status.as_str()
                }
            );
            if source.removed_native_session {
                println!("    native session: removed");
            }
        }
        println!("  note: {}", report.note);
    }
    Ok(())
}

fn cleanup_promotion_session(args: &SessionCleanupArgs) -> Result<SessionCleanupReport> {
    if !args.delete_sources {
        bail!("nothing to clean up; pass --delete-sources to permanently remove source sessions");
    }
    let session_dir = resolve_session_dir(&args.dir)?;
    let manifest = read_folder_session_manifest(&session_dir)?.with_context(|| {
        format!(
            "missing promotion session manifest: {}",
            session_dir.display()
        )
    })?;
    if manifest.kind.as_deref() != Some("promotion") {
        bail!(
            "session {} is not a promotion session; `djinn session cleanup` only applies to kind = \"promotion\"",
            session_dir.display()
        );
    }

    let source_paths = promotion_source_session_dirs(&session_dir)?;
    let mut sources = Vec::new();
    for source in source_paths {
        let exists = source.exists();
        if args.dry_run || !exists {
            sources.push(SessionCleanupSourceReport {
                session_dir: source.display().to_string(),
                exists,
                removed: false,
                removed_native_session: false,
                status: if args.dry_run && exists {
                    "would_remove".to_string()
                } else {
                    "missing".to_string()
                },
            });
            continue;
        }
        let removed = remove_folder_session(&source)?;
        sources.push(SessionCleanupSourceReport {
            session_dir: removed.session_dir,
            exists,
            removed: removed.removed_folder,
            removed_native_session: removed.removed_native_session,
            status: "removed".to_string(),
        });
    }

    let source_count = sources.len();
    let note = if args.dry_run {
        "Dry run: no source sessions were removed. Re-run without --dry-run to permanently delete them."
    } else {
        "Source cleanup complete. The promotion session remains on disk; use `djinn session rm` if you also want to remove it."
    }
    .to_string();

    Ok(SessionCleanupReport {
        dry_run: args.dry_run,
        session_dir: session_dir.display().to_string(),
        delete_sources: args.delete_sources,
        source_count,
        sources,
        note,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionExportPatternReport {
    dry_run: bool,
    session_dir: String,
    output_path: String,
    append: bool,
    candidate_count: usize,
    candidates: Vec<String>,
    wrote: bool,
    preview: Option<String>,
}

fn session_export_pattern(args: SessionExportPatternArgs) -> Result<()> {
    let report = export_pattern_insights(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.dry_run {
        println!("Would export pattern insight(s) to: {}", report.output_path);
        if let Some(preview) = &report.preview {
            println!("\n{preview}");
        }
    } else {
        let verb = if report.append {
            "Appended"
        } else {
            "Exported"
        };
        println!(
            "{verb} {} pattern candidate{} to {}",
            report.candidate_count,
            plural_suffix(report.candidate_count),
            report.output_path
        );
    }
    Ok(())
}

fn export_pattern_insights(args: &SessionExportPatternArgs) -> Result<SessionExportPatternReport> {
    let session_dir = resolve_session_dir(&args.dir)?;
    let manifest = read_folder_session_manifest(&session_dir)?.with_context(|| {
        format!(
            "missing promotion session manifest: {}",
            session_dir.display()
        )
    })?;
    if manifest.kind.as_deref() != Some("promotion")
        || manifest.promotion_type.as_deref() != Some("pattern")
    {
        bail!(
            "session {} is not a pattern promotion session",
            session_dir.display()
        );
    }
    let candidates = resolve_promotion_candidates(&session_dir, args.candidate.as_deref())?
        .into_iter()
        .filter(|candidate| candidate.candidate_type == "pattern")
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        bail!("no pattern candidates found to export");
    }
    let output_path = expand_tilde_path(&args.to.display().to_string());
    if output_path.exists() && !args.append && !args.dry_run {
        bail!(
            "notes file already exists: {} (use --append to add pattern insights)",
            output_path.display()
        );
    }
    let content = render_pattern_export_note(&session_dir, &candidates);
    if !args.dry_run {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating notes export directory {}", parent.display()))?;
        }
        if args.append && output_path.exists() {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&output_path)
                .with_context(|| format!("opening notes file {}", output_path.display()))?;
            file.write_all(format!("\n\n{}", content.trim_end()).as_bytes())
                .with_context(|| format!("appending notes file {}", output_path.display()))?;
            file.write_all(b"\n")
                .with_context(|| format!("appending notes file {}", output_path.display()))?;
        } else {
            fs::write(&output_path, ensure_trailing_newline(&content))
                .with_context(|| format!("writing notes file {}", output_path.display()))?;
        }
    }
    Ok(SessionExportPatternReport {
        dry_run: args.dry_run,
        session_dir: session_dir.display().to_string(),
        output_path: output_path.display().to_string(),
        append: args.append,
        candidate_count: candidates.len(),
        candidates: candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect(),
        wrote: !args.dry_run,
        preview: args.dry_run.then_some(content),
    })
}

fn render_pattern_export_note(session_dir: &Path, candidates: &[PromotionCandidate]) -> String {
    let mut out = String::new();
    out.push_str("# Pattern insight\n\n");
    out.push_str(&format!(
        "Source promotion session: `{}`\n\n",
        session_dir.display()
    ));
    for candidate in candidates {
        out.push_str(&format!("## {}\n\n", candidate.id));
        out.push_str(candidate.text.trim());
        out.push_str("\n\n");
        if let Some(rationale) = candidate
            .rationale
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            out.push_str("### Rationale\n\n");
            out.push_str(rationale);
            out.push_str("\n\n");
        }
        out.push_str("### Evidence\n\n");
        for evidence in &candidate.evidence {
            out.push_str(&format!("- {evidence}\n"));
        }
        out.push('\n');
    }
    out
}

fn promotion_source_session_dirs(session_dir: &Path) -> Result<Vec<PathBuf>> {
    let sources_path = session_dir.join("context").join("sources.toml");
    let content = fs::read_to_string(&sources_path)
        .with_context(|| format!("reading promotion sources {}", sources_path.display()))?;
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    for line in content.lines().map(str::trim) {
        let Some(value) = line
            .strip_prefix("session_dir =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        else {
            continue;
        };
        let path = expand_tilde_path(&value);
        let key = path.display().to_string();
        if seen.insert(key) {
            sources.push(path);
        }
    }
    if sources.is_empty() {
        bail!(
            "promotion session {} has no source sessions in {}",
            session_dir.display(),
            sources_path.display()
        );
    }
    Ok(sources)
}

impl PromotionWritebackStores {
    fn default() -> Self {
        Self {
            memory: memory_store(),
            action: action_store(),
            skill: skill_store(),
            mindweaver_inbox: None,
            mindweaver_sync_command: None,
        }
    }
}

fn session_decision_action_label(action: SessionDecisionAction) -> &'static str {
    match action {
        SessionDecisionAction::Accept => "accept",
        SessionDecisionAction::Deny => "deny",
    }
}

fn render_promotion_candidate_generation_prompt(
    promotion_type: &str,
    source_packet: &str,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("# Djinn promotion candidate generation\n\n");
    prompt.push_str(&format!("Promotion type: `{}`\n\n", promotion_type.trim()));
    prompt.push_str(
        "Read the source packet below and propose high-confidence promotion candidates only. ",
    );
    prompt.push_str("Return one fenced `toml` block per candidate and no other prose. ");
    prompt.push_str("Every candidate must include `type`, `text` (except skill may use `body`), and non-empty `evidence` links copied from the source packet.\n\n");
    prompt.push_str("Required per-type fields: memory requires `scope`, `kind`, and `confidence`; todo requires `kind` and `confidence`; skill requires `name`, `description`, and `body`/`body_path`/`text`; pattern requires `text` and `rationale`.\n\n");
    prompt.push_str("Supported candidate shapes:\n\n");
    prompt.push_str("```toml\ntype = \"memory\"\nid = \"memory-001\"\ntext = \"Durable nugget of wisdom.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\"/path/to/session/summary.md\"]\n```\n\n");
    prompt.push_str("```toml\ntype = \"todo\"\nid = \"todo-001\"\ntext = \"Concrete next action.\"\nscope = \"project:djinn\"\nkind = \"follow-up\"\nconfidence = \"medium\"\nevidence = [\"/path/to/session/turns/turn-1/response.md\"]\n```\n\n");
    prompt.push_str("Todo candidates may optionally include `todo_adapter = \"action\"` (Djinn fallback) or `todo_adapter = \"mindweaver\"` plus MindWeaver metadata such as `area = \"Code\"`, `priority = \"p2\"`, `energy = \"m\"`, `due = \"2026-08-01\"`, `start = \"2026-07-30\"`, or `estimate = \"30\"`. MindWeaver todo accept appends a valid checkbox to the configured MindWeaver inbox; use `--dry-run` to preview the checkbox first.\n\n");
    prompt.push_str("```toml\ntype = \"skill\"\nid = \"skill-001\"\nname = \"reusable-workflow\"\ndescription = \"When to use this workflow.\"\nbody = \"# Skill: reusable-workflow\\n\\n## When to use\\n...\"\nevidence = [\"/path/to/session/context/compacted.md\"]\n```\n\n");
    prompt.push_str("```toml\ntype = \"pattern\"\nid = \"pattern-001\"\ntext = \"Common thread across the source sessions.\"\nrationale = \"Why this is a repeated pattern.\"\nevidence = [\"/path/to/session/summary.md\"]\n```\n\n");
    prompt
        .push_str("If there are no high-confidence candidates, return no fenced TOML blocks.\n\n");
    prompt.push_str("## Source packet\n\n");
    prompt.push_str(source_packet.trim_end());
    prompt.push('\n');
    prompt
}

fn write_generated_promotion_candidates(
    session_dir: &Path,
    expected_type: &str,
    model_output: &str,
    candidates_dir: &Path,
) -> Result<Vec<PromotionGeneratedCandidateReport>> {
    let blocks = extract_toml_fenced_blocks(model_output);
    let mut reports = Vec::new();
    for (idx, block) in blocks.iter().enumerate() {
        let mut content = block.trim().to_string();
        let default_id = format!("{}-{:03}", expected_type.trim(), idx + 1);
        if candidate_string_value(&content, "id").is_none() {
            content = format!("id = {}\n{}", toml_string(&default_id)?, content);
        }
        let id = candidate_string_value(&content, "id").unwrap_or(default_id);
        let path = candidates_dir.join(format!("{}.toml", candidate_file_stem(&id)));
        let candidate = parse_promotion_candidate(session_dir, &path, &content)?;
        if candidate.candidate_type != expected_type.trim() {
            bail!(
                "generated candidate {} has type `{}` but promotion session type is `{}`",
                candidate.id,
                candidate.candidate_type,
                expected_type
            );
        }
        fs::write(&path, ensure_trailing_newline(&content))
            .with_context(|| format!("writing generated promotion candidate {}", path.display()))?;
        let evidence = candidate.evidence.clone();
        reports.push(PromotionGeneratedCandidateReport {
            id: candidate.id,
            candidate_type: candidate.candidate_type,
            path: path.display().to_string(),
            text: candidate.text,
            rationale: candidate.rationale,
            evidence_count: evidence.len(),
            evidence,
        });
    }
    if reports.is_empty() {
        bail!("model response did not contain any fenced TOML promotion candidates");
    }
    Ok(reports)
}

fn extract_toml_fenced_blocks(value: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_toml = false;
    let mut current = String::new();
    for line in value.lines() {
        let trimmed = line.trim();
        if let Some(info) = trimmed.strip_prefix("```") {
            if in_toml {
                blocks.push(current.trim().to_string());
                current.clear();
                in_toml = false;
            } else if info.trim().eq_ignore_ascii_case("toml") {
                in_toml = true;
            }
            continue;
        }
        if in_toml {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks
        .into_iter()
        .filter(|block| !block.trim().is_empty())
        .collect()
}

fn candidate_file_stem(id: &str) -> String {
    let stem = folder_session_slug(id);
    if stem.is_empty() {
        "candidate".to_string()
    } else {
        stem
    }
}

fn write_promotion_candidate_index(
    session_dir: &Path,
    candidates: &[PromotionGeneratedCandidateReport],
) -> Result<PathBuf> {
    let index_path = session_dir.join("outputs").join("candidate-index.toml");
    let mut output = String::new();
    output.push_str("version = 1\n");
    output.push_str(&format!(
        "generated_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    output.push_str(&format!("candidate_count = {}\n", candidates.len()));
    for candidate in candidates {
        output.push_str("\n[[candidates]]\n");
        output.push_str(&format!("id = {}\n", toml_string(&candidate.id)?));
        output.push_str(&format!(
            "type = {}\n",
            toml_string(&candidate.candidate_type)?
        ));
        output.push_str(&format!("path = {}\n", toml_string(&candidate.path)?));
        output.push_str("status = \"candidate\"\n");
        output.push_str(&format!("evidence_count = {}\n", candidate.evidence_count));
    }
    fs::write(&index_path, output).with_context(|| format!("writing {}", index_path.display()))?;
    Ok(index_path)
}

fn write_promotion_generation_summary(
    session_dir: &Path,
    promotion_type: &str,
    candidates: &[PromotionGeneratedCandidateReport],
) -> Result<PathBuf> {
    let summary_path = session_dir.join("summary.md");
    let content = render_promotion_generation_summary(promotion_type, candidates);
    fs::write(&summary_path, content)
        .with_context(|| format!("writing {}", summary_path.display()))?;
    Ok(summary_path)
}

fn render_promotion_generation_summary(
    promotion_type: &str,
    candidates: &[PromotionGeneratedCandidateReport],
) -> String {
    let mut output = String::new();
    output.push_str("# Promotion candidates\n\n");
    output.push_str(&format!("Promotion type: `{}`\n\n", promotion_type.trim()));
    output.push_str(&format!(
        "Generated {} candidate{} for review.\n\n",
        candidates.len(),
        plural_suffix(candidates.len())
    ));
    output.push_str("Use `djinn session accept <promotion-session> <candidate-id> --dry-run` before accepting, or review candidates in the Sessions TUI.\n\n");
    for candidate in candidates {
        output.push_str(&format!(
            "## {} `{}`\n\n",
            candidate.candidate_type, candidate.id
        ));
        if !candidate.text.trim().is_empty() {
            output.push_str(candidate.text.trim());
            output.push_str("\n\n");
        }
        if let Some(rationale) = candidate
            .rationale
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            output.push_str("### Rationale\n\n");
            output.push_str(rationale);
            output.push_str("\n\n");
        }
        output.push_str("### Evidence\n\n");
        if candidate.evidence.is_empty() {
            output.push_str("- _No evidence links recorded._\n\n");
        } else {
            for evidence in &candidate.evidence {
                output.push_str(&format!("- {evidence}\n"));
            }
            output.push('\n');
        }
        output.push_str(&format!("Candidate file: `{}`\n\n", candidate.path));
    }
    output
}

fn append_promotion_candidate_status_events(
    status_path: &Path,
    action: SessionDecisionAction,
    candidates: &[PromotionCandidate],
    writebacks: &[SessionCandidateWritebackReport],
) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    if let Some(parent) = status_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating candidate status directory {}", parent.display()))?;
    }
    let mut output = String::new();
    for candidate in candidates {
        output.push_str("[[events]]\n");
        output.push_str(&format!(
            "decided_at = {}\n",
            toml_string(&chrono::Local::now().to_rfc3339())?
        ));
        output.push_str(&format!("candidate = {}\n", toml_string(&candidate.id)?));
        output.push_str(&format!(
            "type = {}\n",
            toml_string(&candidate.candidate_type)?
        ));
        output.push_str(&format!(
            "action = {}\n",
            toml_string(session_decision_action_label(action))?
        ));
        output.push_str(&format!(
            "status = {}\n",
            toml_string(match action {
                SessionDecisionAction::Accept => "accepted",
                SessionDecisionAction::Deny => "denied",
            })?
        ));
        let durable_writeback = writebacks
            .iter()
            .any(|writeback| writeback.candidate == candidate.id);
        output.push_str(&format!("durable_writeback = {}\n", durable_writeback));
        if let Some(writeback) = writebacks
            .iter()
            .find(|writeback| writeback.candidate == candidate.id)
        {
            output.push_str(&format!(
                "destination = {}\n",
                toml_string(&writeback.destination)?
            ));
            output.push_str(&format!("writeback_id = {}\n", toml_string(&writeback.id)?));
            if let Some(path) = &writeback.path {
                output.push_str(&format!("writeback_path = {}\n", toml_string(path)?));
            }
            if let Some(preview) = &writeback.preview {
                output.push_str(&format!("preview = {}\n", toml_string(preview)?));
            }
        }
        output.push('\n');
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(status_path)
        .with_context(|| format!("opening {}", status_path.display()))?
        .write_all(output.as_bytes())
        .with_context(|| format!("writing {}", status_path.display()))
}

fn resolve_promotion_candidates(
    session_dir: &Path,
    candidate: Option<&str>,
) -> Result<Vec<PromotionCandidate>> {
    promotion_candidate_paths(session_dir, candidate)?
        .iter()
        .map(|path| read_promotion_candidate(session_dir, path))
        .collect()
}

fn promotion_candidate_paths(session_dir: &Path, candidate: Option<&str>) -> Result<Vec<PathBuf>> {
    let candidates_dir = session_dir.join("outputs").join("candidates");
    if let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) {
        let path = resolve_promotion_candidate_path(session_dir, candidate);
        if !path.exists() {
            bail!(
                "promotion candidate not found: {} (expected a .toml candidate under {})",
                candidate,
                candidates_dir.display()
            );
        }
        ensure_promotion_candidate_inside_session(session_dir, &path)?;
        return Ok(vec![path]);
    }

    if !candidates_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(&candidates_dir)
        .with_context(|| format!("reading promotion candidates {}", candidates_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("toml")
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn ensure_promotion_candidate_inside_session(session_dir: &Path, path: &Path) -> Result<()> {
    let session_dir = session_dir
        .canonicalize()
        .with_context(|| format!("resolving session directory {}", session_dir.display()))?;
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving promotion candidate {}", path.display()))?;
    if !path.starts_with(&session_dir) {
        bail!(
            "promotion candidate must live inside the promotion session: {}",
            path.display()
        );
    }
    Ok(())
}

fn resolve_promotion_candidate_path(session_dir: &Path, candidate: &str) -> PathBuf {
    let path = PathBuf::from(candidate);
    if path.is_absolute() {
        return path;
    }
    if candidate.contains(std::path::MAIN_SEPARATOR) || candidate.ends_with(".toml") {
        return session_dir.join(path);
    }
    session_dir
        .join("outputs")
        .join("candidates")
        .join(format!("{candidate}.toml"))
}

fn read_promotion_candidate(session_dir: &Path, path: &Path) -> Result<PromotionCandidate> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading promotion candidate {}", path.display()))?;
    parse_promotion_candidate(session_dir, path, &content)
}

fn parse_promotion_candidate(
    session_dir: &Path,
    path: &Path,
    content: &str,
) -> Result<PromotionCandidate> {
    let id = candidate_string_value(&content, "id").unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("candidate")
            .to_string()
    });
    let candidate_type = candidate_string_value(&content, "type")
        .or_else(|| candidate_string_value(&content, "candidate_type"))
        .unwrap_or_default();
    let text = candidate_string_value(&content, "text")
        .or_else(|| candidate_string_value(&content, "summary"))
        .unwrap_or_default();
    let body = if let Some(body) = candidate_string_value(&content, "body") {
        Some(body)
    } else if let Some(body) = read_candidate_body_path(session_dir, path, &content) {
        Some(body?)
    } else {
        None
    };
    let confidence = candidate_string_value(&content, "confidence").or_else(|| {
        (candidate_type.trim() != "todo").then(|| candidate_string_value(&content, "priority"))?
    });
    let candidate = PromotionCandidate {
        id,
        candidate_type,
        path: path.to_path_buf(),
        text,
        scope: candidate_string_value(&content, "scope"),
        kind: candidate_string_value(&content, "kind"),
        confidence,
        target: candidate_string_value(&content, "target"),
        todo_adapter: candidate_string_value(&content, "todo_adapter")
            .or_else(|| candidate_string_value(&content, "adapter")),
        area: candidate_string_value(&content, "area"),
        priority: candidate_string_value(&content, "priority"),
        energy: candidate_string_value(&content, "energy"),
        due: candidate_string_value(&content, "due"),
        start: candidate_string_value(&content, "start"),
        estimate: candidate_string_value(&content, "estimate")
            .or_else(|| candidate_string_value(&content, "est")),
        rationale: candidate_string_value(&content, "rationale"),
        draft: candidate_string_value(&content, "draft"),
        name: candidate_string_value(&content, "name"),
        description: candidate_string_value(&content, "description"),
        body,
        evidence: candidate_string_array_value(&content, "evidence"),
    };
    validate_promotion_candidate(&candidate)?;
    Ok(candidate)
}

fn read_candidate_body_path(
    session_dir: &Path,
    path: &Path,
    content: &str,
) -> Option<Result<String>> {
    let body_path = candidate_string_value(content, "body_path")?;
    let resolved = path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(body_path);
    Some((|| {
        ensure_promotion_candidate_inside_session(session_dir, &resolved)?;
        fs::read_to_string(&resolved)
            .with_context(|| format!("reading promotion candidate body {}", resolved.display()))
    })())
}

fn candidate_string_value(content: &str, key: &str) -> Option<String> {
    manifest_root_string_value(content, key)
}

fn candidate_string_array_value(content: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key} =");
    candidate_raw_array_value(content, &prefix)
        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn candidate_raw_array_value(content: &str, prefix: &str) -> Option<String> {
    let mut collecting = false;
    let mut value = String::new();
    let mut bracket_depth = 0i32;
    for line in content.lines() {
        let trimmed = line.trim();
        let part = if collecting {
            trimmed
        } else {
            let Some(part) = trimmed.strip_prefix(prefix).map(str::trim) else {
                continue;
            };
            part
        };
        if !value.is_empty() {
            value.push('\n');
        }
        value.push_str(part);
        bracket_depth += part.matches('[').count() as i32;
        bracket_depth -= part.matches(']').count() as i32;
        if bracket_depth <= 0 && value.trim_start().starts_with('[') {
            return Some(value);
        }
        collecting = true;
    }
    None
}

fn validate_promotion_candidate(candidate: &PromotionCandidate) -> Result<()> {
    let candidate_type = candidate.candidate_type.trim();
    if candidate_type.is_empty() {
        bail!(
            "promotion candidate {} is missing `type`",
            candidate.path.display()
        );
    }
    if !matches!(candidate_type, "memory" | "todo" | "skill" | "pattern") {
        bail!(
            "promotion candidate {} has unsupported type `{candidate_type}`; expected memory, todo, skill, or pattern",
            candidate.path.display()
        );
    }
    if candidate.evidence.is_empty() {
        bail!(
            "promotion candidate {} must include at least one evidence link",
            candidate.path.display()
        );
    }
    if candidate
        .evidence
        .iter()
        .any(|evidence| !is_file_native_promotion_evidence(evidence))
    {
        bail!(
            "promotion candidate {} evidence must cite file-native session artifacts such as summary.md, context/compacted.md, or turns/<id>/ files",
            candidate.path.display()
        );
    }
    if let Some(confidence) = candidate.confidence.as_deref() {
        let confidence = confidence.trim();
        if !confidence.is_empty() && !matches!(confidence, "low" | "medium" | "high") {
            bail!(
                "promotion candidate {} confidence must be low, medium, or high",
                candidate.path.display()
            );
        }
    }
    match candidate_type {
        "memory" | "todo" | "pattern" if candidate.text.trim().is_empty() => bail!(
            "promotion candidate {} must include non-empty `text`",
            candidate.path.display()
        ),
        "memory" => {
            require_candidate_field(candidate, candidate.scope.as_deref(), "scope")?;
            require_candidate_field(candidate, candidate.kind.as_deref(), "kind")?;
            require_candidate_field(candidate, candidate.confidence.as_deref(), "confidence")?;
        }
        "skill" => {
            if candidate
                .name
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                bail!(
                    "promotion skill candidate {} must include `name`",
                    candidate.path.display()
                );
            }
            require_candidate_field(candidate, candidate.description.as_deref(), "description")?;
            if candidate
                .body
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
                && candidate.text.trim().is_empty()
            {
                bail!(
                    "promotion skill candidate {} must include `body`, `body_path`, or `text`",
                    candidate.path.display()
                );
            }
        }
        "todo" => {
            require_candidate_field(candidate, candidate.kind.as_deref(), "kind")?;
            require_candidate_field(candidate, candidate.confidence.as_deref(), "confidence")?;
            if candidate.target.as_deref().unwrap_or_default().trim() == "suggestion" {
                bail!(
                    "promotion todo candidate {} targets the suggestion store; promotion todos currently write to durable actions",
                    candidate.path.display()
                );
            }
            validate_todo_candidate_adapter(candidate)?;
        }
        "pattern" => {
            require_candidate_field(candidate, candidate.rationale.as_deref(), "rationale")?;
        }
        _ => {}
    }
    Ok(())
}

fn require_candidate_field(
    candidate: &PromotionCandidate,
    value: Option<&str>,
    field: &str,
) -> Result<()> {
    if value.map(str::trim).unwrap_or_default().is_empty() {
        bail!(
            "promotion {} candidate {} must include `{field}`",
            candidate.candidate_type,
            candidate.path.display()
        );
    }
    Ok(())
}

fn validate_todo_candidate_adapter(candidate: &PromotionCandidate) -> Result<()> {
    let adapter = promotion_todo_adapter(candidate);
    if !matches!(adapter.as_str(), "action" | "mindweaver") {
        bail!(
            "promotion todo candidate {} has unsupported todo_adapter `{adapter}`; expected action or mindweaver",
            candidate.path.display()
        );
    }
    if adapter == "mindweaver" {
        if let Some(area) = candidate
            .area
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if !matches!(
                area,
                "Code" | "Action" | "Reading" | "Amusement" | "Music" | "Exercise" | "Love"
            ) {
                bail!(
                    "promotion todo candidate {} has unsupported MindWeaver area `{area}`",
                    candidate.path.display()
                );
            }
        }
        if let Some(priority) = candidate
            .priority
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if !matches!(priority, "p1" | "p2" | "p3" | "p4" | "p5") {
                bail!(
                    "promotion todo candidate {} has unsupported MindWeaver priority `{priority}`; expected p1..p5",
                    candidate.path.display()
                );
            }
        }
        if let Some(energy) = candidate
            .energy
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if !matches!(energy, "xsm" | "s" | "m" | "l" | "xl") {
                bail!(
                    "promotion todo candidate {} has unsupported MindWeaver energy `{energy}`; expected xsm, s, m, l, or xl",
                    candidate.path.display()
                );
            }
        }
        if let Some(due) = candidate
            .due
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            validate_mindweaver_date(candidate, "due", due)?;
        }
        if let Some(start) = candidate
            .start
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            validate_mindweaver_date(candidate, "start", start)?;
        }
        if let Some(estimate) = candidate
            .estimate
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if estimate.parse::<u64>().is_err() {
                bail!(
                    "promotion todo candidate {} has unsupported MindWeaver estimate `{estimate}`; expected minutes as an integer",
                    candidate.path.display()
                );
            }
        }
    }
    Ok(())
}

fn validate_mindweaver_date(
    candidate: &PromotionCandidate,
    field: &str,
    value: &str,
) -> Result<()> {
    if chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err() {
        bail!(
            "promotion todo candidate {} has unsupported MindWeaver {field} date `{value}`; expected YYYY-MM-DD",
            candidate.path.display()
        );
    }
    Ok(())
}

fn promotion_todo_adapter(candidate: &PromotionCandidate) -> String {
    candidate
        .todo_adapter
        .as_deref()
        .or(candidate.target.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("action")
        .to_lowercase()
}

fn is_file_native_promotion_evidence(evidence: &str) -> bool {
    let evidence = evidence.trim();
    !evidence.is_empty()
        && (evidence.contains("summary.md")
            || evidence.contains("context/")
            || evidence.contains("turns/"))
}

fn writeback_promotion_candidates(
    session_dir: &Path,
    candidates: &[PromotionCandidate],
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Result<Vec<SessionCandidateWritebackReport>> {
    candidates
        .iter()
        .map(|candidate| writeback_promotion_candidate(session_dir, candidate, dry_run, stores))
        .collect()
}

fn writeback_promotion_candidate(
    session_dir: &Path,
    candidate: &PromotionCandidate,
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Result<SessionCandidateWritebackReport> {
    match candidate.candidate_type.as_str() {
        "memory" => {
            ensure_no_duplicate_memory_candidate(candidate, &stores.memory)?;
            let input = MemoryInput {
                text: candidate.text.trim().to_string(),
                scope: candidate.scope.clone(),
                kind: candidate.kind.clone(),
                confidence: candidate.confidence.clone(),
                evidence: candidate.evidence.clone(),
                sources: vec![promotion_candidate_source(session_dir, candidate)],
                ..MemoryInput::default()
            };
            let id = if dry_run {
                candidate.id.clone()
            } else {
                stores.memory.add_input(input)?.id
            };
            Ok(SessionCandidateWritebackReport {
                candidate: candidate.id.clone(),
                candidate_type: candidate.candidate_type.clone(),
                destination: "memory".to_string(),
                id,
                path: None,
                preview: None,
                dry_run,
            })
        }
        "todo" => {
            let adapter = promotion_todo_adapter(candidate);
            if adapter == "mindweaver" {
                return writeback_mindweaver_todo_candidate(candidate, dry_run, stores);
            }
            ensure_no_duplicate_todo_candidate(candidate, &stores.action)?;
            let input = MemoryInput {
                text: candidate.text.trim().to_string(),
                scope: candidate.scope.clone(),
                kind: candidate.kind.clone(),
                confidence: candidate.confidence.clone(),
                evidence: candidate.evidence.clone(),
                sources: vec![promotion_candidate_source(session_dir, candidate)],
                ..MemoryInput::default()
            };
            let id = if dry_run {
                candidate.id.clone()
            } else {
                stores.action.add_input(input)?.id
            };
            Ok(SessionCandidateWritebackReport {
                candidate: candidate.id.clone(),
                candidate_type: candidate.candidate_type.clone(),
                destination: "action".to_string(),
                id,
                path: None,
                preview: None,
                dry_run,
            })
        }
        "skill" => {
            let name = candidate.name.as_deref().unwrap_or(&candidate.id);
            ensure_no_duplicate_skill_candidate(name, &stores.skill)?;
            let description = candidate.description.as_deref().unwrap_or_default();
            let content = render_skill_candidate_content(candidate);
            let (id, path) = if dry_run {
                (name.to_string(), None)
            } else {
                let record = stores
                    .skill
                    .add_with_content(name, description, content, false)?;
                (record.name, Some(record.path.display().to_string()))
            };
            Ok(SessionCandidateWritebackReport {
                candidate: candidate.id.clone(),
                candidate_type: candidate.candidate_type.clone(),
                destination: "skill".to_string(),
                id,
                path,
                preview: None,
                dry_run,
            })
        }
        "pattern" => {
            let accepted_dir = session_dir.join("outputs").join("accepted");
            let accepted_path = accepted_dir.join(format!("{}.md", candidate.id));
            if accepted_path.exists() {
                bail!(
                    "accepted pattern candidate already exists: {}",
                    accepted_path.display()
                );
            }
            if !dry_run {
                fs::create_dir_all(&accepted_dir).with_context(|| {
                    format!(
                        "creating accepted promotion directory {}",
                        accepted_dir.display()
                    )
                })?;
                fs::write(&accepted_path, render_pattern_candidate_content(candidate))
                    .with_context(|| format!("writing {}", accepted_path.display()))?;
            }
            Ok(SessionCandidateWritebackReport {
                candidate: candidate.id.clone(),
                candidate_type: candidate.candidate_type.clone(),
                destination: "pattern_summary".to_string(),
                id: candidate.id.clone(),
                path: Some(accepted_path.display().to_string()),
                preview: None,
                dry_run,
            })
        }
        other => bail!("unsupported promotion candidate type `{other}`"),
    }
}

fn writeback_mindweaver_todo_candidate(
    candidate: &PromotionCandidate,
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Result<SessionCandidateWritebackReport> {
    let preview = render_mindweaver_todo_capture(candidate);
    let inbox_path = if dry_run {
        stores.mindweaver_inbox.clone()
    } else {
        Some(resolve_mindweaver_inbox_path(
            stores.mindweaver_inbox.as_deref(),
        )?)
    };

    if let Some(path) = inbox_path.as_deref() {
        ensure_no_duplicate_mindweaver_todo_candidate(candidate, path)?;
        if !dry_run {
            write_mindweaver_todo_capture_to_path(candidate, path)?;
        }
    }

    Ok(SessionCandidateWritebackReport {
        candidate: candidate.id.clone(),
        candidate_type: candidate.candidate_type.clone(),
        destination: if dry_run {
            "mindweaver_inbox_preview".to_string()
        } else {
            "mindweaver_inbox".to_string()
        },
        id: candidate.id.clone(),
        path: Some(
            inbox_path
                .map(|path| path.display().to_string())
                .unwrap_or_else(resolve_mindweaver_inbox_preview_path),
        ),
        preview: Some(preview),
        dry_run,
    })
}

fn sync_mindweaver_after_writeback(
    writebacks: &[SessionCandidateWritebackReport],
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Result<Vec<SessionPostWritebackReport>> {
    if !writebacks
        .iter()
        .any(|writeback| writeback.destination.starts_with("mindweaver_inbox"))
    {
        return Ok(Vec::new());
    }
    let command = mindweaver_sync_command(stores);
    let command_display = command.join(" ");
    if dry_run {
        return Ok(vec![SessionPostWritebackReport {
            name: "mindweaver_todos_sync".to_string(),
            command: command_display,
            status: "dry_run".to_string(),
            dry_run,
        }]);
    }
    let Some(program) = command.first() else {
        bail!("MindWeaver sync command is empty");
    };
    let status = ProcessCommand::new(program)
        .args(command.iter().skip(1))
        .status()
        .with_context(|| format!("running post-writeback command `{command_display}`"))?;
    if !status.success() {
        bail!(
            "post-writeback command `{command_display}` failed with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated by signal".to_string())
        );
    }
    Ok(vec![SessionPostWritebackReport {
        name: "mindweaver_todos_sync".to_string(),
        command: command_display,
        status: "completed".to_string(),
        dry_run,
    }])
}

fn pending_mindweaver_sync_handoff(
    writebacks: &[SessionCandidateWritebackReport],
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Vec<SessionPostWritebackReport> {
    if dry_run
        || !writebacks
            .iter()
            .any(|writeback| writeback.destination == "mindweaver_inbox")
    {
        return Vec::new();
    }
    vec![SessionPostWritebackReport {
        name: "mindweaver_todos_sync".to_string(),
        command: mindweaver_sync_command(stores).join(" "),
        status: "pending".to_string(),
        dry_run,
    }]
}

fn mindweaver_sync_command(stores: &PromotionWritebackStores) -> Vec<String> {
    stores
        .mindweaver_sync_command
        .clone()
        .unwrap_or_else(|| vec!["mw".to_string(), "todos".to_string(), "sync".to_string()])
}

fn write_mindweaver_todo_capture_to_path(
    candidate: &PromotionCandidate,
    inbox_path: &Path,
) -> Result<()> {
    let existing = match fs::read_to_string(inbox_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", inbox_path.display())),
    };
    let mut lines = if existing.trim().is_empty() {
        Vec::new()
    } else {
        existing.lines().map(str::to_string).collect::<Vec<_>>()
    };
    insert_mindweaver_todo_capture_lines(&mut lines, &render_mindweaver_todo_capture(candidate));
    if let Some(parent) = inbox_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating MindWeaver inbox directory {}", parent.display()))?;
    }
    fs::write(inbox_path, ensure_trailing_newline(&lines.join("\n")))
        .with_context(|| format!("writing MindWeaver inbox {}", inbox_path.display()))
}

fn insert_mindweaver_todo_capture_lines(lines: &mut Vec<String>, capture: &str) {
    ensure_mindweaver_inbox_lines(lines);
    let mut todo_idx = None;
    let mut inbox_idx = None;
    for (idx, line) in lines.iter().enumerate() {
        if line.trim().eq_ignore_ascii_case("## Todo") {
            todo_idx = Some(idx);
        } else if todo_idx.is_some() && line.trim().eq_ignore_ascii_case("### Inbox") {
            inbox_idx = Some(idx);
            break;
        }
    }
    let inbox_idx = if let Some(idx) = inbox_idx {
        idx
    } else {
        if !lines.last().is_none_or(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.extend([
            "## Todo".to_string(),
            "### Inbox".to_string(),
            "### Next".to_string(),
            "### Waiting".to_string(),
        ]);
        lines.len().saturating_sub(3)
    };

    let mut insert_at = inbox_idx + 1;
    while insert_at < lines.len() {
        if lines[insert_at].trim().starts_with("### ") {
            break;
        }
        insert_at += 1;
    }
    let new_lines = capture.lines().map(str::to_string).collect::<Vec<_>>();
    for (offset, line) in new_lines.into_iter().enumerate() {
        lines.insert(insert_at + offset, line);
    }
}

fn ensure_mindweaver_inbox_lines(lines: &mut Vec<String>) {
    if !lines.is_empty() {
        return;
    }
    lines.extend([
        "---".to_string(),
        "id: \"inbox\"".to_string(),
        "domains: [task-index]".to_string(),
        "task_active: true".to_string(),
        "task_scope: inbox".to_string(),
        "task_area: Action".to_string(),
        "---".to_string(),
        String::new(),
        "# Inbox".to_string(),
        "## Todo".to_string(),
        "### Inbox".to_string(),
        "### Next".to_string(),
        "### Waiting".to_string(),
    ]);
}

fn ensure_no_duplicate_memory_candidate(
    candidate: &PromotionCandidate,
    store: &djinn_memory::MemoryStore,
) -> Result<()> {
    let candidate_text = normalized_candidate_text(&candidate.text);
    if candidate_text.is_empty() {
        return Ok(());
    }
    for record in store.list()? {
        if record.status != "active" {
            continue;
        }
        if let Some(similarity) = candidate_duplicate_similarity(&candidate.text, &record.text) {
            if similarity >= 1.0 {
                bail!(
                    "duplicate memory candidate {} matches existing memory {}",
                    candidate.id,
                    record.id
                );
            }
            bail!(
                "near-duplicate memory candidate {} matches existing memory {} (similarity {:.2})",
                candidate.id,
                record.id,
                similarity
            );
        }
    }
    Ok(())
}

fn ensure_no_duplicate_todo_candidate(
    candidate: &PromotionCandidate,
    store: &ActionStore,
) -> Result<()> {
    let candidate_text = normalized_candidate_text(&candidate.text);
    if candidate_text.is_empty() {
        return Ok(());
    }
    for record in store.list()? {
        if record.status != "open" {
            continue;
        }
        if let Some(similarity) = candidate_duplicate_similarity(&candidate.text, &record.text) {
            if similarity >= 1.0 {
                bail!(
                    "duplicate todo candidate {} matches existing action {}",
                    candidate.id,
                    record.id
                );
            }
            bail!(
                "near-duplicate todo candidate {} matches existing action {} (similarity {:.2})",
                candidate.id,
                record.id,
                similarity
            );
        }
    }
    Ok(())
}

fn ensure_no_duplicate_mindweaver_todo_candidate(
    candidate: &PromotionCandidate,
    inbox_path: &Path,
) -> Result<()> {
    let candidate_text = normalized_candidate_text(&candidate.text);
    if candidate_text.is_empty() || !inbox_path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(inbox_path)
        .with_context(|| format!("reading MindWeaver inbox {}", inbox_path.display()))?;
    for line in content.lines() {
        let Some(existing) = open_mindweaver_checkbox_text(line) else {
            continue;
        };
        if let Some(similarity) = candidate_duplicate_similarity(&candidate.text, existing) {
            if similarity >= 1.0 {
                bail!(
                    "duplicate MindWeaver todo candidate {} matches existing open inbox todo in {}",
                    candidate.id,
                    inbox_path.display()
                );
            }
            bail!(
                "near-duplicate MindWeaver todo candidate {} matches existing open inbox todo in {} (similarity {:.2})",
                candidate.id,
                inbox_path.display(),
                similarity
            );
        }
    }
    Ok(())
}

fn open_mindweaver_checkbox_text(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    trimmed
        .strip_prefix("- [ ] ")
        .or_else(|| trimmed.strip_prefix("* [ ] "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn ensure_no_duplicate_skill_candidate(name: &str, store: &SkillStore) -> Result<()> {
    let candidate_name = normalized_candidate_text(name);
    if candidate_name.is_empty() {
        return Ok(());
    }
    for record in store.list()? {
        if normalized_candidate_text(&record.name) == candidate_name {
            bail!(
                "duplicate skill candidate {} matches existing skill {}",
                name,
                record.name
            );
        }
    }
    Ok(())
}

fn normalized_candidate_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

const CANDIDATE_DUPLICATE_SUBSTRING_MIN_CHARS: usize = 48;
const CANDIDATE_DUPLICATE_SUBSTRING_THRESHOLD: f64 = 0.74;
const CANDIDATE_DUPLICATE_JACCARD_THRESHOLD: f64 = 0.78;
const CANDIDATE_DUPLICATE_OVERLAP_THRESHOLD: f64 = 0.92;
const CANDIDATE_DUPLICATE_OVERLAP_MIN_TERMS: usize = 5;

fn candidate_duplicate_similarity(candidate: &str, existing: &str) -> Option<f64> {
    let candidate_text = normalized_candidate_text(candidate);
    let existing_text = normalized_candidate_text(existing);
    if candidate_text.is_empty() || existing_text.is_empty() {
        return None;
    }
    if candidate_text == existing_text {
        return Some(1.0);
    }
    let shorter_len = candidate_text.len().min(existing_text.len());
    let longer_len = candidate_text.len().max(existing_text.len());
    if shorter_len >= CANDIDATE_DUPLICATE_SUBSTRING_MIN_CHARS
        && longer_len > 0
        && (candidate_text.contains(&existing_text) || existing_text.contains(&candidate_text))
    {
        let similarity = shorter_len as f64 / longer_len as f64;
        if similarity >= CANDIDATE_DUPLICATE_SUBSTRING_THRESHOLD {
            return Some(similarity);
        }
    }

    let candidate_terms = candidate_text_terms(&candidate_text);
    let existing_terms = candidate_text_terms(&existing_text);
    if candidate_terms.len().min(existing_terms.len()) < 5 {
        return None;
    }
    let intersection = candidate_terms.intersection(&existing_terms).count();
    let union = candidate_terms.union(&existing_terms).count();
    if union == 0 {
        return None;
    }
    let similarity = intersection as f64 / union as f64;
    if similarity >= CANDIDATE_DUPLICATE_JACCARD_THRESHOLD {
        return Some(similarity);
    }

    let overlap = intersection as f64 / candidate_terms.len().min(existing_terms.len()) as f64;
    (intersection >= CANDIDATE_DUPLICATE_OVERLAP_MIN_TERMS
        && overlap >= CANDIDATE_DUPLICATE_OVERLAP_THRESHOLD)
        .then_some(similarity.max(CANDIDATE_DUPLICATE_JACCARD_THRESHOLD))
}

fn candidate_text_terms(value: &str) -> HashSet<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .map(str::trim)
        .map(normalized_candidate_term)
        .filter(|term| term.len() > 2)
        .filter(|term| !candidate_stop_term(term))
        .collect()
}

fn normalized_candidate_term(term: &str) -> String {
    let mut term = term.to_lowercase();
    if term.len() > 4 && term.ends_with('s') {
        term.pop();
    }
    term
}

fn candidate_stop_term(term: &str) -> bool {
    matches!(
        term,
        "about"
            | "after"
            | "and"
            | "before"
            | "during"
            | "for"
            | "from"
            | "into"
            | "that"
            | "the"
            | "this"
            | "use"
            | "used"
            | "using"
            | "when"
            | "while"
            | "with"
    )
}

fn promotion_candidate_source(session_dir: &Path, candidate: &PromotionCandidate) -> MemorySource {
    MemorySource {
        source_type: "promotion_session".to_string(),
        source: session_dir.display().to_string(),
        source_id: candidate.id.clone(),
        title: candidate.text.chars().take(80).collect(),
        captured_at: chrono::Local::now().to_rfc3339(),
        ..MemorySource::default()
    }
}

fn render_skill_candidate_content(candidate: &PromotionCandidate) -> String {
    let mut content = candidate
        .body
        .clone()
        .filter(|body| !body.trim().is_empty())
        .unwrap_or_else(|| candidate.text.clone());
    content.push_str("\n\n## Evidence\n\n");
    for evidence in &candidate.evidence {
        content.push_str(&format!("- {evidence}\n"));
    }
    content
}

fn render_pattern_candidate_content(candidate: &PromotionCandidate) -> String {
    let mut content = format!("# {}\n\n{}\n\n", candidate.id, candidate.text.trim());
    if let Some(rationale) = &candidate.rationale {
        if !rationale.trim().is_empty() {
            content.push_str(&format!("## Rationale\n\n{}\n\n", rationale.trim()));
        }
    }
    content.push_str("## Evidence\n\n");
    for evidence in &candidate.evidence {
        content.push_str(&format!("- {evidence}\n"));
    }
    content
}

fn render_mindweaver_todo_capture(candidate: &PromotionCandidate) -> String {
    let mut content = format!("- [ ] {}", candidate.text.trim());
    let metadata = mindweaver_todo_metadata(candidate);
    if !metadata.is_empty() {
        content.push_str("\n  - ");
        content.push_str(&metadata.join(" "));
    }
    content
}

fn mindweaver_todo_metadata(candidate: &PromotionCandidate) -> Vec<String> {
    let mut metadata = Vec::new();
    if let Some(priority) = trimmed_non_empty(candidate.priority.as_deref()) {
        metadata.push(priority.to_string());
    }
    if let Some(energy) = trimmed_non_empty(candidate.energy.as_deref()) {
        metadata.push(format!("e:{energy}"));
    }
    if let Some(due) = trimmed_non_empty(candidate.due.as_deref()) {
        metadata.push(format!("due:{due}"));
    }
    if let Some(start) = trimmed_non_empty(candidate.start.as_deref()) {
        metadata.push(format!("start:{start}"));
    }
    if let Some(estimate) = trimmed_non_empty(candidate.estimate.as_deref()) {
        metadata.push(format!("est:{estimate}"));
    }
    if let Some(area) = trimmed_non_empty(candidate.area.as_deref()) {
        metadata.push(format!("area:{area}"));
    }
    metadata
}

fn trimmed_non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn resolve_mindweaver_inbox_preview_path() -> String {
    env::var("MW_TODO_INBOX")
        .or_else(|_| env::var("MW_INBOX_PATH"))
        .or_else(|_| env::var("INBOX_PATH"))
        .unwrap_or_else(|_| "<set MW_TODO_INBOX>".to_string())
}

fn resolve_mindweaver_inbox_path(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env_path("MW_TODO_INBOX")
        .or_else(|| env_path("MW_INBOX_PATH"))
        .or_else(|| env_path("INBOX_PATH"))
    {
        return Ok(path);
    }
    bail!(
        "MindWeaver inbox path is not configured; set MW_TODO_INBOX, MW_INBOX_PATH, or INBOX_PATH before accepting a mindweaver todo candidate"
    )
}

fn env_path(name: &str) -> Option<PathBuf> {
    let value = env::var(name).ok()?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(expand_tilde_path(value))
}

fn expand_tilde_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        return djinn_core::home_dir().join(rest);
    }
    PathBuf::from(value)
}

fn render_session_decision_record(
    action: SessionDecisionAction,
    session_dir: &Path,
    promotion_type: &str,
    candidate: Option<&str>,
    writebacks: &[SessionCandidateWritebackReport],
    post_writebacks: &[SessionPostWritebackReport],
    note: &str,
) -> Result<String> {
    let mut output = String::new();
    output.push_str("version = 1\n");
    output.push_str(&format!(
        "action = {}\n",
        toml_string(session_decision_action_label(action))?
    ));
    output.push_str(&format!(
        "decided_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    output.push_str(&format!(
        "session_dir = {}\n",
        toml_string(&session_dir.display().to_string())?
    ));
    output.push_str(&format!(
        "promotion_type = {}\n",
        toml_string(promotion_type)?
    ));
    if let Some(candidate) = candidate {
        output.push_str(&format!("candidate = {}\n", toml_string(candidate)?));
    }
    output.push_str(&format!("durable_writeback = {}\n", !writebacks.is_empty()));
    output.push_str(&format!("note = {}\n", toml_string(note)?));
    for writeback in writebacks {
        output.push_str("\n[[writebacks]]\n");
        output.push_str(&format!(
            "candidate = {}\n",
            toml_string(&writeback.candidate)?
        ));
        output.push_str(&format!(
            "candidate_type = {}\n",
            toml_string(&writeback.candidate_type)?
        ));
        output.push_str(&format!(
            "destination = {}\n",
            toml_string(&writeback.destination)?
        ));
        output.push_str(&format!("id = {}\n", toml_string(&writeback.id)?));
        if let Some(path) = &writeback.path {
            output.push_str(&format!("path = {}\n", toml_string(path)?));
        }
        if let Some(preview) = &writeback.preview {
            output.push_str(&format!("preview = {}\n", toml_string(preview)?));
        }
    }
    for post in post_writebacks {
        output.push_str("\n[[post_writebacks]]\n");
        output.push_str(&format!("name = {}\n", toml_string(&post.name)?));
        output.push_str(&format!("command = {}\n", toml_string(&post.command)?));
        output.push_str(&format!("status = {}\n", toml_string(&post.status)?));
        output.push_str(&format!("dry_run = {}\n", post.dry_run));
    }
    Ok(output)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionPromoteReport {
    promotion_type: SessionPromoteType,
    promotion_session_dir: String,
    manifest_path: String,
    request_path: String,
    summary_path: String,
    source_packet_path: String,
    sources_path: String,
    session_count: usize,
    sessions: Vec<SessionPromoteSessionReport>,
    packet: String,
    created: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionPromoteSessionReport {
    session_dir: String,
    title: String,
    artifact_count: usize,
    turn_count: usize,
    artifacts: Vec<SessionPromoteArtifactReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionPromoteArtifactReport {
    kind: String,
    path: String,
    chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionPromoteArtifact {
    kind: String,
    path: PathBuf,
    relative_path: String,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionPromoteSession {
    session_dir: PathBuf,
    title: String,
    artifacts: Vec<SessionPromoteArtifact>,
    turn_count: usize,
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

fn create_promotion_session(args: &SessionPromoteArgs) -> Result<SessionPromoteReport> {
    let material = build_session_promote_material(
        &args.dirs,
        args.promotion_type,
        args.max_chars_per_artifact,
    )?;
    let promotion_session_dir = match &args.promotion_session_dir {
        Some(dir) => resolve_session_dir(dir)?,
        None => default_promotion_session_dir(args.promotion_type),
    };
    write_promotion_session(&promotion_session_dir, &material, args.force)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionPromoteMaterial {
    promotion_type: SessionPromoteType,
    sessions: Vec<SessionPromoteSession>,
    packet: String,
}

fn build_session_promote_material(
    dirs: &[PathBuf],
    promotion_type: SessionPromoteType,
    max_chars_per_artifact: usize,
) -> Result<SessionPromoteMaterial> {
    let sessions = dirs
        .iter()
        .map(|dir| collect_session_promote_artifacts(dir))
        .collect::<Result<Vec<_>>>()?;
    let packet = render_session_promote_packet(&sessions, promotion_type, max_chars_per_artifact);
    Ok(SessionPromoteMaterial {
        promotion_type,
        sessions,
        packet,
    })
}

fn write_promotion_session(
    promotion_session_dir: &Path,
    material: &SessionPromoteMaterial,
    force: bool,
) -> Result<SessionPromoteReport> {
    let context_dir = promotion_session_dir.join("context");
    let turns_dir = promotion_session_dir.join("turns");
    let outputs_dir = promotion_session_dir.join("outputs");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    fs::create_dir_all(&turns_dir)
        .with_context(|| format!("creating turns directory {}", turns_dir.display()))?;
    fs::create_dir_all(&outputs_dir)
        .with_context(|| format!("creating outputs directory {}", outputs_dir.display()))?;

    let manifest_path = promotion_session_dir.join("djinn.toml");
    let request_path = promotion_session_dir.join("request.md");
    let summary_path = promotion_session_dir.join("summary.md");
    let source_packet_path = context_dir.join("source-packet.md");
    let sources_path = context_dir.join("sources.toml");
    let context_readme_path = context_dir.join("djinn-context.md");

    let mut created = Vec::new();
    write_promotion_session_file(
        &manifest_path,
        &render_promotion_session_manifest(material)?,
        force,
        &mut created,
    )?;
    write_promotion_session_file(
        &request_path,
        &render_promotion_session_request(material.promotion_type),
        force,
        &mut created,
    )?;
    write_promotion_session_file(&summary_path, "", force, &mut created)?;
    write_promotion_session_file(
        &context_readme_path,
        &promotion_session_context_readme(material.promotion_type),
        force,
        &mut created,
    )?;
    write_promotion_session_file(&source_packet_path, &material.packet, force, &mut created)?;
    write_promotion_session_file(
        &sources_path,
        &render_promotion_sources_manifest(material)?,
        force,
        &mut created,
    )?;

    session_promote_report_from_material(
        promotion_session_dir,
        material,
        created,
        manifest_path,
        request_path,
        summary_path,
        source_packet_path,
        sources_path,
    )
}

#[allow(clippy::too_many_arguments)]
fn session_promote_report_from_material(
    promotion_session_dir: &Path,
    material: &SessionPromoteMaterial,
    created: Vec<String>,
    manifest_path: PathBuf,
    request_path: PathBuf,
    summary_path: PathBuf,
    source_packet_path: PathBuf,
    sources_path: PathBuf,
) -> Result<SessionPromoteReport> {
    let sessions = &material.sessions;
    Ok(SessionPromoteReport {
        promotion_type: material.promotion_type,
        promotion_session_dir: promotion_session_dir.display().to_string(),
        manifest_path: manifest_path.display().to_string(),
        request_path: request_path.display().to_string(),
        summary_path: summary_path.display().to_string(),
        source_packet_path: source_packet_path.display().to_string(),
        sources_path: sources_path.display().to_string(),
        session_count: sessions.len(),
        sessions: sessions
            .iter()
            .map(|session| SessionPromoteSessionReport {
                session_dir: session.session_dir.display().to_string(),
                title: session.title.clone(),
                artifact_count: session.artifacts.len(),
                turn_count: session.turn_count,
                artifacts: session
                    .artifacts
                    .iter()
                    .map(|artifact| SessionPromoteArtifactReport {
                        kind: artifact.kind.clone(),
                        path: artifact.path.display().to_string(),
                        chars: artifact.content.chars().count(),
                    })
                    .collect(),
            })
            .collect(),
        packet: material.packet.clone(),
        created,
    })
}

fn write_promotion_session_file(
    path: &Path,
    content: &str,
    force: bool,
    created: &mut Vec<String>,
) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "promotion session file already exists: {} (use --force to replace generated files)",
            path.display()
        );
    }
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    created.push(path.display().to_string());
    Ok(())
}

fn default_promotion_session_dir(promotion_type: SessionPromoteType) -> PathBuf {
    let now = chrono::Local::now();
    default_folder_session_root().join(format!(
        "promotion-{}-{}-{}",
        session_promote_type_label(promotion_type),
        now.format("%Y%m%dT%H%M%S"),
        now.timestamp_nanos_opt().unwrap_or_default()
    ))
}

fn render_promotion_session_manifest(material: &SessionPromoteMaterial) -> Result<String> {
    let workspace = env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| String::new());
    let mut output = String::new();
    output.push_str("version = 1\n");
    output.push_str("kind = \"promotion\"\n");
    output.push_str(&format!(
        "created_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    output.push_str(&format!(
        "promotion_type = {}\n",
        toml_string(session_promote_type_label(material.promotion_type))?
    ));
    if !workspace.is_empty() {
        output.push_str(&format!("workspace = {}\n", toml_string(&workspace)?));
    }
    output.push_str("\n[context]\n");
    output.push_str("path = \"context\"\n");
    output.push_str("source_packet = \"context/source-packet.md\"\n");
    output.push_str("sources = \"context/sources.toml\"\n");
    output.push_str("\n[promotion]\n");
    output.push_str(&format!(
        "type = {}\n",
        toml_string(session_promote_type_label(material.promotion_type))?
    ));
    output.push_str(&format!("source_count = {}\n", material.sessions.len()));
    Ok(output)
}

fn render_promotion_session_request(promotion_type: SessionPromoteType) -> String {
    format!(
        "# Promotion request\n\nPromotion type: `{}`\n\nUse `context/source-packet.md` as the source material. Preserve evidence links to the source session files when proposing promoted outputs.\n",
        session_promote_type_label(promotion_type)
    )
}

fn promotion_session_context_readme(promotion_type: SessionPromoteType) -> String {
    format!(
        "# Djinn promotion session context\n\nThis folder contains source material for a `{}` promotion session.\n\n- `source-packet.md`: deterministic evidence packet assembled from source sessions.\n- `sources.toml`: source session refs and selected artifact refs.\n\nDo not delete source sessions by default; promoted outputs should keep file-native provenance.\n",
        session_promote_type_label(promotion_type)
    )
}

fn render_promotion_sources_manifest(material: &SessionPromoteMaterial) -> Result<String> {
    let mut output = String::new();
    output.push_str(&format!(
        "promotion_type = {}\n",
        toml_string(session_promote_type_label(material.promotion_type))?
    ));
    output.push_str(&format!("source_count = {}\n", material.sessions.len()));
    for session in &material.sessions {
        output.push_str("\n[[source_sessions]]\n");
        output.push_str(&format!(
            "session_dir = {}\n",
            toml_string(&session.session_dir.display().to_string())?
        ));
        output.push_str(&format!("title = {}\n", toml_string(&session.title)?));
        output.push_str(&format!("turn_count = {}\n", session.turn_count));
        output.push_str(&format!("artifact_count = {}\n", session.artifacts.len()));
        for artifact in &session.artifacts {
            output.push_str("\n[[source_sessions.artifacts]]\n");
            output.push_str(&format!("kind = {}\n", toml_string(&artifact.kind)?));
            output.push_str(&format!(
                "path = {}\n",
                toml_string(&artifact.path.display().to_string())?
            ));
            output.push_str(&format!(
                "relative_path = {}\n",
                toml_string(&artifact.relative_path)?
            ));
            output.push_str(&format!("chars = {}\n", artifact.content.chars().count()));
        }
    }
    Ok(output)
}

fn collect_session_promote_artifacts(dir: &Path) -> Result<SessionPromoteSession> {
    let session_dir = resolve_session_dir(dir)?;
    let title = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(folder_session_display_name)
        .unwrap_or_else(|| session_dir.display().to_string());

    let mut artifacts = Vec::new();
    push_session_promote_artifact(
        &mut artifacts,
        &session_dir,
        "request",
        &session_dir.join("request.md"),
    )?;
    push_session_promote_artifact(
        &mut artifacts,
        &session_dir,
        "summary",
        &session_dir.join("summary.md"),
    )?;
    push_session_promote_artifact(
        &mut artifacts,
        &session_dir,
        "compacted_context",
        &session_dir.join("context").join("compacted.md"),
    )?;

    let turns = read_folder_session_turns(&session_dir.join("turns"))?;
    for turn in &turns {
        if let Some(path) = &turn.request_path {
            push_session_promote_artifact(
                &mut artifacts,
                &session_dir,
                &format!("turn:{}:request", turn.id),
                path,
            )?;
        }
        if let Some(path) = &turn.response_path {
            push_session_promote_artifact(
                &mut artifacts,
                &session_dir,
                &format!("turn:{}:response", turn.id),
                path,
            )?;
        }
    }

    if artifacts.is_empty() {
        bail!(
            "session {} has no promotable artifacts; run `djinn ask --session {}` first or add summary/context files",
            session_dir.display(),
            session_dir.display()
        );
    }

    Ok(SessionPromoteSession {
        session_dir,
        title,
        artifacts,
        turn_count: turns.len(),
    })
}

fn push_session_promote_artifact(
    artifacts: &mut Vec<SessionPromoteArtifact>,
    session_dir: &Path,
    kind: &str,
    path: &Path,
) -> Result<()> {
    let Some(content) = read_optional_markdown_file(path)? else {
        return Ok(());
    };
    artifacts.push(SessionPromoteArtifact {
        kind: kind.to_string(),
        path: path.to_path_buf(),
        relative_path: session_relative_path(session_dir, path),
        content,
    });
    Ok(())
}

fn session_relative_path(session_dir: &Path, path: &Path) -> String {
    path.strip_prefix(session_dir)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn render_session_promote_packet(
    sessions: &[SessionPromoteSession],
    promotion_type: SessionPromoteType,
    max_chars_per_artifact: usize,
) -> String {
    let mut out = String::from("# Djinn Folder Session Promotion Packet\n\n");
    out.push_str(&format!(
        "Promotion type: `{}`\n",
        session_promote_type_label(promotion_type)
    ));
    out.push_str(&format!("Sessions: `{}`\n\n", sessions.len()));
    out.push_str("## Instructions\n\n");
    out.push_str(session_promote_type_instructions(promotion_type));
    out.push_str("\n\nUse only the evidence below. Preserve file-native provenance by citing `session_dir` plus artifact paths such as `summary.md`, `context/compacted.md`, and `turns/<id>/response.md`. Do not invent facts that are not supported by copied evidence.\n");

    for (idx, session) in sessions.iter().enumerate() {
        out.push_str(&format!(
            "\n## Session {}: {}\n\n- session_dir: `{}`\n- turns: `{}`\n- artifacts: `{}`\n",
            idx + 1,
            session.title,
            session.session_dir.display(),
            session.turn_count,
            session.artifacts.len()
        ));
        out.push_str("\n### Provenance\n\n");
        for artifact in &session.artifacts {
            out.push_str(&format!(
                "- `{}`: `{}` ({} chars)\n",
                artifact.kind,
                artifact.relative_path,
                artifact.content.chars().count()
            ));
        }
        out.push_str("\n### Evidence excerpts\n");
        for artifact in &session.artifacts {
            out.push_str(&format!(
                "\n#### {} — `{}`\n\n```text\n{}\n```\n",
                artifact.kind,
                artifact.relative_path,
                truncate(&artifact.content, max_chars_per_artifact)
            ));
        }
    }

    out
}

fn session_promote_type_label(promotion_type: SessionPromoteType) -> &'static str {
    match promotion_type {
        SessionPromoteType::Memory => "memory",
        SessionPromoteType::Todo => "todo",
        SessionPromoteType::Skill => "skill",
        SessionPromoteType::Pattern => "pattern",
    }
}

fn session_promote_type_instructions(promotion_type: SessionPromoteType) -> &'static str {
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionStatusReport {
    session_dir: String,
    manifest_exists: bool,
    session_id: Option<String>,
    native_session_exists: bool,
    profile: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    workspace: Option<String>,
    repo: Option<SessionStatusRepoReport>,
    lifecycle: SessionStatusLifecycleReport,
    files: SessionStatusFileReport,
    turn_count: usize,
    latest_turn: Option<SessionStatusTurnReport>,
    candidates: Option<SessionStatusCandidateReport>,
    context_ingestible_count: usize,
    context_skipped: Vec<String>,
    next_action: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionStatusCandidateReport {
    candidate_count: usize,
    accepted_count: usize,
    denied_count: usize,
    pending_count: usize,
    candidates_dir: String,
    candidate_index_path: Option<String>,
    candidate_status_path: Option<String>,
    entries: Vec<SessionStatusCandidateEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionStatusCandidateEntry {
    id: String,
    candidate_type: Option<String>,
    status: String,
    path: String,
    text: Option<String>,
    rationale: Option<String>,
    evidence: Vec<String>,
    destination: Option<String>,
    writeback_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PromotionCandidateDecisionStatus {
    status: String,
    destination: Option<String>,
    writeback_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionStatusLifecycleReport {
    state: String,
    mode: Option<String>,
    updated_at: Option<String>,
    reason: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionStatusTurnReport {
    id: String,
    request_path: Option<String>,
    response_path: Option<String>,
    has_response: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionStatusRepoReport {
    path: Option<String>,
    link: Option<String>,
    link_exists: bool,
    link_is_symlink: bool,
    link_target: Option<String>,
    link_broken: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionStatusFileReport {
    request_md: bool,
    summary_md: bool,
    context_dir: bool,
    compacted_md: bool,
    turns_dir: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionContextLsReport {
    session_dir: String,
    context_dir: String,
    entries: Vec<SessionContextEntryReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionContextEntryReport {
    name: String,
    path: String,
    kind: String,
    symlink: bool,
    target: Option<String>,
    broken: bool,
    ingestible: bool,
    skip_reason: Option<String>,
    bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionContextAddReport {
    session_dir: String,
    context_dir: String,
    name: String,
    path: String,
    target: String,
    replaced: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionContextRmReport {
    session_dir: String,
    context_dir: String,
    name: String,
    path: String,
    removed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionContextDiscoverReport {
    session_dir: String,
    context_dir: String,
    repo: String,
    dry_run: bool,
    links: Vec<SessionContextDiscoverLink>,
    indexed: Vec<SessionContextDiscoverIndexEntry>,
    ignored: Vec<String>,
    warnings: Vec<String>,
    repo_index_path: String,
    repo_index_written: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionContextDiscoverLink {
    source: String,
    name: String,
    path: String,
    target: String,
    existed: bool,
    created: bool,
    reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionContextDiscoverIndexEntry {
    source: String,
    path: String,
    title: Option<String>,
    reason: String,
}

fn session_status(args: SessionStatusArgs) -> Result<()> {
    let report = folder_session_status(&args.dir)?;
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionLsReport {
    root: String,
    sessions: Vec<FolderSessionSummary>,
    groups: Vec<FolderSessionGroup>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FolderSessionGroup {
    repo: String,
    sessions: Vec<FolderSessionSummary>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FolderSessionSummary {
    name: String,
    display_name: String,
    reference_name: String,
    path: String,
    manifest_exists: bool,
    session_id: Option<String>,
    native_session_exists: bool,
    lifecycle: SessionStatusLifecycleReport,
    created_at: Option<String>,
    updated_at: Option<String>,
    workspace: Option<String>,
    repo_path: Option<String>,
    request_md: bool,
    summary_md: bool,
    summary_preview: Option<String>,
    turn_count: usize,
    latest_turn: Option<SessionStatusTurnReport>,
    candidates: Option<SessionStatusCandidateReport>,
    next_action: Option<String>,
    modified_at: Option<String>,
    modified_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionShortenNamesReport {
    root: String,
    dry_run: bool,
    renamed: Vec<SessionShortenNameEntry>,
    skipped: Vec<SessionShortenNameSkip>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionShortenNameEntry {
    from: String,
    to: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionShortenNameSkip {
    path: String,
    reason: String,
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

fn session_open(args: SessionOpenArgs) -> Result<()> {
    let target = resolve_folder_session_open_target(&args.dir, args.target)?;
    open_editor_path(&target, args.editor)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionRmReport {
    session_dir: String,
    removed_folder: bool,
    session_id: Option<String>,
    removed_native_session: bool,
}

fn session_rm(args: SessionRmArgs) -> Result<()> {
    let report = remove_folder_session(&args.dir)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Removed folder session: {}", report.session_dir);
        if let Some(session_id) = &report.session_id {
            println!(
                "Native session {session_id}: {}",
                if report.removed_native_session {
                    "removed"
                } else {
                    "not found"
                }
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionInitReport {
    session_dir: String,
    manifest_path: String,
    request_path: String,
    summary_path: String,
    context_dir: String,
    turns_dir: String,
    profile: String,
    agent: Option<String>,
    model: String,
    workspace: String,
    repo_link: Option<SessionRepoLinkReport>,
    discovered_context: Option<SessionContextDiscoverReport>,
    config_sources: Vec<String>,
    precedence: Vec<String>,
    created: Vec<String>,
    skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionRepoLinkReport {
    path: String,
    target: String,
}

fn session_init(args: SessionInitArgs) -> Result<()> {
    let report = initialize_folder_session(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Initialized Djinn session: {}", report.session_dir);
        println!("  profile: {}", report.profile);
        if let Some(agent) = &report.agent {
            println!("  agent: {agent}");
        }
        println!("  model: {}", report.model);
        println!("  workspace: {}", report.workspace);
        if let Some(repo_link) = &report.repo_link {
            println!("  repo link: {} -> {}", repo_link.path, repo_link.target);
        }
        if let Some(discovered) = &report.discovered_context {
            let created = discovered.links.iter().filter(|link| link.created).count();
            let existing = discovered.links.iter().filter(|link| link.existed).count();
            println!(
                "  discovered context: {created} linked, {existing} existing, index {}",
                discovered.repo_index_path
            );
        }
        println!("  request: {}", report.request_path);
        println!("  summary: {}", report.summary_path);
        println!("  run: djinn ask --session {}", args.dir.display());
        println!(
            "  done: command exits; answer is written to summary.md and turns/<turn>/response.md"
        );
    }
    Ok(())
}

fn initialize_folder_session(args: &SessionInitArgs) -> Result<SessionInitReport> {
    let session_dir = resolve_session_dir(&args.dir)?;
    fs::create_dir_all(&session_dir)
        .with_context(|| format!("creating session directory {}", session_dir.display()))?;
    let context_dir = session_dir.join("context");
    let turns_dir = session_dir.join("turns");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    fs::create_dir_all(&turns_dir)
        .with_context(|| format!("creating turns directory {}", turns_dir.display()))?;

    let workspace = match &args.link_repo {
        Some(path) => canonical_existing_dir(path, "linked repository")?,
        None => env::current_dir().context("resolving current workspace")?,
    };
    let config_report = load_djinn_config_from_paths(clean_unique_paths(vec![
        default_djinn_config_path(),
        workspace.join(".djinn.json"),
    ]))?;
    let selection = resolve_agent_role_selection_from_config(
        &config_report.effective,
        args.agent.clone(),
        &args.profile,
        args.model.clone(),
    )?;
    let model = resolve_agent_model_from_config(
        selection.model.clone(),
        &config_report.effective,
        &selection.profile,
    );
    validate_session_init_identity(&session_dir, args, &workspace, &selection, &model)?;

    let mut created = Vec::new();
    let mut skipped = Vec::new();
    let request_path = session_dir.join("request.md");
    write_scaffold_file(&request_path, "", args.force, &mut created, &mut skipped)?;
    let summary_path = session_dir.join("summary.md");
    write_scaffold_file(&summary_path, "", args.force, &mut created, &mut skipped)?;
    let readme_path = context_dir.join("djinn-context.md");
    write_scaffold_file(
        &readme_path,
        &session_context_readme(args.link_repo.as_ref(), &workspace),
        args.force,
        &mut created,
        &mut skipped,
    )?;

    let repo_link = if args.link_repo.is_some() {
        Some(link_repo_into_session_context(
            &context_dir,
            &workspace,
            args.force,
            &mut created,
            &mut skipped,
        )?)
    } else {
        None
    };

    let manifest_path = session_dir.join("djinn.toml");
    let manifest = render_session_manifest(
        &selection,
        &model,
        &workspace,
        repo_link.as_ref(),
        &config_report.checked_paths,
    )?;
    write_scaffold_file(
        &manifest_path,
        &manifest,
        args.force,
        &mut created,
        &mut skipped,
    )?;
    let discovered_context = if args.link_repo.is_some() && !args.no_discover_context {
        Some(discover_folder_session_context(&session_dir, false)?)
    } else {
        None
    };

    Ok(SessionInitReport {
        session_dir: session_dir.display().to_string(),
        manifest_path: manifest_path.display().to_string(),
        request_path: request_path.display().to_string(),
        summary_path: summary_path.display().to_string(),
        context_dir: context_dir.display().to_string(),
        turns_dir: turns_dir.display().to_string(),
        profile: selection.profile,
        agent: selection.agent_name,
        model,
        workspace: workspace.display().to_string(),
        repo_link,
        discovered_context,
        config_sources: config_report.checked_paths,
        precedence: vec![
            "global profile/config".to_string(),
            "repo-local config/context".to_string(),
            "session-local files".to_string(),
        ],
        created,
        skipped,
    })
}

fn validate_session_init_identity(
    session_dir: &Path,
    args: &SessionInitArgs,
    workspace: &Path,
    selection: &AgentRoleSelection,
    model: &str,
) -> Result<()> {
    if args.force {
        return Ok(());
    }
    let Some(existing) = read_folder_session_manifest(session_dir)? else {
        return Ok(());
    };
    let mut conflicts = Vec::new();
    push_session_init_conflict(
        &mut conflicts,
        "profile",
        existing.profile.as_deref(),
        Some(&selection.profile),
    );
    push_session_init_conflict(
        &mut conflicts,
        "agent",
        existing.agent.as_deref(),
        selection.agent_name.as_deref(),
    );
    push_session_init_conflict(
        &mut conflicts,
        "model",
        existing.model.as_deref(),
        Some(model),
    );
    push_session_init_conflict(
        &mut conflicts,
        "workspace",
        existing.workspace.as_deref(),
        Some(&workspace.display().to_string()),
    );
    if let Some(repo_path) = &existing.repo_path {
        if args.link_repo.is_some() && repo_path != &workspace.display().to_string() {
            conflicts.push(format!(
                "repo path existing={} requested={}",
                repo_path,
                workspace.display()
            ));
        }
    }
    if conflicts.is_empty() {
        return Ok(());
    }
    bail!(
        "session folder already exists with different identity: {} ({}) (use --force to replace scaffolded metadata)",
        session_dir.display(),
        conflicts.join(", ")
    )
}

fn push_session_init_conflict(
    conflicts: &mut Vec<String>,
    field: &str,
    existing: Option<&str>,
    requested: Option<&str>,
) {
    let existing = existing.map(str::trim).filter(|value| !value.is_empty());
    let requested = requested.map(str::trim).filter(|value| !value.is_empty());
    if let (Some(existing), Some(requested)) = (existing, requested) {
        if existing != requested {
            conflicts.push(format!("{field} existing={existing} requested={requested}"));
        }
    }
}

fn compact_folder_session(
    session_dir: &Path,
    output: Option<&Path>,
) -> Result<SessionCompactReport> {
    let session_dir = resolve_session_dir(session_dir)?;
    let turns_dir = session_dir.join("turns");
    let context_dir = session_dir.join("context");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    let output_path = output
        .map(PathBuf::from)
        .unwrap_or_else(|| context_dir.join("compacted.md"));
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("creating compaction output directory {}", parent.display())
        })?;
    }

    let turns = read_folder_session_turns(&turns_dir)?;
    let generated = render_folder_session_compaction_generated(&session_dir, &turns);
    let existing = fs::read_to_string(&output_path).ok();
    let content = merge_folder_session_compaction_document(existing.as_deref(), &generated);
    fs::write(&output_path, content)
        .with_context(|| format!("writing {}", output_path.display()))?;

    Ok(SessionCompactReport {
        session_dir: session_dir.display().to_string(),
        output_path: output_path.display().to_string(),
        turn_count: turns.len(),
        turns: turns
            .into_iter()
            .map(|turn| CompactedTurnReport {
                id: turn.id,
                request_path: turn.request_path.map(|path| path.display().to_string()),
                response_path: turn.response_path.map(|path| path.display().to_string()),
            })
            .collect(),
    })
}

fn folder_session_status(dir: &Path) -> Result<SessionStatusReport> {
    let session_dir = resolve_session_dir(dir)?;
    let manifest_path = session_dir.join("djinn.toml");
    let manifest = read_folder_session_manifest(&session_dir)?;
    let session_id = manifest
        .as_ref()
        .and_then(|manifest| manifest.session_id.clone());
    let native_session = session_id
        .as_ref()
        .and_then(|id| load_folder_native_agent_session(&session_dir, id));
    let native_session_exists = native_session.is_some();
    let context_dir = session_dir.join("context");
    let turns_dir = session_dir.join("turns");
    let (context_ingestible_count, context_skipped) =
        inspect_folder_session_context_dir(&context_dir)?;
    let request_exists = session_dir.join("request.md").exists();
    let turns = read_folder_session_turns(&turns_dir)?;
    let turn_count = turns.len();
    let latest_turn = turns.last().map(session_status_turn_report);
    let candidates = session_status_candidates(&session_dir)?;
    let lifecycle = session_status_lifecycle(
        &session_dir,
        manifest.as_ref(),
        native_session.as_ref(),
        candidates.as_ref(),
    );
    let next_action = session_status_next_action(
        &session_dir,
        manifest.as_ref(),
        request_exists,
        turn_count,
        &lifecycle,
        candidates.as_ref(),
    );

    Ok(SessionStatusReport {
        session_dir: session_dir.display().to_string(),
        manifest_exists: manifest_path.exists(),
        session_id: session_id.map(|id| id.to_string()),
        native_session_exists,
        profile: manifest
            .as_ref()
            .and_then(|manifest| manifest.profile.clone()),
        agent: manifest
            .as_ref()
            .and_then(|manifest| manifest.agent.clone()),
        model: manifest
            .as_ref()
            .and_then(|manifest| manifest.model.clone()),
        workspace: manifest
            .as_ref()
            .and_then(|manifest| manifest.workspace.clone()),
        repo: manifest
            .as_ref()
            .and_then(|manifest| session_status_repo(&session_dir, manifest)),
        lifecycle,
        files: SessionStatusFileReport {
            request_md: request_exists,
            summary_md: session_dir.join("summary.md").exists(),
            context_dir: context_dir.is_dir(),
            compacted_md: context_dir.join("compacted.md").exists(),
            turns_dir: turns_dir.is_dir(),
        },
        turn_count,
        latest_turn,
        candidates,
        context_ingestible_count,
        context_skipped,
        next_action,
    })
}

fn session_status_lifecycle(
    session_dir: &Path,
    manifest: Option<&FolderSessionManifest>,
    native_session: Option<&AgentSession>,
    candidates: Option<&SessionStatusCandidateReport>,
) -> SessionStatusLifecycleReport {
    if let Some(session) = native_session {
        let lifecycle = lifecycle_for(session);
        SessionStatusLifecycleReport {
            state: lifecycle.state.as_str().to_string(),
            mode: lifecycle.mode.map(|mode| mode.as_str().to_string()),
            updated_at: non_empty_string(&lifecycle.updated_at),
            reason: lifecycle.reason,
            note: lifecycle.note,
        }
    } else if manifest.and_then(|manifest| manifest.kind.as_deref()) == Some("promotion") {
        promotion_session_status_lifecycle(session_dir, candidates)
    } else {
        SessionStatusLifecycleReport {
            state: "not_started".to_string(),
            mode: None,
            updated_at: None,
            reason: None,
            note: None,
        }
    }
}

fn promotion_session_status_lifecycle(
    session_dir: &Path,
    candidates: Option<&SessionStatusCandidateReport>,
) -> SessionStatusLifecycleReport {
    if let Some(run) = latest_background_session_run_status(session_dir).filter(|run| run.alive) {
        return SessionStatusLifecycleReport {
            state: "running".to_string(),
            mode: Some("promotion".to_string()),
            updated_at: run.log_modified_at.clone().or(run.started_at.clone()),
            reason: Some("background_generation".to_string()),
            note: Some(format_background_promotion_run_note(&run)),
        };
    }
    if candidates.is_some_and(|candidates| candidates.candidate_count > 0) {
        return SessionStatusLifecycleReport {
            state: "completed".to_string(),
            mode: Some("promotion".to_string()),
            updated_at: latest_promotion_generation_modified_at(session_dir),
            reason: Some("candidates_generated".to_string()),
            note: Some("Promotion candidates are ready for review.".to_string()),
        };
    }
    if let Some(run) = latest_background_session_run_status(session_dir) {
        return SessionStatusLifecycleReport {
            state: "failed".to_string(),
            mode: Some("promotion".to_string()),
            updated_at: run
                .started_at
                .or_else(|| latest_promotion_generation_modified_at(session_dir)),
            reason: Some("generation_failed".to_string()),
            note: Some(format!(
                "Promotion generation exited before writing valid candidates. Inspect the model response or log: {}",
                run.log_path.as_deref().unwrap_or("unknown")
            )),
        };
    }
    if promotion_generation_has_response(session_dir) {
        return SessionStatusLifecycleReport {
            state: "failed".to_string(),
            mode: Some("promotion".to_string()),
            updated_at: latest_promotion_generation_modified_at(session_dir),
            reason: Some("no_candidates".to_string()),
            note: Some(
                "Promotion generation wrote a response but no candidate TOML files.".to_string(),
            ),
        };
    }
    SessionStatusLifecycleReport {
        state: "not_started".to_string(),
        mode: Some("promotion".to_string()),
        updated_at: latest_promotion_generation_modified_at(session_dir),
        reason: None,
        note: None,
    }
}

fn format_background_promotion_run_note(run: &BackgroundRunStatus) -> String {
    let mut note = format!(
        "Promotion candidate generation is running in the background (pid {}, log {}, {}).",
        run.pid,
        run.log_path.as_deref().unwrap_or("unknown"),
        run.log_bytes
            .map(format_byte_count)
            .unwrap_or_else(|| "log size unknown".to_string())
    );
    if let Some(updated) = &run.log_modified_at {
        note.push_str(&format!(" Log updated {updated}."));
    }
    if let Some(tail) = &run.log_tail {
        note.push_str(&format!(" Last log: {tail}"));
    }
    note
}

fn format_byte_count(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn promotion_generation_has_response(session_dir: &Path) -> bool {
    latest_promotion_generation_response_path(session_dir).is_some()
}

fn latest_promotion_generation_response_path(session_dir: &Path) -> Option<PathBuf> {
    session_dir
        .join("outputs")
        .join("generation")
        .read_dir()
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-response.md"))
        })
        .filter_map(|path| Some((fs::metadata(&path).ok()?.modified().ok()?, path)))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn latest_promotion_generation_modified_at(session_dir: &Path) -> Option<String> {
    let generation_dir = session_dir.join("outputs").join("generation");
    fs::read_dir(generation_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .max()
        .and_then(system_time_to_rfc3339)
}

fn system_time_to_rfc3339(time: SystemTime) -> Option<String> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(
        duration.as_secs() as i64,
        duration.subsec_nanos(),
    )
    .map(|time| time.to_rfc3339())
}

fn session_status_turn_report(turn: &FolderSessionTurnDigest) -> SessionStatusTurnReport {
    SessionStatusTurnReport {
        id: turn.id.clone(),
        request_path: turn
            .request_path
            .as_ref()
            .map(|path| path.display().to_string()),
        response_path: turn
            .response_path
            .as_ref()
            .map(|path| path.display().to_string()),
        has_response: turn.response_path.is_some(),
    }
}

fn session_status_candidates(session_dir: &Path) -> Result<Option<SessionStatusCandidateReport>> {
    let outputs_dir = session_dir.join("outputs");
    let candidates_dir = outputs_dir.join("candidates");
    let candidate_index_path = outputs_dir.join("candidate-index.toml");
    let candidate_status_path = outputs_dir.join("candidate-status.toml");
    let decisions = read_promotion_candidate_statuses(&candidate_status_path)?;
    let entries = read_session_status_candidate_entries(&candidates_dir, &decisions)?;
    let candidate_count = entries.len();
    if candidate_count == 0 && decisions.is_empty() && !candidate_index_path.exists() {
        return Ok(None);
    }
    let accepted_count = entries
        .iter()
        .filter(|entry| entry.status == "accepted")
        .count();
    let denied_count = entries
        .iter()
        .filter(|entry| entry.status == "denied")
        .count();
    let pending_count = entries
        .iter()
        .filter(|entry| entry.status == "pending")
        .count();
    Ok(Some(SessionStatusCandidateReport {
        candidate_count,
        accepted_count,
        denied_count,
        pending_count,
        candidates_dir: candidates_dir.display().to_string(),
        candidate_index_path: candidate_index_path
            .exists()
            .then(|| candidate_index_path.display().to_string()),
        candidate_status_path: candidate_status_path
            .exists()
            .then(|| candidate_status_path.display().to_string()),
        entries,
    }))
}

fn read_session_status_candidate_entries(
    candidates_dir: &Path,
    decisions: &BTreeMap<String, PromotionCandidateDecisionStatus>,
) -> Result<Vec<SessionStatusCandidateEntry>> {
    if !candidates_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(candidates_dir)
        .with_context(|| format!("reading promotion candidates {}", candidates_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|entry| {
            entry.is_file() && entry.extension().and_then(|ext| ext.to_str()) == Some("toml")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .iter()
        .map(|path| read_session_status_candidate_entry(path, decisions))
        .collect()
}

fn read_session_status_candidate_entry(
    path: &Path,
    decisions: &BTreeMap<String, PromotionCandidateDecisionStatus>,
) -> Result<SessionStatusCandidateEntry> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading promotion candidate {}", path.display()))?;
    let id = candidate_string_value(&content, "id").unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("candidate")
            .to_string()
    });
    let decision = decisions.get(&id);
    Ok(SessionStatusCandidateEntry {
        id,
        candidate_type: candidate_string_value(&content, "type"),
        status: decision
            .map(|decision| decision.status.clone())
            .filter(|status| !status.trim().is_empty())
            .unwrap_or_else(|| "pending".to_string()),
        path: path.display().to_string(),
        text: candidate_string_value(&content, "text"),
        rationale: candidate_string_value(&content, "rationale"),
        evidence: candidate_string_array_value(&content, "evidence"),
        destination: decision.and_then(|decision| decision.destination.clone()),
        writeback_path: decision.and_then(|decision| decision.writeback_path.clone()),
    })
}

fn read_promotion_candidate_statuses(
    status_path: &Path,
) -> Result<BTreeMap<String, PromotionCandidateDecisionStatus>> {
    if !status_path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = fs::read_to_string(status_path)
        .with_context(|| format!("reading {}", status_path.display()))?;
    let mut statuses = BTreeMap::new();
    let mut event = PromotionCandidateStatusEvent::default();
    for line in content.lines().map(str::trim) {
        if line.starts_with("[[") {
            record_promotion_candidate_status_event(&mut statuses, &event);
            event = PromotionCandidateStatusEvent::default();
            continue;
        }
        if let Some(value) = line
            .strip_prefix("candidate =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        {
            event.candidate = Some(value);
            continue;
        }
        if let Some(status) = line
            .strip_prefix("status =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        {
            event.status = Some(status);
            continue;
        }
        if let Some(destination) = line
            .strip_prefix("destination =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        {
            event.destination = Some(destination);
            continue;
        }
        if let Some(writeback_path) = line
            .strip_prefix("writeback_path =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        {
            event.writeback_path = Some(writeback_path);
        }
    }
    record_promotion_candidate_status_event(&mut statuses, &event);
    Ok(statuses)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PromotionCandidateStatusEvent {
    candidate: Option<String>,
    status: Option<String>,
    destination: Option<String>,
    writeback_path: Option<String>,
}

fn record_promotion_candidate_status_event(
    statuses: &mut BTreeMap<String, PromotionCandidateDecisionStatus>,
    event: &PromotionCandidateStatusEvent,
) {
    let Some(candidate) = event
        .candidate
        .as_deref()
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
    else {
        return;
    };
    let Some(status) = event
        .status
        .as_deref()
        .map(str::trim)
        .filter(|status| !status.is_empty())
    else {
        return;
    };
    statuses.insert(
        candidate.to_string(),
        PromotionCandidateDecisionStatus {
            status: status.to_string(),
            destination: event.destination.clone(),
            writeback_path: event.writeback_path.clone(),
        },
    );
}

fn format_session_candidate_status(candidates: &SessionStatusCandidateReport) -> String {
    format!(
        "{} total, {} accepted, {} denied, {} pending",
        candidates.candidate_count,
        candidates.accepted_count,
        candidates.denied_count,
        candidates.pending_count
    )
}

fn format_session_candidate_entry(entry: &SessionStatusCandidateEntry) -> String {
    let candidate_type = entry.candidate_type.as_deref().unwrap_or("unknown");
    let mut detail = format!("{} [{}] {}", entry.id, candidate_type, entry.status);
    if let Some(destination) = &entry.destination {
        detail.push_str(&format!(" -> {destination}"));
    }
    if let Some(evidence) = entry.evidence.first() {
        detail.push_str(&format!(" · evidence {evidence}"));
        if entry.evidence.len() > 1 {
            detail.push_str(&format!(" (+{})", entry.evidence.len() - 1));
        }
    }
    if let Some(path) = &entry.writeback_path {
        detail.push_str(&format!(" ({path})"));
    }
    detail
}

fn session_status_next_action(
    session_dir: &Path,
    manifest: Option<&FolderSessionManifest>,
    request_exists: bool,
    turn_count: usize,
    lifecycle: &SessionStatusLifecycleReport,
    candidates: Option<&SessionStatusCandidateReport>,
) -> Option<String> {
    if lifecycle.state == "running" {
        Some(format!(
            "check again: djinn session status {}",
            session_dir.display()
        ))
    } else if manifest.and_then(|manifest| manifest.kind.as_deref()) == Some("promotion")
        && candidates.is_some_and(|candidates| candidates.candidate_count > 0)
    {
        Some(format!(
            "review candidates: djinn session accept {} --dry-run",
            session_dir.display()
        ))
    } else if lifecycle.state == "failed" {
        Some("inspect the failure note, edit request.md or context, then run again".to_string())
    } else if request_exists && turn_count == 0 {
        Some(format!(
            "run request.md: djinn session run {}",
            session_dir.display()
        ))
    } else if turn_count > 0 {
        Some(format!(
            "open latest summary: djinn session open {} summary",
            session_dir.display()
        ))
    } else {
        None
    }
}

fn list_cache_folder_sessions(limit: Option<usize>) -> Result<SessionLsReport> {
    let root = default_folder_session_root();
    list_folder_sessions_in_root(&root, limit)
}

fn list_folder_sessions_in_root(root: &Path, limit: Option<usize>) -> Result<SessionLsReport> {
    let mut summaries = Vec::new();
    if root.is_dir() {
        let mut entries = fs::read_dir(root)
            .with_context(|| format!("reading folder session root {}", root.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            summaries.push(folder_session_summary(&path)?);
        }
        summaries.sort_by(folder_session_summary_order);
        if let Some(limit) = limit {
            summaries.truncate(limit);
        }
    }
    let groups = group_folder_session_summaries(&summaries);
    Ok(SessionLsReport {
        root: root.display().to_string(),
        sessions: summaries,
        groups,
    })
}

fn shorten_cache_folder_session_names(dry_run: bool) -> Result<SessionShortenNamesReport> {
    let root = default_folder_session_root();
    shorten_folder_session_names_in_root(&root, dry_run)
}

fn shorten_folder_session_names_in_root(
    root: &Path,
    dry_run: bool,
) -> Result<SessionShortenNamesReport> {
    let mut renamed = Vec::new();
    let mut skipped = Vec::new();
    if root.is_dir() {
        let mut entries = fs::read_dir(root)
            .with_context(|| format!("reading folder session root {}", root.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let from = entry.path();
            if !from.is_dir() {
                continue;
            }
            let Some(name) = from.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.contains("-agt_") {
                continue;
            }
            let target_name = folder_session_reference_name(name);
            if target_name == name {
                continue;
            }
            let to = root.join(&target_name);
            if to.exists() {
                skipped.push(SessionShortenNameSkip {
                    path: from.display().to_string(),
                    reason: format!("target already exists: {}", to.display()),
                });
                continue;
            }
            renamed.push(SessionShortenNameEntry {
                from: from.display().to_string(),
                to: to.display().to_string(),
            });
            if !dry_run {
                fs::rename(&from, &to)
                    .with_context(|| format!("renaming {} to {}", from.display(), to.display()))?;
            }
        }
    }
    Ok(SessionShortenNamesReport {
        root: root.display().to_string(),
        dry_run,
        renamed,
        skipped,
    })
}

fn folder_session_summary(path: &Path) -> Result<FolderSessionSummary> {
    let manifest = read_folder_session_manifest(path)?;
    let session_id = manifest
        .as_ref()
        .and_then(|manifest| manifest.session_id.clone());
    let native_session = session_id
        .as_ref()
        .and_then(|id| load_folder_native_agent_session(path, id));
    let native_session_exists = native_session.is_some();
    let created_at = native_session
        .as_ref()
        .and_then(|session| non_empty_string(&session.meta.created_at))
        .or_else(|| {
            manifest
                .as_ref()
                .and_then(|manifest| non_empty_string(manifest.created_at.as_deref().unwrap_or("")))
        });
    let updated_at = native_session
        .as_ref()
        .and_then(latest_agent_session_event_created_at)
        .or_else(|| created_at.clone())
        .or_else(|| folder_session_modified_at(path));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("session")
        .to_string();
    let turns = read_folder_session_turns(&path.join("turns"))?;
    let turn_count = turns.len();
    let latest_turn = turns.last().map(session_status_turn_report);
    let request_md = path.join("request.md").exists();
    let candidates = session_status_candidates(path)?;
    let lifecycle = session_status_lifecycle(
        path,
        manifest.as_ref(),
        native_session.as_ref(),
        candidates.as_ref(),
    );
    let next_action = session_status_next_action(
        path,
        manifest.as_ref(),
        request_md,
        turn_count,
        &lifecycle,
        candidates.as_ref(),
    );
    Ok(FolderSessionSummary {
        display_name: folder_session_display_name(&name),
        reference_name: folder_session_reference_name(&name),
        name,
        path: path.display().to_string(),
        manifest_exists: path.join("djinn.toml").exists(),
        session_id: session_id.map(|id| id.to_string()),
        native_session_exists,
        lifecycle,
        created_at,
        updated_at,
        workspace: manifest
            .as_ref()
            .and_then(|manifest| manifest.workspace.clone()),
        repo_path: manifest
            .as_ref()
            .and_then(|manifest| manifest.repo_path.clone()),
        request_md,
        summary_md: path.join("summary.md").exists(),
        summary_preview: folder_session_summary_preview(path),
        turn_count,
        latest_turn,
        candidates,
        next_action,
        modified_at: folder_session_modified_at(path),
        modified_at_ms: folder_session_modified_at_ms(path),
    })
}

fn folder_session_summary_order(
    left: &FolderSessionSummary,
    right: &FolderSessionSummary,
) -> std::cmp::Ordering {
    folder_session_repo_sort_key(left)
        .cmp(&folder_session_repo_sort_key(right))
        .then_with(|| {
            folder_session_recency_sort_key(right).cmp(&folder_session_recency_sort_key(left))
        })
        .then_with(|| left.name.cmp(&right.name))
}

fn folder_session_display_name(name: &str) -> String {
    let stripped = name
        .split_once("-agt_")
        .map(|(prefix, _)| prefix)
        .unwrap_or(name)
        .trim_matches('-')
        .trim();
    if stripped.is_empty() {
        "session".to_string()
    } else {
        stripped.to_string()
    }
}

fn folder_session_reference_name(name: &str) -> String {
    let display = folder_session_display_name(name);
    let Some((_, suffix)) = name.split_once("-agt_") else {
        return display;
    };
    format!(
        "{display}-{}",
        short_agent_session_suffix_from_str(&format!("agt_{suffix}"))
    )
}

fn short_agent_session_suffix(id: &AgentSessionId) -> String {
    short_agent_session_suffix_from_str(&id.to_string())
}

fn short_agent_session_suffix_from_str(value: &str) -> String {
    let raw = value.strip_prefix("agt_").unwrap_or(value);
    let token = raw.split('_').next().unwrap_or(raw);
    let prefix = token
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .take(10)
        .collect::<String>();
    let prefix = if prefix.is_empty() {
        folder_session_slug(token)
            .chars()
            .take(10)
            .collect::<String>()
    } else {
        prefix
    };
    let prefix = if prefix.is_empty() {
        "session".to_string()
    } else {
        prefix
    };
    let digest = Sha256::digest(value.as_bytes());
    let digest = format!("{digest:x}");
    format!("{}-{}", prefix, &digest[..4])
}

fn folder_session_repo_sort_key(session: &FolderSessionSummary) -> String {
    session
        .repo_path
        .as_deref()
        .unwrap_or("~")
        .to_ascii_lowercase()
}

fn folder_session_recency_sort_key(session: &FolderSessionSummary) -> i64 {
    session
        .updated_at
        .as_deref()
        .and_then(parse_session_list_datetime_ms)
        .or(session.modified_at_ms)
        .unwrap_or(0)
}

fn folder_session_summary_preview(path: &Path) -> Option<String> {
    let summary = fs::read_to_string(path.join("summary.md")).ok()?;
    let preview = summary
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .chars()
        .take(80)
        .collect::<String>();
    Some(preview)
}

fn group_folder_session_summaries(sessions: &[FolderSessionSummary]) -> Vec<FolderSessionGroup> {
    let mut groups = Vec::<FolderSessionGroup>::new();
    for session in sessions {
        let repo = folder_session_repo_label(session);
        if let Some(group) = groups.last_mut().filter(|group| group.repo == repo) {
            group.sessions.push(session.clone());
        } else {
            groups.push(FolderSessionGroup {
                repo,
                sessions: vec![session.clone()],
            });
        }
    }
    groups
}

fn folder_session_repo_label(session: &FolderSessionSummary) -> String {
    session
        .repo_path
        .as_deref()
        .map(short_folder_session_path)
        .unwrap_or_else(|| "-".to_string())
}

fn load_folder_native_agent_session(
    session_dir: &Path,
    id: &AgentSessionId,
) -> Option<AgentSession> {
    folder_agent_session_store(session_dir)
        .load_session(id)
        .ok()
        .or_else(|| agent_session_store().load_session(id).ok())
}

fn agent_session_store_for_folder_session(
    session_dir: &Path,
    id: &AgentSessionId,
) -> JsonlAgentSessionStore {
    let folder_store = folder_agent_session_store(session_dir);
    if folder_store.load_session(id).is_ok() {
        folder_store
    } else {
        agent_session_store()
    }
}

fn relocate_agent_session_into_folder(
    source_store: &JsonlAgentSessionStore,
    session_dir: &Path,
    id: &AgentSessionId,
) -> Result<JsonlAgentSessionStore> {
    let folder_store = folder_agent_session_store(session_dir);
    let target_path = folder_store.session_file_path(id);
    if target_path.exists() {
        return Ok(folder_store);
    }

    let source_path = source_store.session_file_path(id);
    if !source_path.exists() {
        source_store
            .load_session(id)
            .with_context(|| format!("loading agent session {id} before moving into folder"))?;
        bail!(
            "agent session {id} exists but its JSONL path is missing: {}",
            source_path.display()
        );
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating native session directory {}", parent.display()))?;
    }
    fs::rename(&source_path, &target_path).or_else(|rename_error| {
        fs::copy(&source_path, &target_path).with_context(|| {
            format!(
                "copying agent session {} to {} after rename failed: {rename_error}",
                source_path.display(),
                target_path.display()
            )
        })?;
        fs::remove_file(&source_path).with_context(|| {
            format!(
                "removing original agent session {} after copying to {}",
                source_path.display(),
                target_path.display()
            )
        })?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(folder_store)
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn latest_agent_session_event_created_at(session: &AgentSession) -> Option<String> {
    session
        .events
        .iter()
        .rev()
        .map(|event| event.created_at.trim().to_string())
        .find(|created_at| !created_at.is_empty())
}

fn folder_session_modified_at(path: &Path) -> Option<String> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(|modified| {
            let datetime: chrono::DateTime<chrono::Local> = modified.into();
            datetime.to_rfc3339()
        })
}

fn folder_session_modified_at_ms(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

fn parse_session_list_datetime_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|datetime| datetime.timestamp_millis())
}

fn format_folder_session_ls(report: &SessionLsReport) -> String {
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
            "  {:<20} {:<12} {:>5}  {:<32} {}",
            "UPDATED", "STATE", "TURNS", "NAME", "SUMMARY"
        ));
        lines.push(format!("  {}", "-".repeat(92)));
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
                "  {:<20} {:<12} {:>5}  {:<32} {}",
                truncate_table_cell(&updated, 20),
                truncate_table_cell(&state, 12),
                session.turn_count,
                truncate_table_cell(
                    &format!(
                        "{}{}",
                        session.reference_name,
                        if session.manifest_exists {
                            ""
                        } else {
                            " (no manifest)"
                        }
                    ),
                    32,
                ),
                summary
            ));
        }
    }
    lines.push(format!(
        "\nTotal: {} folder sessions",
        report.sessions.len()
    ));
    lines.push(String::new());
    lines.join("\n")
}

fn folder_session_summary_state_label(session: &FolderSessionSummary) -> String {
    session
        .lifecycle
        .mode
        .as_deref()
        .map(|mode| format!("{}/{}", session.lifecycle.state, mode))
        .unwrap_or_else(|| session.lifecycle.state.clone())
}

fn format_session_shorten_names_report(report: &SessionShortenNamesReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Cache folder sessions: {}", report.root));
    if report.dry_run {
        lines.push("Dry run: no folders renamed.".to_string());
    }
    if report.renamed.is_empty() {
        lines.push("No legacy long folder names to shorten.".to_string());
    } else {
        lines.push(format!(
            "{} folder name{}:",
            if report.dry_run {
                "Would rename"
            } else {
                "Renamed"
            },
            plural_suffix(report.renamed.len())
        ));
        for entry in &report.renamed {
            let from = Path::new(&entry.from)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&entry.from);
            let to = Path::new(&entry.to)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&entry.to);
            lines.push(format!("  {from} -> {to}"));
        }
    }
    if !report.skipped.is_empty() {
        lines.push("Skipped:".to_string());
        for skipped in &report.skipped {
            lines.push(format!("  {}: {}", skipped.path, skipped.reason));
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

fn short_folder_session_path(value: &str) -> String {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(value)
        .to_string()
}

fn truncate_table_cell(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let mut truncated = value.chars().take(max_chars - 1).collect::<String>();
    truncated.push('…');
    truncated
}

fn compact_session_list_datetime(value: &str) -> String {
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

fn resolve_folder_session_open_target(dir: &Path, target: SessionOpenTarget) -> Result<PathBuf> {
    let session_dir = resolve_session_dir(dir)?;
    let path = match target {
        SessionOpenTarget::Summary => session_dir.join("summary.md"),
        SessionOpenTarget::Request => session_dir.join("request.md"),
        SessionOpenTarget::Context => session_dir.join("context"),
        SessionOpenTarget::Compacted => session_dir.join("context/compacted.md"),
        SessionOpenTarget::Turns => session_dir.join("turns"),
        SessionOpenTarget::Manifest => session_dir.join("djinn.toml"),
        SessionOpenTarget::Repo => resolve_folder_session_repo_open_target(&session_dir)?,
    };
    Ok(path)
}

fn resolve_folder_session_repo_open_target(session_dir: &Path) -> Result<PathBuf> {
    let manifest = read_folder_session_manifest(session_dir)?;
    if let Some(manifest) = manifest {
        if let Some(repo_path) = manifest
            .repo_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            return Ok(PathBuf::from(repo_path));
        }
        if let Some(repo_link) = manifest
            .repo_link
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = PathBuf::from(repo_link);
            return Ok(if path.is_absolute() {
                path
            } else {
                session_dir.join(path)
            });
        }
    }
    let context_dir = session_dir.join("context");
    if context_dir.is_dir() {
        let mut symlink_dirs = Vec::new();
        for entry in fs::read_dir(&context_dir).with_context(|| {
            format!(
                "reading session context directory {}",
                context_dir.display()
            )
        })? {
            let entry = entry?;
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink()
                && fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir())
            {
                symlink_dirs.push(path);
            }
        }
        if symlink_dirs.len() == 1 {
            return Ok(symlink_dirs.remove(0));
        }
    }
    bail!(
        "session has no repo target in djinn.toml or unique context symlink: {}",
        session_dir.display()
    )
}

fn remove_folder_session(dir: &Path) -> Result<SessionRmReport> {
    remove_folder_session_with_store(dir, &agent_session_store())
}

fn remove_folder_session_with_store(
    dir: &Path,
    store: &JsonlAgentSessionStore,
) -> Result<SessionRmReport> {
    let named_reference = is_named_folder_session_reference(dir);
    let session_dir = resolve_session_dir(dir)?;
    if !session_dir.exists() {
        bail!("folder session does not exist: {}", session_dir.display());
    }
    if !session_dir.is_dir() {
        bail!(
            "folder session path is not a directory: {}",
            session_dir.display()
        );
    }
    let manifest_exists = session_dir.join("djinn.toml").exists();
    if !manifest_exists && !named_reference_under_cache_root(&session_dir) && !named_reference {
        bail!(
            "refusing to remove explicit directory without djinn.toml: {}",
            session_dir.display()
        );
    }
    let session_id = session_id_from_session_dir(&session_dir)?;
    let removed_native_session = if let Some(id) = &session_id {
        let folder_store = folder_agent_session_store(&session_dir);
        if folder_store.load_session(id).is_ok() {
            true
        } else if store.load_session(id).is_ok() {
            store.delete_session(id)?;
            true
        } else {
            false
        }
    } else {
        false
    };
    fs::remove_dir_all(&session_dir)
        .with_context(|| format!("removing folder session {}", session_dir.display()))?;
    Ok(SessionRmReport {
        session_dir: session_dir.display().to_string(),
        removed_folder: true,
        session_id: session_id.map(|id| id.to_string()),
        removed_native_session,
    })
}

fn named_reference_under_cache_root(path: &Path) -> bool {
    let root = default_folder_session_root();
    path.parent().is_some_and(|parent| parent == root)
}

fn resolve_existing_folder_session_dir(dir: &Path) -> Result<PathBuf> {
    let session_dir = resolve_session_dir(dir)?;
    if !session_dir.exists() {
        bail!(
            "folder session does not exist: {} (run `djinn session init {}` first)",
            session_dir.display(),
            dir.display()
        );
    }
    if !session_dir.is_dir() {
        bail!(
            "folder session path is not a directory: {}",
            session_dir.display()
        );
    }
    Ok(session_dir)
}

fn list_folder_session_context(session: &Path) -> Result<SessionContextLsReport> {
    let session_dir = resolve_existing_folder_session_dir(session)?;
    let context_dir = session_dir.join("context");
    let entries = inspect_folder_session_context_entries(&context_dir)?;
    Ok(SessionContextLsReport {
        session_dir: session_dir.display().to_string(),
        context_dir: context_dir.display().to_string(),
        entries,
    })
}

fn add_folder_session_context_entry(
    args: &SessionContextAddArgs,
) -> Result<SessionContextAddReport> {
    let session_dir = resolve_existing_folder_session_dir(&args.session)?;
    let context_dir = session_dir.join("context");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    let target = args
        .path
        .canonicalize()
        .with_context(|| format!("resolving context source {}", args.path.display()))?;
    let name = match args.name.as_deref() {
        Some(name) => validate_context_entry_name(name)?.to_string(),
        None => target
            .file_name()
            .and_then(|name| name.to_str())
            .map(validate_context_entry_name)
            .transpose()?
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow!(
                    "context source has no usable basename: {}",
                    target.display()
                )
            })?,
    };
    let link_path = context_dir.join(&name);
    let replaced = replace_existing_context_entry_if_needed(&link_path, args.force)?;
    create_context_symlink(&target, &link_path)?;
    Ok(SessionContextAddReport {
        session_dir: session_dir.display().to_string(),
        context_dir: context_dir.display().to_string(),
        name,
        path: link_path.display().to_string(),
        target: target.display().to_string(),
        replaced,
    })
}

fn remove_folder_session_context_entry(
    session: &Path,
    name: &str,
) -> Result<SessionContextRmReport> {
    let session_dir = resolve_existing_folder_session_dir(session)?;
    let context_dir = session_dir.join("context");
    let name = validate_context_entry_name(name)?.to_string();
    let path = context_dir.join(&name);
    if fs::symlink_metadata(&path).is_err() {
        bail!("context entry does not exist: {}", path.display());
    }
    remove_context_entry_path(&path)?;
    Ok(SessionContextRmReport {
        session_dir: session_dir.display().to_string(),
        context_dir: context_dir.display().to_string(),
        name,
        path: path.display().to_string(),
        removed: true,
    })
}

fn discover_folder_session_context(
    session: &Path,
    dry_run: bool,
) -> Result<SessionContextDiscoverReport> {
    let session_dir = resolve_existing_folder_session_dir(session)?;
    let context_dir = session_dir.join("context");
    if !dry_run {
        fs::create_dir_all(&context_dir)
            .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    }
    let repo = resolve_folder_session_repo_open_target(&session_dir)?
        .canonicalize()
        .with_context(|| "resolving discovered repo path")?;
    let mut warnings = Vec::new();
    let mut links = Vec::new();
    let mut indexed = Vec::new();
    let mut ignored = Vec::new();

    let mut link_specs = discover_repo_context_link_specs(&repo, &mut indexed, &mut ignored)?;
    link_specs.sort_by(|left, right| left.context_path.cmp(&right.context_path));
    link_specs.dedup_by(|left, right| left.context_path == right.context_path);
    for spec in link_specs {
        links.push(apply_discovered_context_link(
            &context_dir,
            &spec,
            dry_run,
            &mut warnings,
        )?);
    }

    collect_repo_index_entries(&repo, &mut indexed, &mut ignored)?;
    indexed.sort_by(|left, right| left.path.cmp(&right.path));
    indexed.dedup_by(|left, right| left.path == right.path);
    ignored.sort();
    ignored.dedup();

    let repo_index_path = context_dir.join("repo-index.md");
    let repo_index = render_context_discovery_repo_index(&repo, &links, &indexed, &ignored);
    let repo_index_written = if dry_run {
        false
    } else {
        fs::write(&repo_index_path, repo_index)
            .with_context(|| format!("writing {}", repo_index_path.display()))?;
        true
    };

    Ok(SessionContextDiscoverReport {
        session_dir: session_dir.display().to_string(),
        context_dir: context_dir.display().to_string(),
        repo: repo.display().to_string(),
        dry_run,
        links,
        indexed,
        ignored,
        warnings,
        repo_index_path: repo_index_path.display().to_string(),
        repo_index_written,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextDiscoveryLinkSpec {
    source: String,
    context_path: PathBuf,
    target: PathBuf,
    reason: String,
}

fn discover_repo_context_link_specs(
    repo: &Path,
    indexed: &mut Vec<SessionContextDiscoverIndexEntry>,
    ignored: &mut Vec<String>,
) -> Result<Vec<ContextDiscoveryLinkSpec>> {
    let mut specs = Vec::new();
    for relative in [
        "AGENTS.md",
        "README.md",
        "CLAUDE.md",
        ".github/copilot-instructions.md",
        ".cursorrules",
        "opencode.json",
        "opencode.jsonc",
    ] {
        push_context_discovery_link_if_exists(
            repo,
            relative,
            Path::new(relative)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(relative),
            "built-in breadcrumb",
            &mut specs,
        )?;
    }
    discover_opencode_config_context(repo, &mut specs, indexed)?;
    discover_simple_markdown_links(
        repo,
        Path::new(".opencode/commands"),
        "opencode-command",
        "opencode command",
        &mut specs,
        ignored,
    )?;
    discover_opencode_skill_links(repo, Path::new(".opencode/skills"), &mut specs, ignored)?;
    discover_simple_markdown_links(
        repo,
        Path::new(".github/instructions"),
        "copilot-instruction",
        "copilot instruction",
        &mut specs,
        ignored,
    )?;
    discover_simple_markdown_links(
        repo,
        Path::new(".github/prompts"),
        "copilot-prompt",
        "copilot prompt",
        &mut specs,
        ignored,
    )?;
    Ok(specs)
}

fn push_context_discovery_link_if_exists(
    repo: &Path,
    relative: &str,
    context_name: &str,
    reason: &str,
    specs: &mut Vec<ContextDiscoveryLinkSpec>,
) -> Result<()> {
    let target = repo.join(relative);
    if target.is_file() {
        specs.push(ContextDiscoveryLinkSpec {
            source: relative.to_string(),
            context_path: PathBuf::from(validate_context_entry_name(context_name)?),
            target: target.canonicalize()?,
            reason: reason.to_string(),
        });
    }
    Ok(())
}

fn discover_opencode_config_context(
    repo: &Path,
    specs: &mut Vec<ContextDiscoveryLinkSpec>,
    indexed: &mut Vec<SessionContextDiscoverIndexEntry>,
) -> Result<()> {
    for config_name in ["opencode.json", "opencode.jsonc"] {
        let path = repo.join(config_name);
        if !path.is_file() {
            continue;
        }
        let Ok(value) = read_json_or_jsonc_value(&path) else {
            continue;
        };
        if let Some(instructions) = value.get("instructions").and_then(|value| value.as_array()) {
            for instruction in instructions.iter().filter_map(|value| value.as_str()) {
                let relative = instruction.trim_start_matches("./");
                let target = repo.join(relative);
                if target.is_file() {
                    let name = target
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("instruction.md");
                    specs.push(ContextDiscoveryLinkSpec {
                        source: relative.to_string(),
                        context_path: PathBuf::from(validate_context_entry_name(name)?),
                        target: target.canonicalize()?,
                        reason: format!("{config_name} instructions"),
                    });
                }
            }
        }
        if let Some(paths) = value
            .get("skills")
            .and_then(|skills| skills.get("paths"))
            .and_then(|paths| paths.as_array())
        {
            for path in paths.iter().filter_map(|value| value.as_str()) {
                discover_opencode_skill_links(repo, Path::new(path), specs, &mut Vec::new())?;
            }
        }
        indexed.push(SessionContextDiscoverIndexEntry {
            source: "opencode".to_string(),
            path: config_name.to_string(),
            title: Some("OpenCode config".to_string()),
            reason: "harness config".to_string(),
        });
    }
    Ok(())
}

fn read_json_or_jsonc_value(path: &Path) -> Result<Value> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&content)
        .or_else(|_| serde_json::from_str(&strip_jsonc_line_comments(&content)))
        .with_context(|| format!("parsing {}", path.display()))
}

fn strip_jsonc_line_comments(content: &str) -> String {
    let mut output = String::new();
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }
        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for comment_ch in chars.by_ref() {
                        if comment_ch == '\n' {
                            output.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for comment_ch in chars.by_ref() {
                        if previous == '*' && comment_ch == '/' {
                            break;
                        }
                        previous = comment_ch;
                    }
                    continue;
                }
                _ => {}
            }
        }
        output.push(ch);
    }
    output
}

fn discover_simple_markdown_links(
    repo: &Path,
    source_dir: &Path,
    context_prefix: &str,
    reason: &str,
    specs: &mut Vec<ContextDiscoveryLinkSpec>,
    ignored: &mut Vec<String>,
) -> Result<()> {
    let root = repo.join(source_dir);
    if !root.is_dir() {
        return Ok(());
    }
    for path in collect_markdown_files_under(repo, source_dir, ignored)? {
        let relative = path.strip_prefix(repo).unwrap_or(&path).to_path_buf();
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("context");
        let name = format!("{}-{}.md", context_prefix, folder_session_slug(stem));
        specs.push(ContextDiscoveryLinkSpec {
            source: relative.display().to_string(),
            context_path: PathBuf::from(validate_context_entry_name(&name)?),
            target: path.canonicalize()?,
            reason: reason.to_string(),
        });
    }
    Ok(())
}

fn discover_opencode_skill_links(
    repo: &Path,
    skills_dir: &Path,
    specs: &mut Vec<ContextDiscoveryLinkSpec>,
    ignored: &mut Vec<String>,
) -> Result<()> {
    let root = repo.join(skills_dir);
    if !root.is_dir() || is_excluded_repo_relative_path(skills_dir) {
        return Ok(());
    }
    let mut entries = fs::read_dir(&root)
        .with_context(|| format!("reading skills directory {}", root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let skill_dir = entry.path();
        if !skill_dir.is_dir() {
            continue;
        }
        let relative_dir = skill_dir.strip_prefix(repo).unwrap_or(&skill_dir);
        if is_excluded_repo_relative_path(relative_dir) {
            ignored.push(relative_dir.display().to_string());
            continue;
        }
        let skill = skill_dir.join("SKILL.md");
        if skill.is_file() {
            let skill_name = skill_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("skill");
            specs.push(ContextDiscoveryLinkSpec {
                source: skill
                    .strip_prefix(repo)
                    .unwrap_or(&skill)
                    .display()
                    .to_string(),
                context_path: PathBuf::from(validate_context_entry_name(&format!(
                    "opencode-skill-{}.md",
                    folder_session_slug(skill_name)
                ))?),
                target: skill.canonicalize()?,
                reason: "opencode skill".to_string(),
            });
        }
    }
    Ok(())
}

fn collect_repo_index_entries(
    repo: &Path,
    indexed: &mut Vec<SessionContextDiscoverIndexEntry>,
    ignored: &mut Vec<String>,
) -> Result<()> {
    for base in [
        Path::new("docs"),
        Path::new("shadow/docs"),
        Path::new("tests"),
    ] {
        for path in collect_markdown_files_under(repo, base, ignored)? {
            let relative = path
                .strip_prefix(repo)
                .unwrap_or(&path)
                .display()
                .to_string();
            indexed.push(SessionContextDiscoverIndexEntry {
                source: "repo-docs".to_string(),
                title: markdown_title(&path)?,
                path: relative,
                reason: "repo documentation index".to_string(),
            });
        }
    }
    Ok(())
}

fn collect_markdown_files_under(
    repo: &Path,
    base: &Path,
    ignored: &mut Vec<String>,
) -> Result<Vec<PathBuf>> {
    let root = repo.join(base);
    let mut files = Vec::new();
    collect_markdown_files_recursive(repo, &root, &mut files, ignored)?;
    files.sort();
    Ok(files)
}

fn collect_markdown_files_recursive(
    repo: &Path,
    path: &Path,
    files: &mut Vec<PathBuf>,
    ignored: &mut Vec<String>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let relative = path.strip_prefix(repo).unwrap_or(path);
    if is_excluded_repo_relative_path(relative) {
        ignored.push(relative.display().to_string());
        return Ok(());
    }
    if path.is_file() {
        if is_markdown_path(path) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !path.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("reading repo context path {}", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        collect_markdown_files_recursive(repo, &entry.path(), files, ignored)?;
    }
    Ok(())
}

fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("md" | "markdown")
    )
}

fn is_excluded_repo_relative_path(path: &Path) -> bool {
    let text = path.display().to_string();
    if text.starts_with(".env") || text.ends_with(".db") {
        return true;
    }
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        matches!(
            name.to_str(),
            Some(".git" | ".venv" | "node_modules" | ".pytest_cache" | ".ruff_cache")
        )
    })
}

fn markdown_title(path: &Path) -> Result<Option<String>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(content.lines().find_map(|line| {
        line.trim()
            .strip_prefix("# ")
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
    }))
}

fn apply_discovered_context_link(
    context_dir: &Path,
    spec: &ContextDiscoveryLinkSpec,
    dry_run: bool,
    warnings: &mut Vec<String>,
) -> Result<SessionContextDiscoverLink> {
    let path = context_dir.join(&spec.context_path);
    if let Some(parent) = path.parent() {
        if !dry_run {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating context directory {}", parent.display()))?;
        }
    }
    let mut existed = false;
    let mut created = false;
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        existed = true;
        if metadata.file_type().is_symlink()
            && fs::read_link(&path)
                .ok()
                .and_then(|target| {
                    if target.is_absolute() {
                        Some(target)
                    } else {
                        path.parent().map(|parent| parent.join(target))
                    }
                })
                .and_then(|target| target.canonicalize().ok())
                .as_deref()
                == Some(spec.target.as_path())
        {
            // Already linked to the desired target.
        } else {
            warnings.push(format!(
                "context path already exists and was not replaced: {}",
                path.display()
            ));
        }
    } else if !dry_run {
        create_context_symlink(&spec.target, &path)?;
        created = true;
    } else {
        created = false;
    }
    Ok(SessionContextDiscoverLink {
        source: spec.source.clone(),
        name: spec
            .context_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("context")
            .to_string(),
        path: path.display().to_string(),
        target: spec.target.display().to_string(),
        existed,
        created,
        reason: spec.reason.clone(),
    })
}

fn render_context_discovery_repo_index(
    repo: &Path,
    links: &[SessionContextDiscoverLink],
    indexed: &[SessionContextDiscoverIndexEntry],
    ignored: &[String],
) -> String {
    let mut output = String::new();
    output.push_str("# Repo context index\n\n");
    output.push_str(&format!("Repo: `{}`\n\n", repo.display()));
    output.push_str("## Linked context\n\n");
    if links.is_empty() {
        output.push_str("No high-signal context links discovered.\n\n");
    } else {
        for link in links {
            output.push_str(&format!("- `{}` — {}\n", link.source, link.reason));
        }
        output.push('\n');
    }
    output.push_str("## Indexed references\n\n");
    if indexed.is_empty() {
        output.push_str("No repo documentation references discovered.\n\n");
    } else {
        for entry in indexed {
            let title = entry
                .title
                .as_ref()
                .map(|title| format!(" — {title}"))
                .unwrap_or_default();
            output.push_str(&format!("- `{}`{} ({})\n", entry.path, title, entry.reason));
        }
        output.push('\n');
    }
    if !ignored.is_empty() {
        output.push_str("## Ignored\n\n");
        for path in ignored {
            output.push_str(&format!("- `{path}`\n"));
        }
        output.push('\n');
    }
    output
}

fn validate_context_entry_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || matches!(name, "." | "..") {
        bail!("context entry name cannot be empty, `.` or `..`");
    }
    let path = Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        bail!("context entry name must be a single path component: {name}");
    }
    Ok(name)
}

fn replace_existing_context_entry_if_needed(path: &Path, force: bool) -> Result<bool> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(false);
    }
    if !force {
        bail!(
            "context entry already exists: {} (use --force to replace)",
            path.display()
        );
    }
    remove_context_entry_path(path)?;
    Ok(true)
}

fn remove_context_entry_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading context entry metadata {}", path.display()))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path).with_context(|| format!("removing context entry {}", path.display()))
    } else if metadata.is_dir() {
        fs::remove_dir_all(path)
            .with_context(|| format!("removing context directory {}", path.display()))
    } else {
        bail!(
            "context entry is not a file, directory, or symlink: {}",
            path.display()
        )
    }
}

fn inspect_folder_session_context_entries(
    context_dir: &Path,
) -> Result<Vec<SessionContextEntryReport>> {
    if !context_dir.exists() {
        return Ok(Vec::new());
    }
    if !context_dir.is_dir() {
        bail!("context path is not a directory: {}", context_dir.display());
    }
    let mut entries = fs::read_dir(context_dir)
        .with_context(|| {
            format!(
                "reading session context directory {}",
                context_dir.display()
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    entries
        .into_iter()
        .map(|entry| inspect_folder_session_context_entry(&entry.path()))
        .collect()
}

fn inspect_folder_session_context_entry(path: &Path) -> Result<SessionContextEntryReport> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("context")
        .to_string();
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading context entry metadata {}", path.display()))?;
    let symlink = metadata.file_type().is_symlink();
    let target = symlink.then(|| fs::read_link(path).ok()).flatten();
    let target_metadata = fs::metadata(path).ok();
    let broken = symlink && target_metadata.is_none();
    let kind = context_entry_kind(&metadata, target_metadata.as_ref());
    let bytes = if metadata.is_file() {
        Some(metadata.len())
    } else {
        target_metadata
            .as_ref()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
    };
    let mut skipped = Vec::new();
    let ingestible = if broken {
        skipped.push(format!("context/{name}: broken symlink"));
        false
    } else {
        read_folder_session_context_file(path, &format!("context/{name}"), &mut skipped)?.is_some()
    };
    Ok(SessionContextEntryReport {
        name,
        path: path.display().to_string(),
        kind,
        symlink,
        target: target.map(|target| target.display().to_string()),
        broken,
        ingestible,
        skip_reason: skipped.into_iter().next(),
        bytes,
    })
}

fn context_entry_kind(metadata: &fs::Metadata, target_metadata: Option<&fs::Metadata>) -> String {
    if metadata.file_type().is_symlink() {
        if let Some(target_metadata) = target_metadata {
            if target_metadata.is_dir() {
                "symlink_dir"
            } else if target_metadata.is_file() {
                "symlink_file"
            } else {
                "symlink_other"
            }
        } else {
            "symlink_broken"
        }
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    }
    .to_string()
}

fn inspect_folder_session_context_dir(context_dir: &Path) -> Result<(usize, Vec<String>)> {
    if !context_dir.is_dir() {
        return Ok((0, Vec::new()));
    }
    let mut entries = fs::read_dir(context_dir)
        .with_context(|| {
            format!(
                "reading session context directory {}",
                context_dir.display()
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    let mut count = 0;
    let mut skipped = Vec::new();
    for entry in entries {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("context")
            .to_string();
        if read_folder_session_context_file(&path, &format!("context/{name}"), &mut skipped)?
            .is_some()
        {
            count += 1;
        }
    }
    Ok((count, skipped))
}

fn session_status_repo(
    session_dir: &Path,
    manifest: &FolderSessionManifest,
) -> Option<SessionStatusRepoReport> {
    if manifest.repo_path.is_none() && manifest.repo_link.is_none() {
        return None;
    }
    let link_path = manifest
        .repo_link
        .as_ref()
        .map(|link| PathBuf::from(link))
        .map(|link| {
            if link.is_absolute() {
                link
            } else {
                session_dir.join(link)
            }
        });
    let (link_exists, link_is_symlink, link_target, link_broken) = link_path
        .as_ref()
        .map(|link| match fs::symlink_metadata(link) {
            Ok(metadata) => {
                let is_symlink = metadata.file_type().is_symlink();
                let target = fs::read_link(link)
                    .ok()
                    .map(|target| target.display().to_string());
                let broken = is_symlink && fs::metadata(link).is_err();
                (true, is_symlink, target, broken)
            }
            Err(_) => (false, false, None, false),
        })
        .unwrap_or((false, false, None, false));
    Some(SessionStatusRepoReport {
        path: manifest.repo_path.clone(),
        link: link_path.map(|path| path.display().to_string()),
        link_exists,
        link_is_symlink,
        link_target,
        link_broken,
    })
}

fn format_folder_session_status(report: &SessionStatusReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Djinn session: {}", report.session_dir));
    lines.push(format!("Manifest: {}", yes_no(report.manifest_exists)));
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
            lines.push(format!("  link exists: {}", yes_no(repo.link_exists)));
            lines.push(format!("  link symlink: {}", yes_no(repo.link_is_symlink)));
            if let Some(target) = &repo.link_target {
                lines.push(format!("  target: {target}"));
            }
            lines.push(format!("  broken: {}", yes_no(repo.link_broken)));
        }
    }
    lines.push("Files:".to_string());
    lines.push(format!("  request.md: {}", yes_no(report.files.request_md)));
    lines.push(format!("  summary.md: {}", yes_no(report.files.summary_md)));
    lines.push(format!("  context/: {}", yes_no(report.files.context_dir)));
    lines.push(format!(
        "  context/compacted.md: {}",
        yes_no(report.files.compacted_md)
    ));
    lines.push(format!("  turns/: {}", yes_no(report.files.turns_dir)));
    lines.push(format!("Turns: {}", report.turn_count));
    if let Some(turn) = &report.latest_turn {
        lines.push("Latest turn:".to_string());
        lines.push(format!("  id: {}", turn.id));
        if let Some(request_path) = &turn.request_path {
            lines.push(format!("  request: {request_path}"));
        }
        if let Some(response_path) = &turn.response_path {
            lines.push(format!("  response: {response_path}"));
        }
        lines.push(format!("  has response: {}", yes_no(turn.has_response)));
    }
    if let Some(candidates) = &report.candidates {
        lines.push("Candidates:".to_string());
        lines.push(format!(
            "  status: {}",
            format_session_candidate_status(candidates)
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
                lines.push(format!("    - {}", format_session_candidate_entry(entry)));
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

fn format_folder_session_context_ls(report: &SessionContextLsReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Session context: {}", report.context_dir));
    if report.entries.is_empty() {
        lines.push("No context entries found.".to_string());
        lines.push(String::new());
        return lines.join("\n");
    }
    lines.push(format!(
        "  {:<28} {:<14} {:<10} {}",
        "NAME", "KIND", "INGEST", "TARGET / REASON"
    ));
    lines.push(format!("  {}", "-".repeat(86)));
    for entry in &report.entries {
        let ingest = if entry.ingestible { "yes" } else { "no" };
        let detail = entry
            .target
            .as_deref()
            .or(entry.skip_reason.as_deref())
            .unwrap_or("");
        lines.push(format!(
            "  {:<28} {:<14} {:<10} {}",
            truncate_table_cell(&entry.name, 28),
            truncate_table_cell(&entry.kind, 14),
            ingest,
            detail
        ));
        if entry.target.is_some() && entry.skip_reason.is_some() {
            lines.push(format!(
                "  {:<28} {:<14} {:<10} {}",
                "",
                "",
                "",
                entry.skip_reason.as_deref().unwrap_or("")
            ));
        }
    }
    lines.push(format!("\nTotal: {} context entries", report.entries.len()));
    lines.push(String::new());
    lines.join("\n")
}

fn format_folder_session_context_discover(report: &SessionContextDiscoverReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} session context from repo: {}",
        if report.dry_run {
            "Discovered"
        } else {
            "Updated"
        },
        report.repo
    ));
    lines.push(format!("Session: {}", report.session_dir));
    lines.push(format!("Context: {}", report.context_dir));
    lines.push(format!(
        "Repo index: {}{}",
        report.repo_index_path,
        if report.repo_index_written {
            " (written)"
        } else if report.dry_run {
            " (dry-run)"
        } else {
            ""
        }
    ));
    if !report.links.is_empty() {
        lines.push("Links:".to_string());
        for link in &report.links {
            let action = if link.created {
                "created"
            } else if link.existed {
                "exists"
            } else if report.dry_run {
                "would create"
            } else {
                "skipped"
            };
            lines.push(format!(
                "  - {action}: {} -> {} ({})",
                link.path, link.target, link.reason
            ));
        }
    } else {
        lines.push("Links: none".to_string());
    }
    if !report.indexed.is_empty() {
        lines.push("Indexed references:".to_string());
        for entry in &report.indexed {
            let title = entry
                .title
                .as_ref()
                .map(|title| format!(" — {title}"))
                .unwrap_or_default();
            lines.push(format!("  - {}{} ({})", entry.path, title, entry.reason));
        }
    }
    if !report.ignored.is_empty() {
        lines.push("Ignored:".to_string());
        for ignored in &report.ignored {
            lines.push(format!("  - {ignored}"));
        }
    }
    if !report.warnings.is_empty() {
        lines.push("Warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderSessionTurnDigest {
    id: String,
    request_path: Option<PathBuf>,
    response_path: Option<PathBuf>,
    request: Option<String>,
    response: Option<String>,
}

fn read_folder_session_turns(turns_dir: &Path) -> Result<Vec<FolderSessionTurnDigest>> {
    if !turns_dir.exists() {
        return Ok(Vec::new());
    }
    if !turns_dir.is_dir() {
        bail!("turns path is not a directory: {}", turns_dir.display());
    }
    let mut entries = fs::read_dir(turns_dir)
        .with_context(|| format!("reading turns directory {}", turns_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());

    let mut turns = Vec::new();
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("turn")
            .to_string();
        let request_path = path.join("request.md");
        let response_path = path.join("response.md");
        let request = read_optional_markdown_file(&request_path)?;
        let response = read_optional_markdown_file(&response_path)?;
        if request.is_none() && response.is_none() {
            continue;
        }
        turns.push(FolderSessionTurnDigest {
            id,
            request_path: request_path.exists().then_some(request_path),
            response_path: response_path.exists().then_some(response_path),
            request,
            response,
        });
    }
    Ok(turns)
}

fn read_optional_markdown_file(path: &Path) -> Result<Option<String>> {
    if !path.exists() || !path.is_file() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let content = content.trim_end().to_string();
    Ok((!content.trim().is_empty()).then_some(content))
}

fn render_folder_session_compaction_generated(
    session_dir: &Path,
    turns: &[FolderSessionTurnDigest],
) -> String {
    let mut output = String::new();
    output.push_str(&format!("Session: `{}`\n", session_dir.display()));
    output.push_str(&format!(
        "Generated: `{}`\n\n",
        chrono::Local::now().to_rfc3339()
    ));
    if turns.is_empty() {
        output.push_str("No turn files found under `turns/`.\n");
        return output;
    }
    output.push_str("## Turn digest\n\n");
    for turn in turns {
        output.push_str(&format!("### {}\n\n", turn.id));
        if let Some(request) = &turn.request {
            output.push_str("**Request**\n\n");
            output.push_str(&markdown_quote_block(&compact_text_snippet(
                request,
                FOLDER_SESSION_COMPACT_SNIPPET_CHARS,
            )));
            output.push_str("\n\n");
        }
        if let Some(response) = &turn.response {
            output.push_str("**Response**\n\n");
            output.push_str(&markdown_quote_block(&compact_text_snippet(
                response,
                FOLDER_SESSION_COMPACT_SNIPPET_CHARS,
            )));
            output.push_str("\n\n");
        }
        let mut links = Vec::new();
        if turn.request_path.is_some() {
            links.push(format!("[request](../turns/{}/request.md)", turn.id));
        }
        if turn.response_path.is_some() {
            links.push(format!("[response](../turns/{}/response.md)", turn.id));
        }
        if !links.is_empty() {
            output.push_str(&format!("Evidence: {}\n\n", links.join(", ")));
        }
    }
    output
}

fn merge_folder_session_compaction_document(existing: Option<&str>, generated: &str) -> String {
    let generated_block = format!(
        "{FOLDER_SESSION_COMPACT_START_MARKER}\n{}\n{FOLDER_SESSION_COMPACT_END_MARKER}",
        generated.trim_end()
    );
    let Some(existing) = existing else {
        return initial_folder_session_compaction_document(&generated_block);
    };
    if let Some(start) = existing.find(FOLDER_SESSION_COMPACT_START_MARKER) {
        if let Some(relative_end) = existing[start..].find(FOLDER_SESSION_COMPACT_END_MARKER) {
            let end = start + relative_end + FOLDER_SESSION_COMPACT_END_MARKER.len();
            let mut output = String::new();
            output.push_str(existing[..start].trim_end());
            output.push_str("\n");
            output.push_str(&generated_block);
            let suffix = existing[end..].trim_start_matches(|ch| ch == '\r' || ch == '\n');
            if !suffix.trim().is_empty() {
                output.push_str("\n\n");
                output.push_str(suffix.trim_end());
            }
            output.push('\n');
            return output;
        }
    }

    let mut output = existing.trim_end().to_string();
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str("## Generated digest\n\n");
    output.push_str(&generated_block);
    output.push('\n');
    output
}

fn initial_folder_session_compaction_document(generated_block: &str) -> String {
    format!(
        "# Compacted session context\n\n## User notes\n\nAdd durable facts, decisions, open questions, and edited summaries here. Djinn preserves this section when regenerating the digest.\n\n## Generated digest\n\n{generated_block}\n"
    )
}

fn compact_text_snippet(value: &str, max_chars: usize) -> String {
    let normalized = value
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    truncate(&normalized, max_chars)
}

fn markdown_quote_block(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn canonical_existing_dir(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving {label} {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("{label} is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

fn write_scaffold_file(
    path: &Path,
    content: &str,
    force: bool,
    created: &mut Vec<String>,
    skipped: &mut Vec<String>,
) -> Result<()> {
    if path.exists() && !force {
        skipped.push(path.display().to_string());
        return Ok(());
    }
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    created.push(path.display().to_string());
    Ok(())
}

fn session_context_readme(link_repo: Option<&PathBuf>, workspace: &Path) -> String {
    let mut output = String::new();
    output.push_str("# Djinn session context\n\n");
    output.push_str("Put durable working notes, decisions, and compacted evidence here. ");
    output.push_str("Djinn treats this folder as session-local context and does not blindly ingest linked folders.\n\n");
    output.push_str(
        "Precedence: global profile/config < repo-local config/context < session-local files.\n",
    );
    if let Some(repo) = link_repo {
        output.push_str(&format!(
            "\nLinked repo requested: `{}`\nResolved workspace: `{}`\n",
            repo.display(),
            workspace.display()
        ));
    }
    output
}

fn link_repo_into_session_context(
    context_dir: &Path,
    repo: &Path,
    force: bool,
    created: &mut Vec<String>,
    skipped: &mut Vec<String>,
) -> Result<SessionRepoLinkReport> {
    let repo_name = repo
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("repo");
    let link_path = context_dir.join(repo_name);
    if let Ok(metadata) = fs::symlink_metadata(&link_path) {
        if metadata.file_type().is_symlink() {
            if let Ok(existing_target) = fs::read_link(&link_path) {
                let existing_target = if existing_target.is_absolute() {
                    existing_target
                } else {
                    link_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(existing_target)
                };
                if existing_target.canonicalize().ok().as_deref() == Some(repo) && !force {
                    skipped.push(link_path.display().to_string());
                    return Ok(SessionRepoLinkReport {
                        path: link_path.display().to_string(),
                        target: repo.display().to_string(),
                    });
                }
            }
            if force {
                fs::remove_file(&link_path)
                    .with_context(|| format!("removing symlink {}", link_path.display()))?;
            } else {
                bail!(
                    "context link already exists and points elsewhere: {} (use --force to replace)",
                    link_path.display()
                );
            }
        } else if metadata.is_file() {
            if force {
                fs::remove_file(&link_path)
                    .with_context(|| format!("removing file {}", link_path.display()))?;
            } else {
                bail!(
                    "context path already exists: {} (use --force to replace files/symlinks)",
                    link_path.display()
                );
            }
        } else {
            bail!(
                "context path already exists and is not a symlink: {}",
                link_path.display()
            );
        }
    }
    create_dir_symlink(repo, &link_path)?;
    created.push(link_path.display().to_string());
    Ok(SessionRepoLinkReport {
        path: link_path.display().to_string(),
        target: repo.display().to_string(),
    })
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(not(any(unix, windows)))]
fn create_dir_symlink(_target: &Path, _link: &Path) -> Result<()> {
    bail!("directory symlinks are not supported on this platform")
}

#[cfg(unix)]
fn create_context_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(windows)]
fn create_context_symlink(target: &Path, link: &Path) -> Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
    .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(not(any(unix, windows)))]
fn create_context_symlink(_target: &Path, _link: &Path) -> Result<()> {
    bail!("context symlinks are not supported on this platform")
}

fn render_session_manifest(
    selection: &AgentRoleSelection,
    model: &str,
    workspace: &Path,
    repo_link: Option<&SessionRepoLinkReport>,
    config_sources: &[String],
) -> Result<String> {
    let mut output = String::new();
    output.push_str("version = 1\n");
    output.push_str(&format!(
        "created_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    output.push_str(&format!("profile = {}\n", toml_string(&selection.profile)?));
    if let Some(agent_name) = &selection.agent_name {
        output.push_str(&format!("agent = {}\n", toml_string(agent_name)?));
    }
    output.push_str(&format!("model = {}\n", toml_string(model)?));
    output.push_str(&format!(
        "workspace = {}\n\n",
        toml_string(&workspace.display().to_string())?
    ));
    output.push_str("[context]\n");
    output.push_str("path = \"context\"\n");
    output.push_str(
        "precedence = [\"global profile/config\", \"repo-local config/context\", \"session-local files\"]\n",
    );
    output.push_str(&format!(
        "config_sources = [{}]\n",
        config_sources
            .iter()
            .map(|source| toml_string(source))
            .collect::<Result<Vec<_>>>()?
            .join(", ")
    ));
    if let Some(repo_link) = repo_link {
        output.push_str("\n[context.repo]\n");
        output.push_str(&format!("path = {}\n", toml_string(&repo_link.target)?));
        output.push_str(&format!("link = {}\n", toml_string(&repo_link.path)?));
    }
    Ok(output)
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

fn resolve_agent_request_prompt(
    prompt: Option<String>,
    session_dir: Option<&Path>,
) -> Result<String> {
    if let Some(prompt) = prompt
        .map(|prompt| prompt.trim_end().to_string())
        .filter(|prompt| !prompt.trim().is_empty())
    {
        return Ok(prompt);
    }
    let Some(session_dir) = session_dir else {
        bail!("agent ask requires a prompt, or --session-dir containing request.md");
    };
    let request_path = session_dir.join("request.md");
    let prompt = fs::read_to_string(&request_path)
        .with_context(|| format!("reading request prompt from {}", request_path.display()))?;
    let prompt = prompt.trim_end().to_string();
    if prompt.trim().is_empty() {
        bail!("request prompt is empty: {}", request_path.display());
    }
    Ok(prompt)
}

fn resolve_session_dir(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("session name or directory path cannot be empty");
    }
    if is_named_folder_session_reference(path) {
        let root = default_folder_session_root();
        let direct = root.join(path);
        if direct.exists() {
            return Ok(direct);
        }
        if let Some(resolved) = resolve_folder_session_reference_name(&root, path)? {
            return Ok(resolved);
        }
        return Ok(direct);
    }
    Ok(path.to_path_buf())
}

fn resolve_folder_session_reference_name(root: &Path, path: &Path) -> Result<Option<PathBuf>> {
    let Some(reference) = path.to_str() else {
        return Ok(None);
    };
    if !root.is_dir() {
        return Ok(None);
    }
    let mut matches = Vec::new();
    let entries = fs::read_dir(root)
        .with_context(|| format!("reading folder session root {}", root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if folder_session_reference_name(name) == reference {
            matches.push(path);
        }
    }
    matches.sort();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => bail!(
            "ambiguous folder session reference `{reference}` matched {} sessions; use the full folder name or path",
            matches.len()
        ),
    }
}

fn default_folder_session_root() -> PathBuf {
    djinn_core::default_cache_dir().join("sessions")
}

fn auto_folder_session_dir(prompt: &str, id: &AgentSessionId) -> PathBuf {
    let title = prompt_title(prompt, "session");
    default_folder_session_root().join(format!(
        "{}-{}",
        folder_session_slug(&title),
        short_agent_session_suffix(id)
    ))
}

fn folder_session_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "session".to_string()
    } else {
        slug
    }
}

fn ensure_folder_session_readme(session_dir: &Path) -> Result<()> {
    let context_dir = session_dir.join("context");
    let readme_path = context_dir.join("djinn-context.md");
    if readme_path.exists() {
        return Ok(());
    }
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    fs::write(&readme_path, session_context_readme(None, Path::new("")))
        .with_context(|| format!("writing {}", readme_path.display()))
}

fn is_named_folder_session_reference(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentSessionDirProjection {
    session_dir: PathBuf,
    turn_dir: PathBuf,
    context_dir: PathBuf,
    summary_path: PathBuf,
    request_path: PathBuf,
}

fn project_agent_session_dir(
    session_dir: &Path,
    session: &AgentSession,
    prompt: &str,
    summary: &str,
) -> Result<AgentSessionDirProjection> {
    fs::create_dir_all(session_dir)
        .with_context(|| format!("creating session directory {}", session_dir.display()))?;
    let context_dir = session_dir.join("context");
    let turns_dir = session_dir.join("turns");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    fs::create_dir_all(&turns_dir)
        .with_context(|| format!("creating turns directory {}", turns_dir.display()))?;

    let summary_path = session_dir.join("summary.md");

    let turn_dir = turns_dir.join(agent_session_turn_dir_name());
    fs::create_dir_all(&turn_dir)
        .with_context(|| format!("creating turn directory {}", turn_dir.display()))?;

    let request_path = session_dir.join("request.md");
    fs::write(&request_path, ensure_trailing_newline(prompt))
        .with_context(|| format!("writing {}", request_path.display()))?;
    fs::write(turn_dir.join("request.md"), ensure_trailing_newline(prompt))
        .with_context(|| format!("writing turn request in {}", turn_dir.display()))?;
    fs::write(&summary_path, ensure_trailing_newline(summary))
        .with_context(|| format!("writing {}", summary_path.display()))?;
    fs::write(
        turn_dir.join("response.md"),
        ensure_trailing_newline(summary),
    )
    .with_context(|| format!("writing turn response in {}", turn_dir.display()))?;

    write_agent_session_toml(session_dir, session)?;

    Ok(AgentSessionDirProjection {
        session_dir: session_dir.to_path_buf(),
        turn_dir,
        context_dir,
        summary_path,
        request_path,
    })
}

fn write_agent_session_toml(session_dir: &Path, session: &AgentSession) -> Result<()> {
    let manifest_path = session_dir.join("djinn.toml");
    let preserved_context = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|content| preserve_manifest_context_sections(&content));
    let mut output = String::new();
    output.push_str(&format!(
        "session_id = {}\n",
        toml_string(&session.id.to_string())?
    ));
    if !session.meta.created_at.trim().is_empty() {
        output.push_str(&format!(
            "created_at = {}\n",
            toml_string(&session.meta.created_at)?
        ));
    }
    output.push_str(&format!("title = {}\n", toml_string(&session.meta.title)?));
    output.push_str(&format!(
        "workspace = {}\n",
        toml_string(&session.meta.workspace)?
    ));
    output.push_str(&format!(
        "profile = {}\n",
        toml_string(&session.meta.profile)?
    ));
    if let Some(runtime_config) = &session.meta.runtime_config {
        if !runtime_config.model.trim().is_empty() {
            output.push_str(&format!(
                "model = {}\n",
                toml_string(&runtime_config.model)?
            ));
        }
    }
    if let Some(agent_name) = &session.meta.agent_name {
        output.push_str(&format!("agent = {}\n", toml_string(agent_name)?));
    }
    output.push_str(&format!(
        "source = {}\n",
        toml_string(&session.meta.source)?
    ));
    if let Some(context) = preserved_context {
        output.push('\n');
        output.push_str(&context);
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    fs::write(&manifest_path, output)
        .with_context(|| format!("writing {}", manifest_path.display()))
}

fn preserve_manifest_context_sections(manifest: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut preserving = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[context") {
            preserving = true;
        }
        if preserving {
            lines.push(line.to_string());
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn toml_string(value: &str) -> Result<String> {
    serde_json::to_string(value).map_err(Into::into)
}

fn ensure_trailing_newline(value: &str) -> String {
    if value.ends_with('\n') {
        value.to_string()
    } else {
        format!("{value}\n")
    }
}

fn agent_session_turn_dir_name() -> String {
    format!(
        "{}-{}",
        chrono::Local::now().format("%Y%m%dT%H%M%S"),
        chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FolderSessionManifest {
    kind: Option<String>,
    session_id: Option<AgentSessionId>,
    created_at: Option<String>,
    promotion_type: Option<String>,
    profile: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    workspace: Option<String>,
    repo_path: Option<String>,
    repo_link: Option<String>,
}

fn read_folder_session_manifest(session_dir: &Path) -> Result<Option<FolderSessionManifest>> {
    let manifest_path = session_dir.join("djinn.toml");
    if !manifest_path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    Ok(Some(parse_folder_session_manifest(&content)))
}

fn parse_folder_session_manifest(manifest: &str) -> FolderSessionManifest {
    FolderSessionManifest {
        kind: manifest_root_string_value(manifest, "kind"),
        session_id: manifest_root_string_value(manifest, "session_id").map(AgentSessionId::new),
        created_at: manifest_root_string_value(manifest, "created_at"),
        promotion_type: manifest_root_string_value(manifest, "promotion_type")
            .or_else(|| manifest_section_string_value(manifest, "promotion", "type")),
        profile: manifest_root_string_value(manifest, "profile"),
        agent: manifest_root_string_value(manifest, "agent"),
        model: manifest_root_string_value(manifest, "model"),
        workspace: manifest_root_string_value(manifest, "workspace"),
        repo_path: manifest_section_string_value(manifest, "context.repo", "path"),
        repo_link: manifest_section_string_value(manifest, "context.repo", "link"),
    }
}

fn session_id_from_session_dir(session_dir: &Path) -> Result<Option<AgentSessionId>> {
    Ok(read_folder_session_manifest(session_dir)?.and_then(|manifest| manifest.session_id))
}

fn manifest_root_string_value(manifest: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} =");
    manifest.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('[') {
            return None;
        }
        let value = line.strip_prefix(&prefix)?.trim();
        parse_manifest_string_value(value)
    })
}

fn manifest_section_string_value(manifest: &str, section: &str, key: &str) -> Option<String> {
    let section_header = format!("[{section}]");
    let prefix = format!("{key} =");
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == section_header;
            continue;
        }
        if in_section {
            if let Some(value) = line.strip_prefix(&prefix) {
                return parse_manifest_string_value(value.trim());
            }
        }
    }
    None
}

fn parse_manifest_string_value(value: &str) -> Option<String> {
    serde_json::from_str::<String>(value)
        .ok()
        .or_else(|| Some(value.trim_matches('"').to_string()))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn session_manifest_workspace_path(manifest: Option<&FolderSessionManifest>) -> Option<PathBuf> {
    manifest
        .and_then(|manifest| manifest.workspace.as_ref().or(manifest.repo_path.as_ref()))
        .map(PathBuf::from)
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

fn resolve_folder_session_context_instructions(
    session_dir: Option<&Path>,
) -> Result<Vec<ResolvedAgentInstruction>> {
    let Some(session_dir) = session_dir else {
        return Ok(Vec::new());
    };
    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    let mut candidates = Vec::<(PathBuf, String)>::new();
    candidates.push((session_dir.join("request.md"), "request.md".to_string()));
    candidates.push((session_dir.join("summary.md"), "summary.md".to_string()));
    let context_dir = session_dir.join("context");
    if context_dir.is_dir() {
        let mut entries = fs::read_dir(&context_dir)
            .with_context(|| {
                format!(
                    "reading session context directory {}",
                    context_dir.display()
                )
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("context")
                .to_string();
            candidates.push((path, format!("context/{name}")));
        }
    }

    let mut resolved = Vec::new();
    let mut skipped = Vec::new();
    let mut total_bytes = 0usize;
    for (path, label) in candidates {
        if resolved.len() >= FOLDER_SESSION_CONTEXT_MAX_FILES {
            skipped.push(format!("{label}: file limit reached"));
            continue;
        }
        let Some(content) = read_folder_session_context_file(&path, &label, &mut skipped)? else {
            continue;
        };
        let content_bytes = content.len();
        if total_bytes + content_bytes > FOLDER_SESSION_CONTEXT_MAX_TOTAL_BYTES {
            skipped.push(format!("{label}: total context byte limit reached"));
            continue;
        }
        total_bytes += content_bytes;
        resolved.push(ResolvedAgentInstruction {
            source: format!("session-context:{label}"),
            content,
        });
    }
    if !skipped.is_empty() {
        resolved.push(ResolvedAgentInstruction {
            source: "session-context:skipped".to_string(),
            content: skipped.join("\n"),
        });
    }
    Ok(resolved)
}

fn read_folder_session_context_file(
    path: &Path,
    label: &str,
    skipped: &mut Vec<String>,
) -> Result<Option<String>> {
    let Ok(symlink_metadata) = fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if symlink_metadata.is_dir() {
        skipped.push(format!("{label}: directory not ingested"));
        return Ok(None);
    }
    if symlink_metadata.file_type().is_symlink() {
        let target_metadata = fs::metadata(path)
            .with_context(|| format!("reading symlink target metadata {}", path.display()))?;
        if target_metadata.is_dir() {
            skipped.push(format!("{label}: symlink directory not ingested"));
            return Ok(None);
        }
    } else if !symlink_metadata.is_file() {
        skipped.push(format!("{label}: not a regular file"));
        return Ok(None);
    }
    if !is_folder_session_context_text_file(path) {
        skipped.push(format!("{label}: unsupported file type"));
        return Ok(None);
    }
    let metadata =
        fs::metadata(path).with_context(|| format!("reading metadata {}", path.display()))?;
    if metadata.len() > FOLDER_SESSION_CONTEXT_MAX_FILE_BYTES {
        skipped.push(format!(
            "{label}: {} bytes exceeds {} byte limit",
            metadata.len(),
            FOLDER_SESSION_CONTEXT_MAX_FILE_BYTES
        ));
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading session context file {}", path.display()))?;
    let content = content.trim_end().to_string();
    if content.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(content))
}

fn is_folder_session_context_text_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if matches!(name, "README" | "NOTES" | "TODO") {
        return true;
    }
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("md" | "markdown" | "txt" | "text")
    )
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

fn session_run(args: SessionRunArgs) -> Result<()> {
    if args.background_worker && args.foreground {
        bail!("--background-worker cannot be combined with --fg");
    }
    let session_dir = resolve_session_dir(&args.dir)?;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct BackgroundRunStatus {
    pid: u32,
    log_path: Option<String>,
    log_bytes: Option<u64>,
    log_modified_at: Option<String>,
    log_tail: Option<String>,
    started_at: Option<String>,
    alive: bool,
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct PromotionGeneratedCandidateReport {
    id: String,
    candidate_type: String,
    path: String,
    text: String,
    rationale: Option<String>,
    evidence: Vec<String>,
    evidence_count: usize,
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
    let session_dir = resolve_session_dir(&args.dir)?;
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
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening background run log {}", log_path.display()))?;
    let err_file = log_file
        .try_clone()
        .with_context(|| format!("cloning background run log {}", log_path.display()))?;
    let exe = env::current_exe().context("resolving current djinn executable")?;
    let mut command = ProcessCommand::new(exe);
    command
        .arg("session")
        .arg("run")
        .arg(session_dir)
        .arg("--background-worker")
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
    write_background_session_run_marker(session_dir, &log_path, pid)?;
    Ok(SessionRunBackgroundReport {
        status: "started".to_string(),
        session_dir: session_dir.display().to_string(),
        pid,
        log_path: log_path.display().to_string(),
        watch_command: format!("djinn session watch {}", session_dir.display()),
    })
}

fn write_background_session_run_marker(
    session_dir: &Path,
    log_path: &Path,
    pid: u32,
) -> Result<()> {
    let marker_path = log_path.with_extension("toml");
    let mut content = String::new();
    content.push_str("version = 1\n");
    content.push_str(&format!(
        "started_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    content.push_str(&format!(
        "session_dir = {}\n",
        toml_string(&session_dir.display().to_string())?
    ));
    content.push_str(&format!("pid = {pid}\n"));
    content.push_str(&format!(
        "log_path = {}\n",
        toml_string(&log_path.display().to_string())?
    ));
    fs::write(&marker_path, content)
        .with_context(|| format!("writing background run marker {}", marker_path.display()))
}

fn latest_background_session_run_status(session_dir: &Path) -> Option<BackgroundRunStatus> {
    let run_dir = session_dir.join(".djinn").join("runs");
    let marker = fs::read_dir(run_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .filter_map(|path| {
            let modified = fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)?
        .1;
    let content = fs::read_to_string(marker).ok()?;
    let pid = manifest_root_string_value(&content, "pid")?
        .parse::<u32>()
        .ok()?;
    let log_path = manifest_root_string_value(&content, "log_path");
    let log_path_buf = log_path.as_ref().map(PathBuf::from);
    let log_metadata = log_path_buf
        .as_deref()
        .and_then(|path| fs::metadata(path).ok());
    Some(BackgroundRunStatus {
        pid,
        log_path,
        log_bytes: log_metadata.as_ref().map(|metadata| metadata.len()),
        log_modified_at: log_metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(system_time_to_rfc3339),
        log_tail: log_path_buf.as_deref().and_then(latest_nonempty_file_line),
        started_at: manifest_root_string_value(&content, "started_at"),
        alive: process_pid_alive(pid),
    })
}

fn latest_nonempty_file_line(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(220).collect())
}

#[cfg(unix)]
fn process_pid_alive(pid: u32) -> bool {
    if let Ok(output) = ProcessCommand::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("stat=")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        if !output.status.success() {
            return false;
        }
        let stat = String::from_utf8_lossy(&output.stdout);
        let stat = stat.trim();
        if !stat.is_empty() {
            return !stat.starts_with('Z');
        }
    }
    ProcessCommand::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_pid_alive(_pid: u32) -> bool {
    false
}

fn background_session_run_log_path(session_dir: &Path) -> Result<PathBuf> {
    let log_dir = session_dir.join(".djinn").join("runs");
    fs::create_dir_all(&log_dir).with_context(|| {
        format!(
            "creating background run log directory {}",
            log_dir.display()
        )
    })?;
    Ok(log_dir.join(format!(
        "session-run-{}.log",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    )))
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

fn session_watch(args: SessionWatchArgs) -> Result<()> {
    if args.interval_ms == 0 {
        bail!("--interval-ms must be greater than zero");
    }
    let started = Instant::now();
    let timeout = args.timeout_seconds.map(Duration::from_secs);
    let interval = Duration::from_millis(args.interval_ms);
    let mut last_key: Option<String> = None;

    loop {
        let report = folder_session_status(&args.dir)?;
        let key = session_watch_snapshot_key(&report)?;
        if last_key.as_deref() != Some(key.as_str()) {
            if args.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print!("{}", format_session_watch_snapshot(&report));
            }
            last_key = Some(key);
        }

        if report.lifecycle.state != "running" {
            return Ok(());
        }
        if let Some(timeout) = timeout {
            if started.elapsed() >= timeout {
                bail!(
                    "timed out watching session after {} seconds: {}",
                    timeout.as_secs(),
                    report.session_dir
                );
            }
        }
        thread::sleep(interval);
    }
}

fn session_watch_snapshot_key(report: &SessionStatusReport) -> Result<String> {
    serde_json::to_string(&serde_json::json!({
        "state": report.lifecycle.state,
        "mode": report.lifecycle.mode,
        "updated_at": report.lifecycle.updated_at,
        "reason": report.lifecycle.reason,
        "note": report.lifecycle.note,
        "turn_count": report.turn_count,
        "latest_turn": report.latest_turn,
        "next_action": report.next_action,
    }))
    .context("serializing session watch snapshot key")
}

fn format_session_watch_snapshot(report: &SessionStatusReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Session: {}", report.session_dir));
    let mode = report
        .lifecycle
        .mode
        .as_deref()
        .map(|mode| format!(" ({mode})"))
        .unwrap_or_default();
    lines.push(format!("State: {}{}", report.lifecycle.state, mode));
    if let Some(updated_at) = &report.lifecycle.updated_at {
        lines.push(format!("Updated: {updated_at}"));
    }
    if let Some(reason) = &report.lifecycle.reason {
        lines.push(format!("Reason: {reason}"));
    }
    if let Some(note) = &report.lifecycle.note {
        lines.push(format!("Note: {note}"));
    }
    lines.push(format!("Turns: {}", report.turn_count));
    if let Some(turn) = &report.latest_turn {
        lines.push(format!("Latest turn: {}", turn.id));
        if let Some(response_path) = &turn.response_path {
            lines.push(format!("Response: {response_path}"));
        } else if let Some(request_path) = &turn.request_path {
            lines.push(format!("Request: {request_path}"));
        }
    }
    if let Some(next_action) = &report.next_action {
        lines.push(format!("Next: {next_action}"));
    }
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
    let session_for_model = store.load_session(&id)?;
    let response = match complete_openai_messages(
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
                    "turn_dir": projection.as_ref().map(|projection| &projection.turn_dir),
                    "response_path": projection.as_ref().map(|projection| projection.turn_dir.join("response.md")),
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
        lines.push(format!(
            "  response: {}",
            projection.turn_dir.join("response.md").display()
        ));
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

fn complete_openai_messages(
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
) -> Result<djinn_agent::ModelResponse> {
    complete_openai_messages_with_progress(
        store,
        id,
        messages,
        model,
        api_key,
        base_url,
        max_tool_rounds,
        profile,
        allowed_tools,
        interactive_permissions,
        |_| Ok(()),
    )
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

fn prompt_title(prompt: &str, fallback: &str) -> String {
    let title = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(fallback);
    title.chars().take(80).collect()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ResolvedAgentInstruction {
    source: String,
    content: String,
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

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
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
            run_folder_session_tui(PathBuf::from(session.path)).map(|_| false)
        }
        djinn_tui::TuiAction::PromoteSessions {
            promotion_type,
            sessions,
        } => promote_tui_sessions(promotion_type, sessions).map(|_| false),
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
    run_folder_session_tui(PathBuf::from(report.promotion_session_dir))
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

fn tui_candidate_row(entry: &SessionStatusCandidateEntry) -> djinn_tui::PromotionCandidateRow {
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

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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

fn open_editor_at(path: &Path, line: usize, editor: Option<String>) -> Result<()> {
    open_editor_path_with_line(path, Some(line), editor)
}

fn open_editor_path(path: &Path, editor: Option<String>) -> Result<()> {
    open_editor_path_with_line(path, None, editor)
}

fn open_editor_path_with_line(
    path: &Path,
    line: Option<usize>,
    editor: Option<String>,
) -> Result<()> {
    let editor = editor.unwrap_or_else(default_editor);
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("editor command is empty");
    };
    let mut cmd = ProcessCommand::new(program);
    cmd.args(parts);
    if let Some(line) = line {
        cmd.arg(format!("+{}", line));
    }
    cmd.arg(path);
    let status = cmd.status()?;
    if !status.success() {
        bail!("editor exited with status {status}");
    }
    Ok(())
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

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
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

fn default_editor() -> String {
    env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "nvim".to_string())
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

fn memory_store() -> djinn_memory::MemoryStore {
    djinn_memory::MemoryStore::default_in(&djinn_core::default_data_dir())
}

fn idea_store() -> IdeaStore {
    IdeaStore::default_in(&djinn_core::default_data_dir())
}

fn action_store() -> ActionStore {
    ActionStore::default_in(&djinn_core::default_data_dir())
}

fn suggestion_store() -> SuggestionStore {
    SuggestionStore::default_in(&djinn_core::default_data_dir())
}

fn skill_store() -> SkillStore {
    SkillStore::default_in(&djinn_core::default_data_dir())
}

fn context_store() -> ContextStore {
    ContextStore::default_in(&djinn_core::default_data_dir())
}

fn agent_session_store() -> JsonlAgentSessionStore {
    JsonlAgentSessionStore::default_in(&djinn_core::default_data_dir())
}

fn folder_agent_session_store(session_dir: &Path) -> JsonlAgentSessionStore {
    JsonlAgentSessionStore::new(session_dir.join(FOLDER_NATIVE_SESSION_DIR))
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
    fn format_permission_preview_renders_full_hunks() {
        let rendered = format_permission_preview(&serde_json::json!({
            "preview": [
                {
                    "operation": "update",
                    "relative_path": "src/lib.rs",
                    "lines_added": 1,
                    "lines_removed": 1,
                    "hunks": [
                        {
                            "lines": [
                                {"kind": "context", "content": "fn answer() -> i32 {"},
                                {"kind": "remove", "content": "    41"},
                                {"kind": "add", "content": "    42"},
                                {"kind": "context", "content": "}"}
                            ]
                        }
                    ]
                }
            ]
        }))
        .unwrap();

        assert!(rendered.contains("- update src/lib.rs (+1/-1)"));
        assert!(rendered.contains("  @@ hunk 1"));
        assert!(rendered.contains("    fn answer() -> i32 {"));
        assert!(rendered.contains("  -     41"));
        assert!(rendered.contains("  +     42"));
    }

    #[test]
    fn terminal_permission_gate_reuses_session_path_scopes() {
        let gate = TerminalPermissionGate::new();
        let request = PermissionRequest {
            action: "apply_patch".to_string(),
            description: "patch".to_string(),
            metadata: serde_json::json!({
                "workspace": "/tmp/work",
                "preview": [
                    {"path": "/tmp/work/a.txt", "relative_path": "a.txt"},
                    {"path": "/tmp/work/b.txt", "relative_path": "b.txt"}
                ]
            }),
        };

        assert!(gate.cached_decision(&request).is_none());
        gate.remember_resources_for_session(
            &request,
            vec!["/tmp/work/a.txt".to_string(), "/tmp/work/b.txt".to_string()],
        );

        assert_eq!(
            gate.cached_decision(&request),
            Some(PermissionDecision::AllowPaths {
                paths: vec!["/tmp/work/a.txt".to_string(), "/tmp/work/b.txt".to_string()]
            })
        );
    }

    #[test]
    fn terminal_permission_gate_does_not_reuse_partial_or_cross_action_scopes() {
        let gate = TerminalPermissionGate::new();
        let request = PermissionRequest {
            action: "apply_patch".to_string(),
            description: "patch".to_string(),
            metadata: serde_json::json!({
                "workspace": "/tmp/work",
                "preview": [
                    {"path": "/tmp/work/a.txt", "relative_path": "a.txt"},
                    {"path": "/tmp/work/b.txt", "relative_path": "b.txt"}
                ]
            }),
        };
        let other_action = PermissionRequest {
            action: "write".to_string(),
            ..request.clone()
        };

        gate.remember_resources_for_session(&request, vec!["/tmp/work/a.txt".to_string()]);

        assert!(gate.cached_decision(&request).is_none());
        assert!(gate.cached_decision(&other_action).is_none());
    }

    #[test]
    fn terminal_permission_gate_reuses_session_resource_scopes() {
        let gate = TerminalPermissionGate::new();
        let request = PermissionRequest {
            action: "shell".to_string(),
            description: "shell".to_string(),
            metadata: serde_json::json!({
                "workspace": "/tmp/work",
                "kind": "shell",
                "resource": "printf hello",
                "resources": ["printf hello"]
            }),
        };

        assert!(gate.cached_decision(&request).is_none());
        gate.remember_resources_for_session(&request, vec!["printf hello".to_string()]);

        assert_eq!(
            gate.cached_decision(&request),
            Some(PermissionDecision::AllowResources {
                resources: vec!["printf hello".to_string()]
            })
        );
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

        let cli = Cli::try_parse_from(["djinn", "session", "bap-questions"]).unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        assert!(args.command.is_none());
        assert_eq!(args.dir, Some(PathBuf::from("bap-questions")));

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
    fn folder_backed_session_projection_writes_turns_and_context_without_duplicate_logs() {
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

        assert_eq!(
            fs::read_to_string(dir.join("request.md")).unwrap(),
            "new request\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("summary.md")).unwrap(),
            "new summary\n"
        );
        assert!(projection.context_dir.exists());
        assert!(projection.turn_dir.join("request.md").exists());
        assert!(projection.turn_dir.join("response.md").exists());
        assert!(dir.join("djinn.toml").exists());
        assert!(!dir.join("logs/summary-history.md").exists());
        assert!(!dir.join("logs/events.jsonl").exists());
        assert!(!dir.join("logs/transcript.md").exists());

        let _ = fs::remove_dir_all(&dir);
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
        assert_eq!(report.turn_count, 1);
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
        let latest = report.latest_turn.as_ref().unwrap();
        assert!(latest.has_response);
        assert!(latest
            .response_path
            .as_deref()
            .unwrap()
            .ends_with("response.md"));
        assert!(report
            .next_action
            .as_deref()
            .unwrap()
            .contains("open latest summary"));
        assert!(text.contains("State: completed"));
        assert!(text.contains("Mode: foreground"));
        assert!(text.contains("State note: all done"));
        assert!(text.contains("response.md"));

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

        let view = folder_session_status_tui_view(&session_dir).unwrap();

        assert_eq!(view.title, "bap-questions");
        assert_eq!(view.state, "not_started");
        assert_eq!(view.turn_count, 1);
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
            folder_session_action_message(
                &djinn_tui::FolderSessionAction::AcceptCandidate("todo-001".to_string()),
                &session_dir,
            ),
            "Accepted candidate todo-001"
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
            },
            turn_count: 1,
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
    fn folder_session_rm_removes_folder_and_linked_native_session_without_force() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-rm-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        fs::create_dir_all(&dir).unwrap();
        let store = temp_agent_store("folder-session-rm");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Folder rm".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        fs::write(dir.join("djinn.toml"), format!("session_id = \"{}\"\n", id)).unwrap();

        let report = remove_folder_session_with_store(&dir, &store).unwrap();

        assert!(report.removed_folder);
        assert_eq!(report.session_id, Some(id.to_string()));
        assert!(report.removed_native_session);
        assert!(!dir.exists());
        assert!(store.load_session(&id).is_err());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_rm_rejects_explicit_non_session_directory() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-rm-guard-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let store = temp_agent_store("folder-session-rm-guard");

        let error = remove_folder_session_with_store(&root, &store).unwrap_err();

        assert!(error
            .to_string()
            .contains("refusing to remove explicit directory without djinn.toml"));
        assert!(root.exists());
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
    fn session_cleanup_deletes_promotion_sources_only_when_requested() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-cleanup-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let source = root.join("source-session");
        fs::create_dir_all(source.join("context")).unwrap();
        fs::write(source.join("djinn.toml"), "version = 1\n").unwrap();
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

        let no_flag = cleanup_promotion_session(&SessionCleanupArgs {
            dir: promotion_dir.clone(),
            delete_sources: false,
            dry_run: false,
            json: false,
        })
        .unwrap_err();
        assert!(no_flag.to_string().contains("--delete-sources"));

        let dry_run = cleanup_promotion_session(&SessionCleanupArgs {
            dir: promotion_dir.clone(),
            delete_sources: true,
            dry_run: true,
            json: false,
        })
        .unwrap();
        assert!(dry_run.dry_run);
        assert_eq!(dry_run.source_count, 1);
        assert_eq!(dry_run.sources[0].status, "would_remove");
        assert!(!dry_run.sources[0].removed);
        assert!(source.exists());
        assert!(promotion_dir.exists());

        let removed = cleanup_promotion_session(&SessionCleanupArgs {
            dir: promotion_dir.clone(),
            delete_sources: true,
            dry_run: false,
            json: false,
        })
        .unwrap();
        assert_eq!(removed.source_count, 1);
        assert!(removed.sources[0].removed);
        assert_eq!(removed.sources[0].status, "removed");
        assert!(!source.exists());
        assert!(promotion_dir.exists());
        assert!(removed.note.contains("djinn session rm"));

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
    fn session_validate_candidates_reports_valid_and_invalid_files_without_writeback() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-validate-candidates-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
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
                "type = \"memory\"\nid = \"memory-001\"\ntext = \"Keep source sessions as promotion provenance.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\"{}/summary.md\"]\n",
                source.display()
            ),
        )
        .unwrap();
        fs::write(
            candidates_dir.join("memory-002.toml"),
            format!(
                "type = \"memory\"\nid = \"memory-002\"\ntext = \"Missing confidence should be invalid.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nevidence = [\"{}/summary.md\"]\n",
                source.display()
            ),
        )
        .unwrap();

        let report = validate_promotion_session_candidates(&SessionValidateCandidatesArgs {
            dir: promotion_dir.clone(),
            candidate: None,
            json: false,
        })
        .unwrap();

        assert_eq!(report.promotion_type, "memory");
        assert_eq!(report.candidate_count, 2);
        assert_eq!(report.valid_count, 1);
        assert_eq!(report.invalid_count, 1);
        assert!(!report.all_valid);
        assert!(report
            .candidates
            .iter()
            .any(|candidate| candidate.id == "memory-001" && candidate.valid));
        let invalid = report
            .candidates
            .iter()
            .find(|candidate| candidate.id == "memory-002")
            .unwrap();
        assert!(!invalid.valid);
        assert_eq!(invalid.candidate_type.as_deref(), Some("memory"));
        assert!(invalid
            .error
            .as_deref()
            .unwrap()
            .contains("memory candidate"));
        assert!(!promotion_dir.join("outputs/decisions").exists());
        assert!(!promotion_dir.join("outputs/candidate-status.toml").exists());

        let single = validate_promotion_session_candidates(&SessionValidateCandidatesArgs {
            dir: promotion_dir.clone(),
            candidate: Some("memory-001".to_string()),
            json: false,
        })
        .unwrap();
        assert!(single.all_valid);
        assert_eq!(single.candidate_count, 1);

        let normal = root.join("normal-session");
        fs::create_dir_all(&normal).unwrap();
        fs::write(normal.join("djinn.toml"), "version = 1\n").unwrap();
        let err = validate_promotion_session_candidates(&SessionValidateCandidatesArgs {
            dir: normal,
            candidate: None,
            json: false,
        })
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
    fn session_export_pattern_writes_readable_notes_file() {
        let root = std::env::temp_dir().join(format!(
            "djinn-pattern-export-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("promotion-pattern");
        let candidates_dir = session_dir.join("outputs/candidates");
        fs::create_dir_all(&candidates_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            "version = 1\nkind = \"promotion\"\npromotion_type = \"pattern\"\n",
        )
        .unwrap();
        fs::write(
            candidates_dir.join("pattern-001.toml"),
            "type = \"pattern\"\nid = \"pattern-001\"\ntext = \"Keep pattern insights in notes after review.\"\nrationale = \"Patterns are synthesis, not durable Djinn records.\"\nevidence = [\n  \"/tmp/source/summary.md\"\n]\n",
        )
        .unwrap();
        let notes_path = root.join("notes/patterns.md");

        let dry_run = export_pattern_insights(&SessionExportPatternArgs {
            dir: session_dir.clone(),
            candidate: Some("pattern-001".to_string()),
            to: notes_path.clone(),
            append: false,
            dry_run: true,
            json: false,
        })
        .unwrap();
        assert!(!notes_path.exists());
        assert!(dry_run
            .preview
            .as_deref()
            .unwrap_or_default()
            .contains("Keep pattern insights in notes after review."));

        let written = export_pattern_insights(&SessionExportPatternArgs {
            dir: session_dir.clone(),
            candidate: Some("pattern-001".to_string()),
            to: notes_path.clone(),
            append: false,
            dry_run: false,
            json: false,
        })
        .unwrap();
        assert!(written.wrote);
        let notes = fs::read_to_string(&notes_path).unwrap();
        assert!(notes.contains("# Pattern insight"));
        assert!(notes.contains("Keep pattern insights in notes after review."));
        assert!(notes.contains("Patterns are synthesis, not durable Djinn records."));
        assert!(notes.contains("/tmp/source/summary.md"));

        let overwrite = export_pattern_insights(&SessionExportPatternArgs {
            dir: session_dir,
            candidate: Some("pattern-001".to_string()),
            to: notes_path,
            append: false,
            dry_run: false,
            json: false,
        })
        .unwrap_err();
        assert!(overwrite.to_string().contains("already exists"));

        let _ = fs::remove_dir_all(&root);
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
            turn_dir: session_dir.join("turns/20260728T120000-1"),
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
        assert!(dir.join("turns").exists());
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
