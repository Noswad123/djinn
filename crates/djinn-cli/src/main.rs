use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
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
use djinn_chats::{ChatRecord, ChatRestoreReport};
use djinn_contexts::{resolve_context, ContextInput, ContextRecord, ContextStore};
use djinn_memory::{
    lifecycle_for, ActionRecord, ActionStore, AgentSession, AgentSessionEvent,
    AgentSessionEventKind, AgentSessionExecutionMode, AgentSessionFilter, AgentSessionId,
    AgentSessionLifecycleState, AgentSessionMeta, AgentSessionPolicyRule,
    AgentSessionPolicySnapshot, AgentSessionRuntimeConfig, AgentSessionStore, AgentSessionSummary,
    FileHistoryEntryId, FileHistoryFilter, FileHistoryRestoreOptions, IdeaRecord, IdeaStore,
    JsonlAgentSessionStore, JsonlFileHistoryStore, MemoryInput, MemoryRecord, MemorySource,
    SuggestionInput, SuggestionRecord, SuggestionStore,
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
    /// Promote raw context, sessions, memories, or tools into a more useful form.
    Promote(PromoteArgs),
    /// Run an external review to create or activate durable knowledge.
    Review(ReviewArgs),
    /// Remove one item.
    Rm(RmArgs),
    /// Clear a collection after confirmation.
    Clear(ClearArgs),
    /// Archive selected records before removing them from active views.
    Archive(ArchiveArgs),
    /// Prune old transient/cache records.
    Prune(PruneArgs),
    /// Discover without writing durable state.
    Scan(ScanArgs),
    /// Write a machine-readable cache/index.
    Index(IndexArgs),
    /// Search a collection.
    Search(SearchArgs),
    /// Watch an external source for new knowledge.
    Watch(WatchArgs),
    /// Install Djinn integrations into external tools.
    Install(InstallArgs),
    /// Uninstall Djinn integrations from external tools.
    Uninstall(UninstallArgs),
    /// Show integration health/status.
    Status(StatusArgs),
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
    /// List sessions.
    Sessions(ListChatsArgs),
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
    /// Show a session by id.
    Session(ShowChatArgs),
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
    /// Add a session from a file.
    Session(AddChatArgs),
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
struct PromoteArgs {
    #[command(subcommand)]
    noun: PromoteNoun,
}

#[derive(Debug, Args)]
struct ReviewArgs {
    #[command(subcommand)]
    source: ReviewSource,
}

#[derive(Debug, Subcommand)]
enum ReviewSource {
    /// Ask OpenCode to review recent Djinn sessions and add active memories.
    Sessions(ReviewChatsArgs),
    /// Ask OpenCode to review one or more memories and create suggestions.
    Memories(ReviewMemoriesArgs),
    /// Ask OpenCode to review one memory and create suggestions.
    Memory(ReviewMemoriesArgs),
    /// Review recent OpenCode sessions.
    Opencode(ReviewOpencodeArgs),
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
struct ReviewChatsArgs {
    /// Optional chat source filter, for example: opencode.
    #[arg(long)]
    source: Option<String>,
    /// Maximum recent sessions to review.
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// Review all matching sessions instead of applying --limit.
    #[arg(long)]
    all: bool,
    /// Optional query filter over chat metadata/content.
    #[arg(long)]
    query: Option<String>,
    /// OpenCode agent to use for the review.
    #[arg(long)]
    agent: Option<String>,
    /// OpenCode run title.
    #[arg(long, default_value = "djinn promotion review")]
    title: String,
    /// OpenCode binary to execute.
    #[arg(long, default_value = "opencode")]
    opencode_bin: String,
    /// Print the prompt instead of running OpenCode.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct ReviewOpencodeArgs {
    /// Maximum recent OpenCode sessions to review.
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// Review all matching OpenCode sessions instead of applying --limit.
    #[arg(long)]
    all: bool,
    /// Optional query filter over chat metadata/content.
    #[arg(long)]
    query: Option<String>,
    /// OpenCode agent to use for the review.
    #[arg(long)]
    agent: Option<String>,
    /// OpenCode run title.
    #[arg(long, default_value = "djinn promotion review")]
    title: String,
    /// OpenCode binary to execute.
    #[arg(long, default_value = "opencode")]
    opencode_bin: String,
    /// Print the prompt instead of running OpenCode.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Subcommand)]
enum PromoteNoun {
    /// Promote one session. Defaults to a local summary.
    Session(ShareChatArgs),
    /// Promote multiple sessions. Defaults to a local summary.
    Sessions(ShareChatsArgs),
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
    /// Remove a session matching an id, source id, or title fragment.
    Session { id: String },
    /// Remove or archive a skill.
    Skill(RmSkillArgs),
}

#[derive(Debug, Args)]
struct ClearArgs {
    #[command(subcommand)]
    noun: ClearNoun,
}

#[derive(Debug, Args)]
struct ArchiveArgs {
    #[command(subcommand)]
    noun: ArchiveNoun,
}

#[derive(Debug, Subcommand)]
enum ClearNoun {
    /// Clear all memories after interactive confirmation.
    Memories {
        /// Skip creating memories.backup-*.jsonl before clearing.
        #[arg(long)]
        no_backup: bool,
    },
    /// Clear all sessions after interactive confirmation.
    Sessions {
        /// Skip creating sessions.backup-*.jsonl before clearing.
        #[arg(long)]
        no_backup: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ArchiveNoun {
    /// Archive selected session rows and remove them from the active session index.
    Sessions(ArchiveChatsArgs),
    /// List session archive files.
    List(ArchiveListArgs),
    /// Show the contents of one session archive file.
    Show(ArchiveShowArgs),
    /// Restore sessions from an archive file.
    Restore(ArchiveRestoreArgs),
    /// Remove one session archive file after confirmation.
    Rm(ArchiveRemoveArgs),
}

#[derive(Debug, Args)]
struct PruneArgs {
    #[command(subcommand)]
    noun: PruneNoun,
}

#[derive(Debug, Subcommand)]
enum PruneNoun {
    /// Remove sessions older than a duration such as 30d or 12days.
    Sessions(PruneChatsArgs),
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
    /// Search sessions.
    Sessions { query: String },
    /// Search local tools.
    Tools(SearchToolsArgs),
    /// Search memories.
    Memories { query: String },
    /// Search suggestions.
    Suggestions { query: String },
}

#[derive(Debug, Args)]
struct WatchArgs {
    #[command(subcommand)]
    source: WatchSource,
}

#[derive(Debug, Subcommand)]
enum WatchSource {
    /// Watch OpenCode conversations.
    Opencode(WatchOpencodeArgs),
}

#[derive(Debug, Args)]
struct InstallArgs {
    #[command(subcommand)]
    target: InstallTarget,
}

#[derive(Debug, Args)]
struct UninstallArgs {
    #[command(subcommand)]
    target: UninstallTarget,
}

#[derive(Debug, Subcommand)]
enum UninstallTarget {
    /// Uninstall the OpenCode Djinn watcher plugin.
    Opencode(OpencodeIntegrationArgs),
}

#[derive(Debug, Args)]
struct StatusArgs {
    #[command(subcommand)]
    target: StatusTarget,
}

#[derive(Debug, Subcommand)]
enum StatusTarget {
    /// Show OpenCode Djinn watcher plugin status.
    Opencode(OpencodeIntegrationArgs),
}

#[derive(Debug, Subcommand)]
enum InstallTarget {
    /// Install the OpenCode plugin that auto-imports sessions into Djinn.
    Opencode(InstallOpencodeArgs),
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
    /// Deprecated legacy interactive chat surface.
    Chat(AgentChatArgs),
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
    /// OpenAI model to use. Defaults the same way as agent chat.
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
    /// OpenAI model to use. Defaults the same way as agent chat.
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
    /// Model to treat as current. Defaults the same way as agent chat.
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
    /// OpenAI model to use. Defaults the same way as agent chat.
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
    /// TUI view to open. Defaults to tools.
    #[arg(value_enum, default_value_t = TuiView::Tools)]
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

#[derive(Debug, Args, Clone)]
struct AgentChatArgs {
    /// Resume an existing agent session id instead of creating a new session.
    #[arg(long)]
    resume: Option<String>,
    /// Human-friendly session title.
    #[arg(long)]
    title: Option<String>,
    /// Workspace path for the session. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Folder-backed session directory for generated artifacts like summary.md.
    #[arg(long = "session-dir")]
    session_dir: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
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
}

#[derive(Debug, Default)]
struct TerminalPermissionGate {
    session_scopes: Mutex<Vec<TerminalApprovalScope>>,
    kitsune_reporter: Option<KitsuneAgentReporterHandle>,
    agent_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalApprovalScope {
    action: String,
    workspace: String,
    resources: HashSet<String>,
}

impl TerminalPermissionGate {
    fn new(kitsune_reporter: Option<KitsuneAgentReporterHandle>, agent_session_id: String) -> Self {
        Self {
            session_scopes: Mutex::new(Vec::new()),
            kitsune_reporter,
            agent_session_id,
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

    fn report_permission_blocked(&self, request: &PermissionRequest) {
        if let Some(reporter) = &self.kitsune_reporter {
            let message = format!("Permission approval required: {}", request.description);
            reporter.report_state(
                KitsuneAgentReportState::Blocked,
                &self.agent_session_id,
                Some(&message),
            );
        }
    }

    fn report_permission_resolved(&self) {
        if let Some(reporter) = &self.kitsune_reporter {
            reporter.report_state(
                KitsuneAgentReportState::Working,
                &self.agent_session_id,
                Some("Permission decision received"),
            );
        }
    }
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

#[derive(Debug, Args)]
struct ListChatsArgs {
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ShowChatArgs {
    /// Session id, source id, or unambiguous title fragment.
    id: String,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct PruneChatsArgs {
    /// Prune sessions older than this duration, for example: 30d or 12days.
    #[arg(long = "older-than")]
    older_than: String,
    /// Skip creating sessions.backup-*.jsonl before pruning.
    #[arg(long)]
    no_backup: bool,
}

#[derive(Debug, Args)]
struct ArchiveChatsArgs {
    /// Optional session ids, source ids, or unambiguous title fragments to archive.
    ids: Vec<String>,
    /// Filter by source, for example: opencode.
    #[arg(long)]
    source: Option<String>,
    /// Filter sessions by id, title, source metadata, path, or content.
    #[arg(long)]
    query: Option<String>,
    /// Maximum number of sessions to archive unless --all or explicit ids are used.
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// Include every matching session. Use deliberately.
    #[arg(long)]
    all: bool,
    /// Print selected sessions without writing an archive or removing rows.
    #[arg(long)]
    dry_run: bool,
    /// Required to actually archive and remove selected session rows.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct ArchiveListArgs {
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ArchiveShowArgs {
    /// Archive path, filename, or basename from ~/.cache/djinn/chat-archives.
    archive: String,
    /// Include session content previews.
    #[arg(long)]
    content: bool,
    /// Maximum characters to show per session when --content is set.
    #[arg(long, default_value_t = 1200)]
    max_chars_per_chat: usize,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ArchiveRestoreArgs {
    /// Archive path, filename, or basename from ~/.cache/djinn/chat-archives.
    archive: String,
    /// Replace existing session rows with matching id or source/source-id.
    #[arg(long)]
    force: bool,
    /// Print what would be restored without mutating the active session index.
    #[arg(long)]
    dry_run: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ArchiveRemoveArgs {
    /// Archive path, filename, or basename from ~/.cache/djinn/chat-archives.
    archive: String,
    /// Print what would be removed without deleting the archive file.
    #[arg(long)]
    dry_run: bool,
    /// Required to actually delete the archive file.
    #[arg(long)]
    force: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
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
struct AddChatArgs {
    /// Markdown, text, or JSON file containing one AI interaction/session. Use '-' for stdin.
    file: PathBuf,
    /// Human-friendly title. Defaults to the first non-empty line or file stem.
    #[arg(long)]
    title: Option<String>,
    /// Generic source name, for example: opencode, manual, cursor, claude.
    #[arg(long)]
    source: Option<String>,
    /// Source-native session id, if available.
    #[arg(long = "source-id")]
    source_id: Option<String>,
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
    /// Chat id, source id, or title fragment to snapshot as optional provenance. Repeatable.
    #[arg(long = "source-chat")]
    source_chats: Vec<String>,
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

#[derive(Debug, Args)]
struct ShareChatArgs {
    /// Session id, source id, or unambiguous title fragment.
    id: String,
    /// Promotion style. Defaults to a local summary.
    #[arg(long, value_enum, default_value_t = ShareChatsMode::Summary)]
    mode: ShareChatsMode,
    /// Maximum characters to include from the session body.
    #[arg(long, default_value_t = 4000)]
    max_chars_per_chat: usize,
    /// Maximum memories the model should return for --mode merge.
    #[arg(long, default_value_t = 20)]
    max_memories: usize,
    /// Archive the source session row after --mode merge writes memories successfully.
    #[arg(long)]
    archive: bool,
    /// Print the merge prompt instead of running the model, writing memories, or archiving.
    #[arg(long)]
    dry_run: bool,
    /// Agent profile name for --mode merge.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Model to use for --mode merge. Prefix with copilot/ to use GitHub Copilot.
    #[arg(long)]
    model: Option<String>,
    /// Provider API token for --mode merge. For copilot/* models, this is a Copilot API token.
    #[arg(long = "api-key")]
    api_key: Option<String>,
    /// Provider endpoint/base URL for --mode merge.
    #[arg(long = "base-url")]
    base_url: Option<String>,
}

#[derive(Debug, Args)]
struct ShareChatsArgs {
    /// Optional session ids, source ids, or unambiguous title fragments to include.
    ids: Vec<String>,
    /// Filter by source, for example: opencode.
    #[arg(long)]
    source: Option<String>,
    /// Filter sessions by id, title, source metadata, path, or content.
    #[arg(long)]
    query: Option<String>,
    /// Maximum number of sessions to include unless --all or explicit ids are used.
    #[arg(long, default_value_t = 10)]
    limit: usize,
    /// Include every matching session. Use deliberately; this can produce a large prompt.
    #[arg(long)]
    all: bool,
    /// Prompt style for the grouped sessions.
    #[arg(long, value_enum, default_value_t = ShareChatsMode::Summary)]
    mode: ShareChatsMode,
    /// Maximum characters to include from each session body.
    #[arg(long, default_value_t = 4000)]
    max_chars_per_chat: usize,
    /// Maximum memories the model should return.
    #[arg(long, default_value_t = 20)]
    max_memories: usize,
    /// Archive selected session rows after --mode merge writes memories successfully.
    #[arg(long)]
    archive: bool,
    /// Print the merge prompt instead of running the model, writing memories, or archiving.
    #[arg(long)]
    dry_run: bool,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Model to use. Prefix with copilot/ to use GitHub Copilot.
    #[arg(long)]
    model: Option<String>,
    /// Provider API token. For copilot/* models, this is a Copilot API token.
    #[arg(long = "api-key")]
    api_key: Option<String>,
    /// Provider endpoint/base URL. For copilot/* models, this is the chat completions endpoint.
    #[arg(long = "base-url")]
    base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ShareChatsMode {
    /// Ask the agent to summarize the grouped sessions.
    Summary,
    /// Ask the agent to find recurring patterns across sessions.
    Pattern,
    /// Ask the agent to propose durable memory commands from cross-chat patterns.
    Memories,
    /// Run the model and write durable memories from selected sessions.
    Merge,
}

#[derive(Debug, Args)]
struct WatchOpencodeArgs {
    /// OpenCode session id. Defaults to the first row from `opencode session list`.
    session_id: Option<String>,
    /// OpenCode binary to execute.
    #[arg(long, default_value = "opencode")]
    opencode_bin: String,
    /// Store unsanitized OpenCode export output. By default Djinn passes --sanitize.
    #[arg(long)]
    unsafe_unsanitized: bool,
    /// Poll every N seconds instead of importing once. If no session id is provided,
    /// each poll imports the current latest session.
    #[arg(long)]
    interval: Option<u64>,
    /// Override the stored chat title.
    #[arg(long)]
    title: Option<String>,
}

#[derive(Debug, Args)]
struct InstallOpencodeArgs {
    /// OpenCode config file to patch. Defaults to ~/.config/opencode/opencode.json.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Plugin file to write. Defaults to ~/.config/opencode/plugins/djinn-watch.js.
    #[arg(long = "plugin-path")]
    plugin_path: Option<PathBuf>,
    /// Only write the plugin file; do not patch opencode.json.
    #[arg(long)]
    no_config_patch: bool,
    /// Print the planned changes without writing files.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct OpencodeIntegrationArgs {
    /// OpenCode config file to inspect/patch. Defaults to ~/.config/opencode/opencode.json.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Plugin file path. Defaults to ~/.config/opencode/plugins/djinn-watch.js.
    #[arg(long = "plugin-path")]
    plugin_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

const OPENCODE_PLUGIN: &str = r#"/**
 * Djinn OpenCode watcher plugin.
 *
 * Keeps Djinn's Rust importer as the source of truth by spawning:
 *   djinn watch opencode <session-id>
 *
 * Environment variables:
 *   DJINN_OPENCODE_DISABLED=1          disable this plugin
 *   DJINN_OPENCODE_DEBUG=1             append debug logs under ~/.cache/djinn
 *   DJINN_OPENCODE_IMPORT_COOLDOWN_MS  debounce assistant-message imports
 *   DJINN_OPENCODE_AUTO_REVIEW=1       opt into background memory reviews
 *   DJINN_OPENCODE_REVIEW_COOLDOWN_MS  debounce background reviews
  *   DJINN_OPENCODE_REVIEW_LIMIT        recent OpenCode sessions per review
 *   DJINN_OPENCODE_REVIEW_AGENT        optional OpenCode review agent
 *   DJINN_BIN=/path/to/djinn           override djinn executable
 */

import { appendFileSync, mkdirSync, readFileSync } from "fs"
import { homedir } from "os"
import { join } from "path"

const DEBUG = process.env.DJINN_OPENCODE_DEBUG === "1"
const DISABLED = process.env.DJINN_OPENCODE_DISABLED === "1"
const CHILD = process.env.DJINN_OPENCODE_PLUGIN_CHILD === "1" || process.env.DJINN_REVIEWER === "1"
const AUTO_REVIEW = process.env.DJINN_OPENCODE_AUTO_REVIEW === "1"
const DJINN_BIN = process.env.DJINN_BIN || "djinn"
const CACHE_DIR = process.env.DJINN_CACHE_DIR || join(homedir(), ".cache", "djinn")
const CONFIG_DIR = process.env.DJINN_CONFIG_DIR || join(homedir(), ".config", "djinn")
const WATCH_STATE_FILE = join(CONFIG_DIR, "watchers", "opencode.json")
const LOG_FILE = join(CACHE_DIR, "opencode-plugin.log")
const DEFAULT_COOLDOWN_MS = 30000
const DEFAULT_REVIEW_COOLDOWN_MS = 3600000

function cooldownMs() {
  const raw = Number(process.env.DJINN_OPENCODE_IMPORT_COOLDOWN_MS || DEFAULT_COOLDOWN_MS)
  return Number.isFinite(raw) && raw > 0 ? raw : DEFAULT_COOLDOWN_MS
}

function reviewCooldownMs() {
  const raw = Number(process.env.DJINN_OPENCODE_REVIEW_COOLDOWN_MS || DEFAULT_REVIEW_COOLDOWN_MS)
  return Number.isFinite(raw) && raw > 0 ? raw : DEFAULT_REVIEW_COOLDOWN_MS
}

function reviewLimit() {
  const raw = Number(process.env.DJINN_OPENCODE_REVIEW_LIMIT || 20)
  return Number.isFinite(raw) && raw > 0 ? String(Math.floor(raw)) : "20"
}

function dbg(...args) {
  if (!DEBUG) return
  try {
    mkdirSync(CACHE_DIR, { recursive: true })
    appendFileSync(LOG_FILE, `[${new Date().toISOString()}] ${args.join(" ")}\n`)
  } catch {}
}

export const DjinnWatchPlugin = async (input) => {
  if (DISABLED || CHILD) {
    dbg("disabled", { DISABLED, CHILD })
    return {}
  }

  let currentSessionId = null
  let timer = null
  let lastReviewAt = 0
  const lastImportAt = new Map()
  const hydrated = new Set()

  function rememberSession(sessionId) {
    if (sessionId) currentSessionId = sessionId
    return currentSessionId
  }

  function spawnImport(sessionId, reason, force = false) {
    sessionId = rememberSession(sessionId)
    if (!sessionId) {
      dbg("skip import: missing session id", reason)
      return
    }

    const now = Date.now()
    const last = lastImportAt.get(sessionId) || 0
    const cooldown = cooldownMs()
    if (!force && now - last < cooldown) {
      dbg("skip import: cooldown", sessionId, reason)
      return
    }
    lastImportAt.set(sessionId, now)

    try {
      const proc = Bun.spawn([DJINN_BIN, "watch", "opencode", sessionId], {
        stdin: "ignore",
        stdout: "ignore",
        stderr: "ignore",
        detached: true,
        env: { ...process.env, DJINN_OPENCODE_PLUGIN_CHILD: "1" },
      })
      try { proc.unref() } catch {}
      dbg("spawned import", sessionId, reason)
    } catch (err) {
      dbg("spawn failed", sessionId, reason, err?.message || err)
    }
  }

  function scheduleImport(sessionId, reason, waitMs = cooldownMs()) {
    rememberSession(sessionId)
    if (!currentSessionId) return
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => {
      timer = null
      spawnImport(currentSessionId, reason)
    }, waitMs)
    try { timer.unref() } catch {}
    dbg("scheduled import", currentSessionId, reason, waitMs)
  }

  function bridgeFor(sessionId) {
    if (!sessionId) return null
    try {
      const raw = readFileSync(WATCH_STATE_FILE, "utf8")
      const state = JSON.parse(raw)
      const session = state?.sessions?.[sessionId]
      if (!session?.djinn_session_id) return null
      return {
        source: "djinn",
        agentSessionId: session.djinn_session_id,
        agentSessionPath: session.djinn_session_path || undefined,
        convertedAt: session.converted_at || undefined,
      }
    } catch (err) {
      dbg("bridge read failed", sessionId, err?.message || err)
      return null
    }
  }

  async function hydrateDjinnBridge(client, sessionId) {
    sessionId = rememberSession(sessionId)
    if (!sessionId || hydrated.has(sessionId)) return
    const bridge = bridgeFor(sessionId)
    if (!bridge) return
    try {
      const current = await client.session.get({ sessionID: sessionId })
      if (current?.error) {
        dbg("bridge get failed", sessionId, current.error?.message || current.error)
        return
      }
      const metadata = { ...(current?.data?.metadata || {}), djinn: bridge }
      const updated = await client.session.update({ sessionID: sessionId, metadata })
      if (updated?.error) {
        dbg("bridge update failed", sessionId, updated.error?.message || updated.error)
        return
      }
      hydrated.add(sessionId)
      dbg("hydrated bridge", sessionId, bridge.agentSessionId)
    } catch (err) {
      dbg("bridge hydrate failed", sessionId, err?.message || err)
    }
  }

  function spawnReview(reason, force = false) {
    if (!AUTO_REVIEW) return
    const now = Date.now()
    const cooldown = reviewCooldownMs()
    if (!force && now - lastReviewAt < cooldown) {
      dbg("skip review: cooldown", reason)
      return
    }
    lastReviewAt = now

    const args = [DJINN_BIN, "review", "sessions", "--source", "opencode", "--limit", reviewLimit()]
    const agent = process.env.DJINN_OPENCODE_REVIEW_AGENT
    if (agent) args.push("--agent", agent)

    try {
      const proc = Bun.spawn(args, {
        stdin: "ignore",
        stdout: "ignore",
        stderr: "ignore",
        detached: true,
        env: { ...process.env, DJINN_OPENCODE_PLUGIN_CHILD: "1", DJINN_REVIEWER: "1" },
      })
      try { proc.unref() } catch {}
      dbg("spawned review", reason)
    } catch (err) {
      dbg("review spawn failed", reason, err?.message || err)
    }
  }

  process.once("beforeExit", () => {
    spawnImport(currentSessionId, "beforeExit", true)
    spawnReview("beforeExit")
  })

  return {
    event: async ({ event }) => {
      try {
        const props = event?.properties || {}
        const info = props.info || {}
        const sessionId = info.id || info.sessionID || props.sessionID
        await hydrateDjinnBridge(input.client, sessionId || currentSessionId)

        switch (event?.type) {
          case "session.created":
            scheduleImport(sessionId, "session.created", 2000)
            break
          case "message.updated":
            rememberSession(sessionId)
            if (info.role === "assistant") {
              scheduleImport(currentSessionId, "assistant-message")
            }
            break
          case "session.idle":
            spawnImport(sessionId || currentSessionId, "session.idle", true)
            spawnReview("session.idle")
            break
        }
      } catch (err) {
        dbg("event error", err?.message || err)
      }
    },
  }
}

export default DjinnWatchPlugin
"#;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct OpencodeWatchState {
    #[serde(default)]
    sessions: HashMap<String, OpencodeSessionState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct OpencodeSessionState {
    #[serde(default)]
    content_hash: String,
    #[serde(default)]
    imported_at: String,
    #[serde(default)]
    chat_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    djinn_session_id: String,
    #[serde(default)]
    djinn_session_path: String,
    #[serde(default)]
    converted_at: String,
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
        Command::Promote(args) => run_promote(args),
        Command::Review(args) => run_review(args),
        Command::Rm(args) => run_rm(args),
        Command::Clear(args) => run_clear(args),
        Command::Archive(args) => run_archive(args),
        Command::Prune(args) => run_prune(args),
        Command::Scan(args) => run_scan(args),
        Command::Index(args) => run_index(args),
        Command::Search(args) => run_search(args),
        Command::Watch(args) => run_watch(args),
        Command::Install(args) => run_install(args),
        Command::Uninstall(args) => run_uninstall(args),
        Command::Status(args) => run_status(args),
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
    if let Some(args) = run_tui(args)? {
        run_interactive_app(args)
    } else {
        Ok(())
    }
}

fn run_list(args: ListArgs) -> Result<()> {
    match args.noun {
        ListNoun::Tools(scope) => list_tools(scope),
        ListNoun::Memories => list_memories(),
        ListNoun::Suggestions => list_suggestions(),
        ListNoun::Ideas => list_ideas(),
        ListNoun::Actions => list_actions(),
        ListNoun::Sessions(args) => list_chats(args),
        ListNoun::Skills(args) => list_skills(args),
        ListNoun::Contexts(args) | ListNoun::Ctx(args) => list_contexts(args),
    }
}

fn run_show(args: ShowArgs) -> Result<()> {
    match args.noun {
        ShowNoun::Session(args) => show_chat(args),
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
        AddNoun::Session(args) => add_chat(args),
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

fn run_promote(args: PromoteArgs) -> Result<()> {
    match args.noun {
        PromoteNoun::Session(args) => promote_session(args),
        PromoteNoun::Sessions(args) => promote_sessions(args),
    }
}

fn run_review(args: ReviewArgs) -> Result<()> {
    match args.source {
        ReviewSource::Sessions(args) => review_chats(args),
        ReviewSource::Memory(args) | ReviewSource::Memories(args) => review_memories(args),
        ReviewSource::Opencode(args) => review_opencode(args),
    }
}

fn run_rm(args: RmArgs) -> Result<()> {
    match args.noun {
        RmNoun::Memory { keyword } => rm_memory(&keyword),
        RmNoun::Session { id } => rm_chat(&id),
        RmNoun::Skill(args) => rm_skill(args),
    }
}

fn run_clear(args: ClearArgs) -> Result<()> {
    match args.noun {
        ClearNoun::Memories { no_backup } => clear_memories(no_backup),
        ClearNoun::Sessions { no_backup } => clear_chats(no_backup),
    }
}

fn run_archive(args: ArchiveArgs) -> Result<()> {
    match args.noun {
        ArchiveNoun::Sessions(args) => archive_chats(args),
        ArchiveNoun::List(args) => list_archives(args),
        ArchiveNoun::Show(args) => show_archive(args),
        ArchiveNoun::Restore(args) => restore_archive(args),
        ArchiveNoun::Rm(args) => remove_archive(args),
    }
}

fn run_prune(args: PruneArgs) -> Result<()> {
    match args.noun {
        PruneNoun::Sessions(args) => prune_chats(args),
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
        SearchNoun::Sessions { query } => search_chats(&query),
        SearchNoun::Tools(args) => search_tools(args),
        SearchNoun::Memories { query } => search_memories(&query),
        SearchNoun::Suggestions { query } => search_suggestions(&query),
    }
}

fn run_watch(args: WatchArgs) -> Result<()> {
    match args.source {
        WatchSource::Opencode(args) => watch_opencode(args),
    }
}

fn run_install(args: InstallArgs) -> Result<()> {
    match args.target {
        InstallTarget::Opencode(args) => install_opencode(args),
    }
}

fn run_uninstall(args: UninstallArgs) -> Result<()> {
    match args.target {
        UninstallTarget::Opencode(args) => uninstall_opencode(args),
    }
}

fn run_status(args: StatusArgs) -> Result<()> {
    match args.target {
        StatusTarget::Opencode(args) => status_opencode(args),
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
                "Djinn integration metadata",
                "Djinn installs an OpenCode watcher plugin, but does not import plugin config.",
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
        AgentCommand::Chat(args) => {
            warn_legacy_agent_command(
                "agent chat",
                Some("use folder-backed `djinn ask` and `djinn session ...`"),
            );
            run_interactive_app(args)
        }
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
    tui.run_folder_session_status(|| folder_session_status_tui_view(&session_dir))?;
    tui.finish()
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
        next_action: report.next_action.clone(),
        note: report
            .lifecycle
            .note
            .clone()
            .or(report.lifecycle.reason.clone()),
    })
}

fn run_session_command(command: SessionCommand) -> Result<()> {
    match command {
        SessionCommand::Init(args) => session_init(args),
        SessionCommand::Run(args) => session_run(args),
        SessionCommand::Watch(args) => session_watch(args),
        SessionCommand::Compact(args) => session_compact(args),
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
    context_ingestible_count: usize,
    context_skipped: Vec<String>,
    next_action: Option<String>,
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
    let turns = read_folder_session_turns(&turns_dir)?;
    let turn_count = turns.len();
    let lifecycle = session_status_lifecycle(native_session.as_ref());
    let latest_turn = turns.last().map(session_status_turn_report);
    let request_exists = session_dir.join("request.md").exists();
    let next_action =
        session_status_next_action(&session_dir, request_exists, turn_count, &lifecycle);

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
        context_ingestible_count,
        context_skipped,
        next_action,
    })
}

fn session_status_lifecycle(native_session: Option<&AgentSession>) -> SessionStatusLifecycleReport {
    if let Some(session) = native_session {
        let lifecycle = lifecycle_for(session);
        SessionStatusLifecycleReport {
            state: lifecycle.state.as_str().to_string(),
            mode: lifecycle.mode.map(|mode| mode.as_str().to_string()),
            updated_at: non_empty_string(&lifecycle.updated_at),
            reason: lifecycle.reason,
            note: lifecycle.note,
        }
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

fn session_status_next_action(
    session_dir: &Path,
    request_exists: bool,
    turn_count: usize,
    lifecycle: &SessionStatusLifecycleReport,
) -> Option<String> {
    if lifecycle.state == "running" {
        Some(format!(
            "check again: djinn session status {}",
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
    let lifecycle = session_status_lifecycle(native_session.as_ref());
    let latest_turn = turns.last().map(session_status_turn_report);
    let request_md = path.join("request.md").exists();
    let next_action = session_status_next_action(path, request_md, turn_count, &lifecycle);
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

fn append_foreground_session_lifecycle_event(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    state: AgentSessionLifecycleState,
    reason: impl Into<String>,
    note: Option<String>,
) -> Result<()> {
    append_agent_session_lifecycle_event(
        store,
        id,
        state,
        AgentSessionExecutionMode::Foreground,
        reason,
        note,
    )
}

fn mark_foreground_session_paused_on_quit(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
) -> Result<()> {
    let session = store.load_session(id)?;
    let lifecycle = lifecycle_for(&session);
    if matches!(
        lifecycle.state,
        AgentSessionLifecycleState::Failed | AgentSessionLifecycleState::Cancelled
    ) {
        return Ok(());
    }
    append_foreground_session_lifecycle_event(
        store,
        id,
        AgentSessionLifecycleState::Paused,
        "chat exited",
        None,
    )
}

fn mark_foreground_session_paused_if_not_terminal(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    reason: impl Into<String>,
) -> Result<()> {
    let session = store.load_session(id)?;
    let lifecycle = lifecycle_for(&session);
    if matches!(
        lifecycle.state,
        AgentSessionLifecycleState::Completed
            | AgentSessionLifecycleState::Failed
            | AgentSessionLifecycleState::Cancelled
    ) {
        return Ok(());
    }
    append_foreground_session_lifecycle_event(
        store,
        id,
        AgentSessionLifecycleState::Paused,
        reason,
        None,
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
    session_id: Option<AgentSessionId>,
    created_at: Option<String>,
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
        session_id: manifest_root_string_value(manifest, "session_id").map(AgentSessionId::new),
        created_at: manifest_root_string_value(manifest, "created_at"),
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
    Ok(SessionRunBackgroundReport {
        status: "started".to_string(),
        session_dir: session_dir.display().to_string(),
        pid,
        log_path: log_path.display().to_string(),
        watch_command: format!("djinn session watch {}", session_dir.display()),
    })
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentChatOutcome {
    Quit {
        session_id: String,
        title: String,
        path: PathBuf,
    },
    Dashboard {
        resume: String,
        initial_tab: djinn_tui::DashboardTab,
    },
    Command {
        resume: String,
        command: djinn_tui::AgentChatCommand,
    },
}

fn run_interactive_app(mut args: AgentChatArgs) -> Result<()> {
    let mut tui = djinn_tui::TuiSession::enter()?;
    loop {
        match agent_chat(&mut tui, args.clone())? {
            AgentChatOutcome::Quit {
                session_id,
                title,
                path,
            } => {
                tui.finish()?;
                println!("Agent session [{session_id}]: {title}");
                println!("Path: {}", path.display());
                return Ok(());
            }
            AgentChatOutcome::Dashboard {
                resume,
                initial_tab,
            } => {
                args = AgentChatArgs {
                    resume: Some(resume),
                    title: None,
                    workspace: None,
                    ..args
                };
                match run_tui_in_session(&mut tui, &default_tui_args(), initial_tab)? {
                    TuiRunOutcome::OpenAgentChat { resume } => {
                        if let Some(resume) = resume {
                            args.resume = Some(resume);
                        }
                    }
                    TuiRunOutcome::Exit => return Ok(()),
                    TuiRunOutcome::Action(action) => {
                        tui.finish()?;
                        handle_tui_action(action, None)?;
                        return Ok(());
                    }
                }
            }
            AgentChatOutcome::Command { resume, command } => {
                args.resume = Some(resume.clone());
                match command {
                    djinn_tui::AgentChatCommand::OpenHelp => {}
                    djinn_tui::AgentChatCommand::ToggleSidebar
                    | djinn_tui::AgentChatCommand::ToggleThoughtDetail
                    | djinn_tui::AgentChatCommand::ScrollHalfPageUp
                    | djinn_tui::AgentChatCommand::ScrollHalfPageDown
                    | djinn_tui::AgentChatCommand::JumpFirstMessage
                    | djinn_tui::AgentChatCommand::JumpPreviousMessage
                    | djinn_tui::AgentChatCommand::JumpNextMessage
                    | djinn_tui::AgentChatCommand::JumpLastMessage
                    | djinn_tui::AgentChatCommand::JumpLastUserMessage => {}
                    djinn_tui::AgentChatCommand::NewSession => {
                        let store = agent_session_store();
                        let id = AgentSessionId::new(resume);
                        let session = store.load_session(&id)?;
                        prepare_foreground_session_args_from_parent(&mut args, &session, false);
                    }
                    djinn_tui::AgentChatCommand::LaunchChildSession => {
                        let store = agent_session_store();
                        let id = AgentSessionId::new(resume);
                        let session = store.load_session(&id)?;
                        prepare_foreground_session_args_from_parent(&mut args, &session, true);
                    }
                    djinn_tui::AgentChatCommand::OpenSessions => {
                        match run_tui_in_session(
                            &mut tui,
                            &default_tui_args(),
                            djinn_tui::DashboardTab::Sessions,
                        )? {
                            TuiRunOutcome::OpenAgentChat { resume } => {
                                if let Some(resume) = resume {
                                    args.resume = Some(resume);
                                }
                            }
                            TuiRunOutcome::Exit => return Ok(()),
                            TuiRunOutcome::Action(action) => {
                                tui.finish()?;
                                handle_tui_action(action, None)?;
                                return Ok(());
                            }
                        }
                    }
                    djinn_tui::AgentChatCommand::AddCredential => {
                        run_djinn_auth_login_in_terminal(&mut tui)?;
                    }
                    djinn_tui::AgentChatCommand::OpenDashboardTab(initial_tab) => {
                        match run_tui_in_session(&mut tui, &default_tui_args(), initial_tab)? {
                            TuiRunOutcome::OpenAgentChat { resume } => {
                                if let Some(resume) = resume {
                                    args.resume = Some(resume);
                                }
                            }
                            TuiRunOutcome::Exit => return Ok(()),
                            TuiRunOutcome::Action(action) => {
                                tui.finish()?;
                                handle_tui_action(action, None)?;
                                return Ok(());
                            }
                        }
                    }
                    djinn_tui::AgentChatCommand::SwitchProfile(profile) => {
                        let store = agent_session_store();
                        let id = AgentSessionId::new(resume);
                        update_agent_session_profile(&store, &id, &profile)?;
                        args.profile = profile;
                        args.model = None;
                    }
                    djinn_tui::AgentChatCommand::SwitchModel(model) => {
                        let store = agent_session_store();
                        let id = AgentSessionId::new(resume);
                        let session = store.load_session(&id)?;
                        let current_model = resolve_agent_model(
                            args.model
                                .clone()
                                .or_else(|| latest_session_model(&session)),
                            &session.meta.profile,
                        )?;
                        update_agent_session_model(&store, &id, &current_model, &model)?;
                        args.model = Some(model);
                    }
                }
            }
        }
    }
}

fn prepare_foreground_session_args_from_parent(
    args: &mut AgentChatArgs,
    session: &AgentSession,
    child: bool,
) {
    args.model = args.model.clone().or_else(|| latest_session_model(session));
    args.profile = session.meta.profile.clone();
    args.agent = session.meta.agent_name.clone();
    args.parent_session = child.then(|| session.id.to_string());
    args.resume = None;
    args.title = None;
    args.workspace = None;
    args.session_dir = None;
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

fn agent_chat(tui: &mut djinn_tui::TuiSession, args: AgentChatArgs) -> Result<AgentChatOutcome> {
    let store = agent_session_store();
    let session_dir = args
        .session_dir
        .as_deref()
        .map(resolve_session_dir)
        .transpose()?;
    let selection = resolve_agent_role_selection(args.agent, &args.profile, args.model)?;
    let profile = selection.profile;
    let resumed = args
        .resume
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let chat_session = prepare_agent_chat_session(
        &store,
        args.resume.as_deref(),
        args.title,
        args.workspace,
        &profile,
        selection.agent_name.clone(),
        selection.model.clone(),
        selection.instructions.clone(),
        selection.tools.clone(),
        parent_session_id_from_arg(args.parent_session),
    )?;
    let id = chat_session.id;
    let id_string = id.to_string();
    let workspace = chat_session.workspace;
    let profile = chat_session.profile;
    let session = store.load_session(&id)?;
    let kitsune_reporter = KitsuneAgentReporter::from_env().map(KitsuneAgentReporterHandle::new);
    if let Some(reporter) = &kitsune_reporter {
        reporter.report_session(&id_string, if resumed { "resume" } else { "new" });
    }
    let _kitsune_release_guard = KitsuneAgentReleaseGuard::new(kitsune_reporter.clone());
    let system_instructions =
        match resolve_agent_instruction_contents(&workspace, &selection.instructions) {
            Ok(instructions) => instructions,
            Err(error) => {
                if let Some(reporter) = &kitsune_reporter {
                    reporter.report_state(
                        KitsuneAgentReportState::Blocked,
                        &id_string,
                        Some("Agent instruction configuration failed"),
                    );
                }
                return Err(error);
            }
        };
    let model = match resolve_agent_model(
        selection.model.or_else(|| latest_session_model(&session)),
        &profile,
    ) {
        Ok(model) => model,
        Err(error) => {
            if let Some(reporter) = &kitsune_reporter {
                reporter.report_state(
                    KitsuneAgentReportState::Blocked,
                    &id_string,
                    Some("Agent model configuration failed"),
                );
            }
            return Err(error);
        }
    };
    let api_key = args.api_key;
    let base_url = args.base_url;
    let max_tool_rounds = args.max_tool_rounds;
    let allowed_tools = selection.tools;
    if let Some(reporter) = &kitsune_reporter {
        reporter.report_state(
            KitsuneAgentReportState::Idle,
            &id_string,
            Some("Agent session ready"),
        );
    }

    let exit = tui.run_agent_chat_with_progress_handler(
        agent_chat_messages(&session),
        djinn_tui::AgentChatStatus {
            session_id: id_string.clone(),
            workspace: workspace.clone(),
            profile: profile.clone(),
            model: model.clone(),
            notice: String::new(),
            command_palette: agent_chat_command_palette(&profile, &model)?,
        },
        |prompt, progress| {
            store.append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: prompt.clone(),
                }),
            )?;
            append_foreground_session_lifecycle_event(
                &store,
                &id,
                AgentSessionLifecycleState::Running,
                "agent turn started",
                None,
            )?;
            maybe_auto_title_agent_session(&store, &id, &prompt)?;
            let session = store.load_session(&id)?;
            let mut progress_timeline = vec![agent_thought_message(initial_agent_thought_detail(
                &prompt,
                max_tool_rounds,
            ))];
            progress(
                agent_chat_messages_with_progress(&session, &progress_timeline),
                "Waiting for model response…".to_string(),
            )?;
            if let Some(reporter) = &kitsune_reporter {
                reporter.report_state(
                    KitsuneAgentReportState::Working,
                    &id_string,
                    Some("Waiting for model response"),
                );
            }
            let completion = complete_openai_messages_with_progress(
                &store,
                &id,
                agent_model_messages(&session, &workspace, &system_instructions),
                model.clone(),
                api_key.clone(),
                base_url.clone(),
                max_tool_rounds,
                &profile,
                allowed_tools.clone(),
                true,
                kitsune_reporter.clone(),
                |event| {
                    let session = store.load_session(&id)?;
                    let notice = agent_progress_notice(&event, max_tool_rounds);
                    if let Some(reporter) = &kitsune_reporter {
                        reporter.report_state(
                            KitsuneAgentReportState::Working,
                            &id_string,
                            Some(&notice),
                        );
                    }
                    if let Some(message) = agent_progress_message(&event, max_tool_rounds) {
                        progress_timeline.push(message);
                    }
                    let messages = agent_chat_messages_with_progress(&session, &progress_timeline);
                    progress(messages, notice)
                },
            );
            match completion {
                Ok(_) => {
                    append_foreground_session_lifecycle_event(
                        &store,
                        &id,
                        AgentSessionLifecycleState::Paused,
                        "agent turn completed",
                        Some("ready for next prompt".to_string()),
                    )?;
                    if let Some(reporter) = &kitsune_reporter {
                        reporter.report_state(
                            KitsuneAgentReportState::Idle,
                            &id_string,
                            Some("Ready for prompt"),
                        );
                    }
                }
                Err(error) => {
                    let _ = append_foreground_session_lifecycle_event(
                        &store,
                        &id,
                        AgentSessionLifecycleState::Failed,
                        "agent turn failed",
                        Some(error.to_string()),
                    );
                    if let Some(reporter) = &kitsune_reporter {
                        reporter.report_state(
                            KitsuneAgentReportState::Blocked,
                            &id_string,
                            Some(kitsune_blocked_message_for_error(&error)),
                        );
                    }
                    return Err(error);
                }
            }
            let session = store.load_session(&id)?;
            if let Some(session_dir) = &session_dir {
                project_agent_session_dir(
                    session_dir,
                    &session,
                    &prompt,
                    latest_agent_assistant_message(&session).unwrap_or_default(),
                )?;
            }
            Ok(agent_chat_messages(&session))
        },
    )?;

    if let djinn_tui::AgentChatExit::Dashboard { initial_tab } = exit {
        mark_foreground_session_paused_if_not_terminal(
            &store,
            &id,
            "chat suspended for dashboard",
        )?;
        return Ok(AgentChatOutcome::Dashboard {
            resume: id_string,
            initial_tab,
        });
    }
    if let djinn_tui::AgentChatExit::Command(command) = exit {
        mark_foreground_session_paused_if_not_terminal(&store, &id, "chat suspended for command")?;
        return Ok(AgentChatOutcome::Command {
            resume: id_string,
            command,
        });
    }

    mark_foreground_session_paused_on_quit(&store, &id)?;
    let session = store.load_session(&id)?;
    Ok(AgentChatOutcome::Quit {
        session_id: id_string,
        title: session.meta.title,
        path: store.session_file_path(&id),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KitsuneAgentReportState {
    Idle,
    Working,
    Blocked,
}

impl KitsuneAgentReportState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone)]
struct KitsuneAgentReporter {
    bin: String,
    pane_id: String,
    seq: u64,
}

#[derive(Debug, Clone)]
struct KitsuneAgentReporterHandle {
    inner: Arc<Mutex<KitsuneAgentReporter>>,
}

impl KitsuneAgentReporterHandle {
    fn new(reporter: KitsuneAgentReporter) -> Self {
        Self {
            inner: Arc::new(Mutex::new(reporter)),
        }
    }

    fn report_session(&self, session_id: &str, session_start_source: &str) {
        if let Ok(mut reporter) = self.inner.lock() {
            reporter.report_session(session_id, session_start_source);
        }
    }

    fn report_state(
        &self,
        state: KitsuneAgentReportState,
        session_id: &str,
        message: Option<&str>,
    ) {
        if let Ok(mut reporter) = self.inner.lock() {
            reporter.report_state(state, session_id, message);
        }
    }

    fn release_agent(&self) {
        if let Ok(mut reporter) = self.inner.lock() {
            reporter.release_agent();
        }
    }
}

#[derive(Debug)]
struct KitsuneAgentReleaseGuard {
    reporter: Option<KitsuneAgentReporterHandle>,
}

impl KitsuneAgentReleaseGuard {
    fn new(reporter: Option<KitsuneAgentReporterHandle>) -> Self {
        Self { reporter }
    }
}

impl Drop for KitsuneAgentReleaseGuard {
    fn drop(&mut self) {
        if let Some(reporter) = self.reporter.take() {
            reporter.release_agent();
        }
    }
}

impl KitsuneAgentReporter {
    fn from_env() -> Option<Self> {
        if env::var("DJINN_KITSUNE_REPORT_DISABLED").ok().as_deref() == Some("1") {
            return None;
        }
        if env::var("KITSUNE_ENV").ok().as_deref() != Some("1") {
            return None;
        }
        let pane_id = env::var("KITSUNE_PANE_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())?;
        let bin = env::var("KITSUNE_BIN_PATH")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "kitsune".to_string());
        Some(Self {
            bin,
            pane_id,
            seq: 0,
        })
    }

    fn report_session(&mut self, session_id: &str, session_start_source: &str) {
        let seq = self.next_seq();
        self.run(kitsune_agent_session_report_args(
            &self.pane_id,
            seq,
            session_id,
            session_start_source,
        ));
    }

    fn report_state(
        &mut self,
        state: KitsuneAgentReportState,
        session_id: &str,
        message: Option<&str>,
    ) {
        let seq = self.next_seq();
        self.run(kitsune_agent_state_report_args(
            &self.pane_id,
            state,
            seq,
            session_id,
            message,
        ));
    }

    fn release_agent(&mut self) {
        let seq = self.next_seq();
        self.run(kitsune_agent_release_report_args(&self.pane_id, seq));
    }

    fn next_seq(&mut self) -> u64 {
        self.seq = self.seq.saturating_add(1);
        self.seq
    }

    fn run(&self, args: Vec<String>) {
        let _ = ProcessCommand::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn kitsune_agent_session_report_args(
    pane_id: &str,
    seq: u64,
    session_id: &str,
    session_start_source: &str,
) -> Vec<String> {
    vec![
        "pane".to_string(),
        "report-agent-session".to_string(),
        pane_id.to_string(),
        "--source".to_string(),
        "kitsune:djinn".to_string(),
        "--agent".to_string(),
        "djinn".to_string(),
        "--seq".to_string(),
        seq.to_string(),
        "--agent-session-id".to_string(),
        session_id.to_string(),
        "--session-start-source".to_string(),
        session_start_source.to_string(),
    ]
}

fn kitsune_agent_state_report_args(
    pane_id: &str,
    state: KitsuneAgentReportState,
    seq: u64,
    session_id: &str,
    message: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "pane".to_string(),
        "report-agent".to_string(),
        pane_id.to_string(),
        "--source".to_string(),
        "kitsune:djinn".to_string(),
        "--agent".to_string(),
        "djinn".to_string(),
        "--state".to_string(),
        state.as_str().to_string(),
        "--seq".to_string(),
        seq.to_string(),
        "--agent-session-id".to_string(),
        session_id.to_string(),
    ];
    if let Some(message) = message.map(str::trim).filter(|value| !value.is_empty()) {
        args.push("--message".to_string());
        args.push(message.to_string());
    }
    args
}

fn kitsune_agent_release_report_args(pane_id: &str, seq: u64) -> Vec<String> {
    vec![
        "pane".to_string(),
        "release-agent".to_string(),
        pane_id.to_string(),
        "--source".to_string(),
        "kitsune:djinn".to_string(),
        "--agent".to_string(),
        "djinn".to_string(),
        "--seq".to_string(),
        seq.to_string(),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedAgentChatSession {
    id: AgentSessionId,
    workspace: String,
    profile: String,
}

fn prepare_agent_chat_session(
    store: &JsonlAgentSessionStore,
    resume: Option<&str>,
    title: Option<String>,
    workspace: Option<PathBuf>,
    profile: &str,
    agent_name: Option<String>,
    model: Option<String>,
    agent_instructions: Vec<String>,
    agent_tools: Vec<String>,
    parent_session_id: Option<AgentSessionId>,
) -> Result<PreparedAgentChatSession> {
    if let Some(resume) = resume.map(str::trim).filter(|value| !value.is_empty()) {
        let id = AgentSessionId::new(resume.to_string());
        let session = store.load_session(&id)?;
        let workspace = if session.meta.workspace.trim().is_empty() {
            resolve_agent_workspace(None)?
        } else {
            session.meta.workspace
        };
        let profile = if session.meta.profile.trim().is_empty() {
            "default".to_string()
        } else {
            session.meta.profile
        };
        return Ok(PreparedAgentChatSession {
            id,
            workspace,
            profile,
        });
    }

    validate_agent_child_session_depth(store, parent_session_id.as_ref())?;
    let workspace = resolve_agent_workspace(workspace)?;
    let resolved_model = resolve_agent_model(model, profile)?;
    let effective_config = agent_effective_config_from_parts(
        workspace.clone(),
        profile.to_string(),
        resolved_model,
        agent_name.clone(),
        agent_instructions,
        agent_tools,
    )?;
    let meta = AgentSessionMeta {
        title: title.unwrap_or_else(|| "Agent chat".to_string()),
        workspace: workspace.clone(),
        profile: profile.to_string(),
        agent_name,
        parent_session_id,
        source: "djinn-agent".to_string(),
        runtime_config: Some(agent_session_runtime_config(&effective_config)),
        ..AgentSessionMeta::default()
    };
    let id = store.create_session(meta)?;
    Ok(PreparedAgentChatSession {
        id,
        workspace,
        profile: profile.to_string(),
    })
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
    let title = prompt_title(prompt, "Agent chat")
        .trim_matches(|ch: char| ch.is_ascii_punctuation() || ch.is_whitespace())
        .trim()
        .to_string();
    if title.is_empty() {
        "Agent chat".to_string()
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

fn agent_chat_command_palette(
    profile: &str,
    model: &str,
) -> Result<Vec<djinn_tui::AgentChatCommandEntry>> {
    let mut entries = Vec::new();
    for candidate in agent_profile_options(profile)? {
        let current = same_agent_option(&candidate, profile);
        entries.push(djinn_tui::AgentChatCommandEntry {
            section: "Profile".to_string(),
            label: if current {
                format!("✓ Current profile · {candidate}")
            } else {
                format!("Switch profile · {candidate}")
            },
            description: if current {
                "current profile".to_string()
            } else {
                "Use this OpenCode/Djinn profile for future turns".to_string()
            },
            command: djinn_tui::AgentChatCommand::SwitchProfile(candidate),
        });
    }
    for candidate in agent_model_options(model)? {
        let current = same_agent_option(&candidate, model);
        entries.push(djinn_tui::AgentChatCommandEntry {
            section: "Model".to_string(),
            label: if current {
                format!("✓ Current model · {candidate}")
            } else {
                format!("Switch model · {candidate}")
            },
            description: if current {
                "current model".to_string()
            } else {
                "Use this model for future turns".to_string()
            },
            command: djinn_tui::AgentChatCommand::SwitchModel(candidate),
        });
    }
    Ok(entries)
}

fn run_djinn_auth_login_in_terminal(tui: &mut djinn_tui::TuiSession) -> Result<()> {
    tui.suspend()?;
    let result = auth_login(AuthLoginArgs {
        provider: None,
        method: None,
    });
    println!("Press Enter to return to Djinn.");
    let _ = io::stdin().read_line(&mut String::new());
    tui.resume()?;
    result
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

fn update_agent_session_profile(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    profile: &str,
) -> Result<bool> {
    let session = store.load_session(id)?;
    if same_agent_option(&session.meta.profile, profile) {
        return Ok(false);
    }
    store.append_event(
        id,
        AgentSessionEvent::new(AgentSessionEventKind::SessionProfileUpdated {
            profile: profile.to_string(),
        }),
    )?;
    Ok(true)
}

fn update_agent_session_model(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    current_model: &str,
    model: &str,
) -> Result<bool> {
    if same_agent_option(current_model, model) {
        return Ok(false);
    }
    store.append_event(
        id,
        AgentSessionEvent::new(AgentSessionEventKind::SessionModelUpdated {
            model: model.to_string(),
        }),
    )?;
    Ok(true)
}

#[allow(dead_code)]
fn update_agent_session_title(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    title: &str,
) -> Result<bool> {
    let title = title.trim();
    if title.is_empty() {
        bail!("agent session title cannot be empty");
    }
    let session = store.load_session(id)?;
    if session.meta.title.trim() == title {
        return Ok(false);
    }
    store.append_event(
        id,
        AgentSessionEvent::new(AgentSessionEventKind::SessionTitleUpdated {
            title: title.to_string(),
        }),
    )?;
    Ok(true)
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

fn complete_openai_prompt(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    prompt: String,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    max_tool_rounds: usize,
    profile: &str,
    system_instructions: &[ResolvedAgentInstruction],
    allowed_tools: Vec<String>,
    interactive_permissions: bool,
) -> Result<djinn_agent::ModelResponse> {
    let workspace = store.load_session(id)?.meta.workspace;
    complete_openai_messages(
        store,
        id,
        vec![
            agent_system_message(&workspace, system_instructions),
            ModelMessage {
                role: ModelRole::User,
                content: prompt,
                tool_call_id: None,
                tool_calls: Vec::new(),
            },
        ],
        model,
        api_key,
        base_url,
        max_tool_rounds,
        profile,
        allowed_tools,
        interactive_permissions,
    )
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
        None,
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
    kitsune_reporter: Option<KitsuneAgentReporterHandle>,
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
            kitsune_reporter,
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
        kitsune_reporter,
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
    kitsune_reporter: Option<KitsuneAgentReporterHandle>,
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
        Some(Arc::new(TerminalPermissionGate::new(
            kitsune_reporter,
            id.to_string(),
        )))
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

fn agent_chat_messages(session: &AgentSession) -> Vec<djinn_tui::AgentChatMessage> {
    let mut calls = HashMap::new();
    let mut messages = Vec::new();
    for event in &session.events {
        match &event.kind {
            AgentSessionEventKind::UserMessage { content } => {
                messages.push(djinn_tui::AgentChatMessage {
                    role: djinn_tui::AgentChatRole::User,
                    content: content.clone(),
                });
            }
            AgentSessionEventKind::AssistantMessage { content } if !content.trim().is_empty() => {
                messages.push(djinn_tui::AgentChatMessage {
                    role: djinn_tui::AgentChatRole::Assistant,
                    content: content.clone(),
                });
            }
            AgentSessionEventKind::ToolCall { id, name, input } => {
                let call = AgentToolCallSummary {
                    name: name.clone(),
                    invocation: summarize_agent_tool_input(name, input),
                };
                calls.insert(id.clone(), call.clone());
                messages.push(djinn_tui::AgentChatMessage {
                    role: djinn_tui::AgentChatRole::Tool,
                    content: format_agent_tool_call_message(name, input),
                });
            }
            AgentSessionEventKind::ToolResult {
                id,
                success,
                output,
            } => {
                let call = calls.get(id);
                messages.push(djinn_tui::AgentChatMessage {
                    role: djinn_tui::AgentChatRole::ToolOutput,
                    content: summarize_agent_tool_result(id, call, output, *success),
                });
            }
            AgentSessionEventKind::Error {
                phase,
                message,
                details,
            } => {
                messages.push(djinn_tui::AgentChatMessage {
                    role: djinn_tui::AgentChatRole::Notice,
                    content: format_agent_error_message(phase, message, details.as_ref()),
                });
            }
            AgentSessionEventKind::Summary { content } => {
                messages.push(djinn_tui::AgentChatMessage {
                    role: djinn_tui::AgentChatRole::Notice,
                    content: format!("summary: {content}"),
                })
            }
            AgentSessionEventKind::Checkpoint { label } => {
                messages.push(djinn_tui::AgentChatMessage {
                    role: djinn_tui::AgentChatRole::Notice,
                    content: format!("checkpoint: {label}"),
                })
            }
            AgentSessionEventKind::SessionCreated { .. }
            | AgentSessionEventKind::SessionTitleUpdated { .. }
            | AgentSessionEventKind::SessionProfileUpdated { .. }
            | AgentSessionEventKind::SessionModelUpdated { .. }
            | AgentSessionEventKind::ModelResponseMetadata { .. }
            | AgentSessionEventKind::ToolExecutionMetadata { .. }
            | AgentSessionEventKind::SessionLifecycleUpdated { .. }
            | AgentSessionEventKind::ChildSessionStatusChanged { .. }
            | AgentSessionEventKind::AssistantMessage { .. } => {}
        }
    }
    messages
}

fn agent_chat_messages_with_progress(
    session: &AgentSession,
    progress_timeline: &[djinn_tui::AgentChatMessage],
) -> Vec<djinn_tui::AgentChatMessage> {
    let mut messages = agent_chat_messages(session);
    messages.extend(progress_timeline.iter().cloned());
    messages
}

fn latest_agent_assistant_message(session: &AgentSession) -> Option<&str> {
    session
        .events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            AgentSessionEventKind::AssistantMessage { content } if !content.trim().is_empty() => {
                Some(content.trim_end())
            }
            _ => None,
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentToolCallSummary {
    name: String,
    invocation: String,
}

fn agent_thought_message(content: impl Into<String>) -> djinn_tui::AgentChatMessage {
    djinn_tui::AgentChatMessage {
        role: djinn_tui::AgentChatRole::Thought,
        content: content.into(),
    }
}

fn initial_agent_thought_detail(prompt: &str, max_tool_rounds: usize) -> String {
    format!(
        "Waiting for model response…\nPrompt focus: {}\nTool-round safety cap: up to {max_tool_rounds} round{} before Djinn stops this turn; pass --max-tool-rounds N if you want a smaller bound.",
        prompt_title(prompt, "(empty prompt)"),
        plural_suffix(max_tool_rounds)
    )
}

fn format_agent_error_message(phase: &str, message: &str, details: Option<&Value>) -> String {
    let mut lines = vec![format!("error [{phase}]: {message}")];
    if let Some(details) = details.filter(|value| !value.is_null()) {
        lines.push(format!("details: {}", compact_json_value(details)));
    }
    lines.join("\n")
}

fn agent_progress_message(
    event: &AgentProgressEvent,
    max_tool_rounds: usize,
) -> Option<djinn_tui::AgentChatMessage> {
    match event {
        AgentProgressEvent::ModelRequestStarted { round } => Some(agent_thought_message(
            format_agent_model_request_thought(*round, max_tool_rounds),
        )),
        AgentProgressEvent::ModelResponseCompleted {
            round,
            elapsed_ms,
            tool_calls,
            planned_tools,
            has_message,
            ..
        } => {
            let label = if *tool_calls > 0 {
                format!(
                    "Planned {}",
                    progress_tool_call_label(planned_tools, *tool_calls)
                )
            } else if *has_message {
                "Drafted response".to_string()
            } else {
                "Completed model turn".to_string()
            };
            let mut details = vec![format!(
                "Tool-round safety cap: completed model request {} of {}; rounds used {}/{}.",
                round.saturating_add(1),
                max_tool_rounds.saturating_add(1),
                *round,
                max_tool_rounds,
            )];
            details.extend(progress_planned_tool_detail_lines(planned_tools));
            Some(agent_thought_message(format!(
                "{label} · {}\n{}",
                format_elapsed_ms(*elapsed_ms),
                details.join("\n")
            )))
        }
        AgentProgressEvent::ToolCallStarted { call, .. } => {
            let summary = summarize_agent_tool_input(&call.name, &call.input);
            Some(agent_thought_message(format!(
                "Running {}: {}\nInput:\n{}",
                call.name,
                summary,
                progress_tool_input_snippet(&call.name, &call.input)
            )))
        }
        AgentProgressEvent::ToolCallCompleted {
            call,
            result,
            elapsed_ms,
            ..
        } => {
            let summary = summarize_agent_tool_output(&result.output, &call.name);
            Some(agent_thought_message(format!(
                "{} {}: {} · {}\nResult:\n{}",
                if result.success { "Finished" } else { "Failed" },
                call.name,
                summary,
                format_elapsed_ms(*elapsed_ms),
                progress_tool_result_snippet(&call.name, &result.output, result.success)
            )))
        }
    }
}

fn format_agent_model_request_thought(round: usize, max_tool_rounds: usize) -> String {
    format!(
        "Planning next step{}…\nTool-round safety cap: model request {} of {}; rounds used {}/{}.\nDetail: the model is deciding whether to answer now or request another tool call; hidden reasoning is not exposed.",
        progress_round_suffix(round),
        round.saturating_add(1),
        max_tool_rounds.saturating_add(1),
        round,
        max_tool_rounds,
    )
}

fn agent_progress_notice(event: &AgentProgressEvent, max_tool_rounds: usize) -> String {
    match event {
        AgentProgressEvent::ModelRequestStarted { round } => format!(
            "Planning next step{} · tool-round cap {}/{}…",
            progress_round_suffix(*round),
            *round,
            max_tool_rounds,
        ),
        AgentProgressEvent::ModelResponseCompleted {
            tool_calls,
            planned_tools,
            ..
        } if *tool_calls > 0 => format!(
            "Planned {}.",
            progress_tool_call_label(planned_tools, *tool_calls)
        ),
        AgentProgressEvent::ModelResponseCompleted { .. } => "Model response received.".to_string(),
        AgentProgressEvent::ToolCallStarted { call, .. } => format!("Running {}…", call.name),
        AgentProgressEvent::ToolCallCompleted { call, result, .. } => format!(
            "{} {}.",
            if result.success { "Finished" } else { "Failed" },
            call.name
        ),
    }
}

fn progress_tool_call_label(planned_tools: &[djinn_agent::ModelToolCall], count: usize) -> String {
    if planned_tools.is_empty() {
        return format!("{count} tool call{}", plural_suffix(count));
    }
    let mut counts = Vec::<(&str, usize)>::new();
    for call in planned_tools {
        if let Some((_, count)) = counts
            .iter_mut()
            .find(|(name, _)| *name == call.name.as_str())
        {
            *count += 1;
        } else {
            counts.push((call.name.as_str(), 1));
        }
    }
    let labels = counts
        .into_iter()
        .map(|(name, count)| {
            if count > 1 {
                format!("{name} ×{count}")
            } else {
                name.to_string()
            }
        })
        .collect::<Vec<_>>();
    let visible = labels
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if labels.len() > 3 {
        format!("{visible}, +{} more", labels.len() - 3)
    } else {
        visible
    }
}

fn progress_planned_tool_detail_lines(planned_tools: &[djinn_agent::ModelToolCall]) -> Vec<String> {
    planned_tools
        .iter()
        .take(6)
        .flat_map(|call| {
            let mut lines = vec![format!(
                "Planned tool: {}: {}",
                call.name,
                summarize_agent_tool_input(&call.name, &call.input)
            )];
            lines.push("Input snippet:".to_string());
            lines.extend(
                progress_tool_input_snippet(&call.name, &call.input)
                    .lines()
                    .map(str::to_string),
            );
            lines
        })
        .collect()
}

fn progress_tool_input_snippet(name: &str, input: &Value) -> String {
    match name {
        "shell" => {
            let command = input.get("command").and_then(Value::as_str).unwrap_or("");
            let workdir = input.get("workdir").and_then(Value::as_str).unwrap_or(".");
            format!("workdir: {workdir}\n$ {command}")
        }
        "read_file" | "list_dir" => input
            .get("path")
            .and_then(Value::as_str)
            .map(|path| format!("path: {path}"))
            .unwrap_or_else(|| compact_json_value(input)),
        "find_files" => {
            let pattern = input.get("pattern").and_then(Value::as_str).unwrap_or("*");
            let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
            format!("pattern: {pattern}\npath: {path}")
        }
        "search_files" => {
            let pattern = input.get("pattern").and_then(Value::as_str).unwrap_or("");
            let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
            format!("pattern: {pattern}\npath: {path}")
        }
        "apply_patch" => input
            .get("patch")
            .and_then(Value::as_str)
            .map(|patch| progress_text_snippet("patch", patch, 8))
            .unwrap_or_else(|| compact_json_value(input)),
        "write_file" => {
            let path = input.get("path").and_then(Value::as_str).unwrap_or("file");
            let content = input.get("content").and_then(Value::as_str).unwrap_or("");
            format!(
                "path: {path}\n{}",
                progress_text_snippet("content", content, 6)
            )
        }
        "edit_file" => {
            let path = input.get("path").and_then(Value::as_str).unwrap_or("file");
            let old_text = input.get("old_text").and_then(Value::as_str).unwrap_or("");
            let new_text = input.get("new_text").and_then(Value::as_str).unwrap_or("");
            format!(
                "path: {path}\n{}\n{}",
                progress_text_snippet("old", old_text, 4),
                progress_text_snippet("new", new_text, 4)
            )
        }
        _ => compact_json_value(input),
    }
}

fn progress_tool_result_snippet(name: &str, output: &Value, success: bool) -> String {
    let status = if success { "ok" } else { "failed" };
    match name {
        "read_file" => {
            let path = output
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown path");
            let content = output.get("content").and_then(Value::as_str).unwrap_or("");
            format!(
                "path: {path}\n{} bytes, {} lines\n{}",
                content.len(),
                content.lines().count(),
                progress_text_snippet("preview", content, 8)
            )
        }
        "apply_patch" | "write_file" | "edit_file" => {
            summarize_mutation_result(name, status, output)
        }
        "shell" => summarize_shell_result(status, None, output),
        "list_dir" | "find_files" | "search_files" => {
            summarize_matches_result(name, status, output)
        }
        _ => summarize_agent_tool_output(output, name),
    }
}

fn progress_text_snippet(label: &str, value: &str, max_lines: usize) -> String {
    let mut lines = vec![format!("{label}:")];
    let total = value.lines().count();
    for line in value.lines().take(max_lines) {
        lines.push(truncate_agent_line(line, 160));
    }
    if total > max_lines {
        lines.push(format!("… {} more lines", total - max_lines));
    }
    if total == 0 {
        lines.push("(empty)".to_string());
    }
    lines.join("\n")
}

fn kitsune_blocked_message_for_error(error: &anyhow::Error) -> &'static str {
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("auth")
        || message.contains("api key")
        || message.contains("token")
        || message.contains("login")
        || message.contains("provider")
        || message.contains("config")
        || message.contains("model")
    {
        "Agent needs provider auth or configuration"
    } else if message.contains("permission") {
        "Agent needs permission approval"
    } else {
        "Agent turn failed"
    }
}

fn progress_round_suffix(round: usize) -> String {
    if round == 0 {
        String::new()
    } else {
        format!(" (round {})", round + 1)
    }
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn format_elapsed_ms(elapsed_ms: u128) -> String {
    if elapsed_ms >= 1_000 {
        format!("{:.1}s", elapsed_ms as f64 / 1_000.0)
    } else {
        format!("{elapsed_ms}ms")
    }
}

fn format_agent_tool_call_message(name: &str, input: &Value) -> String {
    if name == "shell" {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        let workdir = input.get("workdir").and_then(Value::as_str).unwrap_or(".");
        if command.trim().is_empty() {
            return "shell".to_string();
        }
        return format!("# Running in {workdir}\n$ {command}");
    }
    format!("{name}: {}", summarize_agent_tool_input(name, input))
}

fn summarize_agent_tool_input(name: &str, input: &Value) -> String {
    match name {
        "shell" => input
            .get("command")
            .and_then(Value::as_str)
            .map(|command| format!("`{command}`{}", optional_workdir(input)))
            .unwrap_or_else(|| compact_json_value(input)),
        "read_file" | "list_dir" => input
            .get("path")
            .and_then(Value::as_str)
            .map(|path| path.to_string())
            .unwrap_or_else(|| compact_json_value(input)),
        "find_files" => {
            let pattern = input.get("pattern").and_then(Value::as_str).unwrap_or("*");
            let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
            format!("{pattern} in {path}")
        }
        "search_files" => {
            let pattern = input.get("pattern").and_then(Value::as_str).unwrap_or("");
            let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
            format!("/{pattern}/ in {path}")
        }
        "apply_patch" => "workspace patch".to_string(),
        "write_file" => {
            let path = input.get("path").and_then(Value::as_str).unwrap_or("file");
            let content = input.get("content").and_then(Value::as_str).unwrap_or("");
            format!(
                "{path} ({} bytes, {} lines)",
                content.len(),
                content.lines().count()
            )
        }
        "edit_file" => {
            let path = input.get("path").and_then(Value::as_str).unwrap_or("file");
            let old_lines = input
                .get("old_text")
                .and_then(Value::as_str)
                .map(str::lines)
                .map(Iterator::count)
                .unwrap_or_default();
            let new_lines = input
                .get("new_text")
                .and_then(Value::as_str)
                .map(str::lines)
                .map(Iterator::count)
                .unwrap_or_default();
            format!("{path} (+{new_lines}/-{old_lines})")
        }
        _ => compact_json_value(input),
    }
}

fn summarize_agent_tool_result(
    id: &str,
    call: Option<&AgentToolCallSummary>,
    output: &Value,
    success: bool,
) -> String {
    let tool = call
        .map(|call| call.name.as_str())
        .or_else(|| output.get("tool").and_then(Value::as_str))
        .unwrap_or("tool");
    let status = if success { "ok" } else { "failed" };
    match tool {
        "shell" => summarize_shell_result(status, call, output),
        "read_file" => summarize_read_file_result(status, output),
        "list_dir" | "find_files" | "search_files" => {
            summarize_matches_result(tool, status, output)
        }
        "apply_patch" | "write_file" | "edit_file" => {
            summarize_mutation_result(tool, status, output)
        }
        _ => format!(
            "{tool} result: {status}\n{}",
            summarize_agent_tool_output(output, id)
        ),
    }
}

fn optional_workdir(input: &Value) -> String {
    input
        .get("workdir")
        .and_then(Value::as_str)
        .filter(|workdir| !workdir.trim().is_empty())
        .map(|workdir| format!(" in {workdir}"))
        .unwrap_or_default()
}

fn summarize_shell_result(
    status: &str,
    call: Option<&AgentToolCallSummary>,
    output: &Value,
) -> String {
    let mut lines = vec![format!("shell result: {status}")];
    if let Some(call) = call {
        lines.push(format!("command: {}", call.invocation));
    } else if let Some(command) = output.get("command").and_then(Value::as_str) {
        lines.push(format!("command: `{command}`"));
    }
    let mut meta = Vec::new();
    if let Some(exit_code) = output.get("exit_code").and_then(Value::as_i64) {
        meta.push(format!("exit {exit_code}"));
    }
    if output
        .get("timed_out")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        meta.push("timed out".to_string());
    }
    if let Some(duration_ms) = output.get("duration_ms").and_then(Value::as_u64) {
        meta.push(format!("{duration_ms}ms"));
    }
    if !meta.is_empty() {
        lines.push(meta.join(" • "));
    }
    push_output_block(
        &mut lines,
        "stdout",
        output.get("stdout").and_then(Value::as_str),
    );
    push_output_block(
        &mut lines,
        "stderr",
        output.get("stderr").and_then(Value::as_str),
    );
    if lines.len() == 1 {
        lines.push(summarize_agent_tool_output(output, "shell"));
    }
    lines.join("\n")
}

fn summarize_read_file_result(status: &str, output: &Value) -> String {
    let path = output
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("unknown path");
    let content = output.get("content").and_then(Value::as_str).unwrap_or("");
    format!(
        "read_file result: {status}\npath: {path}\n{} bytes, {} lines",
        content.len(),
        content.lines().count()
    )
}

fn summarize_matches_result(tool: &str, status: &str, output: &Value) -> String {
    let path = output.get("path").and_then(Value::as_str).unwrap_or(".");
    let matches = output
        .get("matches")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut lines = vec![format!("{tool} result: {status}"), format!("path: {path}")];
    lines.push(format!("{} matches", matches.len()));
    for item in matches.iter().take(5) {
        let label = item
            .get("relative_path")
            .or_else(|| item.get("path"))
            .and_then(Value::as_str)
            .unwrap_or("match");
        lines.push(format!("- {label}"));
    }
    if matches.len() > 5 {
        lines.push(format!("… {} more", matches.len() - 5));
    }
    lines.join("\n")
}

fn summarize_mutation_result(tool: &str, status: &str, output: &Value) -> String {
    let mut lines = vec![format!("{tool} result: {status}")];
    if let Some(patch_id) = output.get("patch_id").and_then(Value::as_str) {
        lines.push(format!("patch: {patch_id}"));
    }
    if let Some(summary) = output.get("summary").and_then(Value::as_array) {
        lines.push(format!(
            "{} operation{}",
            summary.len(),
            plural_suffix(summary.len())
        ));
        for item in summary.iter().take(6) {
            let operation = item
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("mutation");
            let path = item
                .get("relative_path")
                .or_else(|| item.get("path"))
                .and_then(Value::as_str)
                .unwrap_or("file");
            let added = item
                .get("lines_added")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let removed = item
                .get("lines_removed")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            if let Some(new_path) = item.get("relative_new_path").and_then(Value::as_str) {
                lines.push(format!(
                    "- {operation} {path} -> {new_path} (+{added}/-{removed})"
                ));
            } else {
                lines.push(format!("- {operation} {path} (+{added}/-{removed})"));
            }
        }
        if summary.len() > 6 {
            lines.push(format!("… {} more operations", summary.len() - 6));
        }
    } else if let Some(preview) = output.get("preview").and_then(Value::as_array) {
        lines.push("approval required".to_string());
        for item in preview.iter().take(6) {
            let operation = item
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("mutation");
            let path = item
                .get("relative_path")
                .or_else(|| item.get("path"))
                .and_then(Value::as_str)
                .unwrap_or("file");
            lines.push(format!("- {operation} {path}"));
        }
    }
    if lines.len() == 1 {
        lines.push(summarize_agent_tool_output(output, tool));
    }
    lines.join("\n")
}

fn push_output_block(lines: &mut Vec<String>, label: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    lines.push(format!("{label}:"));
    for line in value.lines().take(8) {
        lines.push(line.to_string());
    }
    let line_count = value.lines().count();
    if line_count > 8 {
        lines.push(format!("… {} more lines", line_count - 8));
    }
}

fn compact_json_value(value: &Value) -> String {
    truncate_agent_line(&value.to_string(), 160)
}

fn truncate_agent_line(value: &str, max_chars: usize) -> String {
    let line = value.lines().next().unwrap_or(value).trim();
    let mut chars = line.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn summarize_agent_tool_output(output: &Value, fallback: &str) -> String {
    if let Some(error) = output.get("error").and_then(Value::as_str) {
        return prompt_title(error, "error");
    }
    if let Some(stdout) = output.get("stdout").and_then(Value::as_str) {
        let title = prompt_title(stdout, "no stdout");
        if !title.is_empty() && title != "no stdout" {
            return title;
        }
    }
    if let Some(path) = output.get("path").and_then(Value::as_str) {
        return path.to_string();
    }
    if let Some(matches) = output.get("matches").and_then(Value::as_array) {
        return format!("{} matches", matches.len());
    }
    match output {
        Value::Object(map) => format!("{} fields", map.len()),
        Value::Array(values) => format!("{} items", values.len()),
        Value::String(value) => prompt_title(value, fallback),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => "null".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TuiRunOutcome {
    OpenAgentChat { resume: Option<String> },
    Exit,
    Action(djinn_tui::TuiAction),
}

fn run_tui(args: TuiArgs) -> Result<Option<AgentChatArgs>> {
    let initial_tab = dashboard_tab(args.view);
    let mut tui = djinn_tui::TuiSession::enter()?;
    let outcome = run_tui_in_session(&mut tui, &args, initial_tab)?;
    tui.finish()?;
    match outcome {
        TuiRunOutcome::OpenAgentChat { resume } => Ok(Some(AgentChatArgs {
            resume,
            ..default_agent_chat_args()
        })),
        TuiRunOutcome::Exit => Ok(None),
        TuiRunOutcome::Action(action) => {
            handle_tui_action(action, args.editor)?;
            Ok(None)
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
    let chats = chats_for_session_picker()?;
    let memories = memory_store().list()?;
    let suggestions = suggestion_store().list()?;
    let skills = skill_records()?;
    let active_context = context_store().active()?;
    let Some(action) = tui.run_dashboard_with_handler(
        tools,
        chats,
        memories,
        suggestions,
        skills,
        active_context,
        initial_tab,
        |action| match action {
            djinn_tui::TuiAction::DeleteMemories(ids) => remove_memories_silent(&ids).map(|_| ()),
            djinn_tui::TuiAction::DeleteChatRows(request) => {
                delete_chat_rows_silent(&request).map(|_| ())
            }
            djinn_tui::TuiAction::DeleteSuggestions(ids) => remove_suggestions(&ids).map(|_| ()),
            djinn_tui::TuiAction::OpenAgentChat
            | djinn_tui::TuiAction::OpenChatSession(_)
            | djinn_tui::TuiAction::OpenTool(_)
            | djinn_tui::TuiAction::OpenSkill(_)
            | djinn_tui::TuiAction::PromoteSessions(_)
            | djinn_tui::TuiAction::ReviewMemory(_) => Ok(()),
        },
    )?
    else {
        return Ok(TuiRunOutcome::Exit);
    };

    if action == djinn_tui::TuiAction::OpenAgentChat {
        return Ok(TuiRunOutcome::OpenAgentChat { resume: None });
    }
    if let djinn_tui::TuiAction::OpenChatSession(request) = &action {
        let resume = match request.kind {
            djinn_tui::ChatSessionKind::DjinnAgent => request.session_id.clone(),
            djinn_tui::ChatSessionKind::OpenCode => {
                convert_opencode_chat_to_agent_session(&request.session_id)?.to_string()
            }
        };
        return Ok(TuiRunOutcome::OpenAgentChat {
            resume: Some(resume),
        });
    }
    if let djinn_tui::TuiAction::PromoteSessions(request) = &action {
        if request.mode == djinn_tui::SessionPromoteMode::Summary {
            let id = create_chat_summary_agent_session(request)?;
            return Ok(TuiRunOutcome::OpenAgentChat {
                resume: Some(id.to_string()),
            });
        }
    }
    Ok(TuiRunOutcome::Action(action))
}

fn handle_tui_action(action: djinn_tui::TuiAction, editor: Option<String>) -> Result<bool> {
    match action {
        djinn_tui::TuiAction::OpenAgentChat => Ok(true),
        djinn_tui::TuiAction::OpenChatSession(request) => match request.kind {
            djinn_tui::ChatSessionKind::DjinnAgent => Ok(true),
            djinn_tui::ChatSessionKind::OpenCode => {
                convert_opencode_chat_to_agent_session(&request.session_id).map(|_| true)
            }
        },
        djinn_tui::TuiAction::OpenTool(entry) => open_tool_entry(&entry, editor).map(|_| false),
        djinn_tui::TuiAction::OpenSkill(entry) => open_skill_entry(&entry, editor).map(|_| false),
        djinn_tui::TuiAction::PromoteSessions(request) => promote_sessions(ShareChatsArgs {
            ids: request.chat_ids,
            source: None,
            query: None,
            limit: 10,
            all: false,
            mode: promote_sessions_mode_from_tui(request.mode),
            max_chars_per_chat: 4000,
            max_memories: 20,
            archive: false,
            dry_run: false,
            profile: "default".to_string(),
            model: None,
            api_key: None,
            base_url: None,
        })
        .map(|_| false),
        djinn_tui::TuiAction::ReviewMemory(id) => accept_memory(AcceptMemoryArgs {
            id,
            agent: None,
            title: "djinn memory suggestion review".to_string(),
            opencode_bin: "opencode".to_string(),
            dry_run: false,
        })
        .map(|_| false),
        djinn_tui::TuiAction::DeleteMemories(ids) => remove_memories_silent(&ids).map(|_| false),
        djinn_tui::TuiAction::DeleteChatRows(request) => {
            delete_chat_rows_silent(&request).map(|_| false)
        }
        djinn_tui::TuiAction::DeleteSuggestions(ids) => remove_suggestions(&ids).map(|_| false),
    }
}

fn create_chat_summary_agent_session(
    request: &djinn_tui::SessionPromoteRequest,
) -> Result<AgentSessionId> {
    let args = ShareChatsArgs {
        ids: request.chat_ids.clone(),
        source: None,
        query: None,
        limit: 10,
        all: false,
        mode: ShareChatsMode::Summary,
        max_chars_per_chat: 4000,
        max_memories: 20,
        archive: false,
        dry_run: false,
        profile: "default".to_string(),
        model: None,
        api_key: None,
        base_url: None,
    };
    let records = chat_store().list()?;
    let selected = select_chats_for_share(&records, &args)?;
    let prompt = format_chat_summary_agent_prompt(&selected, &args);
    let title = if selected.len() == 1 {
        format!("Summarize {}", selected[0].title)
    } else {
        format!("Summarize {} selected sessions", selected.len())
    };
    let store = agent_session_store();
    let id = store.create_session(AgentSessionMeta {
        title,
        workspace: resolve_agent_workspace(None)?,
        profile: "default".to_string(),
        source: "djinn-chat-summary".to_string(),
        ..AgentSessionMeta::default()
    })?;
    store.append_event(
        &id,
        AgentSessionEvent::new(AgentSessionEventKind::UserMessage { content: prompt }),
    )?;
    Ok(id)
}

fn chats_for_session_picker() -> Result<Vec<ChatRecord>> {
    let store = agent_session_store();
    let summaries = store.list_sessions(AgentSessionFilter {
        limit: Some(100),
        ..AgentSessionFilter::default()
    })?;
    let summaries_by_id = summaries
        .iter()
        .map(|summary| (summary.id.to_string(), summary.clone()))
        .collect::<HashMap<_, _>>();
    let state = load_opencode_watch_state().unwrap_or_default();
    let mut chats = chat_store()
        .list()?
        .into_iter()
        .map(|chat| {
            opencode_bridge_session_id(&state, &chat)
                .and_then(|id| summaries_by_id.get(id))
                .map(|summary| converted_opencode_chat_record(&chat, summary, &store))
                .unwrap_or(chat)
        })
        .collect::<Vec<_>>();
    let existing_sessions = chats
        .iter()
        .filter(|chat| chat.source == "djinn-agent" && !chat.source_id.trim().is_empty())
        .map(|chat| chat.source_id.clone())
        .collect::<HashSet<_>>();
    for summary in summaries {
        let id = summary.id.to_string();
        if existing_sessions.contains(&id) {
            continue;
        }
        chats.push(agent_session_chat_record(&summary, &store));
    }
    Ok(chats)
}

fn opencode_bridge_session_id<'a>(
    state: &'a OpencodeWatchState,
    chat: &ChatRecord,
) -> Option<&'a str> {
    if chat.source != "opencode" || chat.source_id.trim().is_empty() {
        return None;
    }
    state
        .sessions
        .get(&chat.source_id)
        .map(|session| session.djinn_session_id.trim())
        .filter(|id| !id.is_empty())
}

fn converted_opencode_chat_record(
    chat: &ChatRecord,
    summary: &AgentSessionSummary,
    store: &JsonlAgentSessionStore,
) -> ChatRecord {
    let mut record = agent_session_chat_record(summary, store);
    record.id = format!("converted:{}:{}", chat.source_id, summary.id);
    record.title = if summary.title.trim().is_empty() {
        format!("{} (converted to Djinn)", chat.title)
    } else {
        format!("{} (converted)", summary.title)
    };
    record.content = format!(
        "Converted from OpenCode session {}.\n\n{}",
        chat.source_id, record.content
    );
    record
}

fn agent_session_chat_record(
    summary: &AgentSessionSummary,
    store: &JsonlAgentSessionStore,
) -> ChatRecord {
    let id = summary.id.to_string();
    let title = if summary.title.trim().is_empty() {
        format!("Djinn agent session {id}")
    } else {
        summary.title.clone()
    };
    let mut content = format!(
        "Djinn agent session\n\nID: {id}\nWorkspace: {}\nProfile: {}\nSource: {}\nEvents: {}\nCreated: {}\nUpdated: {}",
        summary.workspace,
        summary.profile,
        summary.source,
        summary.event_count,
        summary.created_at,
        summary.updated_at
    );
    if let Some(agent_name) = summary
        .agent_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        content.push_str(&format!("\nAgent role: {agent_name}"));
    }
    if let Some(parent_session_id) = &summary.parent_session_id {
        content.push_str(&format!("\nParent session: {parent_session_id}"));
    }

    ChatRecord {
        id: format!("agent:{id}"),
        title,
        content,
        source: "djinn-agent".to_string(),
        source_id: id.clone(),
        source_path: store.session_file_path(&summary.id).display().to_string(),
        content_path: String::new(),
        created_at: summary
            .created_at
            .split('T')
            .next()
            .unwrap_or(&summary.created_at)
            .to_string(),
    }
}

fn convert_opencode_chat_to_agent_session(opencode_session_id: &str) -> Result<AgentSessionId> {
    let opencode_session_id = opencode_session_id.trim();
    if opencode_session_id.is_empty() {
        bail!("OpenCode session id is empty");
    }
    if let Some(existing) = existing_converted_opencode_agent_session(opencode_session_id)? {
        return Ok(existing);
    }

    let chat = chat_store()
        .list()?
        .into_iter()
        .find(|chat| chat.source == "opencode" && chat.source_id == opencode_session_id)
        .with_context(|| format!("finding imported OpenCode chat for {opencode_session_id}"))?;
    let workspace = opencode_export_workspace(&chat.content)
        .or_else(|| {
            env::current_dir()
                .ok()
                .map(|path| path.display().to_string())
        })
        .unwrap_or_default();
    let store = agent_session_store();
    let id = store.create_session(AgentSessionMeta {
        title: if chat.title.trim().is_empty() {
            format!("OpenCode session {opencode_session_id}")
        } else {
            chat.title.clone()
        },
        workspace,
        profile: "default".to_string(),
        source: "opencode".to_string(),
        ..AgentSessionMeta::default()
    })?;
    store.append_event(
        &id,
        AgentSessionEvent::new(AgentSessionEventKind::Checkpoint {
            label: opencode_conversion_checkpoint(opencode_session_id),
        }),
    )?;
    for event in opencode_export_agent_events(&chat.content, opencode_session_id) {
        store.append_event(&id, AgentSessionEvent::new(event))?;
    }
    record_opencode_djinn_bridge(opencode_session_id, &id, &store)?;
    Ok(id)
}

fn record_opencode_djinn_bridge(
    opencode_session_id: &str,
    djinn_session_id: &AgentSessionId,
    store: &JsonlAgentSessionStore,
) -> Result<()> {
    let mut state = load_opencode_watch_state().unwrap_or_default();
    let entry = state
        .sessions
        .entry(opencode_session_id.to_string())
        .or_default();
    entry.djinn_session_id = djinn_session_id.to_string();
    entry.djinn_session_path = store
        .session_file_path(djinn_session_id)
        .display()
        .to_string();
    entry.converted_at = chrono::Local::now().to_rfc3339();
    save_opencode_watch_state(&state)
}

fn existing_converted_opencode_agent_session(
    opencode_session_id: &str,
) -> Result<Option<AgentSessionId>> {
    let store = agent_session_store();
    let checkpoint = opencode_conversion_checkpoint(opencode_session_id);
    for summary in store.list_sessions(AgentSessionFilter {
        source: Some("opencode".to_string()),
        ..AgentSessionFilter::default()
    })? {
        let session = store.load_session(&summary.id)?;
        if session.events.iter().any(|event| {
            matches!(
                &event.kind,
                AgentSessionEventKind::Checkpoint { label } if label == &checkpoint
            )
        }) {
            record_opencode_djinn_bridge(opencode_session_id, &summary.id, &store)?;
            return Ok(Some(summary.id));
        }
    }
    Ok(None)
}

fn opencode_conversion_checkpoint(opencode_session_id: &str) -> String {
    format!("converted-opencode-session:{opencode_session_id}")
}

fn opencode_export_workspace(export: &str) -> Option<String> {
    let value: Value = serde_json::from_str(export).ok()?;
    ["/info/directory", "/info/path/root", "/info/path/cwd"]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn opencode_export_agent_events(export: &str, session_id: &str) -> Vec<AgentSessionEventKind> {
    let Ok(value) = serde_json::from_str::<Value>(export) else {
        return vec![AgentSessionEventKind::Summary {
            content: format!("Converted OpenCode session {session_id}.\n\n{export}"),
        }];
    };
    let mut events = Vec::new();
    for message in value
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = message
            .pointer("/info/role")
            .and_then(Value::as_str)
            .unwrap_or("");
        let content = opencode_message_text(message);
        if content.trim().is_empty() {
            continue;
        }
        match role {
            "user" => events.push(AgentSessionEventKind::UserMessage { content }),
            "assistant" => events.push(AgentSessionEventKind::AssistantMessage { content }),
            _ => events.push(AgentSessionEventKind::Summary {
                content: format!("OpenCode {role} message:\n{content}"),
            }),
        }
    }
    if events.is_empty() {
        events.push(AgentSessionEventKind::Summary {
            content: format!("Converted OpenCode session {session_id}."),
        });
    }
    events
}

fn opencode_message_text(message: &Value) -> String {
    let mut lines = Vec::new();
    for part in message
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match part.get("type").and_then(Value::as_str) {
            Some("text") | Some("reasoning") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    push_nonempty_opencode_line(&mut lines, text);
                }
            }
            Some("tool") => {
                if let Some(title) = part.pointer("/state/title").and_then(Value::as_str) {
                    push_nonempty_opencode_line(&mut lines, &format!("Tool: {title}"));
                } else if let Some(tool) = part.get("tool").and_then(Value::as_str) {
                    push_nonempty_opencode_line(&mut lines, &format!("Tool: {tool}"));
                }
                if let Some(output) = part.pointer("/state/output").and_then(Value::as_str) {
                    push_nonempty_opencode_line(&mut lines, output);
                }
            }
            _ => {}
        }
    }
    lines.join("\n\n")
}

fn push_nonempty_opencode_line(lines: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        lines.push(value.to_string());
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

fn default_tui_args() -> TuiArgs {
    TuiArgs {
        view: TuiView::Tools,
        roots: Vec::new(),
        editor: None,
    }
}

fn default_dashboard_tui_args() -> TuiArgs {
    TuiArgs {
        view: TuiView::Sessions,
        roots: Vec::new(),
        editor: None,
    }
}

fn default_agent_chat_args() -> AgentChatArgs {
    AgentChatArgs {
        resume: None,
        title: None,
        workspace: None,
        session_dir: None,
        profile: "default".to_string(),
        agent: None,
        parent_session: None,
        model: None,
        api_key: None,
        base_url: None,
        max_tool_rounds: DEFAULT_AGENT_MAX_TOOL_ROUNDS,
    }
}

fn promote_sessions_mode_from_tui(mode: djinn_tui::SessionPromoteMode) -> ShareChatsMode {
    match mode {
        djinn_tui::SessionPromoteMode::Summary => ShareChatsMode::Summary,
        djinn_tui::SessionPromoteMode::Pattern => ShareChatsMode::Pattern,
        djinn_tui::SessionPromoteMode::Memories => ShareChatsMode::Memories,
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

fn list_chats(args: ListChatsArgs) -> Result<()> {
    let records = chat_store().list()?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else if records.is_empty() {
        println!("Sessions are empty.");
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!(
                "  {}. [{}] {} — {} chars{}",
                idx + 1,
                record.id,
                record.title,
                record.content.chars().count(),
                format_chat_source_suffix(record)
            );
        }
        println!("\nTotal: {} sessions", records.len());
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

fn add_chat(args: AddChatArgs) -> Result<()> {
    let record = if args.file.as_os_str() == "-" {
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;
        let title = args
            .title
            .clone()
            .or_else(|| args.source_id.clone())
            .unwrap_or_else(|| "stdin chat".to_string());
        chat_store().add_content(
            title,
            content,
            "-".to_string(),
            args.source.as_deref(),
            args.source_id.as_deref(),
        )?
    } else {
        chat_store().add_file(
            &args.file,
            args.title.as_deref(),
            args.source.as_deref(),
            args.source_id.as_deref(),
        )?
    };
    println!(
        "Chat added [{}]: {} ({} chars)",
        record.id,
        record.title,
        record.content.chars().count()
    );
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
    let sources = if args.source_chats.is_empty() {
        Vec::new()
    } else {
        let chats = chat_store().list()?;
        args.source_chats
            .iter()
            .map(|id| resolve_chat(&chats, id).map(memory_source_from_chat))
            .collect::<Result<Vec<_>>>()?
    };
    Ok(MemoryInput {
        text: args.text,
        scope: args.scope,
        kind: args.kind,
        confidence: args.confidence,
        not_before: args.not_before,
        evidence: args.evidence,
        sources,
    })
}

fn memory_source_from_chat(record: &ChatRecord) -> MemorySource {
    MemorySource {
        source_type: "chat".to_string(),
        source: record.source.clone(),
        source_id: record.source_id.clone(),
        chat_id: record.id.clone(),
        title: record.title.clone(),
        captured_at: record.created_at.clone(),
    }
}

fn watch_opencode(args: WatchOpencodeArgs) -> Result<()> {
    if let Some(0) = args.interval {
        bail!("--interval must be greater than zero seconds");
    }

    let cli = djinn_opencode::OpencodeCli::new(args.opencode_bin.clone());
    let sanitize = !args.unsafe_unsanitized;

    loop {
        let mut state = load_opencode_watch_state()?;
        let session_id = match &args.session_id {
            Some(id) => id.clone(),
            None => cli.latest_session_id()?,
        };
        let export = cli.export_session(&session_id, sanitize)?;
        let content_hash = content_hash(&export);
        if state
            .sessions
            .get(&session_id)
            .map(|session| session.content_hash == content_hash)
            .unwrap_or(false)
        {
            println!("OpenCode session unchanged (source-id: {session_id})");
            let Some(seconds) = args.interval else {
                break;
            };
            thread::sleep(Duration::from_secs(seconds));
            continue;
        }
        let title = args
            .title
            .clone()
            .unwrap_or_else(|| djinn_opencode::infer_export_title(&session_id, &export));
        let source_path = if sanitize {
            format!("{} export {} --sanitize", args.opencode_bin, session_id)
        } else {
            format!("{} export {}", args.opencode_bin, session_id)
        };
        let (record, updated) = chat_store().upsert_content(
            title,
            export,
            source_path,
            Some("opencode"),
            Some(&session_id),
        )?;
        state.sessions.insert(
            session_id.clone(),
            OpencodeSessionState {
                content_hash,
                imported_at: chrono::Local::now().to_rfc3339(),
                chat_id: record.id.clone(),
                title: record.title.clone(),
                ..state.sessions.get(&session_id).cloned().unwrap_or_default()
            },
        );
        save_opencode_watch_state(&state)?;
        let action = if updated { "updated" } else { "imported" };
        println!(
            "OpenCode session {action} as chat [{}] (source-id: {})",
            record.id, record.source_id
        );

        let Some(seconds) = args.interval else {
            break;
        };
        thread::sleep(Duration::from_secs(seconds));
    }

    Ok(())
}

fn install_opencode(args: InstallOpencodeArgs) -> Result<()> {
    let config_path = args.config.unwrap_or_else(default_opencode_config_path);
    let plugin_path = args
        .plugin_path
        .map(absolute_path)
        .unwrap_or_else(default_opencode_plugin_path);
    let plugin_entry = opencode_plugin_entry(&config_path, &plugin_path);

    if args.dry_run {
        println!(
            "Would write OpenCode Djinn plugin: {}",
            plugin_path.display()
        );
    } else {
        let changed = djinn_core::write_if_changed(&plugin_path, OPENCODE_PLUGIN.as_bytes())?;
        let status = if changed { "wrote" } else { "unchanged" };
        println!("OpenCode Djinn plugin {status}: {}", plugin_path.display());
    }

    if args.no_config_patch {
        println!("Skipped opencode.json patch. Add this plugin entry manually: {plugin_entry}");
    } else if args.dry_run {
        println!(
            "Would patch OpenCode config: {} (plugin: {plugin_entry})",
            config_path.display()
        );
    } else {
        let changed = patch_opencode_config(&config_path, &plugin_entry)?;
        let status = if changed { "updated" } else { "unchanged" };
        println!(
            "OpenCode config {status}: {} (plugin: {plugin_entry})",
            config_path.display()
        );
    }

    println!("Restart OpenCode for the Djinn plugin to load.");
    Ok(())
}

fn uninstall_opencode(args: OpencodeIntegrationArgs) -> Result<()> {
    let config_path = args.config.unwrap_or_else(default_opencode_config_path);
    let plugin_path = args
        .plugin_path
        .map(absolute_path)
        .unwrap_or_else(default_opencode_plugin_path);
    let plugin_entry = opencode_plugin_entry(&config_path, &plugin_path);

    if plugin_path.exists() {
        fs::remove_file(&plugin_path)
            .with_context(|| format!("removing {}", plugin_path.display()))?;
        println!("Removed OpenCode Djinn plugin: {}", plugin_path.display());
    } else {
        println!(
            "OpenCode Djinn plugin already absent: {}",
            plugin_path.display()
        );
    }

    let changed = unpatch_opencode_config(&config_path, &plugin_entry)?;
    let status = if changed { "updated" } else { "unchanged" };
    println!("OpenCode config {status}: {}", config_path.display());
    println!("Restart OpenCode for plugin changes to take effect.");
    Ok(())
}

fn status_opencode(args: OpencodeIntegrationArgs) -> Result<()> {
    let config_path = args.config.unwrap_or_else(default_opencode_config_path);
    let plugin_path = args
        .plugin_path
        .map(absolute_path)
        .unwrap_or_else(default_opencode_plugin_path);
    let plugin_entry = opencode_plugin_entry(&config_path, &plugin_path);
    let config_contains = opencode_config_contains_plugin(&config_path, &plugin_entry)?;
    let state = load_opencode_watch_state().unwrap_or_default();
    println!("OpenCode Djinn plugin file: {}", plugin_path.display());
    println!("  present: {}", yes_no(plugin_path.exists()));
    println!("OpenCode config: {}", config_path.display());
    println!("  contains plugin entry: {}", yes_no(config_contains));
    println!("Watcher state: {}", opencode_watch_state_path().display());
    println!("  tracked sessions: {}", state.sessions.len());
    for (session_id, session) in state.sessions.iter().take(10) {
        let bridge = if session.djinn_session_id.is_empty() {
            String::new()
        } else {
            format!(", djinn {}", session.djinn_session_id)
        };
        println!(
            "  - {} -> chat {} ({}, {}{})",
            session_id, session.chat_id, session.title, session.imported_at, bridge
        );
    }
    Ok(())
}

fn patch_opencode_config(config_path: &Path, plugin_entry: &str) -> Result<bool> {
    let existing = match fs::read_to_string(config_path) {
        Ok(content) => Some(content),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).with_context(|| format!("reading {}", config_path.display())),
    };
    let (rendered, changed) = patch_opencode_config_content(existing.as_deref(), plugin_entry)
        .with_context(|| format!("patching {}", config_path.display()))?;
    if changed {
        djinn_core::ensure_parent(config_path)?;
        fs::write(config_path, rendered)
            .with_context(|| format!("writing {}", config_path.display()))?;
    }
    Ok(changed)
}

fn unpatch_opencode_config(config_path: &Path, plugin_entry: &str) -> Result<bool> {
    let existing = match fs::read_to_string(config_path) {
        Ok(content) => Some(content),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).with_context(|| format!("reading {}", config_path.display())),
    };
    let Some(existing) = existing else {
        return Ok(false);
    };
    let (rendered, changed) = unpatch_opencode_config_content(&existing, plugin_entry)
        .with_context(|| format!("patching {}", config_path.display()))?;
    if changed {
        djinn_core::ensure_parent(config_path)?;
        fs::write(config_path, rendered)
            .with_context(|| format!("writing {}", config_path.display()))?;
    }
    Ok(changed)
}

fn patch_opencode_config_content(
    existing: Option<&str>,
    plugin_entry: &str,
) -> Result<(String, bool)> {
    let mut value = match existing
        .map(str::trim)
        .filter(|content| !content.is_empty())
    {
        Some(content) => serde_json::from_str::<Value>(content)?,
        None => Value::Object(Map::new()),
    };

    let Value::Object(ref mut object) = value else {
        bail!("OpenCode config must be a JSON object");
    };

    object
        .entry("$schema".to_string())
        .or_insert_with(|| Value::String("https://opencode.ai/config.json".to_string()));
    ensure_opencode_plugin_entry(object, plugin_entry)?;

    let mut rendered = serde_json::to_string_pretty(&value)?;
    rendered.push('\n');
    let changed = existing.map(|content| content != rendered).unwrap_or(true);
    Ok((rendered, changed))
}

fn unpatch_opencode_config_content(existing: &str, plugin_entry: &str) -> Result<(String, bool)> {
    let mut value = serde_json::from_str::<Value>(existing)?;
    let Value::Object(ref mut object) = value else {
        bail!("OpenCode config must be a JSON object");
    };
    let Some(plugin) = object.get_mut("plugin") else {
        let mut rendered = serde_json::to_string_pretty(&value)?;
        rendered.push('\n');
        return Ok((rendered, false));
    };

    let mut changed = false;
    match plugin {
        Value::String(existing_plugin) => {
            if existing_plugin == plugin_entry {
                object.remove("plugin");
                changed = true;
            }
        }
        Value::Array(entries) => {
            let before = entries.len();
            entries.retain(|entry| entry != &Value::String(plugin_entry.to_string()));
            changed = entries.len() != before;
            if entries.is_empty() {
                object.remove("plugin");
            }
        }
        _ => {}
    }
    let mut rendered = serde_json::to_string_pretty(&value)?;
    rendered.push('\n');
    let changed = changed && existing != rendered;
    Ok((rendered, changed))
}

fn opencode_config_contains_plugin(config_path: &Path, plugin_entry: &str) -> Result<bool> {
    let content = match fs::read_to_string(config_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("reading {}", config_path.display())),
    };
    let value = serde_json::from_str::<Value>(&content)?;
    Ok(match value.get("plugin") {
        Some(Value::String(entry)) => entry == plugin_entry,
        Some(Value::Array(entries)) => entries.iter().any(|entry| entry == plugin_entry),
        _ => false,
    })
}

fn ensure_opencode_plugin_entry(object: &mut Map<String, Value>, plugin_entry: &str) -> Result<()> {
    let new_entry = Value::String(plugin_entry.to_string());
    match object.get_mut("plugin") {
        None => {
            object.insert("plugin".to_string(), Value::Array(vec![new_entry]));
        }
        Some(Value::String(existing)) => {
            if existing != plugin_entry {
                let previous = Value::String(existing.clone());
                object.insert(
                    "plugin".to_string(),
                    Value::Array(vec![previous, new_entry]),
                );
            }
        }
        Some(Value::Array(entries)) => {
            if !entries.iter().any(|entry| entry == &new_entry) {
                entries.push(new_entry);
            }
        }
        Some(_) => bail!("OpenCode config field `plugin` must be a string or array"),
    }
    Ok(())
}

fn default_opencode_config_path() -> PathBuf {
    djinn_core::home_dir()
        .join(".config")
        .join("opencode")
        .join("opencode.json")
}

fn default_opencode_plugin_path() -> PathBuf {
    default_opencode_config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("plugins")
        .join("djinn-watch.js")
}

fn absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn opencode_plugin_entry(config_path: &Path, plugin_path: &Path) -> String {
    let config_parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let default_plugin_dir = config_parent.join("plugins");
    if plugin_path.parent() == Some(default_plugin_dir.as_path()) {
        if let Some(file_name) = plugin_path.file_name().and_then(|name| name.to_str()) {
            return format!("./plugins/{file_name}");
        }
    }
    format!("file://{}", plugin_path.display())
}

fn opencode_watch_state_path() -> PathBuf {
    djinn_core::default_data_dir()
        .join("watchers")
        .join("opencode.json")
}

fn load_opencode_watch_state() -> Result<OpencodeWatchState> {
    let path = opencode_watch_state_path();
    if !path.exists() {
        return Ok(OpencodeWatchState::default());
    }
    let content =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("parsing {}", path.display()))
}

fn save_opencode_watch_state(state: &OpencodeWatchState) -> Result<()> {
    let path = opencode_watch_state_path();
    djinn_core::ensure_parent(&path)?;
    fs::write(&path, serde_json::to_string_pretty(state)? + "\n")
        .with_context(|| format!("writing {}", path.display()))
}

fn content_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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

fn clear_chats(no_backup: bool) -> Result<()> {
    if !io::stdin().is_terminal() {
        bail!("refusing to clear sessions from a non-interactive shell");
    }
    print!("Clear Djinn sessions? Type 'clear' to confirm: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if answer.trim() != "clear" {
        println!("Aborted.");
        return Ok(());
    }
    let backup = chat_store().clear_with_backup(!no_backup)?;
    if let Some(info) = backup {
        println!(
            "Sessions cleared ({} records). Backup written to {} and metadata to {}{}",
            info.record_count,
            info.path.display(),
            info.metadata_path.display(),
            info.bodies_path
                .as_ref()
                .map(|path| format!("; bodies copied to {}", path.display()))
                .unwrap_or_default()
        );
    } else {
        println!("Sessions cleared.");
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

fn rm_chat(id: &str) -> Result<()> {
    let removed = chat_store().remove_matching(id)?;
    if removed.is_empty() {
        println!("No sessions matched {id:?}.");
    } else {
        println!("Removed {} sessions:", removed.len());
        for record in removed {
            println!("  - [{}] {}", record.id, record.title);
        }
    }
    Ok(())
}

fn delete_chats_silent(ids: &[String]) -> Result<Vec<ChatRecord>> {
    let chats = chat_store().list()?;
    let resolved = resolve_chat_ids(&chats, ids)?;
    chat_store().remove_ids(&resolved)
}

fn delete_chat_rows_silent(request: &djinn_tui::ChatDeleteRequest) -> Result<()> {
    if !request.chat_ids.is_empty() {
        delete_chats_silent(&request.chat_ids)?;
    }

    if !request.agent_session_ids.is_empty() {
        let store = agent_session_store();
        for id in &request.agent_session_ids {
            store.delete_session(&AgentSessionId::new(id.clone()))?;
        }
    }

    Ok(())
}

fn archive_chats(args: ArchiveChatsArgs) -> Result<()> {
    let records = chat_store().list()?;
    let selection_args = ShareChatsArgs {
        ids: args.ids,
        source: args.source,
        query: args.query,
        limit: args.limit,
        all: args.all,
        mode: ShareChatsMode::Summary,
        max_chars_per_chat: 0,
        max_memories: 20,
        archive: false,
        dry_run: false,
        profile: "default".to_string(),
        model: None,
        api_key: None,
        base_url: None,
    };
    let selected = select_chats_for_share(&records, &selection_args)?;
    if args.dry_run {
        print_archive_chat_selection(&selected, true);
        return Ok(());
    }
    if !args.force {
        print_archive_chat_selection(&selected, false);
        bail!("refusing to archive sessions without --force; rerun with --dry-run to inspect only");
    }

    let Some(path) = archive_chat_records_with_label(&selected, "manual")? else {
        println!("No sessions archived.");
        return Ok(());
    };
    println!(
        "Archived {} sessions to {} and removed them from the active session index.",
        selected.len(),
        path.display()
    );
    for record in selected {
        println!(
            "  - [{}] {}{}",
            record.id,
            record.title,
            format_chat_source_suffix(&record)
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct ChatArchiveSummary {
    name: String,
    path: String,
    record_count: usize,
    byte_size: u64,
}

fn list_archives(args: ArchiveListArgs) -> Result<()> {
    let summaries = chat_archive_summaries()?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }
    if summaries.is_empty() {
        println!("No chat archives found.");
        return Ok(());
    }
    for summary in &summaries {
        println!(
            "{}\t{} sessions\t{} bytes\t{}",
            summary.name, summary.record_count, summary.byte_size, summary.path
        );
    }
    println!("\nTotal: {} chat archives", summaries.len());
    Ok(())
}

fn show_archive(args: ArchiveShowArgs) -> Result<()> {
    let path = resolve_chat_archive_path(&args.archive)?;
    let records = read_chat_archive_records(&path)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "archive": path,
                "record_count": records.len(),
                "records": records,
            }))?
        );
        return Ok(());
    }

    println!("# Chat Archive\n");
    println!("Path: {}", path.display());
    println!("Records: {}", records.len());
    if records.is_empty() {
        return Ok(());
    }

    println!("\n## Sessions\n");
    for (idx, record) in records.iter().enumerate() {
        println!(
            "{}. [{}] {} — {} chars{}",
            idx + 1,
            record.id,
            record.title,
            record.content.chars().count(),
            format_chat_source_suffix(record)
        );
        if !record.source_path.trim().is_empty() {
            println!("   source path: {}", record.source_path);
        }
        if args.content {
            let (content, truncated) = truncate_with_flag(&record.content, args.max_chars_per_chat);
            println!("\n```text");
            println!("{content}");
            if truncated {
                println!(
                    "... chat content truncated to {} chars ...",
                    args.max_chars_per_chat
                );
            }
            println!("```\n");
        }
    }
    Ok(())
}

fn restore_archive(args: ArchiveRestoreArgs) -> Result<()> {
    let path = resolve_chat_archive_path(&args.archive)?;
    let records = read_chat_archive_records(&path)?;
    let report = if args.dry_run {
        preview_chat_restore(records, args.force)?
    } else {
        chat_store().restore_records(records, args.force)?
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "archive": path,
                "dry_run": args.dry_run,
                "force": args.force,
                "report": report,
            }))?
        );
        return Ok(());
    }

    let verb = if args.dry_run {
        "Would restore"
    } else {
        "Restored"
    };
    println!(
        "{verb} {} sessions from {}{}.",
        report.restored.len(),
        path.display(),
        if report.replaced.is_empty() {
            String::new()
        } else {
            format!(" (replacing {} existing)", report.replaced.len())
        }
    );
    for record in &report.restored {
        println!(
            "  - [{}] {}{}",
            record.id,
            record.title,
            format_chat_source_suffix(record)
        );
    }
    if !report.skipped.is_empty() {
        println!(
            "Skipped {} sessions with existing id/source matches{}:",
            report.skipped.len(),
            if args.force {
                ""
            } else {
                " (use --force to replace)"
            }
        );
        for record in &report.skipped {
            println!(
                "  - [{}] {}{}",
                record.id,
                record.title,
                format_chat_source_suffix(record)
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct ChatArchiveRemovalSummary {
    name: String,
    path: String,
    record_count: Option<usize>,
    byte_size: u64,
    dry_run: bool,
    removed: bool,
}

fn remove_archive(args: ArchiveRemoveArgs) -> Result<()> {
    let path = resolve_chat_archive_path(&args.archive)?;
    ensure_removable_chat_archive_path(&path)?;
    let metadata = fs::metadata(&path)
        .with_context(|| format!("reading chat archive metadata {}", path.display()))?;
    let record_count = read_chat_archive_records(&path)
        .map(|records| records.len())
        .ok();
    let summary = ChatArchiveRemovalSummary {
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        path: path.display().to_string(),
        record_count,
        byte_size: metadata.len(),
        dry_run: args.dry_run,
        removed: !args.dry_run && args.force,
    };

    if args.dry_run {
        print_archive_removal_summary(&summary, args.json)?;
        return Ok(());
    }
    if !args.force {
        print_archive_removal_summary(&summary, args.json)?;
        bail!("refusing to remove archive without --force; rerun with --dry-run to inspect only");
    }

    fs::remove_file(&path).with_context(|| format!("removing chat archive {}", path.display()))?;
    print_archive_removal_summary(&summary, args.json)
}

fn print_archive_removal_summary(summary: &ChatArchiveRemovalSummary, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(summary)?);
        return Ok(());
    }
    let record_count = summary
        .record_count
        .map(|count| count.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    if summary.removed {
        println!(
            "Removed archive {} ({} sessions, {} bytes).",
            summary.path, record_count, summary.byte_size
        );
    } else if summary.dry_run {
        println!(
            "Would remove archive {} ({} sessions, {} bytes).",
            summary.path, record_count, summary.byte_size
        );
    } else {
        println!(
            "Selected archive {} ({} sessions, {} bytes).",
            summary.path, record_count, summary.byte_size
        );
    }
    Ok(())
}

fn ensure_removable_chat_archive_path(path: &Path) -> Result<()> {
    let archive_dir = chat_archive_dir().canonicalize().with_context(|| {
        format!(
            "resolving chat archive dir {}",
            chat_archive_dir().display()
        )
    })?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving chat archive {}", path.display()))?;
    if !canonical.starts_with(&archive_dir) {
        bail!(
            "refusing to remove archive outside {}; use a file from `djinn archive list`",
            archive_dir.display()
        );
    }
    if canonical.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        bail!(
            "refusing to remove non-jsonl archive {}",
            canonical.display()
        );
    }
    Ok(())
}

fn chat_archive_summaries() -> Result<Vec<ChatArchiveSummary>> {
    let dir = chat_archive_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut summaries = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let metadata = entry.metadata()?;
        let records = read_chat_archive_records(&path)?;
        summaries.push(ChatArchiveSummary {
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
            path: path.display().to_string(),
            record_count: records.len(),
            byte_size: metadata.len(),
        });
    }
    summaries.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(summaries)
}

fn read_chat_archive_records(path: &Path) -> Result<Vec<ChatRecord>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading chat archive {}", path.display()))?;
    let mut records = Vec::new();
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        records.push(
            serde_json::from_str::<ChatRecord>(line)
                .with_context(|| format!("parsing chat archive {}", path.display()))?,
        );
    }
    Ok(records)
}

fn resolve_chat_archive_path(value: &str) -> Result<PathBuf> {
    let candidate = PathBuf::from(value);
    if candidate.exists() {
        return Ok(candidate);
    }
    let archive_dir = chat_archive_dir();
    let named = archive_dir.join(value);
    if named.exists() {
        return Ok(named);
    }
    if Path::new(value).extension().is_none() {
        let jsonl = archive_dir.join(format!("{value}.jsonl"));
        if jsonl.exists() {
            return Ok(jsonl);
        }
    }
    bail!(
        "chat archive {value:?} not found; run `djinn archive list` to inspect available archives"
    )
}

fn preview_chat_restore(records: Vec<ChatRecord>, overwrite: bool) -> Result<ChatRestoreReport> {
    let existing = chat_store().list()?;
    let mut restored = Vec::new();
    let mut skipped = Vec::new();
    let mut replaced = Vec::new();
    for record in records {
        let matches = existing
            .iter()
            .filter(|existing| chat_restore_conflicts(existing, &record))
            .cloned()
            .collect::<Vec<_>>();
        if !matches.is_empty() && !overwrite {
            skipped.push(record);
        } else {
            restored.push(record);
            replaced.extend(matches);
        }
    }
    Ok(ChatRestoreReport {
        restored,
        skipped,
        replaced,
    })
}

fn chat_restore_conflicts(existing: &ChatRecord, incoming: &ChatRecord) -> bool {
    existing.id == incoming.id
        || (!existing.source.trim().is_empty()
            && !existing.source_id.trim().is_empty()
            && existing.source == incoming.source
            && existing.source_id == incoming.source_id)
}

fn print_archive_chat_selection(records: &[ChatRecord], dry_run: bool) {
    let label = if dry_run { "Would archive" } else { "Selected" };
    println!("{label} {} sessions:", records.len());
    for record in records {
        println!(
            "  - [{}] {} — {} chars{}",
            record.id,
            record.title,
            record.content.chars().count(),
            format_chat_source_suffix(record)
        );
    }
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

fn prune_chats(args: PruneChatsArgs) -> Result<()> {
    let days = parse_days(&args.older_than)?;
    let (pruned, backup) = chat_store().prune_older_than_days(days, !args.no_backup)?;
    if pruned.is_empty() {
        println!("No sessions older than {} were pruned.", args.older_than);
    } else {
        println!(
            "Pruned {} sessions older than {}:",
            pruned.len(),
            args.older_than
        );
        for record in &pruned {
            println!(
                "  - [{}] {} ({})",
                record.id, record.title, record.created_at
            );
        }
    }
    if let Some(info) = backup {
        println!(
            "Backup written to {} and metadata to {}{}",
            info.path.display(),
            info.metadata_path.display(),
            info.bodies_path
                .as_ref()
                .map(|path| format!("; bodies copied to {}", path.display()))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn parse_days(value: &str) -> Result<i64> {
    let trimmed = value.trim().to_lowercase();
    let digits = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    let suffix = trimmed[digits.len()..].trim();
    let days = digits
        .parse::<i64>()
        .with_context(|| format!("parsing duration {value:?}"))?;
    if days <= 0 {
        bail!("--older-than must be greater than zero days");
    }
    match suffix {
        "" | "d" | "day" | "days" => Ok(days),
        _ => bail!("unsupported duration {value:?}; use forms like 30d or 30days"),
    }
}

fn show_chat(args: ShowChatArgs) -> Result<()> {
    let records = chat_store().list()?;
    let record = resolve_chat(&records, &args.id)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(record)?);
        return Ok(());
    }
    println!("# {}\n", record.title);
    println!("ID: {}", record.id);
    println!("Created: {}", record.created_at);
    if !record.source.trim().is_empty() {
        println!("Source type: {}", record.source);
    }
    if !record.source_id.trim().is_empty() {
        println!("Source ID: {}", record.source_id);
    }
    if !record.source_path.trim().is_empty() {
        println!("Source path: {}", record.source_path);
    }
    println!("\n## Content\n");
    println!("{}", record.content);
    Ok(())
}

fn show_memory(id: &str) -> Result<()> {
    let memories = memory_store().list()?;
    let record = resolve_memory(&memories, id)?;
    let chats = chat_store().list().unwrap_or_default();

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
            println!("- {}", format_memory_source(source, &chats));
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

fn search_chats(query: &str) -> Result<()> {
    let query_lower = query.to_lowercase();
    let matches = chat_store()
        .list()?
        .into_iter()
        .filter(|record| chat_matches(record, &query_lower))
        .collect::<Vec<_>>();
    for (idx, record) in matches.iter().enumerate() {
        println!(
            "  {}. [{}] {} — {}",
            idx + 1,
            record.id,
            record.title,
            chat_snippet(record, &query_lower)
        );
    }
    println!("\nTotal: {} matching sessions", matches.len());
    Ok(())
}

fn promote_session(args: ShareChatArgs) -> Result<()> {
    promote_sessions(ShareChatsArgs {
        ids: vec![args.id],
        source: None,
        query: None,
        limit: 1,
        all: false,
        mode: args.mode,
        max_chars_per_chat: args.max_chars_per_chat,
        max_memories: args.max_memories,
        archive: args.archive,
        dry_run: args.dry_run,
        profile: args.profile,
        model: args.model,
        api_key: args.api_key,
        base_url: args.base_url,
    })
}

fn promote_sessions(args: ShareChatsArgs) -> Result<()> {
    if args.mode == ShareChatsMode::Merge {
        return promote_merge(args);
    }
    let records = chat_store().list()?;
    let selected = select_chats_for_share(&records, &args)?;
    match args.mode {
        ShareChatsMode::Summary => println!("{}", format_chats_summary(&selected, &args)),
        ShareChatsMode::Pattern | ShareChatsMode::Memories => {
            let memories = memory_store().list()?;
            println!(
                "{}",
                format_chats_review_prompt(&selected, &args, &memories)
            );
        }
        ShareChatsMode::Merge => unreachable!("merge mode handled above"),
    }
    Ok(())
}

fn promote_merge(args: ShareChatsArgs) -> Result<()> {
    let records = chat_store().list()?;
    let selected = select_chats_for_merge(&records, &args)?;
    let prompt = format_chats_merge_prompt(&selected, &args);
    if args.dry_run {
        println!("{prompt}");
        return Ok(());
    }
    if selected.is_empty() {
        println!("No sessions matched merge selection.");
        return Ok(());
    }

    let profile = args.profile.trim().to_string();
    let model = resolve_agent_model(args.model.clone(), &profile)?;
    let store = agent_session_store();
    let id = store.create_session(AgentSessionMeta {
        title: format!("Promote {} sessions into memories", selected.len()),
        workspace: resolve_agent_workspace(None)?,
        profile: profile.clone(),
        source: "djinn-promote-merge".to_string(),
        ..AgentSessionMeta::default()
    })?;
    store.append_event(
        &id,
        AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
            content: prompt.clone(),
        }),
    )?;
    let response = complete_openai_prompt(
        &store,
        &id,
        prompt,
        model,
        args.api_key,
        args.base_url,
        0,
        &profile,
        &[],
        Vec::new(),
        false,
    )?;
    let merge = parse_chat_merge_response(&response.message.content)?;
    let written = write_chat_merge_memories(&merge, &selected)?;
    let archived = if args.archive && !written.is_empty() {
        archive_chat_records_with_label(&selected, "merge")?
    } else {
        None
    };

    println!(
        "Promoted {} sessions into {} memories.",
        selected.len(),
        written.len()
    );
    if let Some(path) = archived {
        println!("Archived source sessions: {}", path.display());
    } else if args.archive {
        println!("No source sessions archived because no memories were written.");
    }
    println!(
        "Agent session [{}]: {}",
        id,
        store.session_file_path(&id).display()
    );
    for memory in written {
        println!("- [{}] {}", memory.id, memory.text);
    }
    Ok(())
}

fn build_promote_chats_prompt(mut args: ShareChatsArgs) -> Result<String> {
    args.mode = ShareChatsMode::Memories;
    let records = chat_store().list()?;
    let selected = select_chats_for_share(&records, &args)?;
    let memories = memory_store().list()?;
    Ok(format_chats_promotion_prompt(&selected, &args, &memories))
}

fn review_opencode(args: ReviewOpencodeArgs) -> Result<()> {
    review_chats(ReviewChatsArgs {
        source: Some("opencode".to_string()),
        limit: args.limit,
        all: args.all,
        query: args.query,
        agent: args.agent,
        title: args.title,
        opencode_bin: args.opencode_bin,
        dry_run: args.dry_run,
    })
}

fn review_chats(args: ReviewChatsArgs) -> Result<()> {
    let prompt = build_promote_chats_prompt(ShareChatsArgs {
        ids: Vec::new(),
        source: args.source.clone(),
        query: args.query,
        limit: args.limit,
        all: args.all,
        mode: ShareChatsMode::Memories,
        max_chars_per_chat: 4000,
        max_memories: 20,
        archive: false,
        dry_run: false,
        profile: "default".to_string(),
        model: None,
        api_key: None,
        base_url: None,
    })?;

    if args.dry_run {
        println!("{prompt}");
        return Ok(());
    }

    run_opencode_review_prompt(
        &args.opencode_bin,
        &args.title,
        args.agent.as_deref(),
        &prompt,
    )
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

fn run_opencode_review_prompt(
    opencode_bin: &str,
    title: &str,
    agent: Option<&str>,
    prompt: &str,
) -> Result<()> {
    let mut command = ProcessCommand::new(opencode_bin);
    command.arg("run").arg(prompt).arg("--title").arg(title);
    if let Some(agent) = agent.map(str::trim).filter(|value| !value.is_empty()) {
        command.arg("--agent").arg(agent);
    }
    command.env("DJINN_REVIEWER", "1");
    command.env("DJINN_OPENCODE_PLUGIN_CHILD", "1");
    let status = command
        .status()
        .with_context(|| format!("running {opencode_bin} run"))?;
    if !status.success() {
        bail!("{opencode_bin} run exited with status {status}");
    }
    Ok(())
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

fn format_chats_summary(records: &[ChatRecord], args: &ShareChatsArgs) -> String {
    let mut out = String::from("# Djinn Session Summary\n\n");
    out.push_str("This is a local digest of the selected sessions. No model was run.\n\n");
    out.push_str("## Selection\n\n");
    out.push_str(&format!("- Chat count: {}\n", records.len()));
    if let Some(source) = args
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("- Source filter: {source}\n"));
    }
    if let Some(query) = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        out.push_str(&format!("- Query filter: {query}\n"));
    }
    if !args.all && args.ids.is_empty() {
        out.push_str(&format!(
            "- Limit: latest {} matching sessions\n",
            args.limit
        ));
    }

    let redacted_count = records
        .iter()
        .filter(|record| chat_content_appears_redacted(&record.content))
        .count();
    if redacted_count > 0 {
        out.push_str(&format!(
            "- Redacted/sanitized sessions: {redacted_count} (summary detail may be limited)\n"
        ));
    }

    out.push_str("\n## Sessions\n");
    for (idx, record) in records.iter().enumerate() {
        out.push_str(&format!(
            "\n### {}. {}\n\n- ID: `{}`\n- Created: {}\n",
            idx + 1,
            record.title,
            record.id,
            record.created_at
        ));
        if !record.source.trim().is_empty() {
            out.push_str(&format!("- Source type: {}\n", record.source));
        }
        if !record.source_id.trim().is_empty() {
            out.push_str(&format!("- Source ID: {}\n", record.source_id));
        }
        let content = share_chat_content(record);
        out.push_str("\n");
        let excerpt = truncate(&content, args.max_chars_per_chat);
        out.push_str(&excerpt);
        if !excerpt.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn format_chats_merge_prompt(records: &[ChatRecord], args: &ShareChatsArgs) -> String {
    let mut out = String::from("You are promoting Djinn sessions into durable memories.\n\n");
    out.push_str("Group related sessions by topic/workflow. Distill only durable, reusable memories that should become active immediately. Do not create an inbox, candidates, suggestions, or todos. If there is no durable lesson, return an empty memories array.\n\n");
    out.push_str("Return strict JSON only, with this shape:\n\n");
    out.push_str(
        r#"{
  "groups": [
    {"title": "short group title", "chat_ids": ["chat-id"], "rationale": "why these belong together"}
  ],
  "memories": [
    {
      "text": "durable memory text",
      "scope": "project|global|work|personal",
      "kind": "preference|convention|workflow|correction|gotcha",
      "confidence": "medium|high",
      "evidence": ["copied supporting evidence"],
      "source_chat_ids": ["chat-id"]
    }
  ]
}
"#,
    );
    out.push_str("\nRules:\n");
    out.push_str("- Create at most ");
    out.push_str(&args.max_memories.to_string());
    out.push_str(" memories.\n");
    out.push_str("- Prefer fewer, higher-value memories over many small facts.\n");
    out.push_str("- Memories should be actionable later by skills or follow-up actions.\n");
    out.push_str("- Do not include secrets, tokens, private URLs, or sensitive raw data.\n");
    out.push_str("- Preserve provenance with source_chat_ids.\n");
    out.push_str("- If sanitized exports hide content, only create memories supported by readable metadata/text.\n");

    append_chats_bundle(&mut out, records, args.max_chars_per_chat);
    out
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ChatMergeResponse {
    #[serde(default)]
    groups: Vec<ChatMergeGroup>,
    #[serde(default)]
    memories: Vec<ChatMergeMemory>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ChatMergeGroup {
    #[serde(default)]
    title: String,
    #[serde(default)]
    chat_ids: Vec<String>,
    #[serde(default)]
    rationale: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ChatMergeMemory {
    text: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    confidence: String,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    source_chat_ids: Vec<String>,
}

fn parse_chat_merge_response(content: &str) -> Result<ChatMergeResponse> {
    let raw = extract_json_payload(content).unwrap_or_else(|| content.trim().to_string());
    let response: ChatMergeResponse = serde_json::from_str(&raw)
        .with_context(|| "parsing chat merge JSON response from model")?;
    Ok(ChatMergeResponse {
        groups: response.groups,
        memories: response
            .memories
            .into_iter()
            .filter(|memory| !memory.text.trim().is_empty())
            .collect(),
    })
}

fn extract_json_payload(content: &str) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed.to_string());
    }
    let marker = "```json";
    let start = trimmed.find(marker)? + marker.len();
    let rest = &trimmed[start..];
    let end = rest.find("```")?;
    Some(rest[..end].trim().to_string())
}

fn write_chat_merge_memories(
    response: &ChatMergeResponse,
    selected: &[ChatRecord],
) -> Result<Vec<MemoryRecord>> {
    let store = memory_store();
    response
        .memories
        .iter()
        .map(|memory| {
            store.add_input(MemoryInput {
                text: memory.text.trim().to_string(),
                scope: nonempty_string(&memory.scope),
                kind: nonempty_string(&memory.kind),
                confidence: nonempty_string(&memory.confidence),
                not_before: None,
                evidence: memory.evidence.clone(),
                sources: memory_sources_for_chat_ids(selected, &memory.source_chat_ids),
            })
        })
        .collect()
}

fn memory_sources_for_chat_ids(selected: &[ChatRecord], ids: &[String]) -> Vec<MemorySource> {
    ids.iter()
        .filter_map(|id| {
            selected
                .iter()
                .find(|chat| chat.id == *id || chat.source_id == *id)
                .map(memory_source_from_chat)
        })
        .collect()
}

fn nonempty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn archive_chat_records_with_label(records: &[ChatRecord], label: &str) -> Result<Option<PathBuf>> {
    if records.is_empty() {
        return Ok(None);
    }
    let archive_dir = chat_archive_dir();
    fs::create_dir_all(&archive_dir)
        .with_context(|| format!("creating chat archive dir {}", archive_dir.display()))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let label = archive_label(label);
    let path = archive_dir.join(format!("{label}-{stamp}.jsonl"));
    let mut rendered = String::new();
    for record in records {
        rendered.push_str(&serde_json::to_string(record)?);
        rendered.push('\n');
    }
    fs::write(&path, rendered)
        .with_context(|| format!("writing chat archive {}", path.display()))?;
    delete_chats_silent(
        &records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>(),
    )?;
    Ok(Some(path))
}

fn chat_archive_dir() -> PathBuf {
    djinn_core::default_cache_dir().join("chat-archives")
}

fn archive_label(label: &str) -> String {
    let cleaned = label
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase();
    if cleaned.is_empty() {
        "archive".to_string()
    } else {
        cleaned
    }
}

fn format_chat_summary_agent_prompt(records: &[ChatRecord], args: &ShareChatsArgs) -> String {
    let mut out = String::from(
        "Please summarize the selected sessions below so we can continue discussing them.\n\n",
    );
    out.push_str("Focus on:\n");
    out.push_str("- main themes and outcomes;\n");
    out.push_str("- important decisions;\n");
    out.push_str("- unresolved follow-ups;\n");
    out.push_str("- potentially reusable memories, clearly labeled as possible memories only.\n\n");
    out.push_str("Do not write memories automatically. If sanitized exports hide the source text, say that the available detail is limited.\n");
    append_chats_bundle(&mut out, records, args.max_chars_per_chat);
    out
}

fn format_chats_review_prompt(
    records: &[ChatRecord],
    args: &ShareChatsArgs,
    memories: &[MemoryRecord],
) -> String {
    let mut out = String::from("# Djinn Multi-Session Review\n\n");
    out.push_str("You are reviewing a bundle of Djinn sessions. Treat them as a corpus, not as isolated transcripts.\n\n");
    out.push_str("## Review Goal\n\n");
    match args.mode {
        ShareChatsMode::Summary => out.push_str(
            "Summarize the selected sessions. Identify the main themes, decisions, outcomes, unresolved follow-ups, and any stale assumptions. Keep the summary useful for resuming work.\n",
        ),
        ShareChatsMode::Pattern => out.push_str(
            "Identify recurring patterns across the selected sessions: user preferences, repeated corrections, tool/workflow choices, project conventions, safety gotchas, friction points, and implementation habits. Separate high-confidence repeated patterns from one-off observations.\n",
        ),
        ShareChatsMode::Memories | ShareChatsMode::Merge => out.push_str(
            "Propose durable memories only when they are reusable in future work and supported by repeated patterns or explicit user instructions. Return reviewed shell commands the user can run manually; do not invent memories from weak one-off evidence.\n",
        ),
    }
    out.push_str("\n## Output Guidelines\n\n");
    match args.mode {
        ShareChatsMode::Summary => out.push_str(
            "Return Markdown with sections: `Summary`, `Decisions`, `Open Follow-ups`, and `Potential Memories`. Do not write memories automatically.\n",
        ),
        ShareChatsMode::Pattern => out.push_str(
            "Return Markdown with sections: `High-confidence Patterns`, `Possible One-offs`, `Workflow Opportunities`, and `Reviewable Memories`. Do not write memories automatically.\n",
        ),
        ShareChatsMode::Memories | ShareChatsMode::Merge => out.push_str(
            "Return only a short reviewed list of commands. Include scope, kind, confidence, copied evidence, and source session pointers when available. Use `--not-before YYYY-MM-DD` when a memory should not drive suggestions/actions until later. Use this form:\n\n```bash\ndjinn add memory \"...\" --scope project --kind preference --confidence high --not-before 2026-10-01 --evidence \"Repeated evidence from the reviewed sessions ...\" --source-chat SESSION_ID\n```\n\nIf there are no durable lessons, say: `No durable memories recommended.`\n",
        ),
    }
    out.push_str("\nDo not include secrets, credentials, tokens, private URLs, or sensitive raw data. Avoid duplicating existing memories.\n");

    out.push_str("\n## Selection Metadata\n\n");
    out.push_str(&format!("- Session count: {}\n", records.len()));
    out.push_str(&format!("- Mode: {:?}\n", args.mode));
    if let Some(source) = args
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("- Source filter: {source}\n"));
    }
    if let Some(query) = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        out.push_str(&format!("- Query filter: {query}\n"));
    }
    if !args.all && args.ids.is_empty() {
        out.push_str(&format!(
            "- Limit: latest {} matching sessions\n",
            args.limit
        ));
    }

    out.push_str("\n## Existing Memories\n\n```text\n");
    if memories.is_empty() {
        out.push_str("No existing memories recorded.\n");
    } else {
        for record in memories.iter().take(100) {
            out.push_str(&format!("- [{}] {}\n", record.id, record.text));
        }
        if memories.len() > 100 {
            out.push_str(&format!(
                "... {} more memories omitted ...\n",
                memories.len() - 100
            ));
        }
    }
    out.push_str("```\n");

    append_chats_bundle(&mut out, records, args.max_chars_per_chat);
    out
}

fn format_chats_promotion_prompt(
    records: &[ChatRecord],
    args: &ShareChatsArgs,
    memories: &[MemoryRecord],
) -> String {
    let mut out = format_chats_review_prompt(records, args, memories);
    out = out.replace("# Djinn Multi-Chat Review", "# Djinn Multi-Chat Promotion");
    out.push_str(
        "\n\n## Promotion Output\n\nReturn reviewed `djinn add memory` commands. Include scope, kind, confidence, copied evidence, and one or more `--source-chat` pointers when available. Use `--not-before YYYY-MM-DD` when a future activation date is appropriate. Example:\n\n```bash\ndjinn add memory \"...\" --scope project --kind convention --confidence high --not-before 2026-10-01 --evidence \"Repeated across reviewed sessions ...\" --source-chat SESSION_ID\n```\n\nThe user can review memories for downstream suggestions with `djinn accept memory <id>` or remove them with `djinn reject memory <id>`.\n",
    );
    out
}

fn append_chats_bundle(out: &mut String, records: &[ChatRecord], max_chars_per_chat: usize) {
    out.push_str("\n## Sessions\n");
    for (idx, record) in records.iter().enumerate() {
        out.push_str(&format!(
            "\n### Chat {}: {}\n\n- ID: `{}`\n- Created: {}\n",
            idx + 1,
            record.title,
            record.id,
            record.created_at
        ));
        if !record.source.trim().is_empty() {
            out.push_str(&format!("- Source type: {}\n", record.source));
        }
        if !record.source_id.trim().is_empty() {
            out.push_str(&format!("- Source ID: {}\n", record.source_id));
        }
        if !record.source_path.trim().is_empty() {
            out.push_str(&format!("- Source path: {}\n", record.source_path));
        }
        out.push_str("\n```text\n");
        let content = share_chat_content(record);
        let (body, truncated) = truncate_with_flag(&content, max_chars_per_chat);
        out.push_str(&body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
        if truncated {
            out.push_str(&format!(
                "\n... chat content truncated to {max_chars_per_chat} chars ...\n"
            ));
        }
        out.push_str("```\n");
    }
}

fn share_chat_content(record: &ChatRecord) -> String {
    if record.source.trim() == "opencode" {
        if let Some(content) = format_opencode_export_for_share(&record.content) {
            return content;
        }
        if content_looks_like_json(&record.content) {
            return "OpenCode export digest\n- This OpenCode export looked like JSON but could not be parsed, so Djinn did not include the raw payload.\n".to_string();
        }
    }
    record.content.clone()
}

fn content_looks_like_json(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

fn chat_content_appears_redacted(content: &str) -> bool {
    content.contains("[redacted:") || content.contains("\"redacted\"")
}

fn format_opencode_export_for_share(export: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(export).ok()?;
    let messages = value.get("messages").and_then(Value::as_array)?;
    let mut out = String::from("OpenCode export digest\n");
    if let Some(id) = value.pointer("/info/id").and_then(Value::as_str) {
        out.push_str(&format!("- Session: {id}\n"));
    }
    if let Some(slug) = value.pointer("/info/slug").and_then(Value::as_str) {
        out.push_str(&format!("- Slug: {slug}\n"));
    }
    let model = value
        .pointer("/info/model/id")
        .or_else(|| value.pointer("/info/modelID"))
        .and_then(Value::as_str);
    let provider = value
        .pointer("/info/model/providerID")
        .or_else(|| value.pointer("/info/providerID"))
        .and_then(Value::as_str);
    match (provider, model) {
        (Some(provider), Some(model)) => out.push_str(&format!("- Model: {provider}/{model}\n")),
        (None, Some(model)) => out.push_str(&format!("- Model: {model}\n")),
        _ => {}
    }
    out.push_str(&format!("- Messages: {}\n", messages.len()));
    let tool_count = opencode_export_tool_part_count(messages);
    if tool_count > 0 {
        out.push_str(&format!("- Tool calls/results: {tool_count}\n"));
    }
    if chat_content_appears_redacted(export) {
        out.push_str("- Redaction: this export appears sanitized; message/tool text may be unavailable. Re-import unsanitized only when it is safe to do so.\n");
    }

    out.push_str("\nTranscript excerpt:\n");
    let mut appended = 0usize;
    for message in messages {
        let role = message
            .pointer("/info/role")
            .and_then(Value::as_str)
            .unwrap_or("message");
        let text = opencode_message_share_text(message);
        if text.trim().is_empty() {
            continue;
        }
        appended += 1;
        out.push_str(&format!("\n{}:\n{}\n", title_case_role(role), text));
    }
    if appended == 0 {
        out.push_str("\nNo readable message text was available in this export.\n");
    }
    Some(out)
}

fn opencode_export_tool_part_count(messages: &[Value]) -> usize {
    messages
        .iter()
        .flat_map(|message| {
            message
                .get("parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool"))
        .count()
}

fn opencode_message_share_text(message: &Value) -> String {
    let mut lines = Vec::new();
    for part in message
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    push_nonempty_opencode_line(&mut lines, text);
                }
            }
            Some("tool") => {
                if let Some(title) = part.pointer("/state/title").and_then(Value::as_str) {
                    push_nonempty_opencode_line(&mut lines, &format!("Tool: {title}"));
                } else if let Some(tool) = part.get("tool").and_then(Value::as_str) {
                    push_nonempty_opencode_line(&mut lines, &format!("Tool: {tool}"));
                }
                if let Some(output) = part.pointer("/state/output").and_then(Value::as_str) {
                    push_nonempty_opencode_line(&mut lines, output);
                }
            }
            Some("reasoning") | Some("step-start") => {}
            _ => {}
        }
    }
    lines.join("\n\n")
}

fn title_case_role(role: &str) -> String {
    let mut chars = role.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => "Message".to_string(),
    }
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

fn select_chats_for_share(
    records: &[ChatRecord],
    args: &ShareChatsArgs,
) -> Result<Vec<ChatRecord>> {
    let mut selected = if args.ids.is_empty() {
        let source = args
            .source
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let query = args
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_lowercase);
        let matches = records
            .iter()
            .filter(|record| {
                source
                    .map(|source| record.source.eq_ignore_ascii_case(source))
                    .unwrap_or(true)
            })
            .filter(|record| {
                query
                    .as_deref()
                    .map(|query| chat_matches(record, query))
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();

        if args.all {
            matches
        } else {
            let mut latest = matches
                .into_iter()
                .rev()
                .take(args.limit)
                .collect::<Vec<_>>();
            latest.reverse();
            latest
        }
    } else {
        let mut seen = HashSet::new();
        let mut explicit = Vec::new();
        for id in &args.ids {
            let record = resolve_chat(records, id)?;
            if seen.insert(record.id.clone()) {
                explicit.push(record.clone());
            }
        }
        explicit
    };

    if let Some(source) = args
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        selected.retain(|record| record.source.eq_ignore_ascii_case(source));
    }
    if let Some(query) = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_lowercase)
    {
        selected.retain(|record| chat_matches(record, &query));
    }

    if selected.is_empty() {
        bail!("no sessions matched the promote selection");
    }
    Ok(selected)
}

fn select_chats_for_merge(
    records: &[ChatRecord],
    args: &ShareChatsArgs,
) -> Result<Vec<ChatRecord>> {
    select_chats_for_share(records, args)
}

fn resolve_chat<'a>(records: &'a [ChatRecord], id: &str) -> Result<&'a ChatRecord> {
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
                || record.title.to_lowercase().contains(&needle)
                || record.source_id.to_lowercase().contains(&needle)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => bail!("no chat named {id:?} found"),
        many => {
            eprintln!("multiple sessions match {id:?}:");
            for record in many {
                eprintln!("  - [{}] {}", record.id, record.title);
            }
            bail!("chat id is ambiguous")
        }
    }
}

fn resolve_chat_ids(records: &[ChatRecord], ids: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();
    for id in ids {
        let record = resolve_chat(records, id)?;
        if seen.insert(record.id.clone()) {
            resolved.push(record.id.clone());
        }
    }
    Ok(resolved)
}

fn chat_matches(record: &ChatRecord, query: &str) -> bool {
    record.id.to_lowercase().contains(query)
        || record.title.to_lowercase().contains(query)
        || record.source.to_lowercase().contains(query)
        || record.source_id.to_lowercase().contains(query)
        || record.source_path.to_lowercase().contains(query)
        || record.content.to_lowercase().contains(query)
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

fn chat_snippet(record: &ChatRecord, query: &str) -> String {
    record
        .content
        .lines()
        .map(str::trim)
        .find(|line| line.to_lowercase().contains(query))
        .or_else(|| {
            record
                .content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
        })
        .map(|line| truncate(line, 96))
        .unwrap_or_else(|| "(empty chat)".to_string())
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

fn truncate_with_flag(value: &str, max_chars: usize) -> (String, bool) {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    let was_truncated = chars.next().is_some();
    (truncated, was_truncated)
}

fn format_memory_source(source: &MemorySource, chats: &[ChatRecord]) -> String {
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
        if memory_source_chat_exists(source, chats) {
            "available"
        } else {
            "missing/deleted"
        }
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

fn memory_source_chat_exists(source: &MemorySource, chats: &[ChatRecord]) -> bool {
    chats.iter().any(|chat| {
        (!source.chat_id.is_empty() && chat.id == source.chat_id)
            || (!source.source.is_empty()
                && !source.source_id.is_empty()
                && chat.source == source.source
                && chat.source_id == source.source_id)
    })
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

fn format_chat_source_suffix(record: &ChatRecord) -> String {
    if !record.source.trim().is_empty() && !record.source_id.trim().is_empty() {
        format!(" ({}:{})", record.source, record.source_id)
    } else if !record.source_id.trim().is_empty() {
        format!(" ({})", record.source_id)
    } else if !record.source_path.trim().is_empty() {
        format!(" ({})", record.source_path)
    } else {
        String::new()
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

fn chat_store() -> djinn_chats::ChatStore {
    djinn_chats::ChatStore::default_in(&djinn_core::default_cache_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use djinn_memory::{AgentSessionLifecycle, AgentSessionTokenUsage};

    fn test_chat(id: &str, title: &str, source: &str, content: &str) -> ChatRecord {
        ChatRecord {
            id: id.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            source: source.to_string(),
            source_id: format!("source-{id}"),
            source_path: String::new(),
            content_path: String::new(),
            created_at: "2026-07-09".to_string(),
        }
    }

    fn default_share_chats_args() -> ShareChatsArgs {
        ShareChatsArgs {
            ids: Vec::new(),
            source: None,
            query: None,
            limit: 10,
            all: false,
            mode: ShareChatsMode::Summary,
            max_chars_per_chat: 4000,
            max_memories: 20,
            archive: false,
            dry_run: false,
            profile: "default".to_string(),
            model: None,
            api_key: None,
            base_url: None,
        }
    }

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
    fn kitsune_session_report_args_match_pane_api_contract() {
        assert_eq!(
            kitsune_agent_session_report_args("w1:p2", 7, "djinn-session", "new"),
            vec![
                "pane",
                "report-agent-session",
                "w1:p2",
                "--source",
                "kitsune:djinn",
                "--agent",
                "djinn",
                "--seq",
                "7",
                "--agent-session-id",
                "djinn-session",
                "--session-start-source",
                "new",
            ]
        );
    }

    #[test]
    fn kitsune_state_report_args_match_pane_api_contract() {
        assert_eq!(
            kitsune_agent_state_report_args(
                "w1:p2",
                KitsuneAgentReportState::Working,
                8,
                "djinn-session",
                Some("Running tool"),
            ),
            vec![
                "pane",
                "report-agent",
                "w1:p2",
                "--source",
                "kitsune:djinn",
                "--agent",
                "djinn",
                "--state",
                "working",
                "--seq",
                "8",
                "--agent-session-id",
                "djinn-session",
                "--message",
                "Running tool",
            ]
        );
        assert_eq!(
            kitsune_agent_state_report_args(
                "w1:p2",
                KitsuneAgentReportState::Idle,
                9,
                "djinn-session",
                Some("   "),
            ),
            vec![
                "pane",
                "report-agent",
                "w1:p2",
                "--source",
                "kitsune:djinn",
                "--agent",
                "djinn",
                "--state",
                "idle",
                "--seq",
                "9",
                "--agent-session-id",
                "djinn-session",
            ]
        );
        assert_eq!(
            kitsune_agent_state_report_args(
                "w1:p2",
                KitsuneAgentReportState::Blocked,
                10,
                "djinn-session",
                Some("Permission approval required"),
            ),
            vec![
                "pane",
                "report-agent",
                "w1:p2",
                "--source",
                "kitsune:djinn",
                "--agent",
                "djinn",
                "--state",
                "blocked",
                "--seq",
                "10",
                "--agent-session-id",
                "djinn-session",
                "--message",
                "Permission approval required",
            ]
        );
    }

    #[test]
    fn kitsune_release_report_args_match_pane_api_contract() {
        assert_eq!(
            kitsune_agent_release_report_args("w1:p2", 11),
            vec![
                "pane",
                "release-agent",
                "w1:p2",
                "--source",
                "kitsune:djinn",
                "--agent",
                "djinn",
                "--seq",
                "11",
            ]
        );
    }

    #[test]
    fn kitsune_reporter_sequence_is_shared_by_all_report_types() {
        let mut reporter = KitsuneAgentReporter {
            bin: "kitsune".to_string(),
            pane_id: "w1:p2".to_string(),
            seq: 0,
        };

        assert_eq!(reporter.next_seq(), 1);
        assert_eq!(reporter.next_seq(), 2);
        assert_eq!(reporter.next_seq(), 3);
    }

    #[test]
    fn kitsune_blocked_message_classifies_auth_config_and_permission_errors() {
        assert_eq!(
            kitsune_blocked_message_for_error(&anyhow::anyhow!(
                "OpenAI auth is required; run providers login"
            )),
            "Agent needs provider auth or configuration"
        );
        assert_eq!(
            kitsune_blocked_message_for_error(&anyhow::anyhow!(
                "permission requires approval in non-interactive mode"
            )),
            "Agent needs permission approval"
        );
        assert_eq!(
            kitsune_blocked_message_for_error(&anyhow::anyhow!("network timeout")),
            "Agent turn failed"
        );
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
        let gate = TerminalPermissionGate::new(None, String::new());
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
        let gate = TerminalPermissionGate::new(None, String::new());
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
        let gate = TerminalPermissionGate::new(None, String::new());
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
    fn foreground_lifecycle_helpers_pause_on_quit_without_overriding_failures() {
        let store = temp_agent_store("foreground-lifecycle");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Foreground child".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        append_foreground_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Running,
            "agent turn started",
            None,
        )
        .unwrap();
        append_foreground_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Paused,
            "agent turn completed",
            Some("ready for next prompt".to_string()),
        )
        .unwrap();
        mark_foreground_session_paused_on_quit(&store, &id).unwrap();

        let lifecycle = lifecycle_for(&store.load_session(&id).unwrap());
        assert_eq!(lifecycle.state, AgentSessionLifecycleState::Paused);
        assert_eq!(lifecycle.mode, Some(AgentSessionExecutionMode::Foreground));
        assert_eq!(lifecycle.reason.as_deref(), Some("chat exited"));

        let failed_id = store
            .create_session(AgentSessionMeta {
                title: "Failed foreground child".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        append_foreground_session_lifecycle_event(
            &store,
            &failed_id,
            AgentSessionLifecycleState::Failed,
            "agent turn failed",
            Some("boom".to_string()),
        )
        .unwrap();
        mark_foreground_session_paused_on_quit(&store, &failed_id).unwrap();

        let lifecycle = lifecycle_for(&store.load_session(&failed_id).unwrap());
        assert_eq!(lifecycle.state, AgentSessionLifecycleState::Failed);
        assert_eq!(lifecycle.reason.as_deref(), Some("agent turn failed"));
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

        let cli = Cli::try_parse_from([
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
        .unwrap();
        let Some(Command::Agent(agent_args)) = cli.command else {
            panic!("expected agent command");
        };
        let AgentCommand::Chat(args) = agent_args.command else {
            panic!("expected agent chat command");
        };
        assert_eq!(args.agent.as_deref(), Some("planner"));
        assert_eq!(args.parent_session.as_deref(), Some("agt_parent"));
        assert_eq!(args.max_tool_rounds, 8);

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
    fn parses_archive_sessions_selection_command() {
        let cli = Cli::try_parse_from([
            "djinn", "archive", "sessions", "--source", "opencode", "--limit", "25", "--force",
        ])
        .unwrap();

        let Some(Command::Archive(args)) = cli.command else {
            panic!("expected archive command");
        };
        let ArchiveNoun::Sessions(args) = args.noun else {
            panic!("expected archive sessions command");
        };

        assert_eq!(args.source.as_deref(), Some("opencode"));
        assert_eq!(args.limit, 25);
        assert!(args.force);
    }

    #[test]
    fn parses_session_nouns_and_rejects_share_command() {
        let cli = Cli::try_parse_from(["djinn", "list", "sessions", "--json"]).unwrap();
        let Some(Command::List(args)) = cli.command else {
            panic!("expected list command");
        };
        let ListNoun::Sessions(args) = args.noun else {
            panic!("expected list sessions command");
        };
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "add", "session", "./session.md"]).unwrap();
        let Some(Command::Add(args)) = cli.command else {
            panic!("expected add command");
        };
        let AddNoun::Session(args) = args.noun else {
            panic!("expected add session command");
        };
        assert_eq!(args.file, PathBuf::from("./session.md"));

        let cli = Cli::try_parse_from([
            "djinn", "promote", "sessions", "--source", "opencode", "--mode", "pattern",
        ])
        .unwrap();
        let Some(Command::Promote(args)) = cli.command else {
            panic!("expected promote command");
        };
        let PromoteNoun::Sessions(args) = args.noun else {
            panic!("expected promote sessions command");
        };
        assert_eq!(args.source.as_deref(), Some("opencode"));
        assert_eq!(args.mode, ShareChatsMode::Pattern);

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
        assert_eq!(args.model.as_deref(), Some("openai/gpt-5.5"));

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

        let cli = Cli::try_parse_from(["djinn", "session", "bap-questions"]).unwrap();
        let Some(Command::Session(args)) = cli.command else {
            panic!("expected session command");
        };
        assert!(args.command.is_none());
        assert_eq!(args.dir, Some(PathBuf::from("bap-questions")));

        assert!(Cli::try_parse_from(["djinn", "share", "chats"]).is_err());
    }

    #[test]
    fn parses_archive_list_show_restore_and_rm_commands() {
        let cli = Cli::try_parse_from(["djinn", "archive", "list", "--json"]).unwrap();
        let Some(Command::Archive(args)) = cli.command else {
            panic!("expected archive command");
        };
        let ArchiveNoun::List(args) = args.noun else {
            panic!("expected archive list command");
        };
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "djinn",
            "archive",
            "show",
            "manual-20260724.jsonl",
            "--content",
            "--max-chars-per-chat",
            "500",
        ])
        .unwrap();
        let Some(Command::Archive(args)) = cli.command else {
            panic!("expected archive command");
        };
        let ArchiveNoun::Show(args) = args.noun else {
            panic!("expected archive show command");
        };
        assert_eq!(args.archive, "manual-20260724.jsonl");
        assert!(args.content);
        assert_eq!(args.max_chars_per_chat, 500);

        let cli = Cli::try_parse_from([
            "djinn",
            "archive",
            "restore",
            "manual-20260724.jsonl",
            "--force",
            "--dry-run",
        ])
        .unwrap();
        let Some(Command::Archive(args)) = cli.command else {
            panic!("expected archive command");
        };
        let ArchiveNoun::Restore(args) = args.noun else {
            panic!("expected archive restore command");
        };
        assert_eq!(args.archive, "manual-20260724.jsonl");
        assert!(args.force);
        assert!(args.dry_run);

        let cli = Cli::try_parse_from([
            "djinn",
            "archive",
            "rm",
            "manual-20260724.jsonl",
            "--force",
            "--json",
        ])
        .unwrap();
        let Some(Command::Archive(args)) = cli.command else {
            panic!("expected archive command");
        };
        let ArchiveNoun::Rm(args) = args.noun else {
            panic!("expected archive rm command");
        };
        assert_eq!(args.archive, "manual-20260724.jsonl");
        assert!(args.force);
        assert!(args.json);
    }

    #[test]
    fn archive_label_sanitizes_file_prefix() {
        assert_eq!(archive_label("OpenCode Import"), "opencode-import");
        assert_eq!(archive_label("***"), "archive");
    }

    #[test]
    fn agent_chat_messages_summarize_tools_without_raw_json_dump() {
        let session = AgentSession {
            id: AgentSessionId::new("agt_test"),
            meta: AgentSessionMeta::default(),
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "run tests".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolCall {
                    id: "call-1".to_string(),
                    name: "shell".to_string(),
                    input: serde_json::json!({"command": "cargo test"}),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                    id: "call-1".to_string(),
                    output: serde_json::json!({"stdout": "tests passed\n", "exit_code": 0}),
                    success: true,
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "All tests passed.".to_string(),
                }),
            ],
        };

        let messages = agent_chat_messages(&session);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, djinn_tui::AgentChatRole::User);
        assert_eq!(messages[1].role, djinn_tui::AgentChatRole::Tool);
        assert_eq!(messages[1].content, "# Running in .\n$ cargo test");
        assert_eq!(messages[2].role, djinn_tui::AgentChatRole::ToolOutput);
        assert!(messages[2].content.contains("shell result: ok"));
        assert!(messages[2].content.contains("command: `cargo test`"));
        assert!(messages[2].content.contains("stdout:\ntests passed"));
        assert!(!messages[2].content.contains("exit_code"));
        assert_eq!(messages[3].content, "All tests passed.");
    }

    #[test]
    fn agent_chat_messages_identify_search_tool_and_result_context() {
        let session = AgentSession {
            id: AgentSessionId::new("agt_test"),
            meta: AgentSessionMeta::default(),
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::ToolCall {
                    id: "call-2".to_string(),
                    name: "search_files".to_string(),
                    input: serde_json::json!({"pattern": "needle", "path": "src"}),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                    id: "call-2".to_string(),
                    output: serde_json::json!({
                        "path": "/tmp/project/src",
                        "matches": [
                            {"relative_path": "lib.rs"},
                            {"relative_path": "main.rs"}
                        ]
                    }),
                    success: true,
                }),
            ],
        };

        let messages = agent_chat_messages(&session);
        assert_eq!(messages[0].content, "search_files: /needle/ in src");
        assert_eq!(messages[1].role, djinn_tui::AgentChatRole::ToolOutput);
        assert!(messages[1].content.contains("search_files result: ok"));
        assert!(messages[1].content.contains("path: /tmp/project/src"));
        assert!(messages[1].content.contains("2 matches"));
        assert!(messages[1].content.contains("- lib.rs"));
    }

    #[test]
    fn agent_chat_messages_summarize_write_file_mutations() {
        let session = AgentSession {
            id: AgentSessionId::new("agt_test"),
            meta: AgentSessionMeta::default(),
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::ToolCall {
                    id: "call-write".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({"path": "docs/note.md", "content": "hello\nworld\n"}),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                    id: "call-write".to_string(),
                    output: serde_json::json!({
                        "patch_id": "patch_1",
                        "summary": [
                            {"operation": "write", "relative_path": "docs/note.md", "lines_added": 2, "lines_removed": 0}
                        ]
                    }),
                    success: true,
                }),
            ],
        };

        let messages = agent_chat_messages(&session);
        assert_eq!(
            messages[0].content,
            "write_file: docs/note.md (12 bytes, 2 lines)"
        );
        assert!(messages[1].content.contains("write_file result: ok"));
        assert!(messages[1].content.contains("patch: patch_1"));
        assert!(messages[1].content.contains("- write docs/note.md (+2/-0)"));
        assert!(!messages[1].content.contains("relative_path"));
    }

    #[test]
    fn agent_chat_messages_summarize_edit_file_mutations() {
        let session = AgentSession {
            id: AgentSessionId::new("agt_test"),
            meta: AgentSessionMeta::default(),
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::ToolCall {
                    id: "call-edit".to_string(),
                    name: "edit_file".to_string(),
                    input: serde_json::json!({
                        "path": "src/lib.rs",
                        "old_text": "old\ntext",
                        "new_text": "new"
                    }),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                    id: "call-edit".to_string(),
                    output: serde_json::json!({
                        "approval_required": true,
                        "preview": [
                            {"operation": "edit", "relative_path": "src/lib.rs", "lines_added": 1, "lines_removed": 2}
                        ]
                    }),
                    success: false,
                }),
            ],
        };

        let messages = agent_chat_messages(&session);
        assert_eq!(messages[0].content, "edit_file: src/lib.rs (+1/-2)");
        assert!(messages[1].content.contains("edit_file result: failed"));
        assert!(messages[1].content.contains("approval required"));
        assert!(messages[1].content.contains("- edit src/lib.rs"));
        assert!(!messages[1].content.contains("approval_required"));
    }

    #[test]
    fn agent_chat_messages_render_structured_errors() {
        let event = AgentSessionEvent::new(AgentSessionEventKind::Error {
            phase: "model_request".to_string(),
            message: "OpenAI request failed".to_string(),
            details: Some(serde_json::json!({"model": "gpt-test", "round": 0})),
        });
        let session = AgentSession {
            id: AgentSessionId::new("agt_test"),
            meta: AgentSessionMeta::default(),
            events: vec![event.clone()],
        };

        let messages = agent_chat_messages(&session);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, djinn_tui::AgentChatRole::Notice);
        assert!(messages[0]
            .content
            .contains("error [model_request]: OpenAI request failed"));
        assert!(messages[0].content.contains("gpt-test"));
        assert!(!messages[0].content.contains("{\n"));
    }

    #[test]
    fn agent_progress_thoughts_surface_tool_budget_details() {
        let event = AgentProgressEvent::ModelRequestStarted { round: 5 };

        let message = agent_progress_message(&event, 12).unwrap();
        let notice = agent_progress_notice(&event, 12);

        assert!(message.content.contains("Planning next step (round 6)…"));
        assert!(message.content.contains("model request 6 of 13"));
        assert!(message.content.contains("Tool-round safety cap"));
        assert!(message.content.contains("hidden reasoning is not exposed"));
        assert_eq!(
            notice,
            "Planning next step (round 6) · tool-round cap 5/12…"
        );
    }

    #[test]
    fn agent_progress_names_planned_tools_and_includes_input_snippets() {
        let event = AgentProgressEvent::ModelResponseCompleted {
            round: 0,
            elapsed_ms: 2150,
            tool_calls: 1,
            planned_tools: vec![djinn_agent::ModelToolCall {
                id: "call-read".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "Cargo.toml"}),
            }],
            has_message: false,
        };

        let message = agent_progress_message(&event, 12).unwrap();
        let notice = agent_progress_notice(&event, 12);

        assert!(message.content.contains("Planned read_file · 2.1s"));
        assert!(!message.content.contains("Planned 1 tool call"));
        assert!(message
            .content
            .contains("Planned tool: read_file: Cargo.toml"));
        assert!(message.content.contains("Input snippet:\npath: Cargo.toml"));
        assert_eq!(notice, "Planned read_file.");
    }

    #[test]
    fn agent_progress_tool_completion_includes_result_snippet() {
        let event = AgentProgressEvent::ToolCallCompleted {
            round: 0,
            call: djinn_agent::ModelToolCall {
                id: "call-read".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "Cargo.toml"}),
            },
            result: djinn_agent::ToolResult {
                success: true,
                output: serde_json::json!({
                    "path": "Cargo.toml",
                    "content": "[package]\nname = \"djinn\"\nversion = \"0.1.0\"\n"
                }),
            },
            elapsed_ms: 42,
        };

        let message = agent_progress_message(&event, 12).unwrap();

        assert!(message
            .content
            .contains("Finished read_file: Cargo.toml · 42ms"));
        assert!(message.content.contains("Result:\npath: Cargo.toml"));
        assert!(message
            .content
            .contains("preview:\n[package]\nname = \"djinn\""));
    }

    #[test]
    fn agent_chat_messages_with_progress_preserves_timeline_order() {
        let session = AgentSession {
            id: AgentSessionId::new("agt_progress"),
            meta: AgentSessionMeta::default(),
            events: vec![AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                content: "audit the tui".to_string(),
            })],
        };
        let progress = vec![
            agent_thought_message("Waiting for model response…"),
            agent_thought_message("Planning next step…"),
            agent_thought_message("Planned 1 tool call · 2.1s"),
        ];

        let messages = agent_chat_messages_with_progress(&session, &progress);

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, djinn_tui::AgentChatRole::User);
        assert_eq!(messages[1].role, djinn_tui::AgentChatRole::Thought);
        assert_eq!(messages[1].content, "Waiting for model response…");
        assert_eq!(messages[2].content, "Planning next step…");
        assert_eq!(messages[3].content, "Planned 1 tool call · 2.1s");
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
        assert_eq!(report.context_ingestible_count, 1);
        let repo_status = report.repo.as_ref().unwrap();
        assert!(repo_status.link_exists);
        assert!(repo_status.link_is_symlink);
        assert!(!repo_status.link_broken);
        assert!(text.contains("Skipped context:"));
        assert!(text.contains("State: not_started"));
        assert!(text.contains("Latest turn:"));
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
        append_foreground_session_lifecycle_event(
            &store,
            &id,
            AgentSessionLifecycleState::Completed,
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

        let view = folder_session_status_tui_view(&session_dir).unwrap();

        assert_eq!(view.title, "bap-questions");
        assert_eq!(view.state, "not_started");
        assert_eq!(view.turn_count, 1);
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
    fn prepare_agent_chat_session_creates_new_session() {
        let store = temp_agent_store("create");
        let workspace = std::env::temp_dir().join(format!(
            "djinn-cli-agent-chat-workspace-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&workspace).unwrap();
        let parent_id = store
            .create_session(AgentSessionMeta {
                title: "Parent session".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        let prepared = prepare_agent_chat_session(
            &store,
            None,
            Some("Pairing session".to_string()),
            Some(workspace.clone()),
            "review",
            Some("reviewer".to_string()),
            Some("openai/gpt-5.5".to_string()),
            vec!["docs/review.md".to_string()],
            vec!["read_file".to_string()],
            Some(parent_id.clone()),
        )
        .unwrap();
        let loaded = store.load_session(&prepared.id).unwrap();
        let canonical_workspace = workspace.canonicalize().unwrap();

        assert_eq!(prepared.profile, "review");
        assert_eq!(loaded.meta.title, "Pairing session");
        assert_eq!(loaded.meta.profile, "review");
        assert_eq!(loaded.meta.agent_name.as_deref(), Some("reviewer"));
        assert_eq!(
            loaded
                .meta
                .parent_session_id
                .as_ref()
                .map(AgentSessionId::as_str),
            Some(parent_id.as_str())
        );
        assert_eq!(
            loaded.meta.workspace,
            canonical_workspace.display().to_string()
        );
        let runtime_config = loaded.meta.runtime_config.as_ref().unwrap();
        assert_eq!(runtime_config.model, "openai/gpt-5.5");
        assert_eq!(runtime_config.agent_tools, vec!["read_file"]);
        assert!(runtime_config
            .permissions
            .guardrails
            .iter()
            .any(|guardrail| guardrail.contains("session approvals")));
    }

    #[test]
    fn prepare_agent_chat_session_resumes_existing_metadata() {
        let store = temp_agent_store("resume");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Existing chat".to_string(),
                workspace: "/tmp/existing-workspace".to_string(),
                profile: "architect".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        let prepared = prepare_agent_chat_session(
            &store,
            Some(id.as_str()),
            Some("Ignored title".to_string()),
            Some(PathBuf::from("/tmp/ignored-workspace")),
            "ignored-profile",
            Some("ignored-agent".to_string()),
            Some("ignored-model".to_string()),
            vec!["ignored.md".to_string()],
            vec!["shell".to_string()],
            Some(AgentSessionId::new("agt_ignored_parent")),
        )
        .unwrap();

        assert_eq!(prepared.id, id);
        assert_eq!(prepared.workspace, "/tmp/existing-workspace");
        assert_eq!(prepared.profile, "architect");
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
    fn agent_chat_command_palette_marks_current_profile_and_model() {
        let entries = agent_chat_command_palette("default", "openai/gpt-5.5").unwrap();

        assert!(entries
            .iter()
            .any(|entry| entry.label == "✓ Current profile · default"));
        assert!(entries
            .iter()
            .any(|entry| entry.label == "✓ Current model · openai/gpt-5.5"));
    }

    #[test]
    fn foreground_session_args_can_start_fresh_or_child_from_current_session() {
        let mut session = AgentSession {
            id: AgentSessionId::new("agt_parent"),
            meta: AgentSessionMeta {
                profile: "architect".to_string(),
                agent_name: Some("reviewer".to_string()),
                ..AgentSessionMeta::default()
            },
            events: vec![AgentSessionEvent::new(
                AgentSessionEventKind::SessionModelUpdated {
                    model: "openai/gpt-5.5".to_string(),
                },
            )],
        };

        let mut fresh_args = default_agent_chat_args();
        prepare_foreground_session_args_from_parent(&mut fresh_args, &session, false);

        assert_eq!(fresh_args.profile, "architect");
        assert_eq!(fresh_args.agent.as_deref(), Some("reviewer"));
        assert_eq!(fresh_args.model.as_deref(), Some("openai/gpt-5.5"));
        assert_eq!(fresh_args.parent_session, None);
        assert_eq!(fresh_args.resume, None);

        session.meta.profile = "default".to_string();
        let mut child_args = default_agent_chat_args();
        prepare_foreground_session_args_from_parent(&mut child_args, &session, true);

        assert_eq!(child_args.profile, "default");
        assert_eq!(child_args.parent_session.as_deref(), Some("agt_parent"));
        assert_eq!(child_args.title, None);
        assert_eq!(child_args.workspace, None);
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
    fn update_agent_session_profile_skips_noop_and_records_changes() {
        let store = temp_agent_store("profile-noop");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Agent chat".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        assert!(!update_agent_session_profile(&store, &id, " default ").unwrap());
        assert!(update_agent_session_profile(&store, &id, "architect").unwrap());

        let loaded = store.load_session(&id).unwrap();
        assert_eq!(loaded.meta.profile, "architect");
        assert_eq!(
            loaded
                .events
                .iter()
                .filter(|event| matches!(
                    event.kind,
                    AgentSessionEventKind::SessionProfileUpdated { .. }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn update_agent_session_model_skips_noop_and_records_changes() {
        let store = temp_agent_store("model-noop");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Agent chat".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        assert!(
            !update_agent_session_model(&store, &id, "openai/gpt-5.5", " openai/gpt-5.5 ").unwrap()
        );
        assert!(
            update_agent_session_model(&store, &id, "openai/gpt-5.5", "openai/gpt-5.4-mini")
                .unwrap()
        );

        let loaded = store.load_session(&id).unwrap();
        assert_eq!(
            latest_session_model(&loaded).as_deref(),
            Some("openai/gpt-5.4-mini")
        );
        assert_eq!(
            loaded
                .events
                .iter()
                .filter(|event| matches!(
                    event.kind,
                    AgentSessionEventKind::SessionModelUpdated { .. }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn update_agent_session_title_skips_noop_and_records_changes() {
        let store = temp_agent_store("title-noop");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Agent chat".to_string(),
                workspace: "/tmp/workspace".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        assert!(!update_agent_session_title(&store, &id, " Agent chat ").unwrap());
        assert!(update_agent_session_title(&store, &id, "Renamed session").unwrap());

        let loaded = store.load_session(&id).unwrap();
        assert_eq!(loaded.meta.title, "Renamed session");
        assert_eq!(
            loaded
                .events
                .iter()
                .filter(|event| matches!(
                    event.kind,
                    AgentSessionEventKind::SessionTitleUpdated { .. }
                ))
                .count(),
            1
        );
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
    fn patch_opencode_config_adds_schema_and_plugin_array() {
        let (rendered, changed) =
            patch_opencode_config_content(Some("{}\n"), "./plugins/djinn-watch.js").unwrap();
        assert!(changed);
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            parsed["$schema"],
            Value::String("https://opencode.ai/config.json".to_string())
        );
        assert_eq!(
            parsed["plugin"],
            Value::Array(vec![Value::String("./plugins/djinn-watch.js".to_string())])
        );
    }

    #[test]
    fn patch_opencode_config_preserves_existing_plugin_entries() {
        let existing = r#"{"plugin":"opencode-gemini-auth"}
"#;
        let (rendered, _) =
            patch_opencode_config_content(Some(existing), "./plugins/djinn-watch.js").unwrap();
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            parsed["plugin"],
            Value::Array(vec![
                Value::String("opencode-gemini-auth".to_string()),
                Value::String("./plugins/djinn-watch.js".to_string())
            ])
        );
    }

    #[test]
    fn patch_opencode_config_is_idempotent() {
        let (first, _) = patch_opencode_config_content(None, "./plugins/djinn-watch.js").unwrap();
        let (second, changed) =
            patch_opencode_config_content(Some(&first), "./plugins/djinn-watch.js").unwrap();
        assert!(!changed);
        assert_eq!(first, second);
    }

    #[test]
    fn opencode_plugin_hydrates_djinn_session_metadata() {
        assert!(OPENCODE_PLUGIN.contains("hydrateDjinnBridge"));
        assert!(OPENCODE_PLUGIN.contains("client.session.update"));
        assert!(OPENCODE_PLUGIN
            .contains("metadata = { ...(current?.data?.metadata || {}), djinn: bridge }"));
    }

    #[test]
    fn opencode_bridge_session_id_detects_converted_chat() {
        let mut state = OpencodeWatchState::default();
        state.sessions.insert(
            "ses_1".to_string(),
            OpencodeSessionState {
                djinn_session_id: "agt_1".to_string(),
                ..OpencodeSessionState::default()
            },
        );
        let chat = ChatRecord {
            id: "chat".to_string(),
            title: "OpenCode".to_string(),
            content: String::new(),
            source: "opencode".to_string(),
            source_id: "ses_1".to_string(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        };

        assert_eq!(opencode_bridge_session_id(&state, &chat), Some("agt_1"));
    }

    #[test]
    fn converted_opencode_chat_record_points_at_djinn_session() {
        let chat = ChatRecord {
            id: "chat".to_string(),
            title: "OpenCode".to_string(),
            content: String::new(),
            source: "opencode".to_string(),
            source_id: "ses_1".to_string(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: "2026-07-24".to_string(),
        };
        let summary = AgentSessionSummary {
            id: AgentSessionId::new("agt_1"),
            title: "Converted title".to_string(),
            workspace: "/tmp/project".to_string(),
            profile: "default".to_string(),
            agent_name: None,
            parent_session_id: None,
            source: "opencode".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            updated_at: "2026-07-24T01:00:00Z".to_string(),
            event_count: 3,
            lifecycle: AgentSessionLifecycle::default(),
        };
        let store = JsonlAgentSessionStore::default_in(&std::env::temp_dir());

        let record = converted_opencode_chat_record(&chat, &summary, &store);

        assert_eq!(record.source, "djinn-agent");
        assert_eq!(record.source_id, "agt_1");
        assert!(record.id.contains("ses_1"));
        assert!(record.title.contains("converted"));
        assert!(record
            .content
            .contains("Converted from OpenCode session ses_1"));
    }

    #[test]
    fn agent_session_chat_record_surfaces_role_and_parent_metadata() {
        let summary = AgentSessionSummary {
            id: AgentSessionId::new("agt_child"),
            title: "Review diff".to_string(),
            workspace: "/tmp/project".to_string(),
            profile: "default".to_string(),
            agent_name: Some("reviewer".to_string()),
            parent_session_id: Some(AgentSessionId::new("agt_parent")),
            source: "djinn-agent".to_string(),
            created_at: "2026-07-25T00:00:00Z".to_string(),
            updated_at: "2026-07-25T01:00:00Z".to_string(),
            event_count: 7,
            lifecycle: AgentSessionLifecycle::default(),
        };
        let store = JsonlAgentSessionStore::default_in(&std::env::temp_dir());

        let record = agent_session_chat_record(&summary, &store);

        assert_eq!(record.source, "djinn-agent");
        assert_eq!(record.source_id, "agt_child");
        assert!(record.content.contains("Agent role: reviewer"));
        assert!(record.content.contains("Parent session: agt_parent"));
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
    fn opencode_export_agent_events_reads_text_parts() {
        let events = opencode_export_agent_events(
            r#"{
              "info": {"directory": "/tmp/project"},
              "messages": [
                {"info": {"role": "user"}, "parts": [{"type": "text", "text": "hello"}]},
                {"info": {"role": "assistant"}, "parts": [
                  {"type": "reasoning", "text": "thinking"},
                  {"type": "text", "text": "world"}
                ]}
              ]
            }"#,
            "ses_test",
        );

        assert_eq!(
            opencode_export_workspace(r#"{"info":{"directory":"/tmp/project"}}"#).as_deref(),
            Some("/tmp/project")
        );
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            AgentSessionEventKind::UserMessage { content } if content == "hello"
        ));
        assert!(matches!(
            &events[1],
            AgentSessionEventKind::AssistantMessage { content } if content.contains("thinking") && content.contains("world")
        ));
    }

    #[test]
    fn opencode_export_agent_events_falls_back_to_summary_for_raw_export() {
        let events = opencode_export_agent_events("not json", "ses_test");

        assert!(matches!(
            &events[0],
            AgentSessionEventKind::Summary { content } if content.contains("Converted OpenCode session ses_test")
        ));
    }

    #[test]
    fn promote_sessions_renders_opencode_exports_as_digest_not_raw_json() {
        let records = vec![test_chat(
            "opencode-session-ses-test",
            "OpenCode session ses_test",
            "opencode",
            r#"{
              "info": {
                "id": "ses_test",
                "slug": "quiet-cactus",
                "model": {"id": "gpt-5.5", "providerID": "openai"}
              },
              "messages": [
                {"info": {"role": "user"}, "parts": [{"type": "text", "text": "Please fix the CLI output"}]},
                {"info": {"role": "assistant"}, "parts": [
                  {"type": "reasoning", "text": "hidden chain of thought"},
                  {"type": "tool", "tool": "read", "state": {"title": "Read main.rs", "output": "found promote code"}},
                  {"type": "text", "text": "I updated the formatter."}
                ]}
              ]
            }"#,
        )];
        let mut args = default_share_chats_args();
        args.mode = ShareChatsMode::Summary;

        let prompt = format_chats_review_prompt(&records, &args, &[]);

        assert!(prompt.contains("OpenCode export digest"));
        assert!(prompt.contains("- Session: ses_test"));
        assert!(prompt.contains("- Model: openai/gpt-5.5"));
        assert!(prompt.contains("User:\nPlease fix the CLI output"));
        assert!(prompt.contains("Assistant:\nTool: Read main.rs"));
        assert!(prompt.contains("I updated the formatter."));
        assert!(!prompt.contains("\"messages\""));
        assert!(!prompt.contains("hidden chain of thought"));
    }

    #[test]
    fn promote_sessions_warns_when_opencode_export_is_sanitized() {
        let records = vec![test_chat(
            "opencode-session-ses-test",
            "OpenCode session ses_test",
            "opencode",
            r#"{
              "info": {"id": "ses_test"},
              "messages": [
                {"info": {"role": "user"}, "parts": [{"type": "text", "text": "[redacted:text:part]"}]}
              ]
            }"#,
        )];
        let mut args = default_share_chats_args();
        args.mode = ShareChatsMode::Summary;

        let prompt = format_chats_review_prompt(&records, &args, &[]);

        assert!(prompt.contains("OpenCode export digest"));
        assert!(prompt.contains("Redaction: this export appears sanitized"));
        assert!(!prompt.contains("\"parts\""));
    }

    #[test]
    fn promote_sessions_summary_is_human_facing_not_agent_prompt() {
        let records = vec![test_chat(
            "opencode-session-ses-test",
            "OpenCode session ses_test",
            "opencode",
            r#"{
              "info": {"id": "ses_test", "slug": "quiet-cactus"},
              "messages": [
                {"info": {"role": "user"}, "parts": [{"type": "text", "text": "Need a useful summary"}]},
                {"info": {"role": "assistant"}, "parts": [{"type": "text", "text": "Here is the useful result."}]}
              ]
            }"#,
        )];
        let mut args = default_share_chats_args();
        args.mode = ShareChatsMode::Summary;

        let summary = format_chats_summary(&records, &args);

        assert!(summary.starts_with("# Djinn Session Summary"));
        assert!(summary.contains("This is a local digest"));
        assert!(summary.contains("User:\nNeed a useful summary"));
        assert!(summary.contains("Assistant:\nHere is the useful result."));
        assert!(!summary.contains("You are reviewing"));
        assert!(!summary.contains("Return Markdown"));
        assert!(!summary.contains("## Existing Memories"));
        assert!(!summary.contains("\"messages\""));
    }

    #[test]
    fn chat_summary_agent_prompt_uses_digest_context_for_conversation() {
        let records = vec![test_chat(
            "opencode-session-ses-test",
            "OpenCode session ses_test",
            "opencode",
            r#"{
              "info": {"id": "ses_test", "slug": "quiet-cactus"},
              "messages": [
                {"info": {"role": "user"}, "parts": [{"type": "text", "text": "Summarize this in chat"}]},
                {"info": {"role": "assistant"}, "parts": [{"type": "text", "text": "Ready for follow-up."}]}
              ]
            }"#,
        )];
        let mut args = default_share_chats_args();
        args.mode = ShareChatsMode::Summary;

        let prompt = format_chat_summary_agent_prompt(&records, &args);

        assert!(prompt.starts_with("Please summarize the selected sessions"));
        assert!(prompt.contains("so we can continue discussing them"));
        assert!(prompt.contains("OpenCode export digest"));
        assert!(prompt.contains("User:\nSummarize this in chat"));
        assert!(prompt.contains("Assistant:\nReady for follow-up."));
        assert!(!prompt.contains("\"messages\""));
    }

    #[test]
    fn chat_merge_prompt_requires_direct_memories_not_candidates() {
        let records = vec![test_chat(
            "chat-one",
            "Tooling discussion",
            "manual",
            "Prefer using the local wrapper for repeatable tasks.",
        )];
        let args = ShareChatsArgs {
            ids: Vec::new(),
            source: None,
            query: None,
            limit: 50,
            all: false,
            mode: ShareChatsMode::Merge,
            max_chars_per_chat: 4000,
            max_memories: 5,
            archive: true,
            dry_run: true,
            profile: "default".to_string(),
            model: None,
            api_key: None,
            base_url: None,
        };

        let prompt = format_chats_merge_prompt(&records, &args);

        assert!(prompt.contains("Group related sessions by topic/workflow"));
        assert!(prompt.contains("active immediately"));
        assert!(prompt.contains("Do not create an inbox, candidates, suggestions, or todos"));
        assert!(prompt.contains("source_chat_ids"));
        assert!(prompt.contains("Tooling discussion"));
    }

    #[test]
    fn chat_merge_response_parses_fenced_json_and_filters_empty_memories() {
        let parsed = parse_chat_merge_response(
            r#"```json
{
  "groups": [{"title": "Tooling", "chat_ids": ["chat-one"], "rationale": "same topic"}],
  "memories": [
    {"text": "Use local wrappers for repeatable tasks.", "scope": "project", "kind": "workflow", "confidence": "high", "evidence": ["wrapper discussion"], "source_chat_ids": ["chat-one"]},
    {"text": "   "}
  ]
}
```"#,
        )
        .unwrap();

        assert_eq!(parsed.groups.len(), 1);
        assert_eq!(parsed.memories.len(), 1);
        assert_eq!(
            parsed.memories[0].text,
            "Use local wrappers for repeatable tasks."
        );
        assert_eq!(parsed.memories[0].source_chat_ids, vec!["chat-one"]);
    }

    #[test]
    fn chat_merge_memory_sources_preserve_chat_provenance() {
        let records = vec![test_chat("chat-one", "One", "opencode", "ses_one")];

        let sources = memory_sources_for_chat_ids(&records, &["chat-one".to_string()]);

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].chat_id, "chat-one");
        assert_eq!(sources[0].source_id, "source-chat-one");
        assert_eq!(sources[0].title, "One");
    }

    #[test]
    fn opencode_share_content_does_not_fall_back_to_raw_broken_json() {
        let record = test_chat(
            "opencode-session-ses-test",
            "Broken OpenCode session",
            " opencode ",
            r#"{"info":{"id":"ses_test"},"messages":["#,
        );

        let content = share_chat_content(&record);

        assert!(content.contains("looked like JSON but could not be parsed"));
        assert!(!content.contains("\"messages\""));
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
    fn select_chats_for_share_defaults_to_latest_limit() {
        let records = vec![
            test_chat("one", "One", "manual", "first"),
            test_chat("two", "Two", "manual", "second"),
            test_chat("three", "Three", "manual", "third"),
        ];
        let mut args = default_share_chats_args();
        args.limit = 2;
        let selected = select_chats_for_share(&records, &args).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );
    }

    #[test]
    fn select_chats_for_share_filters_by_source_and_query() {
        let records = vec![
            test_chat("one", "One", "manual", "rust notes"),
            test_chat("two", "Two", "opencode", "python notes"),
            test_chat("three", "Three", "opencode", "rust patterns"),
        ];
        let mut args = default_share_chats_args();
        args.source = Some("opencode".to_string());
        args.query = Some("rust".to_string());
        let selected = select_chats_for_share(&records, &args).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "three");
    }

    #[test]
    fn format_chats_review_prompt_includes_memory_mode_commands() {
        let records = vec![test_chat("one", "One", "opencode", "Prefer uv here")];
        let mut args = default_share_chats_args();
        args.mode = ShareChatsMode::Memories;
        let prompt = format_chats_review_prompt(&records, &args, &[]);
        assert!(prompt.contains("# Djinn Multi-Session Review"));
        assert!(prompt.contains("djinn add memory"));
        assert!(prompt.contains("Prefer uv here"));
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
    fn memory_source_format_tolerates_missing_chat() {
        let source = MemorySource {
            source_type: "chat".to_string(),
            source: "opencode".to_string(),
            source_id: "ses_missing".to_string(),
            chat_id: "missing-chat".to_string(),
            title: "Deleted OpenCode session".to_string(),
            captured_at: "2026-07-09".to_string(),
        };
        assert!(!memory_source_chat_exists(&source, &[]));
        let rendered = format_memory_source(&source, &[]);
        assert!(rendered.contains("missing/deleted"));
        assert!(rendered.contains("Deleted OpenCode session"));
    }
}
