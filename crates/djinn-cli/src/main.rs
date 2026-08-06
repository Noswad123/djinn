use std::io::{self, IsTerminal};

use anyhow::{bail, Result};

mod agent;
mod auth;
mod cli_args;
mod commands;
mod config;
mod model;
mod permission;
mod policy;
mod promotion;
mod runtime;
mod session;
mod storage;
mod tui;
mod ui;
mod util;

use auth::openai::run_auth;
use cli_args::{parse_cli, print_cli_help, Command};
use commands::agent::{run_agent, run_agents};
use commands::agent_ask::top_level_ask;
use commands::config::run_config;
use commands::doctor::run_doctor;
use commands::session::run_session;
use commands::top_level::{
    run_accept, run_add, run_clear, run_index, run_ingest, run_list, run_open, run_reject,
    run_review, run_rm, run_scan, run_search, run_show, run_switch,
};
use ui::run_top_level_ui_mode;

pub(crate) const DEFAULT_AGENT_MAX_TOOL_ROUNDS: usize = 128;
const BACKGROUND_RUN_UNRESPONSIVE_SECONDS: i64 = 30 * 60;
pub(crate) const FOLDER_SESSION_COMPACT_SNIPPET_CHARS: usize = 1_200;
pub(crate) const FOLDER_SESSION_COMPACT_START_MARKER: &str = "<!-- djinn:generated:start -->";
pub(crate) const FOLDER_SESSION_COMPACT_END_MARKER: &str = "<!-- djinn:generated:end -->";

fn main() -> Result<()> {
    let cli = parse_cli();
    if cli.buddy {
        if cli.command.is_some() {
            bail!("-b/--ui opens the Djinn UI and cannot be combined with a Djinn subcommand");
        }
        return run_top_level_ui_mode(cli.session);
    }
    if let Some(session) = cli.session {
        if cli.command.is_some() {
            bail!("-s/--session opens a focused Djinn UI session and cannot be combined with a Djinn subcommand unless -b/--ui is also set");
        }
        return run_top_level_ui_mode(Some(session));
    }
    let Some(command) = cli.command else {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            return run_top_level_ui_mode(None);
        }
        print_cli_help()?;
        return Ok(());
    };
    match command {
        Command::List(args) => run_list(args),
        Command::Show(args) => run_show(args),
        Command::Add(args) => run_add(args),
        Command::Accept(args) => run_accept(args),
        Command::Reject(args) => run_reject(args),
        Command::Ingest(args) => run_ingest(args),
        Command::Review(args) => run_review(args),
        Command::Rm(args) => run_rm(args),
        Command::Clear(args) => run_clear(args),
        Command::Scan(args) => run_scan(args),
        Command::Index(args) => run_index(args),
        Command::Search(args) => run_search(args),
        Command::Switch(args) => run_switch(args),
        Command::Open(args) => run_open(args),
        Command::Config(args) => run_config(args),
        Command::Doctor(args) => run_doctor(args),
        Command::Auth(args) => run_auth(args),
        Command::Ask(args) => top_level_ask(args),
        Command::Session(args) => run_session(args),
        Command::Agent(args) => run_agent(args),
        Command::Agents(args) => run_agents(args),
        Command::Tui(_args) => run_top_level_ui_mode(None),
    }
}
