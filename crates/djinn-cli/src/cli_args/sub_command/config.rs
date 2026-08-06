use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(crate) struct ConfigArgs {
    #[command(subcommand)]
    pub(crate) command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Show Djinn's native config, merged from discovered config files.
    Show(ConfigShowArgs),
    /// Diagnose how an external harness config maps into Djinn concepts.
    Doctor(ConfigDoctorArgs),
    /// Preview importing an external harness config into Djinn-native config.
    Import(ConfigImportArgs),
    /// Preview exporting Djinn-native config into an external harness format.
    Export(ConfigExportArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ConfigShowArgs {
    /// Djinn config file path to load. Defaults to discovered native config paths.
    #[arg(long)]
    pub(crate) path: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigImportArgs {
    #[command(subcommand)]
    pub(crate) source: ConfigImportSource,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigExportArgs {
    #[command(subcommand)]
    pub(crate) target: ConfigExportTarget,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigExportTarget {
    /// Export native Djinn config as GitHub Copilot CLI config.
    Copilot(ConfigExportCopilotArgs),
    /// Export native Djinn config as OpenCode config.
    Opencode(ConfigExportOpencodeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ConfigExportCopilotArgs {
    /// Djinn config file path to export. Defaults to discovered native config paths.
    #[arg(long)]
    pub(crate) path: Option<PathBuf>,
    /// Preview the export without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Write the exported Copilot config.
    #[arg(long)]
    pub(crate) write: bool,
    /// Destination Copilot config file. Defaults to ~/.config/github-copilot/config.json.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Allow --write to replace an existing destination file.
    #[arg(long)]
    pub(crate) force: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigExportOpencodeArgs {
    /// Djinn config file path to export. Defaults to discovered native config paths.
    #[arg(long)]
    pub(crate) path: Option<PathBuf>,
    /// Preview the export without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Write the exported OpenCode config.
    #[arg(long)]
    pub(crate) write: bool,
    /// Destination OpenCode config file. Defaults to ~/.config/opencode/opencode.json.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Allow --write to replace an existing destination file.
    #[arg(long)]
    pub(crate) force: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigImportSource {
    /// Import GitHub Copilot CLI config.
    Copilot(ConfigImportCopilotArgs),
    /// Import OpenCode config.
    Opencode(ConfigImportOpencodeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ConfigImportCopilotArgs {
    /// Copilot config file path to inspect. Defaults to discovered GitHub Copilot config paths.
    #[arg(long)]
    pub(crate) path: Option<PathBuf>,
    /// Preview the import without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Write the imported Djinn-native config.
    #[arg(long)]
    pub(crate) write: bool,
    /// Destination Djinn config file. Defaults to ~/.config/djinn/config.json.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Explicitly merge into an existing destination file. This is the default write behavior.
    #[arg(long, requires = "write", conflicts_with = "force")]
    pub(crate) merge: bool,
    /// Allow --write to replace an existing destination file.
    #[arg(long)]
    pub(crate) force: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigImportOpencodeArgs {
    /// OpenCode config file path to inspect. Defaults to Djinn's discovered source paths.
    #[arg(long)]
    pub(crate) path: Option<PathBuf>,
    /// Preview the import without writing files.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Write the imported Djinn-native config.
    #[arg(long)]
    pub(crate) write: bool,
    /// Destination Djinn config file. Defaults to ~/.config/djinn/config.json.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Explicitly merge into an existing destination file. This is the default write behavior.
    #[arg(long, requires = "write", conflicts_with = "force")]
    pub(crate) merge: bool,
    /// Allow --write to replace an existing destination file.
    #[arg(long)]
    pub(crate) force: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigDoctorArgs {
    /// External config source to inspect.
    #[arg(long, value_enum, default_value_t = ConfigSource::Djinn)]
    pub(crate) source: ConfigSource,
    /// Config file path to inspect. Defaults to Djinn's discovered source paths.
    #[arg(long)]
    pub(crate) path: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,
    /// Shortcut for --format json.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ConfigSource {
    Copilot,
    Djinn,
    Opencode,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};
    use crate::config_commands::validate_config_import_mode;

    #[test]
    fn parses_config_doctor_opencode_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "doctor",
            "--source",
            "opencode",
            "--path",
            "/tmp/opencode.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Doctor(args) = args.command else {
            panic!("expected config doctor command");
        };

        assert_eq!(args.source, ConfigSource::Opencode);
        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/opencode.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_doctor_copilot_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "doctor",
            "--source",
            "copilot",
            "--path",
            "/tmp/copilot.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Doctor(args) = args.command else {
            panic!("expected config doctor command");
        };

        assert_eq!(args.source, ConfigSource::Copilot);
        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/copilot.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_show_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "show",
            "--path",
            "/tmp/djinn.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Show(args) = args.command else {
            panic!("expected config show command");
        };

        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_import_opencode_dry_run_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "import",
            "opencode",
            "--dry-run",
            "--path",
            "/tmp/opencode.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Import(args) = args.command else {
            panic!("expected config import command");
        };
        let ConfigImportSource::Opencode(args) = args.source else {
            panic!("expected opencode import source");
        };

        assert!(args.dry_run);
        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/opencode.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_import_copilot_write_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "import",
            "copilot",
            "--write",
            "--output",
            "/tmp/djinn.json",
            "--force",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Import(args) = args.command else {
            panic!("expected config import command");
        };
        let ConfigImportSource::Copilot(args) = args.source else {
            panic!("expected copilot import source");
        };

        assert!(args.write);
        assert!(!args.merge);
        assert!(args.force);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_import_merge_alias_and_rejects_force_conflict() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "import",
            "opencode",
            "--write",
            "--merge",
            "--output",
            "/tmp/djinn.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Import(args) = args.command else {
            panic!("expected config import command");
        };
        let ConfigImportSource::Opencode(args) = args.source else {
            panic!("expected opencode import source");
        };

        assert!(args.write);
        assert!(args.merge);
        assert!(!args.force);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);

        let conflict = Cli::try_parse_from([
            "djinn", "config", "import", "copilot", "--write", "--merge", "--force",
        ]);
        assert!(conflict.is_err());

        assert!(validate_config_import_mode(false, true, true, false).is_ok());
        assert!(validate_config_import_mode(false, true, true, true).is_err());
        assert!(validate_config_import_mode(true, false, true, false).is_err());
    }

    #[test]
    fn parses_config_import_opencode_write_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "import",
            "opencode",
            "--write",
            "--output",
            "/tmp/djinn.json",
            "--force",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Import(args) = args.command else {
            panic!("expected config import command");
        };
        let ConfigImportSource::Opencode(args) = args.source else {
            panic!("expected opencode import source");
        };

        assert!(args.write);
        assert!(!args.merge);
        assert!(args.force);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_export_opencode_dry_run_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "export",
            "opencode",
            "--dry-run",
            "--path",
            "/tmp/djinn.json",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Export(args) = args.command else {
            panic!("expected config export command");
        };
        let ConfigExportTarget::Opencode(args) = args.target else {
            panic!("expected opencode export target");
        };

        assert!(args.dry_run);
        assert_eq!(args.path.as_deref(), Some(Path::new("/tmp/djinn.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_export_copilot_write_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "export",
            "copilot",
            "--write",
            "--output",
            "/tmp/copilot.json",
            "--force",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Export(args) = args.command else {
            panic!("expected config export command");
        };
        let ConfigExportTarget::Copilot(args) = args.target else {
            panic!("expected copilot export target");
        };

        assert!(args.write);
        assert!(args.force);
        assert_eq!(args.output.as_deref(), Some(Path::new("/tmp/copilot.json")));
        assert!(args.json);
    }

    #[test]
    fn parses_config_export_opencode_write_command() {
        let cli = Cli::try_parse_from([
            "djinn",
            "config",
            "export",
            "opencode",
            "--write",
            "--output",
            "/tmp/opencode.json",
            "--force",
            "--json",
        ])
        .unwrap();

        let Some(Command::Config(args)) = cli.command else {
            panic!("expected config command");
        };
        let ConfigCommand::Export(args) = args.command else {
            panic!("expected config export command");
        };
        let ConfigExportTarget::Opencode(args) = args.target else {
            panic!("expected opencode export target");
        };

        assert!(args.write);
        assert!(args.force);
        assert_eq!(
            args.output.as_deref(),
            Some(Path::new("/tmp/opencode.json"))
        );
        assert!(args.json);
    }
}
