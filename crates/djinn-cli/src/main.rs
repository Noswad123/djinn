use std::io::{self, IsTerminal};

use anyhow::{bail, Result};

mod agent;
mod auth;
mod buddy;
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
mod util;

use agent::instructions::ResolvedAgentInstruction;
pub(crate) use agent::roles::{resolve_agent_role_selection_from_config, AgentRoleSelection};
pub(crate) use agent::workspace::{
    clean_unique_paths, load_djinn_config_for_workspace, resolve_agent_workspace,
};
use auth::copilot::*;
use auth::openai::*;
use buddy::*;
pub(crate) use cli_args::*;
pub(crate) use commands::agent::warn_legacy_agent_command;
use commands::agent::{run_agent, run_agents};
pub(crate) use commands::agent_ask::session_run;
use commands::agent_ask::top_level_ask;
use commands::config::run_config;
pub(crate) use commands::context::context_store;
use commands::doctor::run_doctor;
pub(crate) use commands::memory::accept_memory;
pub(crate) use commands::memory::{remove_memories_silent, remove_suggestions};
use commands::session::run_session;
pub(crate) use commands::skills::{open_skill_entry, skill_records, skill_store};
pub(crate) use commands::tools::{open_tool_entry, scan_tools, tool_roots};
use commands::top_level::{
    run_accept, run_add, run_clear, run_index, run_ingest, run_list, run_open, run_reject,
    run_review, run_rm, run_scan, run_search, run_show, run_switch,
};
use config::doctor::*;
use config::model::*;
use config::native::*;
pub(crate) use model::completion::resolve_openai_client;
pub(crate) use model::resolution::*;
pub(crate) use policy::resolution::*;
pub(crate) use promotion::session::{create_promotion_session, session_promote_type_label};
pub(crate) use promotion::validation::SessionValidateCandidateEntry;
pub(crate) use runtime::background_run::latest_background_session_run_status;
use session::context::inspect_folder_session_context_dir;
use session::events::{latest_event_rebuild_backup_path, read_event_turn_pairs};
pub(crate) use session::list::list_folder_sessions_in_root;
pub(crate) use session::list::FolderSessionSummary;
use session::list::{folder_session_event_health_label, list_cache_folder_sessions};
pub(crate) use session::manifest::{
    folder_session_manifest_meta, manifest_root_string_value, parse_manifest_string_value,
    read_folder_session_manifest, session_id_from_session_dir, session_manifest_workspace_path,
    toml_string, write_agent_session_toml, FolderSessionManifest,
};
pub(crate) use session::native::{folder_agent_session_store, load_folder_native_agent_session};
pub(crate) use session::projection::write_folder_session_events_jsonl;
pub(crate) use session::reference::{
    default_folder_session_root, folder_session_display_name, folder_session_reference_name,
    folder_session_slug, is_named_folder_session_reference, resolve_existing_folder_session_dir,
    resolve_existing_folder_session_reference, resolve_session_dir, safe_folder_session_slug,
};
use session::status::{
    folder_session_status, format_session_candidate_entry, format_session_candidate_status,
    latest_promotion_generation_response_path, SessionStatusCandidateEntry,
};
pub(crate) use session::tui::{run_folder_session_tui, tui_candidate_row};
pub(crate) use session::turns::{
    compact_text_snippet, read_folder_session_event_turns, read_folder_session_turns,
    read_optional_markdown_file, FolderSessionTurnDigest,
};
pub(crate) use storage::stores::{
    action_store, agent_session_store, file_history_store, idea_store, memory_store,
    suggestion_store,
};
use tui::dashboard::{default_dashboard_tui_args, run_tui};
pub(crate) use util::path::expand_tilde_path;
pub(crate) use util::text::{
    ensure_trailing_newline, non_empty_string, output_format, plural_suffix, push_unique_string,
    truncate, truncate_table_cell, yes_no,
};

pub(crate) const DEFAULT_AGENT_MAX_TOOL_ROUNDS: usize = 128;
const BACKGROUND_RUN_UNRESPONSIVE_SECONDS: i64 = 30 * 60;
pub(crate) const FOLDER_SESSION_COMPACT_SNIPPET_CHARS: usize = 1_200;
pub(crate) const FOLDER_SESSION_COMPACT_START_MARKER: &str = "<!-- djinn:generated:start -->";
pub(crate) const FOLDER_SESSION_COMPACT_END_MARKER: &str = "<!-- djinn:generated:end -->";

fn main() -> Result<()> {
    let cli = parse_cli();
    if cli.buddy {
        if cli.command.is_some() {
            bail!("-b/--buddy opens Buddy mode and cannot be combined with a Djinn subcommand");
        }
        return run_top_level_buddy_mode(cli.session);
    }
    if let Some(session) = cli.session {
        if cli.command.is_some() {
            bail!("-s/--session opens a focused folder session and cannot be combined with a Djinn subcommand unless -b/--buddy is also set");
        }
        return run_session(SessionArgs {
            command: None,
            dir: Some(session),
            open: false,
            editor: None,
        });
    }
    let Some(command) = cli.command else {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            return run_tui(default_dashboard_tui_args());
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
        Command::Tui(args) => run_tui(args),
    }
}
