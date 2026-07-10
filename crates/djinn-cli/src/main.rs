use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use djinn_chats::ChatRecord;
use djinn_contexts::{resolve_context, ContextInput, ContextRecord, ContextStore};
use djinn_memory::{CandidateStore, MemoryCandidate, MemoryInput, MemoryRecord, MemorySource};
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
    /// Promote raw context into durable-knowledge candidates.
    Promote(PromoteArgs),
    /// Run an external review to organically create memory candidates.
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
    /// List durable distilled lessons and preferences.
    Memories,
    /// List pending/reviewed memory candidates.
    Candidates,
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
    /// Show a durable memory by id or text fragment.
    Memory { id: String },
    /// Show a memory candidate by id or text fragment.
    Candidate { id: String },
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
    /// Add a distilled memory.
    Memory(AddMemoryArgs),
    /// Add a pending memory candidate.
    Candidate(AddMemoryArgs),
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
    /// Accept a memory candidate and write it as a durable memory.
    Candidate { id: String },
}

#[derive(Debug, Args)]
struct RejectArgs {
    #[command(subcommand)]
    noun: RejectNoun,
}

#[derive(Debug, Subcommand)]
enum RejectNoun {
    /// Reject a memory candidate.
    Candidate { id: String },
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
    /// Ask OpenCode to review recent Djinn chats and add memory candidates.
    Chats(ReviewChatsArgs),
    /// Compatibility alias for `djinn review chats --source opencode`.
    Opencode(ReviewOpencodeArgs),
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
    /// Emit a candidate-extraction prompt for one chat.
    Chat(ShareChatArgs),
    /// Emit a candidate-extraction prompt for multiple chats.
    Chats(ShareChatsArgs),
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
    Candidates,
    Memories,
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
    /// Do not act on this memory/candidate before this date, for example: 2026-10-01.
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
 *   DJINN_OPENCODE_AUTO_REVIEW=1       opt into background candidate reviews
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
        Command::Tui(args) => run_tui(args),
    }
}

fn run_list(args: ListArgs) -> Result<()> {
    match args.noun {
        ListNoun::Tools(scope) => list_tools(scope),
        ListNoun::Memories => list_memories(),
        ListNoun::Candidates => list_candidates(),
        ListNoun::Chats(args) => list_chats(args),
        ListNoun::Skills(args) => list_skills(args),
        ListNoun::Contexts(args) | ListNoun::Ctx(args) => list_contexts(args),
    }
}

fn run_show(args: ShowArgs) -> Result<()> {
    match args.noun {
        ShowNoun::Chat(args) => show_chat(args),
        ShowNoun::Memory { id } => show_memory(&id),
        ShowNoun::Candidate { id } => show_candidate(&id),
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
            println!("Memory added [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Candidate(args) => {
            let record = add_candidate(args)?;
            println!("Candidate added [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Skill(args) => add_skill(args),
        AddNoun::Ctx(args) => add_context(args),
    }
}

fn run_accept(args: AcceptArgs) -> Result<()> {
    match args.noun {
        AcceptNoun::Candidate { id } => accept_candidate(&id),
    }
}

fn run_reject(args: RejectArgs) -> Result<()> {
    match args.noun {
        RejectNoun::Candidate { id } => reject_candidate(&id),
    }
}

fn run_promote(args: PromoteArgs) -> Result<()> {
    match args.noun {
        PromoteNoun::Chat(args) => promote_chat(args),
        PromoteNoun::Chats(args) => promote_chats(args),
    }
}

fn run_review(args: ReviewArgs) -> Result<()> {
    match args.source {
        ReviewSource::Chats(args) => review_chats(args),
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

fn run_tui(args: TuiArgs) -> Result<()> {
    let roots = tool_roots(args.roots);
    let tools = scan_tools(&roots)?;
    let chats = chat_store().list()?;
    let candidates = candidate_store().list()?;
    let memories = memory_store().list()?;
    let skills = skill_records()?;
    let active_context = context_store().active()?;
    let Some(action) = djinn_tui::run_dashboard(
        tools,
        chats,
        candidates,
        memories,
        skills,
        active_context,
        dashboard_tab(args.view),
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
        djinn_tui::TuiAction::AcceptCandidate(id) => accept_candidate(&id),
        djinn_tui::TuiAction::RejectCandidate(id) => reject_candidate(&id),
    }
}

fn dashboard_tab(view: TuiView) -> djinn_tui::DashboardTab {
    match view {
        TuiView::Tools => djinn_tui::DashboardTab::Tools,
        TuiView::Chats => djinn_tui::DashboardTab::Chats,
        TuiView::Candidates => djinn_tui::DashboardTab::Candidates,
        TuiView::Memories => djinn_tui::DashboardTab::Memories,
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

fn list_candidates() -> Result<()> {
    let records = candidate_store().list()?;
    if records.is_empty() {
        println!("Memory candidates are empty.");
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!(
                "  {}. [{}] {} ({}){}",
                idx + 1,
                record.id,
                record.text,
                record.status,
                format_candidate_suffix(record)
            );
        }
        println!("\nTotal: {} candidates", records.len());
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

fn add_candidate(args: AddMemoryArgs) -> Result<MemoryCandidate> {
    candidate_store().add_input(memory_input_from_args(args)?)
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

fn accept_candidate(id: &str) -> Result<()> {
    let candidates = candidate_store().list()?;
    let candidate = resolve_candidate(&candidates, id)?.clone();
    if candidate.status == "accepted" {
        println!("Candidate [{}] is already accepted.", candidate.id);
        return Ok(());
    }
    let memory = memory_store().add_input(MemoryInput {
        text: candidate.text.clone(),
        scope: non_empty_option(&candidate.scope),
        kind: non_empty_option(&candidate.kind),
        confidence: non_empty_option(&candidate.confidence),
        not_before: non_empty_option(&candidate.not_before),
        evidence: candidate.evidence.clone(),
        sources: candidate.sources.clone(),
    })?;
    candidate_store().update_status(&candidate.id, "accepted")?;
    println!(
        "Candidate [{}] accepted as memory [{}]: {}",
        candidate.id, memory.id, memory.text
    );
    Ok(())
}

fn reject_candidate(id: &str) -> Result<()> {
    let candidates = candidate_store().list()?;
    let candidate = resolve_candidate(&candidates, id)?.clone();
    candidate_store().update_status(&candidate.id, "rejected")?;
    println!("Candidate [{}] rejected: {}", candidate.id, candidate.text);
    Ok(())
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

fn show_candidate(id: &str) -> Result<()> {
    let candidates = candidate_store().list()?;
    let record = resolve_candidate(&candidates, id)?;
    let chats = chat_store().list().unwrap_or_default();

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
    let candidates = candidate_store().list()?;
    let chats = chat_store().list()?;
    let tools = scan_tools(&tool_roots(Vec::new()))?;
    let watcher_state = format_opencode_watcher_state_for_ideas();
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

    let mut command = ProcessCommand::new(&args.opencode_bin);
    command
        .arg("run")
        .arg(prompt)
        .arg("--title")
        .arg(&args.title);
    if let Some(agent) = args
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.arg("--agent").arg(agent);
    }
    command.env("DJINN_REVIEWER", "1");
    command.env("DJINN_OPENCODE_PLUGIN_CHILD", "1");
    let status = command
        .status()
        .with_context(|| format!("running {} run", args.opencode_bin))?;
    if !status.success() {
        bail!("{} run exited with status {status}", args.opencode_bin);
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
        "\n## Extraction Rules\n\nExtract candidate memories for:\n\n- user preferences and corrections\n- repeated workflows or tool choices\n- project-specific conventions\n- safety rules or gotchas\n- reusable debugging/implementation patterns\n\nDo not extract:\n\n- one-off task status\n- secrets, credentials, tokens, private URLs, or sensitive raw data\n- facts that are already captured in existing memories\n- noisy transcript details that will not help future agents\n\nReturn only a short reviewed list of shell commands the user can run manually. Include enough metadata and copied evidence that the memory remains understandable even if the source chat is deleted later. Use `--not-before YYYY-MM-DD` when a true memory should not drive actions until a future date. Prefer this form:\n\n```bash\ndjinn add memory \"...\" --scope project --kind preference --confidence high --not-before 2026-10-01 --evidence \"User explicitly corrected the agent to ...\" --source-chat CHAT_ID\n```\n\nIf there are no durable lessons, say: `No durable memories recommended.`\n",
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
        "# Djinn Chat Promotion Candidate Extraction",
    );
    out = out.replace("djinn add memory", "djinn add candidate");
    out.push_str(
        "\n\n## Promotion Output\n\nReturn reviewed candidate commands instead of direct memory writes. Use this exact command shape so Djinn can track pending/accepted/rejected lifecycle. Use `--not-before YYYY-MM-DD` for memories that should be remembered now but not acted on until later:\n\n```bash\ndjinn add candidate \"...\" --scope project --kind preference --confidence high --not-before 2026-10-01 --evidence \"Copied durable evidence ...\" --source-chat ",
    );
    out.push_str(&record.id);
    out.push_str(
        "\n```\n\nAfter review, the user can run `djinn list candidates`, `djinn show candidate <id>`, `djinn accept candidate <id>`, or `djinn reject candidate <id>`.\n",
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
    out.push_str("\n## Output Rules\n\n");
    match args.mode {
        ShareChatsMode::Summary => out.push_str(
            "Return Markdown with sections: `Summary`, `Decisions`, `Open Follow-ups`, and `Potential Memories`. Do not write memories automatically.\n",
        ),
        ShareChatsMode::Patterns => out.push_str(
            "Return Markdown with sections: `High-confidence Patterns`, `Possible One-offs`, `Workflow Opportunities`, and `Candidate Memories`. Do not write memories automatically.\n",
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
    out = out.replace("djinn add memory", "djinn add candidate");
    out.push_str(
        "\n\n## Promotion Output\n\nReturn reviewed `djinn add candidate` commands, not `djinn add memory` commands. Include scope, kind, confidence, copied evidence, and one or more `--source-chat` pointers when available. Use `--not-before YYYY-MM-DD` when a future activation date is appropriate. Example:\n\n```bash\ndjinn add candidate \"...\" --scope project --kind convention --confidence high --not-before 2026-10-01 --evidence \"Repeated across reviewed chats ...\" --source-chat CHAT_ID\n```\n\nThe user will accept or reject candidates with `djinn accept candidate <id>` / `djinn reject candidate <id>`.\n",
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
        [] => bail!("no memory candidate named {id:?} found"),
        many => {
            eprintln!("multiple memory candidates match {id:?}:");
            for record in many {
                eprintln!("  - [{}] {} ({})", record.id, record.text, record.status);
            }
            bail!("memory candidate id is ambiguous")
        }
    }
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
        || record.sources.iter().any(|source| {
            source.source_type.to_lowercase().contains(query)
                || source.source.to_lowercase().contains(query)
                || source.source_id.to_lowercase().contains(query)
                || source.chat_id.to_lowercase().contains(query)
                || source.title.to_lowercase().contains(query)
        })
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

fn skill_store() -> SkillStore {
    SkillStore::default_in(&djinn_core::default_data_dir())
}

fn context_store() -> ContextStore {
    ContextStore::default_in(&djinn_core::default_data_dir())
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
