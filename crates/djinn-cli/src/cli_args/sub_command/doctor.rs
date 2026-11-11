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
    /// Inspect the selected Djinn UI command and shared storage capabilities.
    #[command(name = "ui")]
    Ui(DoctorUiArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DoctorUiArgs {
    /// Folder-backed session name or directory whose runtime/djinn.json should be considered.
    #[arg(short = 's', long = "session", value_name = "SESSION")]
    pub(crate) session: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
    /// Also run storage-backed bridge and legacy session-list probes (starts the UI runtime).
    #[arg(long)]
    pub(crate) deep: bool,
    /// Explicitly repair an incompatible recognized source/Homebrew CLI/UI bundle, then verify it.
    #[arg(long, conflicts_with = "deep")]
    pub(crate) repair: bool,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_doctor_ui_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "doctor",
            "ui",
            "--session",
            "rebrand-opencode",
            "--json",
        ])
        .unwrap();

        let Some(Command::Doctor(args)) = cli.command else {
            panic!("expected doctor command");
        };
        let DoctorCommand::Ui(args) = args.command;

        assert_eq!(args.session.as_deref(), Some(Path::new("rebrand-opencode")));
        assert!(args.json);
        assert!(!args.deep);
        assert!(!args.repair);
        assert!(Cli::try_parse_from(["djinn", "doctor", "ui", "--repair"]).is_ok());
        assert!(Cli::try_parse_from(["djinn", "doctor", "ui", "--repair", "--deep"]).is_err());

        assert!(Cli::try_parse_from(["djinn", "doctor", "legacy-ui"]).is_err());
    }
}
