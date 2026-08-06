use clap::Args;

use crate::cli_args::OutputFormat;

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
