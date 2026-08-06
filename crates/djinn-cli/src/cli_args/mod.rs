use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};

mod common;
mod sub_command;
mod top_level;
mod tui;

pub(crate) use common::OutputFormat;
pub(crate) use sub_command::*;
pub(crate) use top_level::*;
pub(crate) use tui::{TuiArgs, TuiView};

#[derive(Debug, Parser)]
#[command(name = "djinn")]
#[command(about = "Local-first companion for OpenCode and other AI coding agents")]
pub(crate) struct Cli {
    /// Deprecated alias for opening the Djinn UI immediately.
    #[arg(short = 'b', long = "buddy")]
    pub(crate) buddy: bool,
    /// Folder-backed session name, path, or Buddy id to open in the Djinn UI.
    #[arg(short = 's', long = "session", value_name = "SESSION")]
    pub(crate) session: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

pub(crate) fn parse_cli() -> Cli {
    Cli::parse()
}

pub(crate) fn print_cli_help() -> std::io::Result<()> {
    Cli::command().print_help()?;
    println!();
    Ok(())
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// List a collection for humans.
    List(ListArgs),
    /// Show detailed information for one item.
    Show(ShowArgs),
    /// Add one item.
    Add(AddArgs),
    /// Accept/complete an item.
    Accept(AcceptArgs),
    /// Reject/remove an item.
    Reject(RejectArgs),
    /// Route memories into suggestions, skills, ideas, or actions.
    Ingest(IngestArgs),
    /// Run an external review to create or activate durable knowledge.
    Review(ReviewArgs),
    /// Remove one item.
    Rm(RmArgs),
    /// Clear a collection after confirmation.
    Clear(ClearArgs),
    /// Discover without writing durable state.
    Scan(ScanArgs),
    /// Write a machine-readable cache/index.
    Index(IndexArgs),
    /// Search a collection.
    Search(SearchArgs),
    /// Switch active context.
    Switch(SwitchArgs),
    /// Open an item in the user's editor.
    Open(OpenArgs),
    /// Inspect Djinn configuration and external harness config adapters.
    Config(ConfigArgs),
    /// Diagnose Djinn runtime integration points.
    Doctor(DoctorArgs),
    /// Manage provider credentials.
    Auth(AuthArgs),
    /// Ask Djinn from a new or existing session without the legacy agent prefix.
    Ask(AgentAskArgs),
    /// Manage folder-backed Djinn work sessions.
    Session(SessionArgs),
    /// Deprecated compatibility shim for old agent-prefixed commands.
    Agent(AgentArgs),
    /// Inspect configured Djinn agent roles.
    Agents(AgentsArgs),
    /// Deprecated alias for opening the Djinn UI.
    Tui(TuiArgs),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_removed_agent_session_command() {
        assert!(Cli::try_parse_from(["djinn", "agent", "session"]).is_err());
        assert!(Cli::try_parse_from([
            "djinn", "agent", "session", "delete", "agt_test", "--force", "--json",
        ])
        .is_err());
        assert!(
            Cli::try_parse_from(["djinn", "agent", "session", "stats", "agt_test", "--json",])
                .is_err()
        );
    }

    #[test]
    fn rejects_removed_archive_command() {
        assert!(Cli::try_parse_from(["djinn", "archive", "list"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "archive", "sessions", "--dry-run"]).is_err());
    }

    #[test]
    fn rejects_removed_saved_row_review_commands() {
        assert!(Cli::try_parse_from(["djinn", "review", "sessions", "--dry-run"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "review", "opencode"]).is_err());
    }

    #[test]
    fn rejects_removed_prune_sessions_command() {
        assert!(
            Cli::try_parse_from(["djinn", "prune", "sessions", "--older-than", "30d",]).is_err()
        );
    }

    #[test]
    fn rejects_removed_legacy_saved_row_commands() {
        assert!(Cli::try_parse_from(["djinn", "add", "session", "./session.md"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "list", "sessions"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "show", "session", "abc"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "search", "sessions", "rust"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "rm", "session", "abc"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "clear", "sessions"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "promote", "sessions"]).is_err());
        assert!(Cli::try_parse_from(["djinn", "promote", "session", "abc"]).is_err());
    }

    #[test]
    fn parses_top_level_buddy_mode_flags() {
        let cli = Cli::try_parse_from(["djinn", "-b"]).unwrap();
        assert!(cli.buddy);
        assert!(cli.session.is_none());
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["djinn", "-b", "-s", "bap-questions"]).unwrap();
        assert!(cli.buddy);
        assert_eq!(cli.session, Some(PathBuf::from("bap-questions")));
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["djinn", "-bs", "bap-questions"]).unwrap();
        assert!(cli.buddy);
        assert_eq!(cli.session, Some(PathBuf::from("bap-questions")));
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["djinn", "-s", "bap-questions"]).unwrap();
        assert!(!cli.buddy);
        assert_eq!(cli.session, Some(PathBuf::from("bap-questions")));
        assert!(cli.command.is_none());
    }
}
