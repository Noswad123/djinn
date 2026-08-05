use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub(crate) struct SwitchArgs {
    #[command(subcommand)]
    pub(crate) noun: SwitchNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SwitchNoun {
    /// Switch the active context.
    Ctx {
        /// Context name, case-insensitive. Falls back to substring matching.
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_switch_context_command() {
        let cli = Cli::try_parse_from(["djinn", "switch", "ctx", "work"]).unwrap();

        let Some(Command::Switch(args)) = cli.command else {
            panic!("expected switch command");
        };
        let SwitchNoun::Ctx { name } = args.noun;

        assert_eq!(name, "work");
    }
}
