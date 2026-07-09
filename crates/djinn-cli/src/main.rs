use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use djinn_memory::MemoryRecord;
use djinn_names::NameEntry;

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
    Tool { name: String },
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
    /// Emit agent-ready context for a chat/session.
    Chat { id: String },
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
    Tools { query: String },
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
    Opencode,
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
    Tool { name: String },
}

#[derive(Debug, Args, Clone)]
struct ToolsScope {
    /// Dotfiles/local tooling root to scan.
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct IndexToolsArgs {
    /// Dotfiles/local tooling root to scan.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Index JSON path. Defaults under the scanned root.
    #[arg(long)]
    index: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::List(ListArgs {
        noun: ListNoun::Tools(ToolsScope { root: None }),
    })) {
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
    }
}

fn run_list(args: ListArgs) -> Result<()> {
    match args.noun {
        ListNoun::Tools(scope) => list_tools(scope),
        ListNoun::Memories => list_memories(),
        ListNoun::Chats => planned("list chats", "will list raw or summarized AI interactions"),
        ListNoun::Skills => planned("list skills", "will list agent skills known to Djinn"),
        ListNoun::Contexts | ListNoun::Ctx => planned(
            "list ctx",
            "will list available work/personal/project contexts",
        ),
    }
}

fn run_show(args: ShowArgs) -> Result<()> {
    match args.noun {
        ShowNoun::Chat { id } => planned(
            &format!("show chat {id}"),
            "will show one raw or summarized AI interaction",
        ),
        ShowNoun::Ctx => planned("show ctx", "will show the active context"),
        ShowNoun::Tool { name } => show_tool(&name),
        ShowNoun::Skill { name } => planned(
            &format!("show skill {name}"),
            "will show one agent skill and its source",
        ),
    }
}

fn run_add(args: AddArgs) -> Result<()> {
    match args.noun {
        AddNoun::Memory { text } => {
            let record = memory_store().add(&text)?;
            println!("Memory added: {}", record.text);
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
        RmNoun::Memory { keyword } => planned(
            &format!("rm memory {keyword}"),
            "will remove memories matching a keyword after confirmation",
        ),
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
            let root = tools_root(scope.root);
            let entries = scan_tools(&root)?;
            println!("Scanned {} tools under {}", entries.len(), root.display());
            Ok(())
        }
    }
}

fn run_index(args: IndexArgs) -> Result<()> {
    match args.noun {
        IndexNoun::Tools(args) => {
            let root = tools_root(args.root);
            let index_path = args
                .index
                .unwrap_or_else(|| djinn_core::default_index_path(&root));
            let (count, changed) = djinn_names::write_index(&root, &index_path)?;
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
            let root = tools_root(scope.root);
            let entries = scan_tools(&root)?;
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
        ShareNoun::Chat { id } => planned(
            &format!("share chat {id}"),
            "will summarize/export one AI interaction for an agent",
        ),
    }
}

fn run_search(args: SearchArgs) -> Result<()> {
    match args.noun {
        SearchNoun::Chats { query } => planned(
            &format!("search chats {query}"),
            "will search raw or summarized AI interactions",
        ),
        SearchNoun::Tools { query } => search_tools(&query),
        SearchNoun::Memories { query } => search_memories(&query),
    }
}

fn run_watch(args: WatchArgs) -> Result<()> {
    match args.source {
        WatchSource::Opencode => planned(
            "watch opencode",
            "will ingest OpenCode conversations into Djinn chats",
        ),
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
        OpenNoun::Tool { name } => planned(
            &format!("open tool {name}"),
            "will open a discovered local tool in $VISUAL, $EDITOR, or nvim",
        ),
    }
}

fn list_tools(scope: ToolsScope) -> Result<()> {
    let root = tools_root(scope.root);
    let entries = scan_tools(&root)?;
    if entries.is_empty() {
        println!("Djinn found 0 tools under {}", root.display());
        return Ok(());
    }
    for entry in entries {
        println!(
            "{}\t{}:{}\t{}",
            entry.name,
            entry.path.display(),
            entry.line,
            entry.description
        );
    }
    Ok(())
}

fn list_memories() -> Result<()> {
    let records = memory_store().list()?;
    if records.is_empty() {
        println!("Memories are empty.");
    } else {
        for (idx, record) in records.iter().enumerate() {
            println!("  {}. {}", idx + 1, record.text);
        }
        println!("\nTotal: {} memories", records.len());
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
    if let Some(path) = backup {
        println!("Memories cleared. Backup written to {}", path.display());
    } else {
        println!("Memories cleared.");
    }
    Ok(())
}

fn show_tool(name: &str) -> Result<()> {
    let entries = scan_tools(&djinn_core::default_dotfiles_root())?;
    let Some(entry) = entries
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(name))
    else {
        bail!("no tool named {name:?} found");
    };
    println!("# {}\n", entry.name);
    println!("{}\n", entry.description);
    println!("Source: {}:{}\n", entry.path.display(), entry.line);
    println!("```text\n{}\n```", entry.preview);
    Ok(())
}

fn search_tools(query: &str) -> Result<()> {
    let query = query.to_lowercase();
    let matches = scan_tools(&djinn_core::default_dotfiles_root())?
        .into_iter()
        .filter(|entry| {
            entry.name.to_lowercase().contains(&query)
                || entry.description.to_lowercase().contains(&query)
                || entry.preview.to_lowercase().contains(&query)
        })
        .collect::<Vec<_>>();
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
    Ok(())
}

fn search_memories(query: &str) -> Result<()> {
    let query = query.to_lowercase();
    let matches = memory_store()
        .list()?
        .into_iter()
        .filter(|record| record.text.to_lowercase().contains(&query))
        .collect::<Vec<_>>();
    for (idx, record) in matches.iter().enumerate() {
        println!("  {}. {}", idx + 1, record.text);
    }
    println!("\nTotal: {} matching memories", matches.len());
    Ok(())
}

fn share_ideas() -> Result<()> {
    let memories = memory_store().list()?;
    let tools = scan_tools(&djinn_core::default_dotfiles_root())?;
    println!("{}", djinn_suggest::build_prompt(&memories, &tools));
    Ok(())
}

fn format_tools_context(entries: &[NameEntry]) -> String {
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
        out.push_str(&format!("- {}\n", record.text));
    }
    out
}

fn tools_root(root: Option<PathBuf>) -> PathBuf {
    root.unwrap_or_else(djinn_core::default_dotfiles_root)
}

fn scan_tools(root: &Path) -> Result<Vec<NameEntry>> {
    djinn_names::scan(root, &djinn_names::default_extensions())
}

fn memory_store() -> djinn_memory::MemoryStore {
    djinn_memory::MemoryStore::default_in(&djinn_core::default_data_dir())
}

fn planned(command: &str, description: &str) -> Result<()> {
    println!("djinn {command}: planned — {description}.");
    Ok(())
}
