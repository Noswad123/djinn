use clap::{Args, Subcommand};

use super::{ListCtxArgs, ListSkillsArgs, ToolsScope};

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
