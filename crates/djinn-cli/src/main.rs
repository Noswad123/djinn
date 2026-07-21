use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use djinn_agent::{
    AgentRuntime, ModelMessage, ModelRequest, ModelRole, OpenAiClient, ToolRegistry,
};
use djinn_chats::ChatRecord;
use djinn_contexts::{resolve_context, ContextInput, ContextRecord, ContextStore};
use djinn_memory::{
    ActionRecord, ActionStore, AgentSessionEvent, AgentSessionEventKind, AgentSessionFilter,
    AgentSessionId, AgentSessionMeta, AgentSessionStore, CandidateStore, IdeaRecord, IdeaStore,
    JsonlAgentSessionStore, MemoryCandidate, MemoryInput, MemoryRecord, MemorySource,
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
    /// Accept a pending item.
    Accept(AcceptArgs),
    /// Reject a pending item.
    Reject(RejectArgs),
    /// Route reviewable memories into suggestions, skills, ideas, actions, or durable memories.
    Ingest(IngestArgs),
    /// Promote raw context into reviewable memories.
    Promote(PromoteArgs),
    /// Run an external review to organically create reviewable memories.
    Review(ReviewArgs),
    /// Remove one item.
    Rm(RmArgs),
    /// Clear a collection after confirmation.
    Clear(ClearArgs),
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
    /// List reviewable memories.
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
    /// Show a reviewable memory by id or text fragment.
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
    /// Add a reviewable memory.
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
    /// Reject reviewable memories and remove them permanently.
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
    /// Route pending reviewable memories into the right durable collection.
    Memories(IngestMemoriesArgs),
    /// Route one pending reviewable memory into the right durable collection.
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
    /// Ask OpenCode to review recent Djinn chats and add reviewable memories.
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
    /// Manage Djinn-native agent sessions.
    Session(AgentSessionArgs),
    /// Record a non-interactive prompt in an agent session.
    Ask(AgentAskArgs),
}

#[derive(Debug, Args)]
struct AgentSessionArgs {
    #[command(subcommand)]
    command: AgentSessionCommand,
}

#[derive(Debug, Subcommand)]
enum AgentSessionCommand {
    /// Create an empty agent session.
    New(AgentSessionNewArgs),
    /// List agent sessions.
    List(AgentSessionListArgs),
    /// Show one agent session.
    Show(AgentSessionShowArgs),
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

#[derive(Debug, Args)]
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
    /// Output JSON instead of text.
    #[arg(long)]
    json: bool,
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

import { appendFileSync, mkdirSync } from "fs"
import { homedir } from "os"
import { join } from "path"

const DEBUG = process.env.DJINN_OPENCODE_DEBUG === "1"
const DISABLED = process.env.DJINN_OPENCODE_DISABLED === "1"
const CHILD = process.env.DJINN_OPENCODE_PLUGIN_CHILD === "1" || process.env.DJINN_REVIEWER === "1"
const AUTO_REVIEW = process.env.DJINN_OPENCODE_AUTO_REVIEW === "1"
const DJINN_BIN = process.env.DJINN_BIN || "djinn"
const CACHE_DIR = process.env.DJINN_CACHE_DIR || join(homedir(), ".cache", "djinn")
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

export const DjinnWatchPlugin = async () => {
  if (DISABLED || CHILD) {
    dbg("disabled", { DISABLED, CHILD })
    return {}
  }

  let currentSessionId = null
  let timer = null
  let lastReviewAt = 0
  const lastImportAt = new Map()

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            return run_tui(TuiArgs {
                view: TuiView::Tools,
                roots: Vec::new(),
                editor: None,
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
        Command::Tui(args) => run_tui(args),
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
            println!(
                "Memory saved [{}]: {} (reinforced {})",
                record.id, record.text, record.reinforcement_count
            );
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
        AgentCommand::Session(args) => run_agent_session(args),
        AgentCommand::Ask(args) => agent_ask(args),
    }
}

fn run_agent_session(args: AgentSessionArgs) -> Result<()> {
    match args.command {
        AgentSessionCommand::New(args) => agent_session_new(args),
        AgentSessionCommand::List(args) => agent_session_list(args),
        AgentSessionCommand::Show(args) => agent_session_show(args),
    }
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

fn agent_ask(args: AgentAskArgs) -> Result<()> {
    let prompt = args.prompt;
    let model = resolve_agent_model(args.model)?;
    let title = args
        .title
        .unwrap_or_else(|| prompt_title(&prompt, "Agent prompt"));
    let meta = AgentSessionMeta {
        title,
        workspace: resolve_agent_workspace(args.workspace)?,
        profile: args.profile,
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

fn complete_openai_prompt(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    prompt: String,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
) -> Result<djinn_agent::ModelResponse> {
    let api_key = api_key
        .or_else(|| env::var("OPENAI_API_KEY").ok())
        .ok_or_else(|| {
            anyhow::anyhow!("OPENAI_API_KEY is required; pass --api-key or set OPENAI_API_KEY")
        })?;
    let base_url = base_url
        .or_else(|| env::var("OPENAI_BASE_URL").ok())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let client = OpenAiClient::with_base_url(api_key, base_url);
    let runtime = AgentRuntime::new(client, store.clone(), ToolRegistry::new());
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .with_context(|| "creating Tokio runtime for OpenAI request")?;
    tokio.block_on(runtime.complete_once(
        id,
        ModelRequest {
            model,
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: prompt,
            }],
            tools: Vec::new(),
        },
    ))
}

fn resolve_agent_model(explicit: Option<String>) -> Result<String> {
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
    if let Some(model) = opencode_default_model()? {
        return Ok(model);
    }
    Ok("gpt-4o-mini".to_string())
}

fn opencode_default_model() -> Result<Option<String>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    opencode_default_model_from_paths(&opencode_model_config_paths(&cwd))
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

fn opencode_default_model_from_paths(paths: &[PathBuf]) -> Result<Option<String>> {
    for path in paths {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        if let Some(model) = opencode_default_model_from_content(&content)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?
        {
            return Ok(Some(model));
        }
    }
    Ok(None)
}

fn opencode_default_model_from_content(content: &str) -> Result<Option<String>> {
    let value: Value = serde_json::from_str(content)?;
    for pointer in [
        "/agents/coder/model",
        "/agents/default/model",
        "/agent/model",
        "/model",
    ] {
        if let Some(model) = value.pointer(pointer).and_then(Value::as_str) {
            let model = model.trim();
            if !model.is_empty() {
                return Ok(Some(model.to_string()));
            }
        }
    }
    Ok(None)
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

fn run_tui(args: TuiArgs) -> Result<()> {
    let roots = tool_roots(args.roots);
    let tools = scan_tools(&roots)?;
    let chats = chat_store().list()?;
    let candidates = pending_memories(candidate_store().list()?);
    let suggestions = suggestion_store().list()?;
    let skills = skill_records()?;
    let active_context = context_store().active()?;
    let Some(action) = djinn_tui::run_dashboard_with_handler(
        tools,
        chats,
        candidates,
        suggestions,
        skills,
        active_context,
        dashboard_tab(args.view),
        |action| match action {
            djinn_tui::TuiAction::RejectCandidates(ids) => reject_memories_silent(&ids).map(|_| ()),
            djinn_tui::TuiAction::DeleteChats(ids) => delete_chats_silent(&ids).map(|_| ()),
            djinn_tui::TuiAction::DeleteSuggestions(ids) => remove_suggestions(&ids).map(|_| ()),
            djinn_tui::TuiAction::OpenTool(_)
            | djinn_tui::TuiAction::OpenSkill(_)
            | djinn_tui::TuiAction::ShareChats(_)
            | djinn_tui::TuiAction::AcceptCandidate(_) => Ok(()),
        },
    )?
    else {
        return Ok(());
    };
    match action {
        djinn_tui::TuiAction::OpenTool(entry) => open_tool_entry(&entry, args.editor),
        djinn_tui::TuiAction::OpenSkill(entry) => open_skill_entry(&entry, args.editor),
        djinn_tui::TuiAction::ShareChats(request) => share_chats(ShareChatsArgs {
            ids: request.chat_ids,
            source: None,
            query: None,
            limit: 10,
            all: false,
            mode: share_chats_mode_from_tui(request.mode),
            context_only: request.context_only,
            max_chars_per_chat: 4000,
        }),
        djinn_tui::TuiAction::AcceptCandidate(id) => accept_memory(AcceptMemoryArgs {
            id,
            agent: None,
            title: "djinn memory suggestion review".to_string(),
            opencode_bin: "opencode".to_string(),
            dry_run: false,
        }),
        djinn_tui::TuiAction::RejectCandidates(ids) => reject_memories_silent(&ids).map(|_| ()),
        djinn_tui::TuiAction::DeleteChats(ids) => delete_chats_silent(&ids).map(|_| ()),
        djinn_tui::TuiAction::DeleteSuggestions(ids) => remove_suggestions(&ids).map(|_| ()),
    }
}

fn dashboard_tab(view: TuiView) -> djinn_tui::DashboardTab {
    match view {
        TuiView::Tools => djinn_tui::DashboardTab::Tools,
        TuiView::Chats => djinn_tui::DashboardTab::Chats,
        TuiView::Memories => djinn_tui::DashboardTab::Candidates,
        TuiView::Suggestions => djinn_tui::DashboardTab::Memories,
        TuiView::Skills => djinn_tui::DashboardTab::Skills,
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
    let records = pending_memories(candidate_store().list()?);
    if records.is_empty() {
        println!("Memories are empty.");
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!(
                "  {}. [{}] {}{}",
                idx + 1,
                record.id,
                record.text,
                format_candidate_suffix(record)
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

fn add_memory(args: AddMemoryArgs) -> Result<MemoryCandidate> {
    candidate_store().add_input(memory_input_from_args(args)?)
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
        let memories = candidate_store().list()?;
        args.source_memories
            .iter()
            .map(|id| {
                let memory = resolve_candidate(&memories, id)?;
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
        println!(
            "  - {} -> chat {} ({}, {})",
            session_id, session.chat_id, session.title, session.imported_at
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
                out.push_str(&format!(
                    "  {}. {} -> chat {} ({}, imported {})\n",
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
                    }
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

fn ingest_memories(args: IngestMemoriesArgs) -> Result<()> {
    let candidates = pending_memories(candidate_store().list()?);
    let resolved_ids = resolve_candidate_ids(&candidates, &args.ids)?;
    let selected = resolved_ids
        .iter()
        .map(|id| resolve_candidate(&candidates, id).cloned())
        .collect::<Result<Vec<_>>>()?;
    let mut outputs = Vec::new();
    for candidate in &selected {
        let target = if args.target == IngestTarget::Auto {
            infer_ingest_target(candidate)
        } else {
            args.target
        };
        outputs.push(ingest_candidate_as(candidate, target, args.force)?);
    }
    if !args.keep {
        candidate_store().remove_ids(&resolved_ids)?;
    }

    println!("Ingested {} memories:", outputs.len());
    for output in outputs {
        println!("  - {output}");
    }
    Ok(())
}

fn ingest_candidate_as(
    candidate: &MemoryCandidate,
    target: IngestTarget,
    force_skill: bool,
) -> Result<String> {
    let input = memory_input_from_candidate(candidate);
    match target {
        IngestTarget::Auto => unreachable!("auto target must be resolved before ingestion"),
        IngestTarget::Memory => {
            let record = memory_store().add_input(input)?;
            Ok(format!("memory [{}]: {}", record.id, record.text))
        }
        IngestTarget::Suggestion => {
            let suggestion = suggestion_store().add_input(SuggestionInput {
                text: candidate.text.clone(),
                target: non_empty_option(&candidate.kind),
                rationale: Some("Created from a reviewable memory.".to_string()),
                draft: None,
                evidence: candidate.evidence.clone(),
                sources: candidate.sources.clone(),
            })?;
            Ok(format!(
                "suggestion [{}]: {}",
                suggestion.id, suggestion.text
            ))
        }
        IngestTarget::Skill => {
            let name = skill_name_from_candidate(candidate);
            let content = skill_content_from_candidate(candidate);
            let skill =
                skill_store().add_with_content(&name, &candidate.text, content, force_skill)?;
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

fn infer_ingest_target(candidate: &MemoryCandidate) -> IngestTarget {
    let haystack = format!("{} {}", candidate.kind, candidate.text).to_lowercase();
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

fn memory_input_from_candidate(candidate: &MemoryCandidate) -> MemoryInput {
    MemoryInput {
        text: candidate.text.clone(),
        scope: non_empty_option(&candidate.scope),
        kind: non_empty_option(&candidate.kind),
        confidence: non_empty_option(&candidate.confidence),
        not_before: non_empty_option(&candidate.not_before),
        evidence: candidate.evidence.clone(),
        sources: candidate.sources.clone(),
    }
}

fn skill_name_from_candidate(candidate: &MemoryCandidate) -> String {
    candidate
        .id
        .split('-')
        .filter(|part| !part.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-")
}

fn skill_content_from_candidate(candidate: &MemoryCandidate) -> String {
    let name = skill_name_from_candidate(candidate);
    let mut out = format!(
        "# Skill: {name}\n\n{}\n\n## When to use\n\n- Use when this remembered workflow applies to the current task.\n\n## Workflow\n\n1. Apply the remembered guidance below.\n\n## Ingested guidance\n\n{}\n",
        candidate.text,
        candidate.text
    );
    if !candidate.evidence.is_empty() {
        out.push_str("\n## Evidence\n\n");
        for evidence in &candidate.evidence {
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
    let removed = reject_memories_silent(ids)?;
    if removed.is_empty() {
        println!("No memories were rejected.");
    } else {
        println!("Rejected and removed {} memories:", removed.len());
        for candidate in removed {
            println!("  - [{}] {}", candidate.id, candidate.text);
        }
    }
    Ok(())
}

fn reject_memories_silent(ids: &[String]) -> Result<Vec<MemoryCandidate>> {
    let candidates = pending_memories(candidate_store().list()?);
    let resolved = resolve_candidate_ids(&candidates, ids)?;
    candidate_store().remove_ids(&resolved)
}

fn pending_memories(records: Vec<MemoryCandidate>) -> Vec<MemoryCandidate> {
    records
        .into_iter()
        .filter(is_pending_memory)
        .collect::<Vec<_>>()
}

fn is_pending_memory(record: &MemoryCandidate) -> bool {
    record.status.trim().is_empty() || record.status.eq_ignore_ascii_case("pending")
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
    let memories = pending_memories(candidate_store().list()?);
    let record = resolve_candidate(&memories, id)?;
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
    println!("Reinforced: {}", record.reinforcement_count);

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
    let matches = pending_memories(candidate_store().list()?)
        .into_iter()
        .filter(|record| candidate_matches(record, &query))
        .collect::<Vec<_>>();
    for (idx, record) in matches.iter().enumerate() {
        println!(
            "  {}. [{}] {}{}",
            idx + 1,
            record.id,
            record.text,
            format_candidate_suffix(record)
        );
    }
    println!("\nTotal: {} matching memories", matches.len());
    Ok(())
}

fn select_memories_for_review(
    records: &[MemoryCandidate],
    args: &ReviewMemoriesArgs,
) -> Result<Vec<MemoryCandidate>> {
    if !args.ids.is_empty() {
        let mut seen = HashSet::new();
        let mut selected = Vec::new();
        for id in &args.ids {
            let record = resolve_candidate(records, id)?;
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
                .map(|query| candidate_matches(record, query))
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
    let candidates = pending_memories(candidate_store().list()?);
    let chats = chat_store().list()?;
    let tools = scan_tools(&tool_roots(Vec::new()))?;
    let watcher_state = format_ideas_pipeline_context(
        &idea_store().list()?,
        &action_store().list()?,
        &format_opencode_watcher_state_for_ideas(),
    );
    println!(
        "{}",
        djinn_suggest::build_prompt_with_pipeline(
            &memories,
            &candidates,
            &chats,
            &tools,
            &watcher_state
        )
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
        let memories = memory_store().list()?;
        println!(
            "{}",
            format_chats_review_prompt(&selected, &args, &memories)
        );
    }
    Ok(())
}

fn promote_chat(args: ShareChatArgs) -> Result<()> {
    let records = chat_store().list()?;
    let record = resolve_chat(&records, &args.id)?;
    let memories = memory_store().list()?;
    println!("{}", format_chat_candidate_prompt(record, &memories));
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
    Ok(format_chats_candidate_prompt(&selected, &args, &memories))
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
    let memories = pending_memories(candidate_store().list()?);
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
    memories: &[MemoryCandidate],
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
        "\n## Extraction Guidelines\n\nExtract reviewable memories for:\n\n- user preferences and corrections\n- repeated workflows or tool choices\n- project-specific conventions\n- safety constraints or gotchas\n- reusable debugging/implementation patterns\n\nDo not extract:\n\n- one-off task status\n- secrets, credentials, tokens, private URLs, or sensitive raw data\n- facts that are already captured in existing memories\n- noisy transcript details that will not help future agents\n\nReturn only a short reviewed list of shell commands the user can run manually. Include enough metadata and copied evidence that the memory remains understandable even if the source chat is deleted later. Use `--not-before YYYY-MM-DD` when a true memory should not drive actions until a future date. Prefer this form:\n\n```bash\ndjinn add memory \"...\" --scope project --kind preference --confidence high --not-before 2026-10-01 --evidence \"User explicitly corrected the agent to ...\" --source-chat CHAT_ID\n```\n\nIf there are no durable lessons, say: `No durable memories recommended.`\n",
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

fn format_chat_candidate_prompt(record: &ChatRecord, memories: &[MemoryRecord]) -> String {
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
        "\n```\n\nAfter review, the user can run `djinn list memories`, `djinn show memory <id>`, `djinn accept memory <id>`, or `djinn reject memory <id>`.\n",
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

fn format_chats_candidate_prompt(
    records: &[ChatRecord],
    args: &ShareChatsArgs,
    memories: &[MemoryRecord],
) -> String {
    let mut out = format_chats_review_prompt(records, args, memories);
    out = out.replace("# Djinn Multi-Chat Review", "# Djinn Multi-Chat Promotion");
    out.push_str(
        "\n\n## Promotion Output\n\nReturn reviewed `djinn add memory` commands. Include scope, kind, confidence, copied evidence, and one or more `--source-chat` pointers when available. Use `--not-before YYYY-MM-DD` when a future activation date is appropriate. Example:\n\n```bash\ndjinn add memory \"...\" --scope project --kind convention --confidence high --not-before 2026-10-01 --evidence \"Repeated across reviewed chats ...\" --source-chat CHAT_ID\n```\n\nThe user will accept or reject memories with `djinn accept memory <id>` / `djinn reject memory <id>`.\n",
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
        let (body, truncated) = truncate_with_flag(&record.content, max_chars_per_chat);
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

fn resolve_candidate<'a>(records: &'a [MemoryCandidate], id: &str) -> Result<&'a MemoryCandidate> {
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

fn resolve_candidate_ids(records: &[MemoryCandidate], ids: &[String]) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();
    for id in ids {
        let record = resolve_candidate(records, id)?;
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

fn candidate_matches(record: &MemoryCandidate, query: &str) -> bool {
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

fn format_candidate_suffix(record: &MemoryCandidate) -> String {
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
    if record.reinforcement_count > 1 {
        parts.push("reinforced");
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

fn candidate_store() -> CandidateStore {
    CandidateStore::default_in(&djinn_core::default_data_dir())
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

    fn test_candidate(kind: &str, text: &str) -> MemoryCandidate {
        MemoryCandidate {
            id: "candidate".to_string(),
            text: text.to_string(),
            created_at: "2026-07-09".to_string(),
            status: "pending".to_string(),
            scope: "project:djinn".to_string(),
            kind: kind.to_string(),
            confidence: "medium".to_string(),
            not_before: String::new(),
            evidence: Vec::new(),
            sources: Vec::new(),
            reinforcement_count: 1,
        }
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
    fn opencode_default_model_reads_coder_agent_model() {
        let model = opencode_default_model_from_content(
            r#"{
              "agents": {
                "coder": { "model": "gpt-4.1" },
                "task": { "model": "gpt-4.1-mini" }
              }
            }"#,
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("gpt-4.1"));
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

        let model = opencode_default_model_from_paths(&[missing, first, second]).unwrap();
        assert_eq!(model.as_deref(), Some("gpt-4.1"));
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
    fn infer_ingest_target_routes_candidate_kinds() {
        assert_eq!(
            infer_ingest_target(&test_candidate("instruction", "Use uv")),
            IngestTarget::Suggestion
        );
        assert_eq!(
            infer_ingest_target(&test_candidate("skill-proposal", "Reusable workflow")),
            IngestTarget::Skill
        );
        assert_eq!(
            infer_ingest_target(&test_candidate("idea", "Consider better search")),
            IngestTarget::Idea
        );
        assert_eq!(
            infer_ingest_target(&test_candidate("action", "TODO: review docs")),
            IngestTarget::Action
        );
        assert_eq!(
            infer_ingest_target(&test_candidate("preference", "Prefer concise output")),
            IngestTarget::Suggestion
        );
    }

    #[test]
    fn pending_memories_excludes_accepted_items() {
        let mut accepted = test_candidate("preference", "Already reviewed");
        accepted.id = "accepted".to_string();
        accepted.status = "accepted".to_string();
        let mut pending = test_candidate("preference", "Needs review");
        pending.id = "pending".to_string();

        let records = pending_memories(vec![accepted, pending]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "pending");
    }

    #[test]
    fn format_memory_review_prompt_creates_suggestions_from_memories() {
        let memories = vec![MemoryCandidate {
            id: "djinn-session-note".to_string(),
            text: "Djinn implementation session detail".to_string(),
            created_at: "2026-07-09".to_string(),
            status: "pending".to_string(),
            scope: "project:djinn".to_string(),
            kind: "implementation-note".to_string(),
            confidence: "medium".to_string(),
            not_before: String::new(),
            evidence: vec!["Captured during a Djinn session.".to_string()],
            sources: Vec::new(),
            reinforcement_count: 1,
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
