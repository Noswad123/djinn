use clap::{Args, Subcommand};

use super::ToolsScope;

#[derive(Debug, Args)]
pub(crate) struct ScanArgs {
    #[command(subcommand)]
    pub(crate) noun: ScanNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ScanNoun {
    /// Scan local tools and print a summary.
    Tools(ToolsScope),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_scan_tools_command() {
        let cli = Cli::try_parse_from(["djinn", "scan", "tools", "--root", "/tmp/tools", "--json"])
            .unwrap();

        let Some(Command::Scan(args)) = cli.command else {
            panic!("expected scan command");
        };
        let ScanNoun::Tools(args) = args.noun;

        assert_eq!(args.roots, vec![PathBuf::from("/tmp/tools")]);
        assert!(args.json);
    }
}
