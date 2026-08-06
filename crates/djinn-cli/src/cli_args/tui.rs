use std::path::PathBuf;

use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Args)]
pub(crate) struct TuiArgs {
    /// Legacy Rust TUI view hint. The command now opens the Buddy-first Djinn UI.
    #[arg(value_enum, default_value_t = TuiView::Sessions)]
    pub(crate) view: TuiView,
    /// Legacy Rust TUI tooling root hint. Ignored by the Buddy-first Djinn UI.
    #[arg(long = "root")]
    pub(crate) roots: Vec<PathBuf>,
    /// Legacy Rust TUI editor hint. Ignored by the Buddy-first Djinn UI.
    #[arg(long)]
    pub(crate) editor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TuiView {
    Tools,
    Sessions,
    Memories,
    Suggestions,
    Skills,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_tui_without_view_defaults_to_sessions() {
        let cli = Cli::try_parse_from(["djinn", "tui"]).unwrap();
        let Some(Command::Tui(args)) = cli.command else {
            panic!("expected tui command");
        };

        assert_eq!(args.view, TuiView::Sessions);
    }

    #[test]
    fn parses_tui_sessions_view() {
        let cli = Cli::try_parse_from(["djinn", "tui", "sessions"]).unwrap();
        let Some(Command::Tui(args)) = cli.command else {
            panic!("expected tui command");
        };

        assert_eq!(args.view, TuiView::Sessions);
    }

    #[test]
    fn rejects_removed_tui_workspaces_view() {
        assert!(Cli::try_parse_from(["djinn", "tui", "workspaces"]).is_err());
    }
}
