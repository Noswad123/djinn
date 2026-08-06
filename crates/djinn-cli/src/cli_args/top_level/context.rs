use std::path::PathBuf;

use clap::Args;

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
