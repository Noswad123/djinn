use clap::{Args, Subcommand};

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
