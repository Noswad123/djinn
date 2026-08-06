use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use super::OutputFormat;

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_top_level_collection_commands() {
        let cli = Cli::try_parse_from(["djinn", "list", "tools", "--root", "/tmp/tools", "--json"])
            .unwrap();
        let Some(Command::List(args)) = cli.command else {
            panic!("expected list command");
        };
        let ListNoun::Tools(args) = args.noun else {
            panic!("expected list tools command");
        };
        assert_eq!(args.roots, vec![PathBuf::from("/tmp/tools")]);
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "show", "skill", "reviewer", "--json"]).unwrap();
        let Some(Command::Show(args)) = cli.command else {
            panic!("expected show command");
        };
        let ShowNoun::Skill(args) = args.noun else {
            panic!("expected show skill command");
        };
        assert_eq!(args.name, "reviewer");
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "add", "memory", "prefer small commits"]).unwrap();
        let Some(Command::Add(args)) = cli.command else {
            panic!("expected add command");
        };
        let AddNoun::Memory(args) = args.noun else {
            panic!("expected add memory command");
        };
        assert_eq!(args.text, "prefer small commits");
    }
}
