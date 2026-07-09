use std::env;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use djinn_chats::ChatRecord;
use djinn_memory::MemoryRecord;
use djinn_tools::ToolEntry;

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
    /// Remove one item.
    Rm(RmArgs),
    /// Clear a collection after confirmation.
    Clear(ClearArgs),
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
    /// List raw or summarized AI interactions.
    Chats,
    /// List agent skills known to Djinn.
    Skills,
    /// List available contexts.
    Contexts,
    /// Alias for contexts; ctx has no plural form.
    Ctx,
}

#[derive(Debug, Args)]
struct ShowArgs {
    #[command(subcommand)]
    noun: ShowNoun,
}

#[derive(Debug, Subcommand)]
enum ShowNoun {
    /// Show a chat/session by id.
    Chat { id: String },
    /// Show the active context.
    Ctx,
    /// Show a tool by name.
    Tool(ToolLookupArgs),
    /// Show a skill by name.
    Skill { name: String },
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
    Memory { text: String },
    /// Add or scaffold a skill.
    Skill { name: String },
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
    Skill { name: String },
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
    Skills,
    /// Emit a memory-extraction prompt for a chat/session.
    Chat(ShareChatArgs),
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
struct SwitchArgs {
    #[command(subcommand)]
    noun: SwitchNoun,
}

#[derive(Debug, Subcommand)]
enum SwitchNoun {
    /// Switch the active context.
    Ctx { name: String },
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
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    roots: Vec<PathBuf>,
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
struct ShareChatArgs {
    /// Chat id, source id, or unambiguous title fragment.
    id: String,
    /// Emit raw context only instead of a memory-extraction prompt.
    #[arg(long)]
    context_only: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            return run_tui(TuiArgs { roots: Vec::new() });
        }
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };
    match command {
        Command::List(args) => run_list(args),
        Command::Show(args) => run_show(args),
        Command::Add(args) => run_add(args),
        Command::Rm(args) => run_rm(args),
        Command::Clear(args) => run_clear(args),
        Command::Scan(args) => run_scan(args),
        Command::Index(args) => run_index(args),
        Command::Share(args) => run_share(args),
        Command::Search(args) => run_search(args),
        Command::Watch(args) => run_watch(args),
        Command::Switch(args) => run_switch(args),
        Command::Open(args) => run_open(args),
        Command::Tui(args) => run_tui(args),
    }
}

fn run_list(args: ListArgs) -> Result<()> {
    match args.noun {
        ListNoun::Tools(scope) => list_tools(scope),
        ListNoun::Memories => list_memories(),
        ListNoun::Chats => list_chats(),
        ListNoun::Skills => planned("list skills", "will list agent skills known to Djinn"),
        ListNoun::Contexts | ListNoun::Ctx => planned(
            "list ctx",
            "will list available work/personal/project contexts",
        ),
    }
}

fn run_show(args: ShowArgs) -> Result<()> {
    match args.noun {
        ShowNoun::Chat { id } => show_chat(&id),
        ShowNoun::Ctx => planned("show ctx", "will show the active context"),
        ShowNoun::Tool(args) => show_tool(args),
        ShowNoun::Skill { name } => planned(
            &format!("show skill {name}"),
            "will show one agent skill and its source",
        ),
    }
}

fn run_add(args: AddArgs) -> Result<()> {
    match args.noun {
        AddNoun::Chat(args) => add_chat(args),
        AddNoun::Memory { text } => {
            let record = memory_store().add(&text)?;
            println!("Memory added [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Skill { name } => planned(
            &format!("add skill {name}"),
            "will create or scaffold an agent skill",
        ),
    }
}

fn run_rm(args: RmArgs) -> Result<()> {
    match args.noun {
        RmNoun::Memory { keyword } => rm_memory(&keyword),
        RmNoun::Skill { name } => planned(
            &format!("rm skill {name}"),
            "will archive or remove an agent skill",
        ),
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
        ShareNoun::Skills => planned(
            "share skills",
            "will emit agent-ready context for known skills",
        ),
        ShareNoun::Chat(args) => share_chat(args),
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

fn run_switch(args: SwitchArgs) -> Result<()> {
    match args.noun {
        SwitchNoun::Ctx { name } => planned(
            &format!("switch ctx {name}"),
            "will switch the active context/persona",
        ),
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
    djinn_tui::run_tools(tools)
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
            println!("  {}. [{}] {}", idx + 1, record.id, record.text);
        }
        println!("\nTotal: {} memories", records.len());
    }
    Ok(())
}

fn list_chats() -> Result<()> {
    let records = chat_store().list()?;
    if records.is_empty() {
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

fn watch_opencode(args: WatchOpencodeArgs) -> Result<()> {
    if let Some(0) = args.interval {
        bail!("--interval must be greater than zero seconds");
    }

    let cli = djinn_opencode::OpencodeCli::new(args.opencode_bin.clone());
    let sanitize = !args.unsafe_unsanitized;

    loop {
        let session_id = match &args.session_id {
            Some(id) => id.clone(),
            None => cli.latest_session_id()?,
        };
        let export = cli.export_session(&session_id, sanitize)?;
        let title = args
            .title
            .clone()
            .unwrap_or_else(|| format!("OpenCode session {session_id}"));
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

fn show_chat(id: &str) -> Result<()> {
    let records = chat_store().list()?;
    let record = resolve_chat(&records, id)?;
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
        .filter(|record| {
            record.id.to_lowercase().contains(&query) || record.text.to_lowercase().contains(&query)
        })
        .collect::<Vec<_>>();
    for (idx, record) in matches.iter().enumerate() {
        println!("  {}. [{}] {}", idx + 1, record.id, record.text);
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
    let tools = scan_tools(&tool_roots(Vec::new()))?;
    println!("{}", djinn_suggest::build_prompt(&memories, &tools));
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

fn open_tool(args: OpenToolArgs) -> Result<()> {
    let roots = tool_roots(args.roots);
    let entries = scan_tools(&roots)?;
    let entry = resolve_tool(&entries, &args.name)?;
    let editor = args.editor.unwrap_or_else(default_editor);
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("editor command is empty");
    };
    let mut cmd = ProcessCommand::new(program);
    cmd.args(parts);
    cmd.arg(format!("+{}", entry.line));
    cmd.arg(&entry.path);
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

fn format_memories_context(records: &[MemoryRecord]) -> String {
    let mut out = String::from("# Djinn Memories\n\n");
    if records.is_empty() {
        out.push_str("No memories recorded.\n");
        return out;
    }
    for record in records {
        out.push_str(&format!("- `[{}]` {}\n", record.id, record.text));
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
        "\n## Extraction Rules\n\nExtract candidate memories for:\n\n- user preferences and corrections\n- repeated workflows or tool choices\n- project-specific conventions\n- safety rules or gotchas\n- reusable debugging/implementation patterns\n\nDo not extract:\n\n- one-off task status\n- secrets, credentials, tokens, private URLs, or sensitive raw data\n- facts that are already captured in existing memories\n- noisy transcript details that will not help future agents\n\nReturn only a short reviewed list of shell commands the user can run manually, using this exact form:\n\n```bash\ndjinn add memory \"...\"\n```\n\nIf there are no durable lessons, say: `No durable memories recommended.`\n",
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

fn chat_store() -> djinn_chats::ChatStore {
    djinn_chats::ChatStore::default_in(&djinn_core::default_cache_dir())
}

fn planned(command: &str, description: &str) -> Result<()> {
    println!("djinn {command}: planned — {description}.");
    Ok(())
}
