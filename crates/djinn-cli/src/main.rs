use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use djinn_agent::{
    tools_with_policies_file_history_and_gate, AgentProgressEvent, AgentRuntime, ModelMessage,
    ModelRequest, ModelRole, OpenAiAuth, OpenAiClient, OpenAiOAuth, PermissionDecision,
    PermissionEffect, PermissionGate, PermissionPolicy, PermissionRequest, PermissionRule,
    ReadAccessEffect, ReadAccessPolicy, ReadAccessRule, ToolSpec,
};
use djinn_chats::{ChatRecord, ChatRestoreReport};
use djinn_contexts::{resolve_context, ContextInput, ContextRecord, ContextStore};
use djinn_memory::{
    ActionRecord, ActionStore, AgentSession, AgentSessionEvent, AgentSessionEventKind,
    AgentSessionFilter, AgentSessionId, AgentSessionMeta, AgentSessionStore, AgentSessionSummary,
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
    /// Promote raw context into active memories.
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
    /// Emit agent-ready context or prompts.
    Share(ShareArgs),
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
    /// Run or inspect Djinn-native agent sessions.
    Agent(AgentArgs),
    /// Open the unified terminal dashboard.
    Tui(TuiArgs),
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
    /// List raw or summarized AI interactions.
    Chats(ListChatsArgs),
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
    /// Show a chat/session by id.
    Chat(ShowChatArgs),
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
    /// Add a raw or summarized AI interaction from a file.
    Chat(AddChatArgs),
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
    /// Ask OpenCode to review recent Djinn chats and add active memories.
    Chats(ReviewChatsArgs),
    /// Ask OpenCode to review one or more memories and create suggestions.
    Memories(ReviewMemoriesArgs),
    /// Ask OpenCode to review one memory and create suggestions.
    Memory(ReviewMemoriesArgs),
    /// Compatibility alias for `djinn review chats --source opencode`.
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
    /// Maximum recent chats to review.
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// Review all matching chats instead of applying --limit.
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
    /// Maximum recent OpenCode chats to review.
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// Review all matching OpenCode chats instead of applying --limit.
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
    /// Emit a memory-extraction prompt for one chat.
    Chat(ShareChatArgs),
    /// Emit a memory-extraction prompt for multiple chats.
    Chats(ShareChatsArgs),
    /// Review one or more memories and create suggestions.
    Memory(ReviewMemoriesArgs),
    /// Review one or more memories and create suggestions.
    Memories(ReviewMemoriesArgs),
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
    /// Remove a chat matching an id, source id, or title fragment.
    Chat { id: String },
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
    /// Clear all chats after interactive confirmation.
    Chats {
        /// Skip creating chats.backup-*.jsonl before clearing.
        #[arg(long)]
        no_backup: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ArchiveNoun {
    /// Archive selected chat rows and remove them from the active chat index.
    Chats(ArchiveChatsArgs),
    /// List chat archive files.
    List(ArchiveListArgs),
    /// Show the contents of one chat archive file.
    Show(ArchiveShowArgs),
    /// Restore chats from an archive file.
    Restore(ArchiveRestoreArgs),
    /// Remove one chat archive file after confirmation.
    Rm(ArchiveRemoveArgs),
}

#[derive(Debug, Args)]
struct PruneArgs {
    #[command(subcommand)]
    noun: PruneNoun,
}

#[derive(Debug, Subcommand)]
enum PruneNoun {
    /// Remove chats older than a duration such as 30d or 12days.
    Chats(PruneChatsArgs),
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
struct ShareArgs {
    #[command(subcommand)]
    noun: ShareNoun,
}

#[derive(Debug, Subcommand)]
enum ShareNoun {
    /// Emit agent-ready context for local tools.
    Tools(ToolsScope),
    /// Emit agent-ready context for memories.
    Memories,
    /// Emit agent-ready context for open suggestions.
    Suggestions,
    /// Emit an agent-ready improvement prompt from Djinn's current knowledge.
    Ideas,
    /// Emit agent-ready context for skills.
    Skills(ShareSkillsArgs),
    /// Emit a memory-extraction prompt for a chat/session.
    Chat(ShareChatArgs),
    /// Emit an agent prompt from multiple chats/sessions.
    Chats(ShareChatsArgs),
    /// Distill selected chats into active memories and optionally archive them.
    Merge(ShareMergeArgs),
}

#[derive(Debug, Args)]
struct SearchArgs {
    #[command(subcommand)]
    noun: SearchNoun,
}

#[derive(Debug, Subcommand)]
enum SearchNoun {
    /// Search chats/sessions.
    Chats { query: String },
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
    /// Install the OpenCode plugin that auto-imports sessions into Djinn chats.
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

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Inspect discovered agent profiles and models.
    Config(AgentConfigArgs),
    /// Inspect built-in agent runtime tools.
    Tools(AgentToolsArgs),
    /// Manage Djinn-native agent sessions.
    Session(AgentSessionArgs),
    /// Inspect or restore apply_patch file-history entries.
    FileHistory(AgentFileHistoryArgs),
    /// Record a non-interactive prompt in an agent session.
    Ask(AgentAskArgs),
    /// Open an interactive terminal chat with the Djinn agent runtime.
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

#[derive(Debug, Args)]
struct AgentSessionArgs {
    #[command(subcommand)]
    command: AgentSessionCommand,
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
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum AgentSessionCommand {
    /// Create an empty agent session.
    New(AgentSessionNewArgs),
    /// List agent sessions.
    List(AgentSessionListArgs),
    /// Show one agent session.
    Show(AgentSessionShowArgs),
    /// Rename an agent session by appending a title metadata event.
    Rename(AgentSessionRenameArgs),
    /// Delete an agent session JSONL file.
    Delete(AgentSessionDeleteArgs),
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
struct ShareSkillsArgs {
    /// Include skill file contents, truncated per skill.
    #[arg(long)]
    include_content: bool,
    /// Maximum characters per skill when --include-content is used.
    #[arg(long, default_value_t = 2000)]
    max_chars_per_skill: usize,
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
struct AgentSessionNewArgs {
    /// Human-friendly session title.
    #[arg(long)]
    title: Option<String>,
    /// Workspace path for the session. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Session source label.
    #[arg(long, default_value = "djinn-agent")]
    source: String,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentSessionListArgs {
    /// Filter by exact workspace string.
    #[arg(long)]
    workspace: Option<String>,
    /// Filter by exact agent profile.
    #[arg(long)]
    profile: Option<String>,
    /// Filter by exact source label.
    #[arg(long)]
    source: Option<String>,
    /// Maximum sessions to list.
    #[arg(long)]
    limit: Option<usize>,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentSessionShowArgs {
    /// Agent session id.
    id: String,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentSessionRenameArgs {
    /// Agent session id.
    id: String,
    /// New human-friendly session title.
    title: String,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct AgentSessionDeleteArgs {
    /// Agent session id.
    id: String,
    /// Actually delete the session file.
    #[arg(long)]
    force: bool,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
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
    /// Prompt to send to OpenAI.
    prompt: String,
    /// Human-friendly session title. Defaults to a trimmed prompt preview.
    #[arg(long)]
    title: Option<String>,
    /// Workspace path for the session. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
    /// OpenAI model to use. Defaults to DJINN_OPENAI_MODEL or gpt-4o-mini.
    #[arg(long)]
    model: Option<String>,
    /// OpenAI API key. Defaults to OPENAI_API_KEY.
    #[arg(long = "api-key")]
    api_key: Option<String>,
    /// OpenAI-compatible base URL. Defaults to OPENAI_BASE_URL or https://api.openai.com/v1.
    #[arg(long = "base-url")]
    base_url: Option<String>,
    /// Maximum model/tool-call rounds before stopping.
    #[arg(long = "max-tool-rounds", default_value_t = 5)]
    max_tool_rounds: usize,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
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
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
    /// OpenAI model to use. Defaults to OpenCode config, DJINN_OPENAI_MODEL, or gpt-4o-mini.
    #[arg(long)]
    model: Option<String>,
    /// OpenAI API key. Defaults to OpenCode config/auth or OPENAI_API_KEY.
    #[arg(long = "api-key")]
    api_key: Option<String>,
    /// OpenAI-compatible base URL. Defaults to OPENAI_BASE_URL or https://api.openai.com/v1.
    #[arg(long = "base-url")]
    base_url: Option<String>,
    /// Maximum model/tool-call rounds before stopping.
    #[arg(long = "max-tool-rounds", default_value_t = 5)]
    max_tool_rounds: usize,
}

struct TerminalPermissionGate;

#[async_trait]
impl PermissionGate for TerminalPermissionGate {
    async fn approve(&self, request: PermissionRequest) -> Result<PermissionDecision> {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            return match djinn_tui::run_approval_dialog(request.metadata.clone())? {
                djinn_tui::ApprovalDecision::Approve => Ok(PermissionDecision::Allow),
                djinn_tui::ApprovalDecision::Deny => Ok(PermissionDecision::Deny),
            };
        }
        eprintln!("\nPermission approval required: {}", request.description);
        eprint!("{}", format_permission_preview(&request.metadata)?);
        eprint!("Approve this patch? [y/N] ");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        if answer == "y" || answer == "yes" {
            Ok(PermissionDecision::Allow)
        } else {
            Ok(PermissionDecision::Deny)
        }
    }
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
    /// Chat id, source id, or unambiguous title fragment.
    id: String,
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct PruneChatsArgs {
    /// Prune chats older than this duration, for example: 30d or 12days.
    #[arg(long = "older-than")]
    older_than: String,
    /// Skip creating chats.backup-*.jsonl before pruning.
    #[arg(long)]
    no_backup: bool,
}

#[derive(Debug, Args)]
struct ArchiveChatsArgs {
    /// Optional chat ids, source ids, or unambiguous title fragments to archive.
    ids: Vec<String>,
    /// Filter by source, for example: opencode.
    #[arg(long)]
    source: Option<String>,
    /// Filter chats by id, title, source metadata, path, or content.
    #[arg(long)]
    query: Option<String>,
    /// Maximum number of chats to archive unless --all or explicit ids are used.
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// Include every matching chat. Use deliberately.
    #[arg(long)]
    all: bool,
    /// Print selected chats without writing an archive or removing rows.
    #[arg(long)]
    dry_run: bool,
    /// Required to actually archive and remove selected chat rows.
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
    /// Include chat content previews.
    #[arg(long)]
    content: bool,
    /// Maximum characters to show per chat when --content is set.
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
    /// Replace existing chat rows with matching id or source/source-id.
    #[arg(long)]
    force: bool,
    /// Print what would be restored without mutating the active chat index.
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
    Chats,
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
    /// Chat id, source id, or unambiguous title fragment.
    id: String,
    /// Emit raw context only instead of a memory-extraction prompt.
    #[arg(long)]
    context_only: bool,
}

#[derive(Debug, Args)]
struct ShareChatsArgs {
    /// Optional chat ids, source ids, or unambiguous title fragments to include.
    ids: Vec<String>,
    /// Filter by source, for example: opencode.
    #[arg(long)]
    source: Option<String>,
    /// Filter chats by id, title, source metadata, path, or content.
    #[arg(long)]
    query: Option<String>,
    /// Maximum number of chats to include unless --all or explicit ids are used.
    #[arg(long, default_value_t = 10)]
    limit: usize,
    /// Include every matching chat. Use deliberately; this can produce a large prompt.
    #[arg(long)]
    all: bool,
    /// Prompt style for the grouped chats.
    #[arg(long, value_enum, default_value_t = ShareChatsMode::Patterns)]
    mode: ShareChatsMode,
    /// Emit bundled chat context only, without summary/pattern/memory instructions.
    #[arg(long)]
    context_only: bool,
    /// Maximum characters to include from each chat body.
    #[arg(long, default_value_t = 4000)]
    max_chars_per_chat: usize,
}

#[derive(Debug, Args)]
struct ShareMergeArgs {
    /// Optional chat ids, source ids, or unambiguous title fragments to include.
    ids: Vec<String>,
    /// Filter by source, for example: opencode.
    #[arg(long)]
    source: Option<String>,
    /// Filter chats by id, title, source metadata, path, or content.
    #[arg(long)]
    query: Option<String>,
    /// Maximum number of chats to include unless --all or explicit ids are used.
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// Include every matching chat. Use deliberately; this can produce a large prompt.
    #[arg(long)]
    all: bool,
    /// Maximum characters to include from each chat body.
    #[arg(long, default_value_t = 4000)]
    max_chars_per_chat: usize,
    /// Maximum memories the model should return.
    #[arg(long, default_value_t = 20)]
    max_memories: usize,
    /// Archive selected chat rows after memories are written successfully.
    #[arg(long)]
    archive: bool,
    /// Print the merge prompt instead of running the model, writing memories, or archiving.
    #[arg(long)]
    dry_run: bool,
    /// Agent profile name.
    #[arg(long, default_value = "default")]
    profile: String,
    /// OpenAI model to use. Defaults to OpenCode config, DJINN_OPENAI_MODEL, or gpt-4o-mini.
    #[arg(long)]
    model: Option<String>,
    /// OpenAI API key. Defaults to OpenCode config/auth or OPENAI_API_KEY.
    #[arg(long = "api-key")]
    api_key: Option<String>,
    /// OpenAI-compatible base URL. Defaults to OPENAI_BASE_URL or https://api.openai.com/v1.
    #[arg(long = "base-url")]
    base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ShareChatsMode {
    /// Ask the agent to summarize the grouped chats.
    Summary,
    /// Ask the agent to find recurring patterns across chats.
    Patterns,
    /// Ask the agent to propose durable memory commands from cross-chat patterns.
    Memories,
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
 *   DJINN_OPENCODE_REVIEW_LIMIT        recent OpenCode chats per review
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

    const args = [DJINN_BIN, "review", "chats", "--source", "opencode", "--limit", reviewLimit()]
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
            return run_interactive_app(AgentChatArgs {
                resume: None,
                title: None,
                workspace: None,
                profile: "default".to_string(),
                model: None,
                api_key: None,
                base_url: None,
                max_tool_rounds: 5,
            });
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
        Command::Share(args) => run_share(args),
        Command::Search(args) => run_search(args),
        Command::Watch(args) => run_watch(args),
        Command::Install(args) => run_install(args),
        Command::Uninstall(args) => run_uninstall(args),
        Command::Status(args) => run_status(args),
        Command::Switch(args) => run_switch(args),
        Command::Open(args) => run_open(args),
        Command::Agent(args) => run_agent(args),
        Command::Tui(args) => {
            if let Some(args) = run_tui(args)? {
                run_interactive_app(args)
            } else {
                Ok(())
            }
        }
    }
}

fn run_list(args: ListArgs) -> Result<()> {
    match args.noun {
        ListNoun::Tools(scope) => list_tools(scope),
        ListNoun::Memories => list_memories(),
        ListNoun::Suggestions => list_suggestions(),
        ListNoun::Ideas => list_ideas(),
        ListNoun::Actions => list_actions(),
        ListNoun::Chats(args) => list_chats(args),
        ListNoun::Skills(args) => list_skills(args),
        ListNoun::Contexts(args) | ListNoun::Ctx(args) => list_contexts(args),
    }
}

fn run_show(args: ShowArgs) -> Result<()> {
    match args.noun {
        ShowNoun::Chat(args) => show_chat(args),
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
        AddNoun::Chat(args) => add_chat(args),
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
        PromoteNoun::Chat(args) => promote_chat(args),
        PromoteNoun::Chats(args) => promote_chats(args),
        PromoteNoun::Memory(args) | PromoteNoun::Memories(args) => review_memories(args),
    }
}

fn run_review(args: ReviewArgs) -> Result<()> {
    match args.source {
        ReviewSource::Chats(args) => review_chats(args),
        ReviewSource::Memory(args) | ReviewSource::Memories(args) => review_memories(args),
        ReviewSource::Opencode(args) => review_opencode(args),
    }
}

fn run_rm(args: RmArgs) -> Result<()> {
    match args.noun {
        RmNoun::Memory { keyword } => rm_memory(&keyword),
        RmNoun::Chat { id } => rm_chat(&id),
        RmNoun::Skill(args) => rm_skill(args),
    }
}

fn run_clear(args: ClearArgs) -> Result<()> {
    match args.noun {
        ClearNoun::Memories { no_backup } => clear_memories(no_backup),
        ClearNoun::Chats { no_backup } => clear_chats(no_backup),
    }
}

fn run_archive(args: ArchiveArgs) -> Result<()> {
    match args.noun {
        ArchiveNoun::Chats(args) => archive_chats(args),
        ArchiveNoun::List(args) => list_archives(args),
        ArchiveNoun::Show(args) => show_archive(args),
        ArchiveNoun::Restore(args) => restore_archive(args),
        ArchiveNoun::Rm(args) => remove_archive(args),
    }
}

fn run_prune(args: PruneArgs) -> Result<()> {
    match args.noun {
        PruneNoun::Chats(args) => prune_chats(args),
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

fn run_share(args: ShareArgs) -> Result<()> {
    match args.noun {
        ShareNoun::Tools(scope) => {
            let roots = tool_roots(scope.roots);
            let entries = scan_tools(&roots)?;
            println!("{}", format_tools_context(&entries));
            Ok(())
        }
        ShareNoun::Memories => {
            let records = memory_store().list()?;
            println!("{}", format_memories_context(&records));
            Ok(())
        }
        ShareNoun::Suggestions => share_suggestions(),
        ShareNoun::Ideas => share_ideas(),
        ShareNoun::Skills(args) => share_skills(args),
        ShareNoun::Chat(args) => share_chat(args),
        ShareNoun::Chats(args) => share_chats(args),
        ShareNoun::Merge(args) => share_merge(args),
    }
}

fn run_search(args: SearchArgs) -> Result<()> {
    match args.noun {
        SearchNoun::Chats { query } => search_chats(&query),
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

fn run_agent(args: AgentArgs) -> Result<()> {
    match args.command {
        AgentCommand::Config(args) => run_agent_config(args),
        AgentCommand::Tools(args) => run_agent_tools(args),
        AgentCommand::Session(args) => run_agent_session(args),
        AgentCommand::FileHistory(args) => run_agent_file_history(args),
        AgentCommand::Ask(args) => agent_ask(args),
        AgentCommand::Chat(args) => run_interactive_app(args),
    }
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

fn run_agent_session(args: AgentSessionArgs) -> Result<()> {
    match args.command {
        AgentSessionCommand::New(args) => agent_session_new(args),
        AgentSessionCommand::List(args) => agent_session_list(args),
        AgentSessionCommand::Show(args) => agent_session_show(args),
        AgentSessionCommand::Rename(args) => agent_session_rename(args),
        AgentSessionCommand::Delete(args) => agent_session_delete(args),
    }
}

fn run_agent_file_history(args: AgentFileHistoryArgs) -> Result<()> {
    match args.command {
        AgentFileHistoryCommand::List(args) => agent_file_history_list(args),
        AgentFileHistoryCommand::Restore(args) => agent_file_history_restore(args),
    }
}

fn agent_config_list(args: AgentConfigListArgs) -> Result<()> {
    let current_profile = args.profile.trim().to_string();
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
    let config = resolve_agent_effective_config(args.workspace, args.profile, args.model)?;
    print!(
        "{}",
        format_agent_effective_config(&config, output_format(args.format, args.json))?
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct AgentEffectiveConfig {
    workspace: String,
    profile: String,
    model: String,
    read_access: ReadAccessPolicy,
    permissions: PermissionPolicy,
}

fn resolve_agent_effective_config(
    workspace: Option<PathBuf>,
    profile: String,
    model: Option<String>,
) -> Result<AgentEffectiveConfig> {
    let profile = profile.trim().to_string();
    let workspace = resolve_agent_workspace(workspace)?;
    let workspace_path = Path::new(&workspace);
    Ok(AgentEffectiveConfig {
        model: resolve_agent_model(model, &profile)?,
        read_access: resolve_agent_read_access_policy(&profile, workspace_path)?,
        permissions: resolve_agent_permission_policy(&profile, workspace_path)?,
        workspace,
        profile,
    })
}

fn agent_tools_list(args: AgentToolsListArgs) -> Result<()> {
    let specs = agent_tool_specs(args.workspace, &args.profile)?;
    print!(
        "{}",
        format_agent_tool_specs(&specs, output_format(args.format, args.json))?
    );
    Ok(())
}

fn agent_tools_show(args: AgentToolsShowArgs) -> Result<()> {
    let specs = agent_tool_specs(args.workspace, &args.profile)?;
    let spec = resolve_agent_tool_spec(&specs, &args.name)?;
    print!(
        "{}",
        format_agent_tool_spec(spec, output_format(args.format, args.json))?
    );
    Ok(())
}

fn agent_tool_specs(workspace: Option<PathBuf>, profile: &str) -> Result<Vec<ToolSpec>> {
    let workspace = resolve_agent_workspace(workspace)?;
    let workspace_path = Path::new(&workspace);
    let read_access = resolve_agent_read_access_policy(profile, workspace_path)?;
    let permissions = resolve_agent_permission_policy(profile, workspace_path)?;
    let registry = tools_with_policies_file_history_and_gate(
        workspace_path,
        read_access,
        permissions,
        None,
        None,
    )?;
    Ok(registry.specs())
}

fn agent_session_new(args: AgentSessionNewArgs) -> Result<()> {
    let meta = AgentSessionMeta {
        title: args
            .title
            .unwrap_or_else(|| "Untitled agent session".to_string()),
        workspace: resolve_agent_workspace(args.workspace)?,
        profile: args.profile,
        source: args.source,
        ..AgentSessionMeta::default()
    };
    let store = agent_session_store();
    let id = store.create_session(meta)?;
    let session = store.load_session(&id)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&session)?);
    } else {
        println!("Agent session created [{}]: {}", id, session.meta.title);
        println!("Workspace: {}", session.meta.workspace);
        println!("Path: {}", store.session_file_path(&id).display());
    }
    Ok(())
}

fn agent_session_list(args: AgentSessionListArgs) -> Result<()> {
    let sessions = agent_session_store().list_sessions(AgentSessionFilter {
        workspace: args.workspace,
        profile: args.profile,
        source: args.source,
        limit: args.limit,
    })?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
    } else if sessions.is_empty() {
        println!("Agent sessions are empty.");
    } else {
        for (idx, session) in sessions.iter().enumerate() {
            println!(
                "  {}. [{}] {} — {} events — {}",
                idx + 1,
                session.id,
                if session.title.is_empty() {
                    "Untitled agent session"
                } else {
                    &session.title
                },
                session.event_count,
                session.workspace
            );
        }
        println!("\nTotal: {} agent sessions", sessions.len());
    }
    Ok(())
}

fn agent_session_show(args: AgentSessionShowArgs) -> Result<()> {
    let id = AgentSessionId::new(args.id);
    let session = agent_session_store().load_session(&id)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&session)?);
        return Ok(());
    }

    println!(
        "# {}",
        if session.meta.title.is_empty() {
            "Untitled agent session"
        } else {
            &session.meta.title
        }
    );
    println!("ID: {}", session.id);
    println!("Workspace: {}", session.meta.workspace);
    println!("Profile: {}", session.meta.profile);
    println!("Source: {}", session.meta.source);
    println!("Created: {}", session.meta.created_at);
    if session.events.is_empty() {
        println!("\nNo events recorded.");
    } else {
        println!("\nEvents:");
        for event in &session.events {
            println!("- {} {}", event.created_at, format_agent_event(event));
        }
    }
    Ok(())
}

fn agent_session_rename(args: AgentSessionRenameArgs) -> Result<()> {
    let id = AgentSessionId::new(args.id);
    let title = args.title.trim().to_string();
    if title.is_empty() {
        bail!("agent session title cannot be empty");
    }
    let store = agent_session_store();
    let changed = update_agent_session_title(&store, &id, &title)?;
    let session = store.load_session(&id)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": session.id,
                "title": session.meta.title,
                "changed": changed,
                "path": store.session_file_path(&id),
            }))?
        );
    } else if changed {
        println!("Renamed agent session [{}]: {}", id, session.meta.title);
    } else {
        println!(
            "Agent session [{}] already has title: {}",
            id, session.meta.title
        );
    }
    Ok(())
}

fn agent_session_delete(args: AgentSessionDeleteArgs) -> Result<()> {
    if !args.force {
        bail!("refusing to delete agent session without --force");
    }

    let id = AgentSessionId::new(args.id);
    let store = agent_session_store();
    let path = store.session_file_path(&id);
    let session = store.delete_session(&id)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": session.id,
                "title": session.meta.title,
                "deleted": true,
                "path": path,
            }))?
        );
    } else {
        println!(
            "Deleted agent session [{}]: {}",
            id,
            if session.meta.title.is_empty() {
                "Untitled agent session"
            } else {
                &session.meta.title
            }
        );
        println!("Path: {}", path.display());
    }
    Ok(())
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

fn agent_ask(args: AgentAskArgs) -> Result<()> {
    let prompt = args.prompt;
    let profile = args.profile;
    let model = resolve_agent_model(args.model, &profile)?;
    let title = args
        .title
        .unwrap_or_else(|| prompt_title(&prompt, "Agent prompt"));
    let meta = AgentSessionMeta {
        title,
        workspace: resolve_agent_workspace(args.workspace)?,
        profile: profile.clone(),
        source: "djinn-agent".to_string(),
        ..AgentSessionMeta::default()
    };
    let store = agent_session_store();
    let id = store.create_session(meta)?;
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
        model.clone(),
        args.api_key,
        args.base_url,
        args.max_tool_rounds,
        &profile,
        !args.json,
    )?;
    let session = store.load_session(&id)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "completed",
                "provider": "openai",
                "model": model,
                "response": response,
                "session": session,
            }))?
        );
    } else {
        println!("{}", response.message.content);
        println!("\nAgent session [{}]: {}", id, session.meta.title);
        println!("Path: {}", store.session_file_path(&id).display());
    }
    Ok(())
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
                    djinn_tui::AgentChatCommand::NewSession => {
                        let store = agent_session_store();
                        let id = AgentSessionId::new(resume);
                        let session = store.load_session(&id)?;
                        args.model = args.model.or_else(|| latest_session_model(&session));
                        args.profile = session.meta.profile;
                        args.resume = None;
                        args.title = None;
                        args.workspace = None;
                    }
                    djinn_tui::AgentChatCommand::OpenSessions => {
                        match run_tui_in_session(
                            &mut tui,
                            &default_tui_args(),
                            djinn_tui::DashboardTab::Chats,
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

fn agent_chat(tui: &mut djinn_tui::TuiSession, args: AgentChatArgs) -> Result<AgentChatOutcome> {
    let store = agent_session_store();
    let chat_session = prepare_agent_chat_session(
        &store,
        args.resume.as_deref(),
        args.title,
        args.workspace,
        &args.profile,
    )?;
    let id = chat_session.id;
    let workspace = chat_session.workspace;
    let profile = chat_session.profile;
    let session = store.load_session(&id)?;
    let model = resolve_agent_model(
        args.model.or_else(|| latest_session_model(&session)),
        &profile,
    )?;
    let api_key = args.api_key;
    let base_url = args.base_url;
    let max_tool_rounds = args.max_tool_rounds;

    let exit = tui.run_agent_chat_with_progress_handler(
        agent_chat_messages(&session),
        djinn_tui::AgentChatStatus {
            session_id: id.to_string(),
            workspace: workspace.clone(),
            profile: profile.clone(),
            model: model.clone(),
            notice: "History is secondary here; type a prompt to run the agent.".to_string(),
            command_palette: agent_chat_command_palette(&profile, &model)?,
        },
        |prompt, progress| {
            store.append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: prompt.clone(),
                }),
            )?;
            maybe_auto_title_agent_session(&store, &id, &prompt)?;
            let session = store.load_session(&id)?;
            progress(
                agent_chat_messages(&session)
                    .into_iter()
                    .chain([agent_thought_message("Waiting for model response…")])
                    .collect(),
                "Waiting for model response…".to_string(),
            )?;
            complete_openai_messages_with_progress(
                &store,
                &id,
                agent_model_messages(&session, &workspace),
                model.clone(),
                api_key.clone(),
                base_url.clone(),
                max_tool_rounds,
                &profile,
                true,
                |event| {
                    let session = store.load_session(&id)?;
                    let mut messages = agent_chat_messages(&session);
                    if let Some(message) = agent_progress_message(&event) {
                        messages.push(message);
                    }
                    progress(messages, agent_progress_notice(&event))
                },
            )?;
            let session = store.load_session(&id)?;
            Ok(agent_chat_messages(&session))
        },
    )?;

    if let djinn_tui::AgentChatExit::Dashboard { initial_tab } = exit {
        return Ok(AgentChatOutcome::Dashboard {
            resume: id.to_string(),
            initial_tab,
        });
    }
    if let djinn_tui::AgentChatExit::Command(command) = exit {
        return Ok(AgentChatOutcome::Command {
            resume: id.to_string(),
            command,
        });
    }

    let session = store.load_session(&id)?;
    Ok(AgentChatOutcome::Quit {
        session_id: id.to_string(),
        title: session.meta.title,
        path: store.session_file_path(&id),
    })
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

    let workspace = resolve_agent_workspace(workspace)?;
    let meta = AgentSessionMeta {
        title: title.unwrap_or_else(|| "Agent chat".to_string()),
        workspace: workspace.clone(),
        profile: profile.to_string(),
        source: "djinn-agent".to_string(),
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
    let mut entries = vec![
        djinn_tui::AgentChatCommandEntry {
            section: "Session".to_string(),
            label: "New session".to_string(),
            description: "Start a fresh Agent chat with the current profile/model".to_string(),
            command: djinn_tui::AgentChatCommand::NewSession,
        },
        djinn_tui::AgentChatCommandEntry {
            section: "Session".to_string(),
            label: "Resume session…".to_string(),
            description: "Open the Chats/session picker".to_string(),
            command: djinn_tui::AgentChatCommand::OpenSessions,
        },
    ];
    for (label, tab) in [
        ("Open Tools", djinn_tui::DashboardTab::Tools),
        ("Open Chats", djinn_tui::DashboardTab::Chats),
        ("Open Memories", djinn_tui::DashboardTab::Memories),
        ("Open Suggestions", djinn_tui::DashboardTab::Suggestions),
        ("Open Skills", djinn_tui::DashboardTab::Skills),
    ] {
        entries.push(djinn_tui::AgentChatCommandEntry {
            section: "Navigation".to_string(),
            label: label.to_string(),
            description: "Jump to this dashboard tab".to_string(),
            command: djinn_tui::AgentChatCommand::OpenDashboardTab(tab),
        });
    }
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
    for value in opencode_config_values()? {
        for container in ["agent", "agents"] {
            if let Some(agents) = value.get(container).and_then(Value::as_object) {
                profiles.extend(agents.keys().cloned());
            }
        }
        if let Some(default_agent) = value.get("default_agent").and_then(Value::as_str) {
            profiles.push(default_agent.to_string());
        }
    }
    Ok(clean_unique_options(profiles))
}

fn agent_model_options(current: &str) -> Result<Vec<String>> {
    let mut models = vec![current.trim().to_string(), "gpt-4o-mini".to_string()];
    if let Ok(model) = env::var("DJINN_OPENAI_MODEL") {
        models.push(model);
    }
    for value in opencode_config_values()? {
        for pointer in ["/model", "/small_model", "/agent/model"] {
            if let Some(model) = json_string_pointer(&value, pointer) {
                models.push(model);
            }
        }
        for container in ["agent", "agents"] {
            if let Some(agents) = value.get(container).and_then(Value::as_object) {
                for agent in agents.values() {
                    if let Some(model) = agent.get("model").and_then(Value::as_str) {
                        models.push(model.to_string());
                    }
                }
            }
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
        format!("Profile: {}", config.profile),
        format!("Model: {}", config.model),
        String::new(),
        "Read access:".to_string(),
    ];
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
    lines.push(String::new());
    Ok(lines.join("\n"))
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

fn opencode_config_values() -> Result<Vec<Value>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut values = Vec::new();
    for path in opencode_model_config_paths(&cwd) {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        values.push(
            serde_json::from_str(&content)
                .with_context(|| format!("parsing OpenCode config {}", path.display()))?,
        );
    }
    Ok(values)
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

fn complete_openai_prompt(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    prompt: String,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    max_tool_rounds: usize,
    profile: &str,
    interactive_permissions: bool,
) -> Result<djinn_agent::ModelResponse> {
    let workspace = store.load_session(id)?.meta.workspace;
    complete_openai_messages(
        store,
        id,
        vec![
            agent_system_message(&workspace),
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
    interactive_permissions: bool,
    mut on_progress: F,
) -> Result<djinn_agent::ModelResponse>
where
    F: FnMut(AgentProgressEvent) -> Result<()>,
{
    let auth = resolve_openai_auth(api_key)?;
    let client = match auth {
        OpenAiAuth::ApiKey(api_key) => {
            let base_url = base_url
                .or_else(|| env::var("OPENAI_BASE_URL").ok())
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            OpenAiClient::with_base_url(api_key, base_url)
        }
        OpenAiAuth::OAuth(oauth) => OpenAiClient::with_oauth(oauth),
    };
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
        Some(Arc::new(TerminalPermissionGate))
    } else {
        None
    };
    let runtime = AgentRuntime::new(
        client,
        store.clone(),
        tools_with_policies_file_history_and_gate(
            workspace.clone(),
            read_access,
            permissions,
            Some(file_history),
            permission_gate,
        )?,
    );
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .with_context(|| "creating Tokio runtime for OpenAI request")?;
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

const OPENCODE_OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENCODE_OPENAI_OAUTH_ISSUER: &str = "https://auth.openai.com";
const OPENCODE_OPENAI_CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";

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
    if let Some(api_key) = opencode_openai_api_key()? {
        return Ok(OpenAiAuth::ApiKey(api_key));
    }
    if let Some(auth) = opencode_auth_openai_auth()? {
        return Ok(auth);
    }
    Err(anyhow::anyhow!(
        "OpenAI auth is required; pass --api-key, set OPENAI_API_KEY, configure providers.openai.apiKey in OpenCode config, or connect an OpenCode OpenAI API/OAuth credential"
    ))
}

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

fn oauth_access_token_is_current(oauth: &OpenCodeOpenAiOAuthCredential) -> bool {
    !oauth.access.is_empty() && oauth.expires > current_time_millis()
}

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
    if let Some(model) = explicit
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
    {
        return Ok(model);
    }
    if let Ok(model) = env::var("DJINN_OPENAI_MODEL") {
        let model = model.trim().to_string();
        if !model.is_empty() {
            return Ok(model);
        }
    }
    if let Some(model) = opencode_default_model(profile)? {
        return Ok(model);
    }
    Ok("gpt-4o-mini".to_string())
}

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
    if let Some(rules) = opencode_read_access_rules(profile, workspace)? {
        policy.rules.extend(rules);
    }
    Ok(policy)
}

fn resolve_agent_permission_policy(profile: &str, workspace: &Path) -> Result<PermissionPolicy> {
    let mut policy = PermissionPolicy::allow_by_default();
    if let Some(rules) = opencode_permission_policy_rules(profile, workspace)? {
        policy.rules.extend(rules);
    }
    Ok(policy)
}

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

fn format_agent_event(event: &AgentSessionEvent) -> String {
    match &event.kind {
        AgentSessionEventKind::SessionCreated { .. } => "session created".to_string(),
        AgentSessionEventKind::SessionTitleUpdated { title } => format!("title: {title}"),
        AgentSessionEventKind::SessionProfileUpdated { profile } => format!("profile: {profile}"),
        AgentSessionEventKind::SessionModelUpdated { model } => format!("model: {model}"),
        AgentSessionEventKind::UserMessage { content } => {
            format!("user: {}", prompt_title(content, "(empty)"))
        }
        AgentSessionEventKind::AssistantMessage { content } => {
            format!("assistant: {}", prompt_title(content, "(empty)"))
        }
        AgentSessionEventKind::ToolCall { id, name, .. } => format!("tool call {id}: {name}"),
        AgentSessionEventKind::ToolResult { id, success, .. } => {
            format!(
                "tool result {id}: {}",
                if *success { "ok" } else { "failed" }
            )
        }
        AgentSessionEventKind::Summary { content } => {
            format!("summary: {}", prompt_title(content, "(empty)"))
        }
        AgentSessionEventKind::Checkpoint { label } => format!("checkpoint: {label}"),
    }
}

fn agent_system_message(workspace: &str) -> ModelMessage {
    ModelMessage {
        role: ModelRole::System,
        content: format!(
            "You are running in workspace `{workspace}`. Read-only filesystem tools may also access other paths such as the user's home directory when the configured access policy allows it. Use absolute paths, `~`, or `$HOME` for non-workspace locations."
        ),
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

fn agent_model_messages(session: &AgentSession, workspace: &str) -> Vec<ModelMessage> {
    let mut messages = vec![agent_system_message(workspace)];
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
            | AgentSessionEventKind::AssistantMessage { .. } => {}
        }
    }
    messages
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

fn agent_progress_message(event: &AgentProgressEvent) -> Option<djinn_tui::AgentChatMessage> {
    match event {
        AgentProgressEvent::ModelRequestStarted { round } => Some(agent_thought_message(format!(
            "Planning next step{}…",
            progress_round_suffix(*round)
        ))),
        AgentProgressEvent::ModelResponseCompleted {
            elapsed_ms,
            tool_calls,
            has_message,
            ..
        } => {
            let label = if *tool_calls > 0 {
                format!(
                    "Planned {tool_calls} tool call{}",
                    plural_suffix(*tool_calls)
                )
            } else if *has_message {
                "Drafted response".to_string()
            } else {
                "Completed model turn".to_string()
            };
            Some(agent_thought_message(format!(
                "{label} · {}",
                format_elapsed_ms(*elapsed_ms)
            )))
        }
        AgentProgressEvent::ToolCallStarted { call, .. } => Some(agent_thought_message(format!(
            "Running {}",
            summarize_agent_tool_input(&call.name, &call.input)
        ))),
        AgentProgressEvent::ToolCallCompleted {
            call,
            result,
            elapsed_ms,
            ..
        } => Some(agent_thought_message(format!(
            "{} {} · {}",
            if result.success { "Finished" } else { "Failed" },
            call.name,
            format_elapsed_ms(*elapsed_ms)
        ))),
    }
}

fn agent_progress_notice(event: &AgentProgressEvent) -> String {
    match event {
        AgentProgressEvent::ModelRequestStarted { .. } => "Planning next step…".to_string(),
        AgentProgressEvent::ModelResponseCompleted { tool_calls, .. } if *tool_calls > 0 => {
            format!(
                "Planned {tool_calls} tool call{}.",
                plural_suffix(*tool_calls)
            )
        }
        AgentProgressEvent::ModelResponseCompleted { .. } => "Model response received.".to_string(),
        AgentProgressEvent::ToolCallStarted { call, .. } => format!("Running {}…", call.name),
        AgentProgressEvent::ToolCallCompleted { call, result, .. } => format!(
            "{} {}.",
            if result.success { "Finished" } else { "Failed" },
            call.name
        ),
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
            | djinn_tui::TuiAction::ShareChats(_)
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
    if let djinn_tui::TuiAction::ShareChats(request) = &action {
        if request.mode == djinn_tui::ChatShareMode::Summary && !request.context_only {
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
        djinn_tui::TuiAction::ShareChats(request) => share_chats(ShareChatsArgs {
            ids: request.chat_ids,
            source: None,
            query: None,
            limit: 10,
            all: false,
            mode: share_chats_mode_from_tui(request.mode),
            context_only: request.context_only,
            max_chars_per_chat: 4000,
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
    request: &djinn_tui::ChatShareRequest,
) -> Result<AgentSessionId> {
    let args = ShareChatsArgs {
        ids: request.chat_ids.clone(),
        source: None,
        query: None,
        limit: 10,
        all: false,
        mode: ShareChatsMode::Summary,
        context_only: false,
        max_chars_per_chat: 4000,
    };
    let records = chat_store().list()?;
    let selected = select_chats_for_share(&records, &args)?;
    let prompt = format_chat_summary_agent_prompt(&selected, &args);
    let title = if selected.len() == 1 {
        format!("Summarize {}", selected[0].title)
    } else {
        format!("Summarize {} selected chats", selected.len())
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
    ChatRecord {
        id: format!("agent:{id}"),
        title,
        content: format!(
            "Djinn agent session\n\nID: {id}\nWorkspace: {}\nProfile: {}\nSource: {}\nEvents: {}\nCreated: {}\nUpdated: {}",
            summary.workspace,
            summary.profile,
            summary.source,
            summary.event_count,
            summary.created_at,
            summary.updated_at
        ),
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
        TuiView::Chats => djinn_tui::DashboardTab::Chats,
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

fn default_agent_chat_args() -> AgentChatArgs {
    AgentChatArgs {
        resume: None,
        title: None,
        workspace: None,
        profile: "default".to_string(),
        model: None,
        api_key: None,
        base_url: None,
        max_tool_rounds: 5,
    }
}

fn share_chats_mode_from_tui(mode: djinn_tui::ChatShareMode) -> ShareChatsMode {
    match mode {
        djinn_tui::ChatShareMode::Summary => ShareChatsMode::Summary,
        djinn_tui::ChatShareMode::Patterns => ShareChatsMode::Patterns,
        djinn_tui::ChatShareMode::Memories => ShareChatsMode::Memories,
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
        println!("Chats are empty.");
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
        println!("\nTotal: {} chats", records.len());
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

fn format_opencode_watcher_state_for_ideas() -> String {
    match load_opencode_watch_state() {
        Ok(state) if state.sessions.is_empty() => "No OpenCode sessions tracked yet.".to_string(),
        Ok(state) => {
            let mut out = format!("Tracked sessions: {}\n", state.sessions.len());
            for (idx, (session_id, session)) in state.sessions.iter().take(20).enumerate() {
                let bridge = if session.djinn_session_id.is_empty() {
                    String::new()
                } else {
                    format!(", djinn {}", session.djinn_session_id)
                };
                out.push_str(&format!(
                    "  {}. {} -> chat {} ({}, imported {}{})\n",
                    idx + 1,
                    session_id,
                    if session.chat_id.is_empty() {
                        "unknown"
                    } else {
                        &session.chat_id
                    },
                    if session.title.is_empty() {
                        "untitled"
                    } else {
                        &session.title
                    },
                    if session.imported_at.is_empty() {
                        "unknown"
                    } else {
                        &session.imported_at
                    },
                    bridge
                ));
            }
            if state.sessions.len() > 20 {
                out.push_str(&format!(
                    "... {} more tracked sessions omitted ...\n",
                    state.sessions.len() - 20
                ));
            }
            out
        }
        Err(err) => format!("Watcher state unavailable: {err}"),
    }
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
        bail!("refusing to clear chats from a non-interactive shell");
    }
    print!("Clear Djinn chats? Type 'clear' to confirm: ");
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
            "Chats cleared ({} records). Backup written to {} and metadata to {}{}",
            info.record_count,
            info.path.display(),
            info.metadata_path.display(),
            info.bodies_path
                .as_ref()
                .map(|path| format!("; bodies copied to {}", path.display()))
                .unwrap_or_default()
        );
    } else {
        println!("Chats cleared.");
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
        println!("No chats matched {id:?}.");
    } else {
        println!("Removed {} chats:", removed.len());
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
        context_only: true,
        max_chars_per_chat: 0,
    };
    let selected = select_chats_for_share(&records, &selection_args)?;
    if args.dry_run {
        print_archive_chat_selection(&selected, true);
        return Ok(());
    }
    if !args.force {
        print_archive_chat_selection(&selected, false);
        bail!("refusing to archive chats without --force; rerun with --dry-run to inspect only");
    }

    let Some(path) = archive_chat_records_with_label(&selected, "manual")? else {
        println!("No chats archived.");
        return Ok(());
    };
    println!(
        "Archived {} chats to {} and removed them from the active chat index.",
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
            "{}\t{} chats\t{} bytes\t{}",
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

    println!("\n## Chats\n");
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
        "{verb} {} chats from {}{}.",
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
            "Skipped {} chats with existing id/source matches{}:",
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
            "Removed archive {} ({} chats, {} bytes).",
            summary.path, record_count, summary.byte_size
        );
    } else if summary.dry_run {
        println!(
            "Would remove archive {} ({} chats, {} bytes).",
            summary.path, record_count, summary.byte_size
        );
    } else {
        println!(
            "Selected archive {} ({} chats, {} bytes).",
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
    println!("{label} {} chats:", records.len());
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
        println!("No chats older than {} were pruned.", args.older_than);
    } else {
        println!(
            "Pruned {} chats older than {}:",
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
    println!("\nTotal: {} matching chats", matches.len());
    Ok(())
}

fn share_ideas() -> Result<()> {
    let memories = memory_store().list()?;
    let chats = chat_store().list()?;
    let tools = scan_tools(&tool_roots(Vec::new()))?;
    let watcher_state = format_ideas_pipeline_context(
        &idea_store().list()?,
        &action_store().list()?,
        &format_opencode_watcher_state_for_ideas(),
    );
    println!(
        "{}",
        djinn_suggest::build_prompt_with_pipeline(&memories, &chats, &tools, &watcher_state)
    );
    Ok(())
}

fn share_suggestions() -> Result<()> {
    let records = suggestion_store().list()?;
    println!("{}", format_suggestions_context(&records));
    Ok(())
}

fn format_ideas_pipeline_context(
    ideas: &[IdeaRecord],
    actions: &[ActionRecord],
    watcher_state: &str,
) -> String {
    let mut out = String::new();
    out.push_str("## Saved ideas\n");
    if ideas.is_empty() {
        out.push_str("No saved ideas.\n");
    } else {
        for idea in ideas.iter().take(50) {
            out.push_str(&format!(
                "- [{}] {}{}\n",
                idea.id,
                idea.text,
                format_idea_suffix(idea)
            ));
        }
    }
    out.push_str("\n## Open actions\n");
    if actions.is_empty() {
        out.push_str("No open actions.\n");
    } else {
        for action in actions
            .iter()
            .filter(|action| !action.status.eq_ignore_ascii_case("done"))
            .take(50)
        {
            out.push_str(&format!(
                "- [{}] {}{}\n",
                action.id,
                action.text,
                format_action_suffix(action)
            ));
        }
    }
    out.push_str("\n## Watcher state\n");
    out.push_str(watcher_state);
    out
}

fn share_skills(args: ShareSkillsArgs) -> Result<()> {
    let records = skill_records()?;
    println!("{}", format_skills_context(&records, &args));
    Ok(())
}

fn share_chat(args: ShareChatArgs) -> Result<()> {
    let records = chat_store().list()?;
    let record = resolve_chat(&records, &args.id)?;
    if args.context_only {
        println!("{}", format_chat_context(record));
    } else {
        let memories = memory_store().list()?;
        println!(
            "{}",
            format_chat_memory_extraction_prompt(record, &memories)
        );
    }
    Ok(())
}

fn share_chats(args: ShareChatsArgs) -> Result<()> {
    let records = chat_store().list()?;
    let selected = select_chats_for_share(&records, &args)?;
    if args.context_only {
        println!("{}", format_chats_context(&selected, &args));
    } else {
        match args.mode {
            ShareChatsMode::Summary => println!("{}", format_chats_summary(&selected, &args)),
            ShareChatsMode::Patterns | ShareChatsMode::Memories => {
                let memories = memory_store().list()?;
                println!(
                    "{}",
                    format_chats_review_prompt(&selected, &args, &memories)
                );
            }
        }
    }
    Ok(())
}

fn share_merge(args: ShareMergeArgs) -> Result<()> {
    let records = chat_store().list()?;
    let selected = select_chats_for_merge(&records, &args)?;
    let prompt = format_chats_merge_prompt(&selected, &args);
    if args.dry_run {
        println!("{prompt}");
        return Ok(());
    }
    if selected.is_empty() {
        println!("No chats matched merge selection.");
        return Ok(());
    }

    let profile = args.profile.trim().to_string();
    let model = resolve_agent_model(args.model.clone(), &profile)?;
    let store = agent_session_store();
    let id = store.create_session(AgentSessionMeta {
        title: format!("Merge {} chats into memories", selected.len()),
        workspace: resolve_agent_workspace(None)?,
        profile: profile.clone(),
        source: "djinn-share-merge".to_string(),
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
        "Merged {} chats into {} memories.",
        selected.len(),
        written.len()
    );
    if let Some(path) = archived {
        println!("Archived source chats: {}", path.display());
    } else if args.archive {
        println!("No source chats archived because no memories were written.");
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

fn promote_chat(args: ShareChatArgs) -> Result<()> {
    let records = chat_store().list()?;
    let record = resolve_chat(&records, &args.id)?;
    let memories = memory_store().list()?;
    println!("{}", format_chat_promotion_prompt(record, &memories));
    Ok(())
}

fn promote_chats(args: ShareChatsArgs) -> Result<()> {
    println!("{}", build_promote_chats_prompt(args)?);
    Ok(())
}

fn build_promote_chats_prompt(mut args: ShareChatsArgs) -> Result<String> {
    args.mode = ShareChatsMode::Memories;
    args.context_only = false;
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
        context_only: false,
        max_chars_per_chat: 4000,
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
    let editor = editor.unwrap_or_else(default_editor);
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("editor command is empty");
    };
    let mut cmd = ProcessCommand::new(program);
    cmd.args(parts);
    cmd.arg(format!("+{}", line));
    cmd.arg(path);
    let status = cmd.status()?;
    if !status.success() {
        bail!("editor exited with status {status}");
    }
    Ok(())
}

fn format_tools_context(entries: &[ToolEntry]) -> String {
    let mut out = String::from("# Local Tools\n\nThese local tools are available to the user:\n\n");
    if entries.is_empty() {
        out.push_str("No local tools discovered.\n");
        return out;
    }
    for entry in entries {
        out.push_str(&format!(
            "- `{}`: {}\n  Source: {}:{}\n",
            entry.name,
            entry.description,
            entry.path.display(),
            entry.line
        ));
    }
    out.push_str("\nPrefer these existing local tools before inventing new scripts.\n");
    out
}

fn format_skills_context(records: &[SkillRecord], args: &ShareSkillsArgs) -> String {
    let mut out = String::from("# Local Agent Skills\n\nThese reusable local workflows are available to the user/agent environment. Prefer an existing skill when it matches the task instead of inventing a new procedure.\n\n");
    if records.is_empty() {
        out.push_str("No skills discovered.\n");
        return out;
    }
    for record in records {
        out.push_str(&format!(
            "- `{}`: {}\n  Source: {}\n  Path: {}\n  Managed by Djinn: {}\n",
            record.name,
            if record.description.is_empty() {
                "No description"
            } else {
                record.description.as_str()
            },
            record.source,
            record.path.display(),
            if record.managed { "yes" } else { "no" }
        ));
        if args.include_content {
            match read_skill_content(record) {
                Ok(content) => {
                    out.push_str("  Instructions preview:\n\n```markdown\n");
                    out.push_str(&truncate(&content, args.max_chars_per_skill));
                    if content.chars().count() > args.max_chars_per_skill {
                        out.push_str(&format!(
                            "\n... skill content truncated to {} chars ...\n",
                            args.max_chars_per_skill
                        ));
                    }
                    out.push_str("```\n");
                }
                Err(error) => {
                    out.push_str(&format!("  Instructions preview unavailable: {error}\n"));
                }
            }
        }
    }
    out.push_str("\nUse `djinn show skill <name>` to inspect a skill before relying on it.\n");
    out
}

fn format_memories_context(records: &[MemoryRecord]) -> String {
    let mut out = String::from("# Djinn Memories\n\n");
    if records.is_empty() {
        out.push_str("No memories recorded.\n");
        return out;
    }
    for record in records {
        out.push_str(&format!("- `[{}]` {}\n", record.id, record.text));
        let mut details = Vec::new();
        if !record.scope.trim().is_empty() {
            details.push(format!("scope: {}", record.scope));
        }
        if !record.kind.trim().is_empty() {
            details.push(format!("kind: {}", record.kind));
        }
        if !record.confidence.trim().is_empty() {
            details.push(format!("confidence: {}", record.confidence));
        }
        if !record.not_before.trim().is_empty() {
            details.push(format!("not-before: {}", record.not_before));
        }
        if !record.sources.is_empty() {
            details.push(format!("sources: {}", record.sources.len()));
        }
        if !details.is_empty() {
            out.push_str(&format!("  Metadata: {}\n", details.join(", ")));
        }
        if !record.evidence.is_empty() {
            out.push_str("  Evidence:\n");
            for evidence in record.evidence.iter().take(3) {
                out.push_str(&format!("  - {}\n", evidence));
            }
            if record.evidence.len() > 3 {
                out.push_str(&format!(
                    "  - ... {} more evidence items omitted ...\n",
                    record.evidence.len() - 3
                ));
            }
        }
    }
    out
}

fn format_suggestions_context(records: &[SuggestionRecord]) -> String {
    let mut out = String::from("# Djinn Suggestions\n\n");
    out.push_str("Suggestions are review outcomes and todo-like next steps. They are removed when accepted/done or rejected.\n\n");
    if records.is_empty() {
        out.push_str("No open suggestions recorded.\n");
        return out;
    }
    for record in records {
        out.push_str(&format!("- `[{}]` {}\n", record.id, record.text));
        let mut details = Vec::new();
        if !record.target.trim().is_empty() {
            details.push(format!("target: {}", record.target));
        }
        if !record.status.trim().is_empty() {
            details.push(format!("status: {}", record.status));
        }
        if !record.sources.is_empty() {
            details.push(format!("sources: {}", record.sources.len()));
        }
        if !details.is_empty() {
            out.push_str(&format!("  Metadata: {}\n", details.join(", ")));
        }
        if !record.rationale.trim().is_empty() {
            out.push_str(&format!("  Rationale: {}\n", record.rationale));
        }
    }
    out
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

fn format_chat_context(record: &ChatRecord) -> String {
    let mut out = format!(
        "# Djinn Chat\n\n- ID: `{}`\n- Title: {}\n- Created: {}\n",
        record.id, record.title, record.created_at
    );
    if !record.source_path.trim().is_empty() {
        out.push_str(&format!("- Source path: {}\n", record.source_path));
    }
    if !record.source.trim().is_empty() {
        out.push_str(&format!("- Source type: {}\n", record.source));
    }
    if !record.source_id.trim().is_empty() {
        out.push_str(&format!("- Source ID: {}\n", record.source_id));
    }
    out.push_str("\nUse this chat as source context for the next agent action.\n\n");
    out.push_str("## Chat Content\n\n```text\n");
    out.push_str(&record.content);
    if !record.content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n");
    out
}

fn format_chat_memory_extraction_prompt(record: &ChatRecord, memories: &[MemoryRecord]) -> String {
    let mut out = format!(
        "# Djinn Chat Memory Extraction\n\nYou are reviewing a saved Djinn chat. Extract durable memories only when they are reusable in future work.\n\n## Chat Metadata\n\n- ID: `{}`\n- Title: {}\n- Created: {}\n",
        record.id, record.title, record.created_at
    );
    if !record.source.trim().is_empty() {
        out.push_str(&format!("- Source type: {}\n", record.source));
    }
    if !record.source_id.trim().is_empty() {
        out.push_str(&format!("- Source ID: {}\n", record.source_id));
    }
    if !record.source_path.trim().is_empty() {
        out.push_str(&format!("- Source path: {}\n", record.source_path));
    }

    out.push_str(
        "\n## Extraction Guidelines\n\nExtract active memories for:\n\n- user preferences and corrections\n- repeated workflows or tool choices\n- project-specific conventions\n- safety constraints or gotchas\n- reusable debugging/implementation patterns\n\nDo not extract:\n\n- one-off task status\n- secrets, credentials, tokens, private URLs, or sensitive raw data\n- facts that are already captured in existing memories\n- noisy transcript details that will not help future agents\n\nReturn only a short reviewed list of shell commands the user can run manually. Include enough metadata and copied evidence that the memory remains understandable even if the source chat is deleted later. Use `--not-before YYYY-MM-DD` when a true memory should not drive actions until a future date. Prefer this form:\n\n```bash\ndjinn add memory \"...\" --scope project --kind preference --confidence high --not-before 2026-10-01 --evidence \"User explicitly corrected the agent to ...\" --source-chat CHAT_ID\n```\n\nIf there are no durable lessons, say: `No durable memories recommended.`\n",
    );

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
    out.push_str("```\n\n## Chat Content\n\n```text\n");
    out.push_str(&record.content);
    if !record.content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n");
    out
}

fn format_chat_promotion_prompt(record: &ChatRecord, memories: &[MemoryRecord]) -> String {
    let mut out = format_chat_memory_extraction_prompt(record, memories);
    out = out.replace(
        "# Djinn Chat Memory Extraction",
        "# Djinn Chat Promotion Memory Extraction",
    );
    out.push_str(
        "\n\n## Promotion Output\n\nReturn reviewed `djinn add memory` commands. Use this exact command shape so Djinn can track review lifecycle. Use `--not-before YYYY-MM-DD` for memories that should be remembered now but not acted on until later:\n\n```bash\ndjinn add memory \"...\" --scope project --kind preference --confidence high --not-before 2026-10-01 --evidence \"Copied durable evidence ...\" --source-chat ",
    );
    out.push_str(&record.id);
    out.push_str(
        "\n```\n\nAfter review, the user can run `djinn list memories`, `djinn show memory <id>`, `djinn accept memory <id>` to review for suggestions, or `djinn reject memory <id>` to remove it.\n",
    );
    out
}

fn format_chats_context(records: &[ChatRecord], args: &ShareChatsArgs) -> String {
    let mut out = String::from("# Djinn Chats Bundle\n\n");
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
        out.push_str(&format!("- Limit: latest {} matching chats\n", args.limit));
    }
    out.push_str("\nUse these chats together as source context for the next agent action.\n");
    append_chats_bundle(&mut out, records, args.max_chars_per_chat);
    out
}

fn format_chats_summary(records: &[ChatRecord], args: &ShareChatsArgs) -> String {
    let mut out = String::from("# Djinn Chat Summary\n\n");
    out.push_str("This is a local digest of the selected saved chats. No model was run.\n\n");
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
        out.push_str(&format!("- Limit: latest {} matching chats\n", args.limit));
    }

    let redacted_count = records
        .iter()
        .filter(|record| chat_content_appears_redacted(&record.content))
        .count();
    if redacted_count > 0 {
        out.push_str(&format!(
            "- Redacted/sanitized chats: {redacted_count} (summary detail may be limited)\n"
        ));
    }

    out.push_str("\n## Chats\n");
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

fn format_chats_merge_prompt(records: &[ChatRecord], args: &ShareMergeArgs) -> String {
    let mut out = String::from("You are merging saved Djinn chats into durable memories.\n\n");
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

    let share_args = ShareChatsArgs {
        ids: args.ids.clone(),
        source: args.source.clone(),
        query: args.query.clone(),
        limit: args.limit,
        all: args.all,
        mode: ShareChatsMode::Memories,
        context_only: true,
        max_chars_per_chat: args.max_chars_per_chat,
    };
    append_chats_bundle(&mut out, records, share_args.max_chars_per_chat);
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
        "Please summarize the selected saved chats below so we can continue discussing them.\n\n",
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
    let mut out = String::from("# Djinn Multi-Chat Review\n\n");
    out.push_str("You are reviewing a bundle of saved Djinn chats. Treat them as a corpus, not as isolated transcripts.\n\n");
    out.push_str("## Review Goal\n\n");
    match args.mode {
        ShareChatsMode::Summary => out.push_str(
            "Summarize the selected chats. Identify the main themes, decisions, outcomes, unresolved follow-ups, and any stale assumptions. Keep the summary useful for resuming work.\n",
        ),
        ShareChatsMode::Patterns => out.push_str(
            "Identify recurring patterns across the selected chats: user preferences, repeated corrections, tool/workflow choices, project conventions, safety gotchas, friction points, and implementation habits. Separate high-confidence repeated patterns from one-off observations.\n",
        ),
        ShareChatsMode::Memories => out.push_str(
            "Propose durable memories only when they are reusable in future work and supported by repeated patterns or explicit user instructions. Return reviewed shell commands the user can run manually; do not invent memories from weak one-off evidence.\n",
        ),
    }
    out.push_str("\n## Output Guidelines\n\n");
    match args.mode {
        ShareChatsMode::Summary => out.push_str(
            "Return Markdown with sections: `Summary`, `Decisions`, `Open Follow-ups`, and `Potential Memories`. Do not write memories automatically.\n",
        ),
        ShareChatsMode::Patterns => out.push_str(
            "Return Markdown with sections: `High-confidence Patterns`, `Possible One-offs`, `Workflow Opportunities`, and `Reviewable Memories`. Do not write memories automatically.\n",
        ),
        ShareChatsMode::Memories => out.push_str(
            "Return only a short reviewed list of commands. Include scope, kind, confidence, copied evidence, and source chat pointers when available. Use `--not-before YYYY-MM-DD` when a memory should not drive suggestions/actions until later. Use this form:\n\n```bash\ndjinn add memory \"...\" --scope project --kind preference --confidence high --not-before 2026-10-01 --evidence \"Repeated evidence from the reviewed chats ...\" --source-chat CHAT_ID\n```\n\nIf there are no durable lessons, say: `No durable memories recommended.`\n",
        ),
    }
    out.push_str("\nDo not include secrets, credentials, tokens, private URLs, or sensitive raw data. Avoid duplicating existing memories.\n");

    out.push_str("\n## Selection Metadata\n\n");
    out.push_str(&format!("- Chat count: {}\n", records.len()));
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
        out.push_str(&format!("- Limit: latest {} matching chats\n", args.limit));
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
        "\n\n## Promotion Output\n\nReturn reviewed `djinn add memory` commands. Include scope, kind, confidence, copied evidence, and one or more `--source-chat` pointers when available. Use `--not-before YYYY-MM-DD` when a future activation date is appropriate. Example:\n\n```bash\ndjinn add memory \"...\" --scope project --kind convention --confidence high --not-before 2026-10-01 --evidence \"Repeated across reviewed chats ...\" --source-chat CHAT_ID\n```\n\nThe user can review memories for downstream suggestions with `djinn accept memory <id>` or remove them with `djinn reject memory <id>`.\n",
    );
    out
}

fn append_chats_bundle(out: &mut String, records: &[ChatRecord], max_chars_per_chat: usize) {
    out.push_str("\n## Chats\n");
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
        bail!("no chats matched the share selection");
    }
    Ok(selected)
}

fn select_chats_for_merge(
    records: &[ChatRecord],
    args: &ShareMergeArgs,
) -> Result<Vec<ChatRecord>> {
    select_chats_for_share(
        records,
        &ShareChatsArgs {
            ids: args.ids.clone(),
            source: args.source.clone(),
            query: args.query.clone(),
            limit: args.limit,
            all: args.all,
            mode: ShareChatsMode::Memories,
            context_only: false,
            max_chars_per_chat: args.max_chars_per_chat,
        },
    )
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
            eprintln!("multiple chats match {id:?}:");
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
            mode: ShareChatsMode::Patterns,
            context_only: false,
            max_chars_per_chat: 4000,
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
    fn parses_agent_session_delete_command() {
        let cli = Cli::try_parse_from([
            "djinn", "agent", "session", "delete", "agt_test", "--force", "--json",
        ])
        .unwrap();

        let Some(Command::Agent(agent_args)) = cli.command else {
            panic!("expected agent command");
        };
        let AgentCommand::Session(session_args) = agent_args.command else {
            panic!("expected agent session command");
        };
        let AgentSessionCommand::Delete(delete_args) = session_args.command else {
            panic!("expected agent session delete command");
        };

        assert_eq!(delete_args.id, "agt_test");
        assert!(delete_args.force);
        assert!(delete_args.json);
    }

    #[test]
    fn parses_archive_chats_selection_command() {
        let cli = Cli::try_parse_from([
            "djinn", "archive", "chats", "--source", "opencode", "--limit", "25", "--force",
        ])
        .unwrap();

        let Some(Command::Archive(args)) = cli.command else {
            panic!("expected archive command");
        };
        let ArchiveNoun::Chats(args) = args.noun else {
            panic!("expected archive chats command");
        };

        assert_eq!(args.source.as_deref(), Some("opencode"));
        assert_eq!(args.limit, 25);
        assert!(args.force);
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
            profile: "architect".to_string(),
            model: "openai/gpt-5.5".to_string(),
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
        };

        let rendered = format_agent_effective_config(&config, OutputFormat::Text).unwrap();

        assert!(rendered.contains("Agent effective config"));
        assert!(rendered.contains("Workspace: /tmp/project"));
        assert!(rendered.contains("Profile: architect"));
        assert!(rendered.contains("Model: openai/gpt-5.5"));
        assert!(rendered.contains("allow root: /tmp/project"));
        assert!(rendered.contains("deny root: /tmp/project/secrets"));
        assert!(rendered.contains("Allow: */docs/*"));
        assert!(rendered.contains("Ask: write *.rs"));
        assert!(rendered.contains("destructive-action guardrails always apply"));
    }

    #[test]
    fn format_agent_effective_config_outputs_json() {
        let config = AgentEffectiveConfig {
            workspace: "/tmp/project".to_string(),
            profile: "default".to_string(),
            model: "gpt-4o-mini".to_string(),
            read_access: ReadAccessPolicy::allow_by_default(),
            permissions: PermissionPolicy::allow_by_default(),
        };

        let rendered = format_agent_effective_config(&config, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["workspace"], "/tmp/project");
        assert_eq!(value["profile"], "default");
        assert_eq!(value["model"], "gpt-4o-mini");
        assert_eq!(value["permissions"]["rules"].as_array().unwrap().len(), 0);
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
                AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                    id: "call-1".to_string(),
                    output: serde_json::json!({"stdout": "ignored"}),
                    success: true,
                }),
            ],
        };

        let messages = agent_model_messages(&session, "/tmp/project");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ModelRole::System);
        assert_eq!(messages[1].role, ModelRole::User);
        assert_eq!(messages[1].content, "hello");
        assert_eq!(messages[2].role, ModelRole::Assistant);
        assert_eq!(messages[2].content, "hi");
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

        let prepared = prepare_agent_chat_session(
            &store,
            None,
            Some("Pairing session".to_string()),
            Some(workspace.clone()),
            "review",
        )
        .unwrap();
        let loaded = store.load_session(&prepared.id).unwrap();
        let canonical_workspace = workspace.canonicalize().unwrap();

        assert_eq!(prepared.profile, "review");
        assert_eq!(loaded.meta.title, "Pairing session");
        assert_eq!(loaded.meta.profile, "review");
        assert_eq!(
            loaded.meta.workspace,
            canonical_workspace.display().to_string()
        );
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
        assert!(entries.iter().any(|entry| {
            entry.section == "Session"
                && entry.label == "New session"
                && entry.command == djinn_tui::AgentChatCommand::NewSession
        }));
        assert!(entries.iter().any(|entry| {
            entry.section == "Navigation"
                && entry.label == "Open Tools"
                && entry.command
                    == djinn_tui::AgentChatCommand::OpenDashboardTab(djinn_tui::DashboardTab::Tools)
        }));
        assert!(entries.iter().any(|entry| {
            entry.section == "Navigation"
                && entry.label == "Open Suggestions"
                && entry.command
                    == djinn_tui::AgentChatCommand::OpenDashboardTab(
                        djinn_tui::DashboardTab::Suggestions,
                    )
        }));
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
            source: "opencode".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            updated_at: "2026-07-24T01:00:00Z".to_string(),
            event_count: 3,
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
    fn share_chats_renders_opencode_exports_as_digest_not_raw_json() {
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
                  {"type": "tool", "tool": "read", "state": {"title": "Read main.rs", "output": "found share code"}},
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
    fn share_chats_warns_when_opencode_export_is_sanitized() {
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
    fn share_chats_summary_is_human_facing_not_agent_prompt() {
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

        assert!(summary.starts_with("# Djinn Chat Summary"));
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

        assert!(prompt.starts_with("Please summarize the selected saved chats"));
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
        let args = ShareMergeArgs {
            ids: Vec::new(),
            source: None,
            query: None,
            limit: 50,
            all: false,
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
        assert!(prompt.contains("# Djinn Multi-Chat Review"));
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
