use std::io::{self, IsTerminal};

use anyhow::{bail, Result};
use clap::{CommandFactory, Parser};

mod agent_ask_command;
mod agent_commands;
mod agent_config;
mod agent_file_history;
mod agent_instructions;
mod agent_messages;
mod agent_roles;
mod agent_runtime_config;
mod agent_session_meta;
mod agent_workspace;
mod background_run;
mod buddy;
mod buddy_consolidate;
mod cli_args;
mod config_commands;
mod config_doctor;
mod config_format;
mod config_model;
mod config_native;
mod config_preview;
mod config_write;
mod context_commands;
mod copilot_auth;
mod doctor_commands;
mod editor;
mod memory_commands;
mod model_completion;
mod model_resolution;
mod openai_auth;
mod path_util;
mod permission_gate;
mod policy_resolution;
mod promotion_candidate;
mod promotion_cleanup;
mod promotion_decision;
mod promotion_export;
mod promotion_generation;
mod promotion_session;
mod promotion_validation;
mod prompt;
mod session_artifact;
mod session_commands;
mod session_compact;
mod session_context;
mod session_events;
mod session_init;
mod session_list;
mod session_manifest;
mod session_native;
mod session_projection;
mod session_reference;
mod session_registry;
mod session_remove;
mod session_run_support;
mod session_status;
mod session_transcript;
mod session_tui;
mod session_turns;
mod session_watch;
mod shell;
mod skills_commands;
mod stores;
mod text;
mod toml_util;
mod tools_commands;
mod top_level_commands;
mod tui_dashboard;
pub(crate) use agent_ask_command::session_run;
use agent_ask_command::top_level_ask;
pub(crate) use agent_commands::warn_legacy_agent_command;
use agent_commands::{run_agent, run_agents};
use agent_instructions::ResolvedAgentInstruction;
pub(crate) use agent_roles::{resolve_agent_role_selection_from_config, AgentRoleSelection};
pub(crate) use agent_workspace::{
    clean_unique_paths, load_djinn_config_for_workspace, resolve_agent_workspace,
};
pub(crate) use background_run::latest_background_session_run_status;
use buddy::*;
pub(crate) use cli_args::*;
use config_commands::run_config;
use config_doctor::*;
use config_model::*;
use config_native::*;
pub(crate) use context_commands::context_store;
use copilot_auth::*;
use doctor_commands::run_doctor;
pub(crate) use memory_commands::accept_memory;
pub(crate) use memory_commands::{remove_memories_silent, remove_suggestions};
use model_completion::resolve_openai_client;
use model_resolution::*;
use openai_auth::*;
pub(crate) use path_util::expand_tilde_path;
use policy_resolution::*;
pub(crate) use promotion_session::{create_promotion_session, session_promote_type_label};
pub(crate) use promotion_validation::SessionValidateCandidateEntry;
pub(crate) use prompt::prompt_title;
use session_commands::run_session;
use session_context::inspect_folder_session_context_dir;
use session_events::{
    latest_event_rebuild_backup_path, projected_event_turn_id, read_event_turn_pairs,
    validate_folder_session_events,
};
pub(crate) use session_list::list_folder_sessions_in_root;
pub(crate) use session_list::FolderSessionSummary;
use session_list::{folder_session_event_health_label, list_cache_folder_sessions};
pub(crate) use session_manifest::{
    folder_session_manifest_meta, manifest_root_string_value, parse_manifest_string_value,
    read_folder_session_manifest, session_id_from_session_dir, session_manifest_workspace_path,
    toml_string, write_agent_session_toml, FolderSessionManifest,
};
pub(crate) use session_native::{folder_agent_session_store, load_folder_native_agent_session};
pub(crate) use session_projection::write_folder_session_events_jsonl;
pub(crate) use session_reference::{
    default_folder_session_root, folder_session_display_name, folder_session_reference_name,
    folder_session_slug, is_named_folder_session_reference, resolve_existing_folder_session_dir,
    resolve_existing_folder_session_reference, resolve_existing_folder_session_reference_in_root,
    resolve_session_dir, safe_folder_session_slug,
};
use session_status::{
    folder_session_status, format_session_candidate_entry, format_session_candidate_status,
    latest_promotion_generation_response_path, SessionStatusCandidateEntry,
};
pub(crate) use session_tui::{run_folder_session_tui, tui_candidate_row};
pub(crate) use session_turns::{
    compact_text_snippet, read_folder_session_event_turns, read_folder_session_turns,
    read_optional_markdown_file, FolderSessionTurnDigest,
};
pub(crate) use skills_commands::{open_skill_entry, skill_records, skill_store};
pub(crate) use stores::{
    action_store, agent_session_store, file_history_store, idea_store, memory_store,
    suggestion_store,
};
pub(crate) use text::{
    ensure_trailing_newline, non_empty_string, output_format, plural_suffix, push_unique_string,
    truncate, truncate_table_cell, yes_no,
};
pub(crate) use toml_util::upsert_toml_root_string;
pub(crate) use tools_commands::{open_tool_entry, scan_tools, tool_roots};
use top_level_commands::{
    run_accept, run_add, run_clear, run_index, run_ingest, run_list, run_open, run_reject,
    run_review, run_rm, run_scan, run_search, run_show, run_switch,
};
use tui_dashboard::{default_dashboard_tui_args, run_tui};

pub(crate) const DEFAULT_AGENT_MAX_TOOL_ROUNDS: usize = 128;
const BACKGROUND_RUN_UNRESPONSIVE_SECONDS: i64 = 30 * 60;
pub(crate) const FOLDER_SESSION_COMPACT_SNIPPET_CHARS: usize = 1_200;
pub(crate) const FOLDER_SESSION_COMPACT_START_MARKER: &str = "<!-- djinn:generated:start -->";
pub(crate) const FOLDER_SESSION_COMPACT_END_MARKER: &str = "<!-- djinn:generated:end -->";

fn main() -> Result<()> {
    let cli = Cli::parse();
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
        Cli::command().print_help()?;
        println!();
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
