use clap::{Args, Subcommand};

use super::RmSkillArgs;

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
