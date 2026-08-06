use clap::{Args, Subcommand};

use super::{AddCtxArgs, AddMemoryArgs, AddSkillArgs, AddSuggestionArgs};

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
