use std::path::PathBuf;

use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Args)]
pub(crate) struct TuiArgs {
    /// TUI view to open. Defaults to sessions.
    #[arg(value_enum, default_value_t = TuiView::Sessions)]
    pub(crate) view: TuiView,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Editor command for opening tools. Defaults to VISUAL, then EDITOR, then nvim.
    #[arg(long)]
    pub(crate) editor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TuiView {
    Tools,
    Sessions,
    Memories,
    Suggestions,
    Skills,
}
