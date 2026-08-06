use clap::{Args, Subcommand};

use super::AcceptMemoryArgs;

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
