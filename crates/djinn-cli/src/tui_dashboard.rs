use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::{
    accept_memory, context_store, create_promotion_session, folder_session_event_health_label,
    format_session_candidate_entry, format_session_candidate_status, list_cache_folder_sessions,
    memory_store, open_skill_entry, open_tool_entry, plural_suffix, remove_memories_silent,
    remove_suggestions, run_folder_session_tui, scan_tools, session_promote_type_label,
    skill_records, suggestion_store, tool_roots, tui_candidate_row, AcceptMemoryArgs,
    SessionPromoteArgs, SessionPromoteType, TuiArgs, TuiView,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum TuiRunOutcome {
    Exit,
    Action(djinn_tui::TuiAction),
}

pub(crate) fn run_tui(args: TuiArgs) -> Result<()> {
    let initial_tab = dashboard_tab(args.view);
    let mut tui = djinn_tui::TuiSession::enter()?;
    let outcome = run_tui_in_session(&mut tui, &args, initial_tab)?;
    tui.finish()?;
    match outcome {
        TuiRunOutcome::Exit => Ok(()),
        TuiRunOutcome::Action(action) => {
            handle_tui_action(action, args.editor)?;
            Ok(())
        }
    }
}

fn run_tui_in_session(
    tui: &mut djinn_tui::TuiSession,
    args: &TuiArgs,
    initial_tab: djinn_tui::DashboardTab,
) -> Result<TuiRunOutcome> {
    let roots = tool_roots(args.roots.clone());
    let tools = scan_tools(&roots)?;
    let sessions = session_records_for_dashboard()?;
    let memories = memory_store().list()?;
    let suggestions = suggestion_store().list()?;
    let skills = skill_records()?;
    let active_context = context_store().active()?;
    let Some(action) = tui.run_dashboard_with_handler(
        tools,
        sessions,
        memories,
        suggestions,
        skills,
        active_context,
        initial_tab,
        |action| match action {
            djinn_tui::TuiAction::DeleteMemories(ids) => remove_memories_silent(&ids).map(|_| ()),
            djinn_tui::TuiAction::DeleteSuggestions(ids) => remove_suggestions(&ids).map(|_| ()),
            djinn_tui::TuiAction::OpenSession(_)
            | djinn_tui::TuiAction::PromoteSessions { .. }
            | djinn_tui::TuiAction::OpenTool(_)
            | djinn_tui::TuiAction::OpenSkill(_)
            | djinn_tui::TuiAction::ReviewMemory(_) => Ok(()),
        },
    )?
    else {
        return Ok(TuiRunOutcome::Exit);
    };

    Ok(TuiRunOutcome::Action(action))
}

fn handle_tui_action(action: djinn_tui::TuiAction, editor: Option<String>) -> Result<bool> {
    match action {
        djinn_tui::TuiAction::OpenSession(session) => {
            run_folder_session_tui(PathBuf::from(session.path), editor).map(|_| false)
        }
        djinn_tui::TuiAction::PromoteSessions {
            promotion_type,
            sessions,
        } => promote_tui_sessions(promotion_type, sessions, editor).map(|_| false),
        djinn_tui::TuiAction::OpenTool(entry) => open_tool_entry(&entry, editor).map(|_| false),
        djinn_tui::TuiAction::OpenSkill(entry) => open_skill_entry(&entry, editor).map(|_| false),
        djinn_tui::TuiAction::ReviewMemory(id) => accept_memory(AcceptMemoryArgs {
            id,
            agent: None,
            title: "djinn memory suggestion review".to_string(),
            opencode_bin: "opencode".to_string(),
            dry_run: false,
        })
        .map(|_| false),
        djinn_tui::TuiAction::DeleteMemories(ids) => remove_memories_silent(&ids).map(|_| false),
        djinn_tui::TuiAction::DeleteSuggestions(ids) => remove_suggestions(&ids).map(|_| false),
    }
}

fn promote_tui_sessions(
    promotion_type: djinn_tui::DashboardPromotionType,
    sessions: Vec<djinn_tui::SessionRecord>,
    editor: Option<String>,
) -> Result<()> {
    if sessions.is_empty() {
        bail!("select at least one session to promote");
    }
    let args = SessionPromoteArgs {
        dirs: sessions
            .iter()
            .map(|session| PathBuf::from(&session.path))
            .collect(),
        promotion_type: session_promote_type_from_dashboard(promotion_type),
        promotion_session_dir: None,
        max_chars_per_artifact: 1200,
        force: false,
        json: false,
    };
    let report = create_promotion_session(&args)?;
    println!(
        "Created {} promotion session from {} selected session{}: {}",
        session_promote_type_label(args.promotion_type),
        report.session_count,
        plural_suffix(report.session_count),
        report.promotion_session_dir
    );
    run_folder_session_tui(PathBuf::from(report.promotion_session_dir), editor)
}

fn session_promote_type_from_dashboard(
    promotion_type: djinn_tui::DashboardPromotionType,
) -> SessionPromoteType {
    match promotion_type {
        djinn_tui::DashboardPromotionType::Memory => SessionPromoteType::Memory,
        djinn_tui::DashboardPromotionType::Todo => SessionPromoteType::Todo,
        djinn_tui::DashboardPromotionType::Skill => SessionPromoteType::Skill,
        djinn_tui::DashboardPromotionType::Pattern => SessionPromoteType::Pattern,
    }
}

fn session_records_for_dashboard() -> Result<Vec<djinn_tui::SessionRecord>> {
    let report = list_cache_folder_sessions(None)?;
    Ok(report
        .sessions
        .into_iter()
        .map(|session| djinn_tui::SessionRecord {
            name: session.display_name,
            reference_name: session.reference_name,
            path: session.path,
            state: session.lifecycle.state,
            mode: session.lifecycle.mode,
            updated_at: session.updated_at.or(session.modified_at),
            repo_path: session.repo_path.or(session.workspace),
            summary_preview: session.summary_preview,
            turn_count: session.turn_count,
            event_health: folder_session_event_health_label(&session.event_health),
            candidate_status: session
                .candidates
                .as_ref()
                .map(format_session_candidate_status),
            candidate_details: session
                .candidates
                .as_ref()
                .map(|candidates| {
                    candidates
                        .entries
                        .iter()
                        .map(format_session_candidate_entry)
                        .collect()
                })
                .unwrap_or_default(),
            candidate_entries: session
                .candidates
                .as_ref()
                .map(|candidates| candidates.entries.iter().map(tui_candidate_row).collect())
                .unwrap_or_default(),
            next_action: session.next_action,
        })
        .collect())
}

pub(crate) fn dashboard_tab(view: TuiView) -> djinn_tui::DashboardTab {
    match view {
        TuiView::Tools => djinn_tui::DashboardTab::Tools,
        TuiView::Sessions => djinn_tui::DashboardTab::Sessions,
        TuiView::Memories => djinn_tui::DashboardTab::Memories,
        TuiView::Suggestions => djinn_tui::DashboardTab::Suggestions,
        TuiView::Skills => djinn_tui::DashboardTab::Skills,
    }
}

pub(crate) fn default_dashboard_tui_args() -> TuiArgs {
    TuiArgs {
        view: TuiView::Sessions,
        roots: Vec::new(),
        editor: None,
    }
}
