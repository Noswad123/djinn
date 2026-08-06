use std::path::PathBuf;

use clap::{Args, Subcommand};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    #[command(subcommand)]
    pub(crate) noun: SearchNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SearchNoun {
    /// Search local tools.
    Tools(SearchToolsArgs),
    /// Search memories.
    Memories { query: String },
    /// Search suggestions.
    Suggestions { query: String },
}

#[derive(Debug, Args)]
pub(crate) struct SearchToolsArgs {
    pub(crate) query: String,
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_search_commands() {
        let cli = Cli::try_parse_from([
            "djinn",
            "search",
            "tools",
            "jira",
            "--root",
            "/tmp/tools",
            "--json",
        ])
        .unwrap();
        let Some(Command::Search(args)) = cli.command else {
            panic!("expected search command");
        };
        let SearchNoun::Tools(args) = args.noun else {
            panic!("expected search tools command");
        };
        assert_eq!(args.query, "jira");
        assert_eq!(args.roots, vec![PathBuf::from("/tmp/tools")]);
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "search", "memories", "rust"]).unwrap();
        let Some(Command::Search(args)) = cli.command else {
            panic!("expected search command");
        };
        let SearchNoun::Memories { query } = args.noun else {
            panic!("expected search memories command");
        };
        assert_eq!(query, "rust");

        let cli = Cli::try_parse_from(["djinn", "search", "suggestions", "docs"]).unwrap();
        let Some(Command::Search(args)) = cli.command else {
            panic!("expected search command");
        };
        let SearchNoun::Suggestions { query } = args.noun else {
            panic!("expected search suggestions command");
        };
        assert_eq!(query, "docs");
    }
}
