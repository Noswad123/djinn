use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub(crate) struct ClearArgs {
    #[command(subcommand)]
    pub(crate) noun: ClearNoun,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClearNoun {
    /// Clear all memories after interactive confirmation.
    Memories {
        /// Skip creating memories.backup-*.jsonl before clearing.
        #[arg(long)]
        no_backup: bool,
    },
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_clear_memories_command() {
        let cli = Cli::try_parse_from(["djinn", "clear", "memories", "--no-backup"]).unwrap();

        let Some(Command::Clear(args)) = cli.command else {
            panic!("expected clear command");
        };
        let ClearNoun::Memories { no_backup } = args.noun;

        assert!(no_backup);
    }
}
