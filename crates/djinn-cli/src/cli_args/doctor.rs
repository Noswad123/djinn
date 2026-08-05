use std::path::PathBuf;

use clap::{Args, Subcommand};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(crate) struct DoctorArgs {
    #[command(subcommand)]
    pub(crate) command: DoctorCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DoctorCommand {
    /// Show which Buddy command Djinn will use without launching Buddy.
    Buddy(DoctorBuddyArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DoctorBuddyArgs {
    /// Folder-backed session name or directory whose runtime/buddy.json should be considered.
    #[arg(short = 's', long = "session", value_name = "SESSION")]
    pub(crate) session: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_doctor_buddy_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "doctor",
            "buddy",
            "--session",
            "rebrand-opencode",
            "--json",
        ])
        .unwrap();

        let Some(Command::Doctor(args)) = cli.command else {
            panic!("expected doctor command");
        };
        let DoctorCommand::Buddy(args) = args.command;

        assert_eq!(args.session.as_deref(), Some(Path::new("rebrand-opencode")));
        assert!(args.json);
    }
}
