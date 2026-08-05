use std::path::PathBuf;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub(crate) struct IndexArgs {
    #[command(subcommand)]
    pub(crate) noun: IndexNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum IndexNoun {
    /// Write the local tools JSON index.
    Tools(IndexToolsArgs),
}

#[derive(Debug, Args)]
pub(crate) struct IndexToolsArgs {
    /// Local tooling root to scan. Repeatable. Defaults to DJINN_TOOL_ROOTS or ~/.dotfiles.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Index JSON path. Defaults under the scanned root.
    #[arg(long)]
    pub(crate) index: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_index_tools_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "index",
            "tools",
            "--root",
            "/tmp/tools",
            "--index",
            "/tmp/tools-index.json",
        ])
        .unwrap();

        let Some(Command::Index(args)) = cli.command else {
            panic!("expected index command");
        };
        let IndexNoun::Tools(args) = args.noun;

        assert_eq!(args.roots, vec![PathBuf::from("/tmp/tools")]);
        assert_eq!(args.index, Some(PathBuf::from("/tmp/tools-index.json")));
    }
}
