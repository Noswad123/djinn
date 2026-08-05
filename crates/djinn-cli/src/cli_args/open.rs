use clap::{Args, Subcommand};

use super::OpenToolArgs;

#[derive(Debug, Args)]
pub(crate) struct OpenArgs {
    #[command(subcommand)]
    pub(crate) noun: OpenNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum OpenNoun {
    /// Open a local tool source by name.
    Tool(OpenToolArgs),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_open_tool_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "open",
            "tool",
            "waystone",
            "--root",
            "/tmp/tools",
            "--editor",
            "nvim",
        ])
        .unwrap();

        let Some(Command::Open(args)) = cli.command else {
            panic!("expected open command");
        };
        let OpenNoun::Tool(args) = args.noun;

        assert_eq!(args.name, "waystone");
        assert_eq!(args.roots, vec![PathBuf::from("/tmp/tools")]);
        assert_eq!(args.editor.as_deref(), Some("nvim"));
    }
}
