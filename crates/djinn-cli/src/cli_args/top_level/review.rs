use clap::{Args, Subcommand};

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
