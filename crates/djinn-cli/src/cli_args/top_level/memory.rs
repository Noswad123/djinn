use clap::Args;

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
