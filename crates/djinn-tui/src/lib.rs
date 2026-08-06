mod approval;
mod filter;
mod grouped_select;
mod keys;
mod style;
mod terminal;

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use djinn_contexts::ContextRecord;
use djinn_memory::{MemoryRecord, SuggestionRecord};
use djinn_skills::SkillRecord;
use djinn_tools::ToolEntry;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use serde_json::Value;

#[cfg(test)]
pub(crate) use approval::ApprovalDialogApp;
pub use approval::{
    approval_preview_file_lines, ApprovalDecision, ApprovalPreviewFile, ApprovalPreviewHunk,
    ApprovalPreviewLine, ApprovalPreviewLineKind, ApprovalPreviewState,
};
use filter::{fuzzy_match, selected_visible_position, FilterState};
use grouped_select::{GroupedSelectItem, GroupedSelectState};
use keys::*;
use style::*;
use terminal::{enter_terminal, leave_terminal, resume_terminal, suspend_terminal, TuiTerminal};

pub struct TuiSession {
    terminal: TuiTerminal,
    active: bool,
}

impl TuiSession {
    pub fn enter() -> Result<Self> {
        Ok(Self {
            terminal: enter_terminal()?,
            active: true,
        })
    }

    pub fn run_dashboard_with_handler<F>(
        &mut self,
        tools: Vec<ToolEntry>,
        sessions: Vec<SessionRecord>,
        memories: Vec<MemoryRecord>,
        suggestions: Vec<SuggestionRecord>,
        skills: Vec<SkillRecord>,
        active_context: Option<ContextRecord>,
        initial_tab: DashboardTab,
        mut on_continue_action: F,
    ) -> Result<Option<TuiAction>>
    where
        F: FnMut(TuiAction) -> Result<()>,
    {
        run_dashboard_loop(
            &mut self.terminal,
            tools,
            sessions,
            memories,
            suggestions,
            skills,
            active_context,
            initial_tab,
            Some(&mut on_continue_action),
        )
    }

    pub fn run_folder_session_status<F>(
        &mut self,
        mut load: F,
    ) -> Result<Option<FolderSessionAction>>
    where
        F: FnMut() -> Result<FolderSessionStatusView>,
    {
        run_folder_session_status_loop(&mut self.terminal, &mut load)
    }

    pub fn finish(mut self) -> Result<()> {
        if self.active {
            leave_terminal(&mut self.terminal)?;
            self.active = false;
        }
        Ok(())
    }

    pub fn suspend(&mut self) -> Result<()> {
        if self.active {
            suspend_terminal(&mut self.terminal)?;
            self.active = false;
        }
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if !self.active {
            resume_terminal(&mut self.terminal)?;
            self.active = true;
        }
        Ok(())
    }
}

impl Drop for TuiSession {
    fn drop(&mut self) {
        if self.active {
            let _ = leave_terminal(&mut self.terminal);
            self.active = false;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSessionStatusView {
    pub title: String,
    pub state: String,
    pub mode: Option<String>,
    pub promotion_type: Option<String>,
    pub session_dir: String,
    pub summary_path: Option<String>,
    pub request_path: Option<String>,
    pub response_path: Option<String>,
    pub turn_count: usize,
    pub event_count: usize,
    pub candidate_status: Option<String>,
    pub candidate_details: Vec<String>,
    pub candidate_entries: Vec<PromotionCandidateRow>,
    pub next_action: Option<String>,
    pub note: Option<String>,
    pub message: Option<String>,
    pub latest_generation_response_path: Option<String>,
    pub latest_run_log_path: Option<String>,
    pub events_path: Option<String>,
    pub latest_event_rebuild_backup_path: Option<String>,
    pub candidates_dir: Option<String>,
    pub source_packet_path: Option<String>,
    pub sources_manifest_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionCandidateRow {
    pub id: String,
    pub candidate_type: Option<String>,
    pub status: String,
    pub path: String,
    pub text: Option<String>,
    pub rationale: Option<String>,
    pub evidence: Vec<String>,
    pub destination: Option<String>,
    pub writeback_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub name: String,
    pub reference_name: String,
    pub path: String,
    pub state: String,
    pub mode: Option<String>,
    pub updated_at: Option<String>,
    pub repo_path: Option<String>,
    pub summary_preview: Option<String>,
    pub turn_count: usize,
    pub event_health: String,
    pub candidate_status: Option<String>,
    pub candidate_details: Vec<String>,
    pub candidate_entries: Vec<PromotionCandidateRow>,
    pub next_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderSessionAction {
    Run,
    Buddy,
    Watch,
    OpenSummary,
    EditRequest,
    OpenContext,
    DiscoverContext,
    ValidateCandidates,
    ValidateCandidate(String),
    ShowValidateEventsCommand,
    ShowEventsCommand,
    ShowEventsWriteCommand,
    ShowEventsRestoreCommand(String),
    AcceptCandidate(String),
    AcceptCandidateAndSyncMindweaver(String),
    DenyCandidate(String),
    OpenCandidate(String),
    OpenPath(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FolderSessionCommand {
    Action(FolderSessionAction),
    OpenHelp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderSessionCommandEntry {
    section: String,
    label: String,
    description: String,
    command: FolderSessionCommand,
}

impl GroupedSelectItem for FolderSessionCommandEntry {
    fn section(&self) -> &str {
        &self.section
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn description(&self) -> &str {
        &self.description
    }
}

fn folder_session_command_entry(
    section: &str,
    label: &str,
    description: &str,
    command: FolderSessionCommand,
) -> FolderSessionCommandEntry {
    FolderSessionCommandEntry {
        section: section.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        command,
    }
}

pub fn run_tools(tools: Vec<ToolEntry>) -> Result<()> {
    let mut terminal = enter_terminal()?;
    let result = run_tools_loop(&mut terminal, tools);
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_dashboard(
    tools: Vec<ToolEntry>,
    sessions: Vec<SessionRecord>,
    memories: Vec<MemoryRecord>,
    suggestions: Vec<SuggestionRecord>,
    skills: Vec<SkillRecord>,
    active_context: Option<ContextRecord>,
    initial_tab: DashboardTab,
) -> Result<Option<TuiAction>> {
    let mut terminal = enter_terminal()?;
    let result = run_dashboard_loop(
        &mut terminal,
        tools,
        sessions,
        memories,
        suggestions,
        skills,
        active_context,
        initial_tab,
        None,
    );
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_dashboard_with_handler<F>(
    tools: Vec<ToolEntry>,
    sessions: Vec<SessionRecord>,
    memories: Vec<MemoryRecord>,
    suggestions: Vec<SuggestionRecord>,
    skills: Vec<SkillRecord>,
    active_context: Option<ContextRecord>,
    initial_tab: DashboardTab,
    mut on_continue_action: F,
) -> Result<Option<TuiAction>>
where
    F: FnMut(TuiAction) -> Result<()>,
{
    let mut terminal = enter_terminal()?;
    let result = run_dashboard_loop(
        &mut terminal,
        tools,
        sessions,
        memories,
        suggestions,
        skills,
        active_context,
        initial_tab,
        Some(&mut on_continue_action),
    );
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_approval_dialog(metadata: Value) -> Result<ApprovalDecision> {
    let mut terminal = enter_terminal()?;
    let result = approval::run_approval_dialog_loop(&mut terminal, metadata);
    leave_terminal(&mut terminal)?;
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiAction {
    OpenSession(SessionRecord),
    PromoteSessions {
        promotion_type: DashboardPromotionType,
        sessions: Vec<SessionRecord>,
    },
    OpenTool(ToolEntry),
    OpenSkill(SkillRecord),
    ReviewMemory(String),
    DeleteMemories(Vec<String>),
    DeleteSuggestions(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardPromotionType {
    Memory,
    Todo,
    Skill,
    Pattern,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DashboardCommandEntry {
    section: String,
    label: String,
    description: String,
    command: DashboardCommand,
}

impl GroupedSelectItem for DashboardCommandEntry {
    fn section(&self) -> &str {
        &self.section
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn description(&self) -> &str {
        &self.description
    }
}

fn dashboard_command_entry(
    section: &str,
    label: &str,
    description: &str,
    command: DashboardCommand,
) -> DashboardCommandEntry {
    DashboardCommandEntry {
        section: section.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        command,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardCommand {
    OpenTab(DashboardTab),
    OpenHelp,
    ToggleFilter,
    OpenSelected,
    PromoteSelectedSessions(DashboardPromotionType),
    ToggleSelected,
    ToggleAll,
    AcceptSelected,
    RejectSelected,
    DeleteSelected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardTab {
    Tools,
    Sessions,
    Memories,
    Suggestions,
    Skills,
}

impl DashboardTab {
    fn index(self) -> usize {
        match self {
            DashboardTab::Tools => 0,
            DashboardTab::Sessions => 1,
            DashboardTab::Memories => 2,
            DashboardTab::Suggestions => 3,
            DashboardTab::Skills => 4,
        }
    }

    fn from_index(index: usize) -> Self {
        match index % DASHBOARD_TABS.len() {
            0 => DashboardTab::Tools,
            1 => DashboardTab::Sessions,
            2 => DashboardTab::Memories,
            3 => DashboardTab::Suggestions,
            _ => DashboardTab::Skills,
        }
    }
}

const DASHBOARD_TABS: [&str; 5] = ["Tools", "Sessions", "Memories", "Suggestions", "Skills"];

fn run_tools_loop(terminal: &mut TuiTerminal, tools: Vec<ToolEntry>) -> Result<()> {
    let mut app = ToolsApp::new(tools);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if !actionable_key_event(&key) {
                    continue;
                }
                if app.filter.editing {
                    match key.code {
                        KeyCode::Char('/') => app.toggle_filter(),
                        KeyCode::Backspace => app.filter_backspace(),
                        KeyCode::Enter | KeyCode::Esc => app.filter.editing = false,
                        KeyCode::Char(ch) => app.filter_push(ch),
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('/') => app.toggle_filter(),
                    KeyCode::Char('j') | KeyCode::Down => app.next(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous(),
                    KeyCode::Char('d') | KeyCode::PageDown => app.scroll_down(),
                    KeyCode::Char('u') | KeyCode::PageUp => app.scroll_up(),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn run_folder_session_status_loop<F>(
    terminal: &mut TuiTerminal,
    load: &mut F,
) -> Result<Option<FolderSessionAction>>
where
    F: FnMut() -> Result<FolderSessionStatusView>,
{
    let mut selected_candidate = 0usize;
    let mut palette = GroupedSelectState::default();
    let mut help_open = false;
    loop {
        let view = load()?;
        if selected_candidate >= view.candidate_entries.len() {
            selected_candidate = view.candidate_entries.len().saturating_sub(1);
        }
        terminal.draw(|frame| {
            draw_folder_session_status(frame, &view, selected_candidate, &mut palette, help_open)
        })?;
        if event::poll(Duration::from_millis(1000))? {
            if let Event::Key(key) = event::read()? {
                if !actionable_key_event(&key) {
                    continue;
                }
                if help_open {
                    match key.code {
                        _ if dashboard_help_key(key.code, key.modifiers) => help_open = false,
                        KeyCode::Esc | KeyCode::Enter => help_open = false,
                        KeyCode::Char('q') => return Ok(None),
                        _ => {}
                    }
                    continue;
                }

                if palette.open {
                    match key.code {
                        KeyCode::Esc => palette.close(),
                        KeyCode::Backspace => {
                            palette.backspace_query_or_close();
                            normalize_folder_session_palette_selection(
                                &mut palette,
                                &view,
                                selected_candidate,
                            );
                        }
                        _ if palette_next_key(key.code, key.modifiers) => {
                            let visible = visible_folder_session_palette_indices(
                                &view,
                                selected_candidate,
                                &palette,
                            );
                            palette.next(&visible);
                        }
                        _ if palette_previous_key(key.code, key.modifiers) => {
                            let visible = visible_folder_session_palette_indices(
                                &view,
                                selected_candidate,
                                &palette,
                            );
                            palette.previous(&visible);
                        }
                        KeyCode::Enter => {
                            if let Some(command) = selected_folder_session_palette_command(
                                &view,
                                selected_candidate,
                                &palette,
                            ) {
                                palette.close();
                                match command {
                                    FolderSessionCommand::Action(action) => {
                                        return Ok(Some(action))
                                    }
                                    FolderSessionCommand::OpenHelp => help_open = true,
                                }
                            } else {
                                palette.close();
                            }
                        }
                        KeyCode::Char(ch) if palette_text_key(key.modifiers) => {
                            palette.push_query(ch);
                            normalize_folder_session_palette_selection(
                                &mut palette,
                                &view,
                                selected_candidate,
                            );
                        }
                        _ => {}
                    }
                    continue;
                }

                if dashboard_palette_key(key.code, key.modifiers) {
                    palette.open();
                    normalize_folder_session_palette_selection(
                        &mut palette,
                        &view,
                        selected_candidate,
                    );
                    continue;
                }

                if dashboard_help_key(key.code, key.modifiers) {
                    help_open = true;
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                    KeyCode::Char('j') | KeyCode::Down if !view.candidate_entries.is_empty() => {
                        selected_candidate =
                            (selected_candidate + 1) % view.candidate_entries.len();
                    }
                    KeyCode::Char('k') | KeyCode::Up if !view.candidate_entries.is_empty() => {
                        selected_candidate = if selected_candidate == 0 {
                            view.candidate_entries.len().saturating_sub(1)
                        } else {
                            selected_candidate.saturating_sub(1)
                        };
                    }
                    code => {
                        if let Some(action) =
                            folder_session_candidate_action_for_key(code, &view, selected_candidate)
                        {
                            return Ok(Some(action));
                        }
                        if let Some(action) = folder_session_action_for_key(code) {
                            return Ok(Some(action));
                        }
                    }
                }
            }
        }
    }
}

fn folder_session_action_for_key(code: KeyCode) -> Option<FolderSessionAction> {
    match code {
        KeyCode::Char('r') => Some(FolderSessionAction::Run),
        KeyCode::Char('b') => Some(FolderSessionAction::Buddy),
        KeyCode::Char('w') => Some(FolderSessionAction::Watch),
        KeyCode::Char('o') => Some(FolderSessionAction::OpenSummary),
        KeyCode::Char('e') => Some(FolderSessionAction::EditRequest),
        KeyCode::Char('c') => Some(FolderSessionAction::OpenContext),
        KeyCode::Char('d') => Some(FolderSessionAction::DiscoverContext),
        _ => None,
    }
}

fn folder_session_command_palette(
    view: &FolderSessionStatusView,
    selected_candidate: usize,
) -> Vec<FolderSessionCommandEntry> {
    let mut entries = vec![
        folder_session_command_entry(
            "Session",
            "Run session",
            "Run request.md for this folder-backed session",
            FolderSessionCommand::Action(FolderSessionAction::Run),
        ),
        folder_session_command_entry(
            "Session",
            "Open Djinn UI chat",
            "Launch Buddy with request.md on stdin and capture the final response",
            FolderSessionCommand::Action(FolderSessionAction::Buddy),
        ),
        folder_session_command_entry(
            "Session",
            "Watch session",
            "Poll this session until it is no longer running",
            FolderSessionCommand::Action(FolderSessionAction::Watch),
        ),
        folder_session_command_entry(
            "Artifacts",
            "Open summary",
            "Open summary.md in your editor",
            FolderSessionCommand::Action(FolderSessionAction::OpenSummary),
        ),
        folder_session_command_entry(
            "Artifacts",
            "Edit request",
            "Open request.md in your editor",
            FolderSessionCommand::Action(FolderSessionAction::EditRequest),
        ),
        folder_session_command_entry(
            "Artifacts",
            "Open context",
            "Open the session context directory",
            FolderSessionCommand::Action(FolderSessionAction::OpenContext),
        ),
        folder_session_command_entry(
            "Artifacts",
            "Discover context",
            "Link high-signal project context into this session",
            FolderSessionCommand::Action(FolderSessionAction::DiscoverContext),
        ),
        folder_session_command_entry(
            "Help",
            "Show keybindings",
            "Show focused session keybindings",
            FolderSessionCommand::OpenHelp,
        ),
        folder_session_command_entry(
            "Event ledger",
            "Show validate-events command",
            "Show the read-only command that checks events.jsonl against turns/",
            FolderSessionCommand::Action(FolderSessionAction::ShowValidateEventsCommand),
        ),
        folder_session_command_entry(
            "Event ledger",
            "Show events preview command",
            "Show the read-only command that previews turns projected from events.jsonl",
            FolderSessionCommand::Action(FolderSessionAction::ShowEventsCommand),
        ),
        folder_session_command_entry(
            "Event ledger",
            "Show events rebuild command",
            "Show the explicit --write command for rebuilding turns/ from events.jsonl",
            FolderSessionCommand::Action(FolderSessionAction::ShowEventsWriteCommand),
        ),
    ];
    if let Some(path) = &view.latest_event_rebuild_backup_path {
        entries.push(folder_session_command_entry(
            "Event ledger",
            "Show latest events restore command",
            "Show the explicit command for restoring the latest event rebuild backup",
            FolderSessionCommand::Action(FolderSessionAction::ShowEventsRestoreCommand(
                path.clone(),
            )),
        ));
    }
    if view.mode.as_deref() == Some("promotion") {
        entries.push(folder_session_command_entry(
            "Promotion",
            "Validate all candidates",
            "Check candidate TOML without accepting or rerunning the model",
            FolderSessionCommand::Action(FolderSessionAction::ValidateCandidates),
        ));
    }
    if let Some(path) = &view.latest_generation_response_path {
        entries.push(folder_session_command_entry(
            "Artifacts",
            "Open latest generation response",
            "Inspect the most recent model response for this promotion session",
            FolderSessionCommand::Action(FolderSessionAction::OpenPath(path.clone())),
        ));
    }
    if let Some(path) = &view.latest_run_log_path {
        entries.push(folder_session_command_entry(
            "Artifacts",
            "Open latest run log",
            "Inspect the latest background run log",
            FolderSessionCommand::Action(FolderSessionAction::OpenPath(path.clone())),
        ));
    }
    if let Some(path) = &view.events_path {
        entries.push(folder_session_command_entry(
            "Artifacts",
            "Open events ledger",
            "Inspect events.jsonl",
            FolderSessionCommand::Action(FolderSessionAction::OpenPath(path.clone())),
        ));
    }
    if let Some(path) = &view.candidates_dir {
        entries.push(folder_session_command_entry(
            "Artifacts",
            "Open candidates directory",
            "Browse generated candidate TOML files",
            FolderSessionCommand::Action(FolderSessionAction::OpenPath(path.clone())),
        ));
    }
    if let Some(path) = &view.source_packet_path {
        entries.push(folder_session_command_entry(
            "Artifacts",
            "Open source packet",
            "Inspect the evidence packet sent to the model",
            FolderSessionCommand::Action(FolderSessionAction::OpenPath(path.clone())),
        ));
    }
    if let Some(path) = &view.sources_manifest_path {
        entries.push(folder_session_command_entry(
            "Artifacts",
            "Open sources manifest",
            "Inspect source session refs and selected artifacts",
            FolderSessionCommand::Action(FolderSessionAction::OpenPath(path.clone())),
        ));
    }
    if let Some(candidate) = view.candidate_entries.get(selected_candidate) {
        entries.extend([
            folder_session_command_entry(
                "Candidate",
                "Validate selected candidate",
                &format!("Check {} without accepting it", candidate.id),
                FolderSessionCommand::Action(FolderSessionAction::ValidateCandidate(
                    candidate.id.clone(),
                )),
            ),
            folder_session_command_entry(
                "Candidate",
                "Accept selected candidate",
                &format!("Accept {} through the canonical CLI path", candidate.id),
                FolderSessionCommand::Action(FolderSessionAction::AcceptCandidate(
                    candidate.id.clone(),
                )),
            ),
            folder_session_command_entry(
                "Candidate",
                "Accept selected candidate and sync MindWeaver",
                &format!("Accept {} and run mw todos sync", candidate.id),
                FolderSessionCommand::Action(
                    FolderSessionAction::AcceptCandidateAndSyncMindweaver(candidate.id.clone()),
                ),
            ),
            folder_session_command_entry(
                "Candidate",
                "Deny selected candidate",
                &format!("Deny {} through the canonical CLI path", candidate.id),
                FolderSessionCommand::Action(FolderSessionAction::DenyCandidate(
                    candidate.id.clone(),
                )),
            ),
            folder_session_command_entry(
                "Candidate",
                "Open selected candidate file",
                &format!("Open {}", candidate.path),
                FolderSessionCommand::Action(FolderSessionAction::OpenCandidate(
                    candidate.path.clone(),
                )),
            ),
        ]);
    }
    entries
}

fn visible_folder_session_palette_indices(
    view: &FolderSessionStatusView,
    selected_candidate: usize,
    palette: &GroupedSelectState,
) -> Vec<usize> {
    grouped_select::visible_indices(
        &folder_session_command_palette(view, selected_candidate),
        &palette.query,
    )
}

fn normalize_folder_session_palette_selection(
    palette: &mut GroupedSelectState,
    view: &FolderSessionStatusView,
    selected_candidate: usize,
) {
    let visible = visible_folder_session_palette_indices(view, selected_candidate, palette);
    palette.normalize_selection(&visible);
}

fn selected_folder_session_palette_command(
    view: &FolderSessionStatusView,
    selected_candidate: usize,
    palette: &GroupedSelectState,
) -> Option<FolderSessionCommand> {
    let entries = folder_session_command_palette(view, selected_candidate);
    let visible = grouped_select::visible_indices(&entries, &palette.query);
    grouped_select::selected_item(&entries, &visible, palette.selected, |entry| {
        entry.command.clone()
    })
}

fn folder_session_candidate_action_for_key(
    code: KeyCode,
    view: &FolderSessionStatusView,
    selected_candidate: usize,
) -> Option<FolderSessionAction> {
    let candidate = view.candidate_entries.get(selected_candidate)?;
    match code {
        KeyCode::Char('a') => Some(FolderSessionAction::AcceptCandidate(candidate.id.clone())),
        KeyCode::Char('m') => Some(FolderSessionAction::AcceptCandidateAndSyncMindweaver(
            candidate.id.clone(),
        )),
        KeyCode::Char('x') => Some(FolderSessionAction::DenyCandidate(candidate.id.clone())),
        KeyCode::Char('p') | KeyCode::Enter => {
            Some(FolderSessionAction::OpenCandidate(candidate.path.clone()))
        }
        _ => None,
    }
}

fn draw_folder_session_status(
    frame: &mut ratatui::Frame<'_>,
    view: &FolderSessionStatusView,
    selected_candidate: usize,
    palette: &mut GroupedSelectState,
    help_open: bool,
) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(area);
    let mode = view
        .mode
        .as_deref()
        .map(|mode| format!(" ({mode})"))
        .unwrap_or_default();
    let title = Paragraph::new(vec![Line::from(vec![
        Span::styled("Djinn session ", dim_style()),
        Span::styled(
            view.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(format!("{}{}", view.state, mode), status_style(&view.state)),
    ])])
    .block(block(" Session "));
    frame.render_widget(title, chunks[0]);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Folder: ", dim_style()),
            Span::raw(&view.session_dir),
        ]),
        Line::from(vec![
            Span::styled("Turns:  ", dim_style()),
            Span::raw(view.turn_count.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Events: ", dim_style()),
            Span::raw(view.event_count.to_string()),
        ]),
    ];
    if let Some(path) = &view.request_path {
        lines.push(Line::from(vec![
            Span::styled("Request: ", dim_style()),
            Span::raw(path),
        ]));
    }
    if let Some(path) = &view.summary_path {
        lines.push(Line::from(vec![
            Span::styled("Summary: ", dim_style()),
            Span::raw(path),
        ]));
    }
    if let Some(path) = &view.response_path {
        lines.push(Line::from(vec![
            Span::styled("Response:", dim_style()),
            Span::raw(format!(" {path}")),
        ]));
    }
    if let Some(note) = &view.note {
        lines.push(Line::from(vec![
            Span::styled("Note:    ", dim_style()),
            Span::raw(note),
        ]));
    }
    if let Some(message) = &view.message {
        lines.push(Line::from(vec![
            Span::styled("Status:  ", dim_style()),
            Span::raw(message),
        ]));
    }
    if let Some(candidates) = &view.candidate_status {
        lines.push(Line::from(vec![
            Span::styled("Candidates:", dim_style()),
            Span::raw(format!(" {candidates}")),
        ]));
        if view.candidate_entries.is_empty() {
            for detail in view.candidate_details.iter().take(4) {
                lines.push(Line::from(vec![
                    Span::styled("  - ", dim_style()),
                    Span::raw(detail.clone()),
                ]));
            }
        } else {
            for (idx, candidate) in view.candidate_entries.iter().take(4).enumerate() {
                let marker = if idx == selected_candidate {
                    "> "
                } else {
                    "  "
                };
                lines.push(Line::from(vec![
                    Span::styled(marker, dim_style()),
                    Span::raw(format_promotion_candidate_row(candidate)),
                ]));
            }
            if let Some(candidate) = view.candidate_entries.get(selected_candidate) {
                lines.push(Line::from(""));
                lines.extend(selected_promotion_candidate_detail_lines(candidate));
            }
        }
    }
    if let Some(next) = &view.next_action {
        lines.push(Line::from(vec![
            Span::styled("Next:    ", dim_style()),
            Span::raw(next),
        ]));
    }
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(block(" Artifacts "));
    frame.render_widget(body, chunks[1]);

    let footer =
        Paragraph::new("r run · o summary · Ctrl+P commands · q/Esc quit").style(dim_style());
    frame.render_widget(footer, chunks[2]);

    if help_open {
        draw_folder_session_help(frame);
    }
    if palette.open {
        draw_folder_session_palette(frame, view, selected_candidate, palette);
    }
}

fn draw_folder_session_palette(
    frame: &mut ratatui::Frame<'_>,
    view: &FolderSessionStatusView,
    selected_candidate: usize,
    palette: &mut GroupedSelectState,
) {
    let entries = folder_session_command_palette(view, selected_candidate);
    let visible = grouped_select::visible_indices(&entries, &palette.query);
    let (body_lines, selected_row) =
        grouped_select::body_lines_and_selected_row(&entries, &visible, palette.selected);
    let area = centered_rect(68, 50, frame.area());
    let inner = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);
    palette.ensure_selection_visible(chunks[2].height as usize, selected_row, body_lines.len());
    let search_line = Line::from(vec![
        Span::styled("Search: ", dim_style()),
        if palette.query.is_empty() {
            Span::styled("find action…", dim_style())
        } else {
            Span::raw(palette.query.clone())
        },
    ]);
    let body = Paragraph::new(body_lines)
        .style(base_style())
        .scroll((palette.scroll.min(u16::MAX as usize) as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, area);
    frame.render_widget(block("Command palette"), area);
    frame.render_widget(Paragraph::new(search_line).style(base_style()), chunks[0]);
    frame.render_widget(body, chunks[2]);
    let cursor_x = area
        .x
        .saturating_add(1)
        .saturating_add("Search: ".len() as u16)
        .saturating_add(palette.query.chars().count() as u16)
        .min(area.right().saturating_sub(2));
    frame.set_cursor_position(Position::new(cursor_x, area.y.saturating_add(1)));
}

fn draw_folder_session_help(frame: &mut ratatui::Frame<'_>) {
    let area = centered_rect(68, 62, frame.area());
    let lines = vec![
        Line::from(Span::styled("Focused session", title_style())),
        Line::from(""),
        Line::from(Span::styled("Global", title_style())),
        Line::from(vec![
            Span::styled("Ctrl+P", selected_style()),
            Span::raw(" open command palette"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+/", selected_style()),
            Span::raw(" open or close this help"),
        ]),
        Line::from(vec![
            Span::styled("q / Esc", selected_style()),
            Span::raw(" quit focused session view"),
        ]),
        Line::from(""),
        Line::from(Span::styled("Session", title_style())),
        Line::from(vec![
            Span::styled("r", selected_style()),
            Span::raw(" run request.md"),
        ]),
        Line::from(vec![
            Span::styled("w", selected_style()),
            Span::raw(" watch session status"),
        ]),
        Line::from(vec![
            Span::styled("o", selected_style()),
            Span::raw(" open summary.md"),
        ]),
        Line::from(vec![
            Span::styled("e", selected_style()),
            Span::raw(" edit request.md"),
        ]),
        Line::from(vec![
            Span::styled("c / d", selected_style()),
            Span::raw(" open or discover context"),
        ]),
        Line::from(""),
        Line::from(Span::styled("Promotion candidates", title_style())),
        Line::from(vec![
            Span::styled("↑/↓ or j/k", selected_style()),
            Span::raw(" move selected candidate"),
        ]),
        Line::from(vec![
            Span::styled("a", selected_style()),
            Span::raw(" accept selected candidate"),
        ]),
        Line::from(vec![
            Span::styled("m", selected_style()),
            Span::raw(" accept selected candidate and run MindWeaver sync"),
        ]),
        Line::from(vec![
            Span::styled("x", selected_style()),
            Span::raw(" deny selected candidate"),
        ]),
        Line::from(vec![
            Span::styled("p / Enter", selected_style()),
            Span::raw(" open selected candidate file"),
        ]),
    ];
    let help = Paragraph::new(lines)
        .block(block("Keybindings"))
        .style(base_style())
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, area);
    frame.render_widget(help, area);
}

fn status_style(state: &str) -> Style {
    match state {
        "running" => info_style(),
        "completed" => success_style(),
        "failed" => error_style(),
        "cancelled" => warning_style(),
        _ => base_style(),
    }
}

fn run_dashboard_loop(
    terminal: &mut TuiTerminal,
    tools: Vec<ToolEntry>,
    sessions: Vec<SessionRecord>,
    memories: Vec<MemoryRecord>,
    suggestions: Vec<SuggestionRecord>,
    skills: Vec<SkillRecord>,
    active_context: Option<ContextRecord>,
    initial_tab: DashboardTab,
    mut on_continue_action: Option<&mut dyn FnMut(TuiAction) -> Result<()>>,
) -> Result<Option<TuiAction>> {
    let mut app = DashboardApp::new(
        tools,
        sessions,
        memories,
        suggestions,
        skills,
        active_context,
        initial_tab,
    );
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if !actionable_key_event(&key) {
                    continue;
                }
                if app.help_open {
                    match key.code {
                        _ if dashboard_help_key(key.code, key.modifiers) => app.close_help(),
                        KeyCode::Esc | KeyCode::Enter => app.close_help(),
                        KeyCode::Char('q') => return Ok(None),
                        _ => {}
                    }
                    continue;
                }

                if app.palette.open {
                    match key.code {
                        KeyCode::Esc => app.close_palette(),
                        KeyCode::Backspace => app.backspace_palette_query_or_close(),
                        _ if palette_next_key(key.code, key.modifiers) => app.next_palette_item(),
                        _ if palette_previous_key(key.code, key.modifiers) => {
                            app.previous_palette_item()
                        }
                        KeyCode::Enter => {
                            if let Some(command) = app.selected_palette_command() {
                                app.close_palette();
                                if let Some(action) = handle_dashboard_command(
                                    &mut app,
                                    &mut on_continue_action,
                                    command,
                                )? {
                                    return Ok(Some(action));
                                }
                            } else {
                                app.close_palette();
                            }
                        }
                        KeyCode::Char(ch) if palette_text_key(key.modifiers) => {
                            app.push_palette_query(ch)
                        }
                        _ => {}
                    }
                    continue;
                }

                if dashboard_palette_key(key.code, key.modifiers) {
                    app.open_palette();
                    continue;
                }

                if dashboard_help_key(key.code, key.modifiers) {
                    app.open_help();
                    continue;
                }

                if app.filter_editing() {
                    match key.code {
                        KeyCode::Char('/') => app.toggle_filter(),
                        KeyCode::Backspace => app.filter_backspace(),
                        KeyCode::Enter | KeyCode::Esc => app.finish_filter_edit(),
                        KeyCode::Char(ch) => app.filter_push(ch),
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                    KeyCode::Char('/') => app.toggle_filter(),
                    KeyCode::Tab => app.next_tab(),
                    KeyCode::BackTab => app.previous_tab(),
                    KeyCode::Char('j') | KeyCode::Down => app.next_item(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous_item(),
                    KeyCode::Char('d') | KeyCode::PageDown => app.scroll_down(),
                    KeyCode::Char('u') | KeyCode::PageUp => app.scroll_up(),
                    KeyCode::Char(' ') => app.toggle_selected(),
                    KeyCode::Char('a') => {
                        if app.active_tab == DashboardTab::Memories {
                            if let Some(id) = app.memories.selected_memory_id() {
                                return Ok(Some(TuiAction::ReviewMemory(id)));
                            }
                        } else {
                            app.toggle_all();
                        }
                    }
                    KeyCode::Char('A') => app.toggle_all(),
                    KeyCode::Enter => match app.active_tab {
                        DashboardTab::Tools => {
                            if let Some(tool) = app.tools.selected_tool().cloned() {
                                return Ok(Some(TuiAction::OpenTool(tool)));
                            }
                        }
                        DashboardTab::Sessions => {
                            if let Some(session) = app.sessions.selected_session().cloned() {
                                return Ok(Some(TuiAction::OpenSession(session)));
                            }
                        }
                        DashboardTab::Skills => {
                            if let Some(skill) = app.skills.selected_skill().cloned() {
                                return Ok(Some(TuiAction::OpenSkill(skill)));
                            }
                        }
                        DashboardTab::Memories | DashboardTab::Suggestions => {}
                    },
                    KeyCode::Char('r') => {
                        if app.active_tab == DashboardTab::Memories {
                            let ids = app.memories.selected_memory_ids();
                            if !ids.is_empty() {
                                let action = TuiAction::DeleteMemories(ids);
                                if handle_continue_action(
                                    &mut app,
                                    &mut on_continue_action,
                                    action.clone(),
                                )? {
                                    continue;
                                }
                                return Ok(Some(action));
                            }
                        }
                    }
                    KeyCode::Char('x') | KeyCode::Delete => match app.active_tab {
                        DashboardTab::Memories => {
                            let ids = app.memories.selected_memory_ids();
                            if !ids.is_empty() {
                                let action = TuiAction::DeleteMemories(ids);
                                if handle_continue_action(
                                    &mut app,
                                    &mut on_continue_action,
                                    action.clone(),
                                )? {
                                    continue;
                                }
                                return Ok(Some(action));
                            }
                        }
                        DashboardTab::Suggestions => {
                            let ids = app.suggestions.selected_suggestion_ids();
                            if !ids.is_empty() {
                                let action = TuiAction::DeleteSuggestions(ids);
                                if handle_continue_action(
                                    &mut app,
                                    &mut on_continue_action,
                                    action.clone(),
                                )? {
                                    continue;
                                }
                                return Ok(Some(action));
                            }
                        }
                        DashboardTab::Tools | DashboardTab::Sessions | DashboardTab::Skills => {}
                    },
                    _ => {}
                }
            }
        }
    }
}

fn handle_continue_action(
    app: &mut DashboardApp,
    on_continue_action: &mut Option<&mut dyn FnMut(TuiAction) -> Result<()>>,
    action: TuiAction,
) -> Result<bool> {
    let Some(handler) = on_continue_action.as_deref_mut() else {
        return Ok(false);
    };
    handler(action.clone())?;
    app.apply_completed_action(&action);
    Ok(true)
}

fn handle_dashboard_command(
    app: &mut DashboardApp,
    on_continue_action: &mut Option<&mut dyn FnMut(TuiAction) -> Result<()>>,
    command: DashboardCommand,
) -> Result<Option<TuiAction>> {
    match command {
        DashboardCommand::OpenTab(tab) => app.active_tab = tab,
        DashboardCommand::OpenHelp => app.open_help(),
        DashboardCommand::ToggleFilter => app.toggle_filter(),
        DashboardCommand::OpenSelected => match app.active_tab {
            DashboardTab::Tools => {
                if let Some(tool) = app.tools.selected_tool().cloned() {
                    return Ok(Some(TuiAction::OpenTool(tool)));
                }
            }
            DashboardTab::Sessions => {
                if let Some(session) = app.sessions.selected_session().cloned() {
                    return Ok(Some(TuiAction::OpenSession(session)));
                }
            }
            DashboardTab::Skills => {
                if let Some(skill) = app.skills.selected_skill().cloned() {
                    return Ok(Some(TuiAction::OpenSkill(skill)));
                }
            }
            DashboardTab::Memories | DashboardTab::Suggestions => {}
        },
        DashboardCommand::PromoteSelectedSessions(promotion_type) => {
            let sessions = app.sessions.selected_sessions();
            if !sessions.is_empty() {
                return Ok(Some(TuiAction::PromoteSessions {
                    promotion_type,
                    sessions,
                }));
            }
        }
        DashboardCommand::ToggleSelected => app.toggle_selected(),
        DashboardCommand::ToggleAll => app.toggle_all(),
        DashboardCommand::AcceptSelected => {
            if let Some(id) = app.memories.selected_memory_id() {
                return Ok(Some(TuiAction::ReviewMemory(id)));
            }
        }
        DashboardCommand::RejectSelected => {
            if let Some(action) = app.reject_selected_action() {
                if handle_continue_action(app, on_continue_action, action.clone())? {
                    return Ok(None);
                }
                return Ok(Some(action));
            }
        }
        DashboardCommand::DeleteSelected => {
            if let Some(action) = app.delete_selected_action() {
                if handle_continue_action(app, on_continue_action, action.clone())? {
                    return Ok(None);
                }
                return Ok(Some(action));
            }
        }
    }
    Ok(None)
}

struct DashboardApp {
    active_tab: DashboardTab,
    tools: ToolsApp,
    sessions: SessionsApp,
    memories: MemoriesApp,
    suggestions: SuggestionsApp,
    skills: SkillsApp,
    active_context: Option<ContextRecord>,
    help_open: bool,
    palette: GroupedSelectState,
}

impl DashboardApp {
    fn new(
        tools: Vec<ToolEntry>,
        sessions: Vec<SessionRecord>,
        memories: Vec<MemoryRecord>,
        suggestions: Vec<SuggestionRecord>,
        skills: Vec<SkillRecord>,
        active_context: Option<ContextRecord>,
        initial_tab: DashboardTab,
    ) -> Self {
        Self {
            active_tab: initial_tab,
            tools: ToolsApp::new(tools),
            sessions: SessionsApp::new(sessions),
            memories: MemoriesApp::new(memories),
            suggestions: SuggestionsApp::new(suggestions),
            skills: SkillsApp::new(skills),
            active_context,
            help_open: false,
            palette: GroupedSelectState::default(),
        }
    }

    fn open_help(&mut self) {
        self.palette.close();
        self.help_open = true;
    }

    fn close_help(&mut self) {
        self.help_open = false;
    }

    fn open_palette(&mut self) {
        self.help_open = false;
        self.palette.open();
        self.normalize_palette_selection();
    }

    fn close_palette(&mut self) {
        self.palette.close();
    }

    fn push_palette_query(&mut self, ch: char) {
        self.palette.push_query(ch);
        self.normalize_palette_selection();
    }

    fn backspace_palette_query_or_close(&mut self) {
        self.palette.backspace_query_or_close();
        self.normalize_palette_selection();
    }

    fn next_palette_item(&mut self) {
        let visible = self.visible_palette_indices();
        self.palette.next(&visible);
    }

    fn previous_palette_item(&mut self) {
        let visible = self.visible_palette_indices();
        self.palette.previous(&visible);
    }

    fn selected_palette_command(&self) -> Option<DashboardCommand> {
        let visible = self.visible_palette_indices();
        grouped_select::selected_item(
            &self.dashboard_command_palette(),
            &visible,
            self.palette.selected,
            |entry| entry.command,
        )
    }

    fn visible_palette_indices(&self) -> Vec<usize> {
        grouped_select::visible_indices(&self.dashboard_command_palette(), &self.palette.query)
    }

    fn normalize_palette_selection(&mut self) {
        let visible = self.visible_palette_indices();
        self.palette.normalize_selection(&visible);
    }

    fn dashboard_command_palette(&self) -> Vec<DashboardCommandEntry> {
        let mut entries = vec![
            DashboardCommandEntry {
                section: "Navigation".to_string(),
                label: "Open Tools".to_string(),
                description: "Jump to Tools".to_string(),
                command: DashboardCommand::OpenTab(DashboardTab::Tools),
            },
            DashboardCommandEntry {
                section: "Navigation".to_string(),
                label: "Open Sessions".to_string(),
                description: "Jump to folder-backed sessions".to_string(),
                command: DashboardCommand::OpenTab(DashboardTab::Sessions),
            },
            DashboardCommandEntry {
                section: "Navigation".to_string(),
                label: "Open Memories".to_string(),
                description: "Jump to active memories".to_string(),
                command: DashboardCommand::OpenTab(DashboardTab::Memories),
            },
            DashboardCommandEntry {
                section: "Navigation".to_string(),
                label: "Open Suggestions".to_string(),
                description: "Jump to suggestions".to_string(),
                command: DashboardCommand::OpenTab(DashboardTab::Suggestions),
            },
            DashboardCommandEntry {
                section: "Navigation".to_string(),
                label: "Open Skills".to_string(),
                description: "Jump to Skills".to_string(),
                command: DashboardCommand::OpenTab(DashboardTab::Skills),
            },
            DashboardCommandEntry {
                section: "Help".to_string(),
                label: "Open Help".to_string(),
                description: "Show dashboard keybindings".to_string(),
                command: DashboardCommand::OpenHelp,
            },
        ];
        entries.extend(self.active_tab_command_palette());
        entries
    }

    fn active_tab_command_palette(&self) -> Vec<DashboardCommandEntry> {
        match self.active_tab {
            DashboardTab::Tools => vec![
                dashboard_command_entry(
                    "Tools",
                    "Open selected tool",
                    "Open the highlighted tool preview target",
                    DashboardCommand::OpenSelected,
                ),
                dashboard_command_entry(
                    "Tools",
                    "Filter tools",
                    "Edit the Tools filter",
                    DashboardCommand::ToggleFilter,
                ),
            ],
            DashboardTab::Sessions => vec![
                dashboard_command_entry(
                    "Sessions",
                    "Open selected session",
                    "Open the highlighted folder-backed session",
                    DashboardCommand::OpenSelected,
                ),
                dashboard_command_entry(
                    "Sessions",
                    "Promote selected sessions to memories",
                    "Create a memory promotion session from checked sessions",
                    DashboardCommand::PromoteSelectedSessions(DashboardPromotionType::Memory),
                ),
                dashboard_command_entry(
                    "Sessions",
                    "Promote selected sessions to todos",
                    "Create a todo promotion session from checked sessions",
                    DashboardCommand::PromoteSelectedSessions(DashboardPromotionType::Todo),
                ),
                dashboard_command_entry(
                    "Sessions",
                    "Promote selected sessions to skills",
                    "Create a skill promotion session from checked sessions",
                    DashboardCommand::PromoteSelectedSessions(DashboardPromotionType::Skill),
                ),
                dashboard_command_entry(
                    "Sessions",
                    "Promote selected sessions to patterns",
                    "Create a pattern promotion session from checked sessions",
                    DashboardCommand::PromoteSelectedSessions(DashboardPromotionType::Pattern),
                ),
                dashboard_command_entry(
                    "Sessions",
                    "Toggle selected session",
                    "Check or uncheck the highlighted session",
                    DashboardCommand::ToggleSelected,
                ),
                dashboard_command_entry(
                    "Sessions",
                    "Select all visible sessions",
                    "Toggle all filtered session rows",
                    DashboardCommand::ToggleAll,
                ),
                dashboard_command_entry(
                    "Sessions",
                    "Filter sessions",
                    "Edit the Sessions filter",
                    DashboardCommand::ToggleFilter,
                ),
            ],
            DashboardTab::Memories => vec![
                dashboard_command_entry(
                    "Memories",
                    "Review selected memory",
                    "Review the highlighted memory for suggested actions",
                    DashboardCommand::AcceptSelected,
                ),
                dashboard_command_entry(
                    "Memories",
                    "Toggle selected memory",
                    "Select or unselect the highlighted memory",
                    DashboardCommand::ToggleSelected,
                ),
                dashboard_command_entry(
                    "Memories",
                    "Select all visible memories",
                    "Toggle all filtered memory rows",
                    DashboardCommand::ToggleAll,
                ),
                dashboard_command_entry(
                    "Memories",
                    "Remove selected memories",
                    "Remove selected memories",
                    DashboardCommand::RejectSelected,
                ),
                dashboard_command_entry(
                    "Memories",
                    "Filter memories",
                    "Edit the Memories filter",
                    DashboardCommand::ToggleFilter,
                ),
            ],
            DashboardTab::Suggestions => vec![
                dashboard_command_entry(
                    "Suggestions",
                    "Toggle selected suggestion",
                    "Select or unselect the highlighted suggestion",
                    DashboardCommand::ToggleSelected,
                ),
                dashboard_command_entry(
                    "Suggestions",
                    "Select all visible suggestions",
                    "Toggle all filtered suggestion rows",
                    DashboardCommand::ToggleAll,
                ),
                dashboard_command_entry(
                    "Suggestions",
                    "Remove selected suggestions",
                    "Remove selected suggestions",
                    DashboardCommand::DeleteSelected,
                ),
                dashboard_command_entry(
                    "Suggestions",
                    "Filter suggestions",
                    "Edit the Suggestions filter",
                    DashboardCommand::ToggleFilter,
                ),
            ],
            DashboardTab::Skills => vec![
                dashboard_command_entry(
                    "Skills",
                    "Open selected skill",
                    "Open the highlighted skill",
                    DashboardCommand::OpenSelected,
                ),
                dashboard_command_entry(
                    "Skills",
                    "Filter skills",
                    "Edit the Skills filter",
                    DashboardCommand::ToggleFilter,
                ),
            ],
        }
    }

    fn next_tab(&mut self) {
        self.active_tab = DashboardTab::from_index(self.active_tab.index() + 1);
    }

    fn previous_tab(&mut self) {
        self.active_tab = DashboardTab::from_index(
            (self.active_tab.index() + DASHBOARD_TABS.len() - 1) % DASHBOARD_TABS.len(),
        );
    }

    fn next_item(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.next(),
            DashboardTab::Sessions => self.sessions.next(),
            DashboardTab::Memories => self.memories.next(),
            DashboardTab::Suggestions => self.suggestions.next(),
            DashboardTab::Skills => self.skills.next(),
        }
    }

    fn previous_item(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.previous(),
            DashboardTab::Sessions => self.sessions.previous(),
            DashboardTab::Memories => self.memories.previous(),
            DashboardTab::Suggestions => self.suggestions.previous(),
            DashboardTab::Skills => self.skills.previous(),
        }
    }

    fn scroll_down(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.scroll_down(),
            DashboardTab::Sessions => self.sessions.scroll_down(),
            DashboardTab::Memories => self.memories.scroll_down(),
            DashboardTab::Suggestions => self.suggestions.scroll_down(),
            DashboardTab::Skills => self.skills.scroll_down(),
        }
    }

    fn scroll_up(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.scroll_up(),
            DashboardTab::Sessions => self.sessions.scroll_up(),
            DashboardTab::Memories => self.memories.scroll_up(),
            DashboardTab::Suggestions => self.suggestions.scroll_up(),
            DashboardTab::Skills => self.skills.scroll_up(),
        }
    }

    fn filter_editing(&self) -> bool {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter.editing,
            DashboardTab::Sessions => self.sessions.filter.editing,
            DashboardTab::Memories => self.memories.filter.editing,
            DashboardTab::Suggestions => self.suggestions.filter.editing,
            DashboardTab::Skills => self.skills.filter.editing,
        }
    }

    fn toggle_filter(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.toggle_filter(),
            DashboardTab::Sessions => self.sessions.toggle_filter(),
            DashboardTab::Memories => self.memories.toggle_filter(),
            DashboardTab::Suggestions => self.suggestions.toggle_filter(),
            DashboardTab::Skills => self.skills.toggle_filter(),
        }
    }

    fn filter_push(&mut self, ch: char) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter_push(ch),
            DashboardTab::Sessions => self.sessions.filter_push(ch),
            DashboardTab::Memories => self.memories.filter_push(ch),
            DashboardTab::Suggestions => self.suggestions.filter_push(ch),
            DashboardTab::Skills => self.skills.filter_push(ch),
        }
    }

    fn filter_backspace(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter_backspace(),
            DashboardTab::Sessions => self.sessions.filter_backspace(),
            DashboardTab::Memories => self.memories.filter_backspace(),
            DashboardTab::Suggestions => self.suggestions.filter_backspace(),
            DashboardTab::Skills => self.skills.filter_backspace(),
        }
    }

    fn finish_filter_edit(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter.editing = false,
            DashboardTab::Sessions => self.sessions.filter.editing = false,
            DashboardTab::Memories => self.memories.filter.editing = false,
            DashboardTab::Suggestions => self.suggestions.filter.editing = false,
            DashboardTab::Skills => self.skills.filter.editing = false,
        }
    }

    fn toggle_selected(&mut self) {
        match self.active_tab {
            DashboardTab::Sessions => self.sessions.toggle_selected(),
            DashboardTab::Memories => self.memories.toggle_selected(),
            DashboardTab::Suggestions => self.suggestions.toggle_selected(),
            DashboardTab::Tools | DashboardTab::Skills => {}
        }
    }

    fn toggle_all(&mut self) {
        match self.active_tab {
            DashboardTab::Sessions => self.sessions.toggle_all(),
            DashboardTab::Memories => self.memories.toggle_all(),
            DashboardTab::Suggestions => self.suggestions.toggle_all(),
            DashboardTab::Tools | DashboardTab::Skills => {}
        }
    }

    fn reject_selected_action(&self) -> Option<TuiAction> {
        match self.active_tab {
            DashboardTab::Memories => {
                let ids = self.memories.selected_memory_ids();
                (!ids.is_empty()).then_some(TuiAction::DeleteMemories(ids))
            }
            DashboardTab::Tools
            | DashboardTab::Sessions
            | DashboardTab::Suggestions
            | DashboardTab::Skills => None,
        }
    }

    fn delete_selected_action(&self) -> Option<TuiAction> {
        match self.active_tab {
            DashboardTab::Memories => self.reject_selected_action(),
            DashboardTab::Suggestions => {
                let ids = self.suggestions.selected_suggestion_ids();
                (!ids.is_empty()).then_some(TuiAction::DeleteSuggestions(ids))
            }
            DashboardTab::Tools | DashboardTab::Sessions | DashboardTab::Skills => None,
        }
    }

    fn palette_body_lines_and_selected_row(&self) -> (Vec<Line<'static>>, Option<usize>) {
        let entries = self.dashboard_command_palette();
        let visible = self.visible_palette_indices();
        grouped_select::body_lines_and_selected_row(&entries, &visible, self.palette.selected)
    }

    fn ensure_palette_selection_visible(
        &mut self,
        body_height: usize,
        selected_row: Option<usize>,
        total_lines: usize,
    ) {
        self.palette
            .ensure_selection_visible(body_height, selected_row, total_lines);
    }

    fn apply_completed_action(&mut self, action: &TuiAction) {
        match action {
            TuiAction::DeleteMemories(ids) => self.memories.remove_ids(ids),
            TuiAction::DeleteSuggestions(ids) => self.suggestions.remove_ids(ids),
            TuiAction::OpenSession(_)
            | TuiAction::PromoteSessions { .. }
            | TuiAction::OpenTool(_)
            | TuiAction::OpenSkill(_)
            | TuiAction::ReviewMemory(_) => {}
        }
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(frame.area());

        let header_title = self.header_title();
        let tabs = Tabs::new(
            DASHBOARD_TABS
                .iter()
                .map(|tab| Line::from(Span::styled(*tab, dim_style())))
                .collect::<Vec<_>>(),
        )
        .block(block(&header_title))
        .select(self.active_tab.index())
        .style(dim_style())
        .highlight_style(selected_style());
        frame.render_widget(Clear, chunks[0]);
        frame.render_widget(tabs, chunks[0]);

        match self.active_tab {
            DashboardTab::Tools => self.tools.draw_body(frame, chunks[1]),
            DashboardTab::Sessions => self.sessions.draw_body(frame, chunks[1]),
            DashboardTab::Memories => self.memories.draw_body(frame, chunks[1]),
            DashboardTab::Suggestions => self.suggestions.draw_body(frame, chunks[1]),
            DashboardTab::Skills => self.skills.draw_body(frame, chunks[1]),
        }

        frame.render_widget(Clear, chunks[2]);
        frame.render_widget(
            Paragraph::new("Ctrl+P commands • Ctrl+/ help • q quit").style(dim_style()),
            chunks[2],
        );

        if self.help_open {
            self.draw_help(frame);
        }
        if self.palette.open {
            self.draw_palette(frame);
        }
    }

    fn draw_palette(&mut self, frame: &mut ratatui::Frame) {
        let area = centered_rect(68, 50, frame.area());
        let inner = Rect::new(
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);
        let search_line = Line::from(vec![
            Span::styled("Search: ", dim_style()),
            if self.palette.query.is_empty() {
                Span::styled("find action…", dim_style())
            } else {
                Span::raw(self.palette.query.clone())
            },
        ]);
        let (body_lines, selected_row) = self.palette_body_lines_and_selected_row();
        self.ensure_palette_selection_visible(
            chunks[2].height as usize,
            selected_row,
            body_lines.len(),
        );
        let body = Paragraph::new(body_lines)
            .style(base_style())
            .scroll((self.palette.scroll.min(u16::MAX as usize) as u16, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, area);
        frame.render_widget(block("Command palette"), area);
        frame.render_widget(Paragraph::new(search_line).style(base_style()), chunks[0]);
        frame.render_widget(body, chunks[2]);
        let cursor_x = area
            .x
            .saturating_add(1)
            .saturating_add("Search: ".len() as u16)
            .saturating_add(self.palette.query.chars().count() as u16)
            .min(area.right().saturating_sub(2));
        frame.set_cursor_position(Position::new(cursor_x, area.y.saturating_add(1)));
    }

    fn draw_help(&self, frame: &mut ratatui::Frame) {
        let area = centered_rect(68, 64, frame.area());
        let lines = vec![
            Line::from(Span::styled("Dashboard", title_style())),
            Line::from(""),
            Line::from(Span::styled("Global", title_style())),
            Line::from(vec![
                Span::styled("Tab / Shift+Tab", selected_style()),
                Span::raw(" move between tabs"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+/", selected_style()),
                Span::raw(" open or close this help"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+P", selected_style()),
                Span::raw(" open command palette scoped to this tab"),
            ]),
            Line::from(vec![
                Span::styled("/", selected_style()),
                Span::raw(" filter current tab; / again clears while editing"),
            ]),
            Line::from(vec![
                Span::styled("↑/↓ or j/k", selected_style()),
                Span::raw(" move selection"),
            ]),
            Line::from(vec![
                Span::styled("PgUp/PgDn or u/d", selected_style()),
                Span::raw(" scroll preview"),
            ]),
            Line::from(vec![
                Span::styled("q", selected_style()),
                Span::raw(" quit"),
            ]),
            Line::from(""),
            Line::from(Span::styled("Tools", title_style())),
            Line::from(vec![
                Span::styled("Enter", selected_style()),
                Span::raw(" open selected tool"),
            ]),
            Line::from(""),
            Line::from(Span::styled("Sessions", title_style())),
            Line::from(vec![
                Span::styled("Enter", selected_style()),
                Span::raw(" open focused folder-backed session"),
            ]),
            Line::from(vec![
                Span::styled("Space", selected_style()),
                Span::raw(" check/uncheck session for promotion"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+P", selected_style()),
                Span::raw(" promote checked sessions to memory/todo/skill/pattern"),
            ]),
            Line::from(vec![
                Span::styled("/", selected_style()),
                Span::raw(" filter by name, repo, state, path, or summary"),
            ]),
            Line::from(""),
            Line::from(Span::styled("Memories & Suggestions", title_style())),
            Line::from(vec![
                Span::styled("a", selected_style()),
                Span::raw(" accept/review selected item where supported"),
            ]),
            Line::from(vec![
                Span::styled("r / x", selected_style()),
                Span::raw(" reject/remove selected item where supported"),
            ]),
            Line::from(""),
            Line::from(Span::styled("Skills", title_style())),
            Line::from(vec![
                Span::styled("Enter", selected_style()),
                Span::raw(" open selected skill"),
            ]),
        ];
        let help = Paragraph::new(lines)
            .block(block("Help"))
            .style(base_style())
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, area);
        frame.render_widget(help, area);
    }

    fn header_title(&self) -> String {
        match self.active_context.as_ref() {
            Some(ctx) if !ctx.name.trim().is_empty() => format!("Djinn  ctx: {}", ctx.name),
            _ => "Djinn  ctx: none".to_string(),
        }
    }
}

struct ToolsApp {
    tools: Vec<ToolEntry>,
    selected: usize,
    preview_scroll: u16,
    filter: FilterState,
}

impl ToolsApp {
    fn new(tools: Vec<ToolEntry>) -> Self {
        Self {
            tools,
            selected: 0,
            preview_scroll: 0,
            filter: FilterState::default(),
        }
    }

    fn next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[(pos + 1).min(visible.len() - 1)];
        self.preview_scroll = 0;
    }

    fn previous(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[pos.saturating_sub(1)];
        self.preview_scroll = 0;
    }

    fn scroll_down(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_add(8);
    }

    fn scroll_up(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_sub(8);
    }

    fn selected_tool(&self) -> Option<&ToolEntry> {
        self.tools
            .get(self.selected)
            .filter(|tool| self.tool_matches(tool))
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.tools
            .iter()
            .enumerate()
            .filter_map(|(idx, tool)| self.tool_matches(tool).then_some(idx))
            .collect()
    }

    fn tool_matches(&self, tool: &ToolEntry) -> bool {
        fuzzy_match(&self.filter.query, &tool.name)
    }

    fn ensure_selection_visible(&mut self) {
        let visible = self.visible_indices();
        if let Some(first) = visible.first() {
            if selected_visible_position(self.selected, &visible).is_none() {
                self.selected = *first;
            }
        }
        self.preview_scroll = 0;
    }

    fn toggle_filter(&mut self) {
        self.filter.toggle();
        self.ensure_selection_visible();
    }

    fn filter_push(&mut self, ch: char) {
        self.filter.push(ch);
        self.ensure_selection_visible();
    }

    fn filter_backspace(&mut self) {
        self.filter.backspace();
        self.ensure_selection_visible();
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(frame.area());

        self.draw_body(frame, chunks[0]);

        let help = Paragraph::new(
            "↑/k ↓/j move • / filter/clear • PgUp/u PgDn/d scroll preview • q/Esc quit",
        )
        .style(dim_style());
        frame.render_widget(Clear, chunks[1]);
        frame.render_widget(help, chunks[1]);
    }

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
            .split(area);

        let visible = self.visible_indices();
        let items = if self.tools.is_empty() {
            vec![ListItem::new("No tools discovered").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No tools match filter").style(dim_style())]
        } else {
            visible
                .iter()
                .map(|idx| {
                    let tool = &self.tools[*idx];
                    ListItem::new(vec![
                        Line::from(Span::styled(tool.name.clone(), title_style())),
                        Line::from(Span::styled(tool.description.clone(), dim_style())),
                    ])
                })
                .collect::<Vec<_>>()
        };

        let mut state = ListState::default();
        if !visible.is_empty() {
            state.select(selected_visible_position(self.selected, &visible));
        }
        let title = format!("Tools ({})", self.filter.label());
        let list = List::new(items)
            .block(block(&title))
            .style(base_style())
            .highlight_style(highlight_style())
            .highlight_symbol("› ");
        frame.render_widget(Clear, body[0]);
        frame.render_stateful_widget(list, body[0], &mut state);

        let preview = self
            .selected_tool()
            .map(tool_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview_title = self
            .selected_tool()
            .map(|tool| tool.name.as_str())
            .unwrap_or("Tool");
        let preview = Paragraph::new(preview)
            .block(block(preview_title))
            .style(base_style())
            .scroll((self.preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);
    }
}

struct SessionsApp {
    sessions: Vec<SessionRecord>,
    selected: usize,
    preview_scroll: u16,
    checked: HashSet<String>,
    filter: FilterState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionListRow {
    Header(String),
    Session(usize),
}

impl SessionsApp {
    fn new(mut sessions: Vec<SessionRecord>) -> Self {
        sessions.sort_by(session_dashboard_order);
        Self {
            sessions,
            selected: 0,
            preview_scroll: 0,
            checked: HashSet::new(),
            filter: FilterState::default(),
        }
    }

    fn next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[(pos + 1).min(visible.len() - 1)];
        self.preview_scroll = 0;
    }

    fn previous(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[pos.saturating_sub(1)];
        self.preview_scroll = 0;
    }

    fn scroll_down(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_add(8);
    }

    fn scroll_up(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_sub(8);
    }

    fn selected_session(&self) -> Option<&SessionRecord> {
        self.sessions
            .get(self.selected)
            .filter(|session| self.session_matches(session))
    }

    fn selected_sessions(&self) -> Vec<SessionRecord> {
        self.sessions
            .iter()
            .filter(|session| self.checked.contains(&session.path))
            .cloned()
            .collect()
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.sessions
            .iter()
            .enumerate()
            .filter_map(|(idx, session)| self.session_matches(session).then_some(idx))
            .collect()
    }

    fn visible_rows(&self) -> Vec<SessionListRow> {
        let mut rows = Vec::new();
        let mut last_group = None::<String>;
        for idx in self.visible_indices() {
            let group = session_repo_group_label(&self.sessions[idx]);
            if last_group.as_deref() != Some(group.as_str()) {
                rows.push(SessionListRow::Header(group.clone()));
                last_group = Some(group);
            }
            rows.push(SessionListRow::Session(idx));
        }
        rows
    }

    fn session_matches(&self, session: &SessionRecord) -> bool {
        fuzzy_match(&self.filter.query, &session.name)
            || fuzzy_match(&self.filter.query, &session.reference_name)
            || fuzzy_match(&self.filter.query, &session.path)
            || fuzzy_match(&self.filter.query, &session.state)
            || fuzzy_match(&self.filter.query, &session.event_health)
            || fuzzy_match(&self.filter.query, &session_state_badge(session))
            || fuzzy_match(&self.filter.query, &session_repo_group_label(session))
            || session
                .repo_path
                .as_ref()
                .is_some_and(|repo| fuzzy_match(&self.filter.query, repo))
            || session
                .next_action
                .as_ref()
                .is_some_and(|action| fuzzy_match(&self.filter.query, action))
            || session
                .summary_preview
                .as_ref()
                .is_some_and(|summary| fuzzy_match(&self.filter.query, summary))
    }

    fn ensure_selection_visible(&mut self) {
        let visible = self.visible_indices();
        if let Some(first) = visible.first() {
            if selected_visible_position(self.selected, &visible).is_none() {
                self.selected = *first;
            }
        }
        self.preview_scroll = 0;
    }

    fn toggle_filter(&mut self) {
        self.filter.toggle();
        self.ensure_selection_visible();
    }

    fn filter_push(&mut self, ch: char) {
        self.filter.push(ch);
        self.ensure_selection_visible();
    }

    fn filter_backspace(&mut self) {
        self.filter.backspace();
        self.ensure_selection_visible();
    }

    fn toggle_selected(&mut self) {
        if let Some(path) = self.selected_session().map(|session| session.path.clone()) {
            if !self.checked.insert(path.clone()) {
                self.checked.remove(&path);
            }
        }
    }

    fn toggle_all(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let all_checked = visible
            .iter()
            .all(|idx| self.checked.contains(&self.sessions[*idx].path));
        if all_checked {
            for idx in visible {
                self.checked.remove(&self.sessions[idx].path);
            }
        } else {
            for idx in visible {
                self.checked.insert(self.sessions[idx].path.clone());
            }
        }
    }

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(area);

        let visible = self.visible_indices();
        let rows = self.visible_rows();
        let items = if self.sessions.is_empty() {
            vec![ListItem::new("No sessions found").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No sessions match filter").style(dim_style())]
        } else {
            rows.iter()
                .map(|row| match row {
                    SessionListRow::Header(group) => ListItem::new(Line::from(Span::styled(
                        format!("▾ {group}"),
                        title_style(),
                    ))),
                    SessionListRow::Session(idx) => {
                        let session = &self.sessions[*idx];
                        let checkbox = if self.checked.contains(&session.path) {
                            "[x] "
                        } else {
                            "[ ] "
                        };
                        let mut lines = vec![
                            Line::from(vec![
                                Span::styled(checkbox, dim_style()),
                                Span::styled(session_state_badge(session), dim_style()),
                                Span::raw(" "),
                                Span::styled(session.name.clone(), title_style()),
                            ]),
                            Line::from(Span::styled(session_list_metadata(session), dim_style())),
                        ];
                        if let Some(next) = session.next_action.as_deref() {
                            lines.push(Line::from(vec![
                                Span::styled("Action: ", title_style()),
                                Span::raw(next.to_string()),
                            ]));
                        }
                        ListItem::new(lines)
                    }
                })
                .collect::<Vec<_>>()
        };

        let mut state = ListState::default();
        if !visible.is_empty() {
            state.select(selected_session_row_position(self.selected, &rows));
        }
        let title = format!(
            "Sessions ({} / {} visible, {} selected, {})",
            visible.len(),
            self.sessions.len(),
            self.checked.len(),
            self.filter.label()
        );
        let list = List::new(items)
            .block(block(&title))
            .style(base_style())
            .highlight_style(highlight_style())
            .highlight_symbol("› ");
        frame.render_widget(Clear, body[0]);
        frame.render_stateful_widget(list, body[0], &mut state);

        let preview = self
            .selected_session()
            .map(session_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview_title = self
            .selected_session()
            .map(|session| session.name.as_str())
            .unwrap_or("Session");
        let preview = Paragraph::new(preview)
            .block(block(preview_title))
            .style(base_style())
            .scroll((self.preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);
    }
}

fn session_list_metadata(session: &SessionRecord) -> String {
    let mode = session.mode.as_deref().unwrap_or("-");
    let updated = session.updated_at.as_deref().unwrap_or("unknown");
    let candidates = session
        .candidate_status
        .as_deref()
        .map(|status| format!(" · candidates {status}"))
        .unwrap_or_default();
    format!(
        "{} · {} turns · events {}{} · updated {}",
        mode, session.turn_count, session.event_health, candidates, updated
    )
}

fn selected_session_row_position(selected: usize, rows: &[SessionListRow]) -> Option<usize> {
    rows.iter()
        .position(|row| matches!(row, SessionListRow::Session(idx) if *idx == selected))
}

fn session_dashboard_order(left: &SessionRecord, right: &SessionRecord) -> std::cmp::Ordering {
    session_repo_group_sort_key(left)
        .cmp(&session_repo_group_sort_key(right))
        .then_with(|| right.updated_at.cmp(&left.updated_at))
        .then_with(|| right.state.cmp(&left.state))
        .then_with(|| left.name.cmp(&right.name))
}

fn session_repo_group_sort_key(session: &SessionRecord) -> String {
    session
        .repo_path
        .as_deref()
        .map(|repo| format!("0:{}", repo.to_lowercase()))
        .unwrap_or_else(|| "1:~no-repo".to_string())
}

fn session_repo_group_label(session: &SessionRecord) -> String {
    let Some(repo) = session
        .repo_path
        .as_deref()
        .filter(|repo| !repo.trim().is_empty())
    else {
        return "No linked repo".to_string();
    };
    let name = Path::new(repo)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(repo);
    format!("Repo: {name}")
}

fn session_state_badge(session: &SessionRecord) -> String {
    match (session.state.as_str(), session.mode.as_deref()) {
        ("running", Some("promotion")) => "▶ generating".to_string(),
        ("running", _) => "▶ running".to_string(),
        ("failed", Some("promotion")) => "⚠ promotion failed".to_string(),
        ("failed", _) => "⚠ failed".to_string(),
        ("completed", Some("promotion")) => "✓ candidates ready".to_string(),
        ("completed", _) => "✓ completed".to_string(),
        ("paused", _) => "Ⅱ paused".to_string(),
        ("not_started", Some("promotion")) => "○ promotion draft".to_string(),
        ("not_started", _) => "○ draft".to_string(),
        (state, Some(mode)) => format!("{state}/{mode}"),
        (state, None) => state.to_string(),
    }
}

fn format_promotion_candidate_row(candidate: &PromotionCandidateRow) -> String {
    let candidate_type = candidate.candidate_type.as_deref().unwrap_or("unknown");
    let mut detail = format!("{} [{}] {}", candidate.id, candidate_type, candidate.status);
    if let Some(destination) = &candidate.destination {
        detail.push_str(&format!(" -> {destination}"));
    }
    if let Some(evidence) = candidate.evidence.first() {
        detail.push_str(&format!(" · evidence {evidence}"));
        if candidate.evidence.len() > 1 {
            detail.push_str(&format!(" (+{})", candidate.evidence.len() - 1));
        }
    }
    if let Some(path) = &candidate.writeback_path {
        detail.push_str(&format!(" ({path})"));
    }
    detail
}

fn selected_promotion_candidate_detail_lines(
    candidate: &PromotionCandidateRow,
) -> Vec<Line<'static>> {
    let candidate_type = candidate.candidate_type.as_deref().unwrap_or("unknown");
    let mut lines = vec![
        Line::from(Span::styled("Selected candidate", title_style())),
        Line::from(vec![
            Span::styled("Id:      ", dim_style()),
            Span::raw(candidate.id.clone()),
            Span::styled("  Type: ", dim_style()),
            Span::raw(candidate_type.to_string()),
            Span::styled("  Status: ", dim_style()),
            Span::raw(candidate.status.clone()),
        ]),
        Line::from(vec![
            Span::styled("File:    ", dim_style()),
            Span::raw(candidate.path.clone()),
        ]),
    ];
    if let Some(destination) = &candidate.destination {
        lines.push(Line::from(vec![
            Span::styled("Dest:    ", dim_style()),
            Span::raw(destination.clone()),
        ]));
    }
    if let Some(path) = &candidate.writeback_path {
        lines.push(Line::from(vec![
            Span::styled("Wrote:   ", dim_style()),
            Span::raw(path.clone()),
        ]));
    }
    if let Some(text) = candidate
        .text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(Line::from(vec![
            Span::styled("Text:    ", dim_style()),
            Span::raw(text.to_string()),
        ]));
    }
    if let Some(rationale) = candidate
        .rationale
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(Line::from(vec![
            Span::styled("Why:     ", dim_style()),
            Span::raw(rationale.to_string()),
        ]));
    }
    if candidate.evidence.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Evidence:", dim_style()),
            Span::raw(" -"),
        ]));
    } else {
        lines.push(Line::from(Span::styled("Evidence:", dim_style())));
        for evidence in candidate.evidence.iter().take(3) {
            lines.push(Line::from(vec![
                Span::styled("  - ", dim_style()),
                Span::raw(evidence.clone()),
            ]));
        }
        if candidate.evidence.len() > 3 {
            lines.push(Line::from(Span::styled(
                format!("  +{} more evidence links", candidate.evidence.len() - 3),
                dim_style(),
            )));
        }
    }
    lines
}

fn session_preview(session: &SessionRecord) -> String {
    let mut lines = vec![
        format!("Name: {}", session.name),
        format!("Reference: {}", session.reference_name),
        format!("Path: {}", session.path),
        format!("Status: {}", session_state_badge(session)),
        format!("State: {}", session.state),
        format!("Mode: {}", session.mode.as_deref().unwrap_or("-")),
        format!("Turns: {}", session.turn_count),
        format!("Events: {}", session.event_health),
        format!(
            "Updated: {}",
            session.updated_at.as_deref().unwrap_or("unknown")
        ),
        format!("Group: {}", session_repo_group_label(session)),
    ];
    if let Some(repo) = &session.repo_path {
        lines.push(format!("Repo: {repo}"));
    }
    if let Some(next) = &session.next_action {
        lines.push(String::new());
        lines.push(format!("Next action: {next}"));
    }
    if let Some(candidates) = &session.candidate_status {
        lines.push(format!("Candidates: {candidates}"));
        if session.candidate_entries.is_empty() {
            for detail in session.candidate_details.iter().take(8) {
                lines.push(format!("  - {detail}"));
            }
        } else {
            for candidate in session.candidate_entries.iter().take(8) {
                lines.push(format!("  - {}", format_promotion_candidate_row(candidate)));
            }
        }
    }
    lines.push(String::new());
    lines.push("Enter opens the focused session view.".to_string());
    lines.push(
        "Focused shortcuts: r run, w watch, o summary, e request, c context, d discover."
            .to_string(),
    );
    if let Some(summary) = &session.summary_preview {
        lines.push(String::new());
        lines.push("Summary".to_string());
        lines.push(summary.clone());
    }
    lines.join("\n")
}

struct SuggestionsApp {
    suggestions: Vec<SuggestionRecord>,
    selected: usize,
    preview_scroll: u16,
    checked: HashSet<String>,
    filter: FilterState,
}

impl SuggestionsApp {
    fn new(suggestions: Vec<SuggestionRecord>) -> Self {
        Self {
            suggestions,
            selected: 0,
            preview_scroll: 0,
            checked: HashSet::new(),
            filter: FilterState::default(),
        }
    }

    fn next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[(pos + 1).min(visible.len() - 1)];
        self.preview_scroll = 0;
    }

    fn previous(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[pos.saturating_sub(1)];
        self.preview_scroll = 0;
    }

    fn scroll_down(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_add(8);
    }

    fn scroll_up(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_sub(8);
    }

    fn selected_suggestion(&self) -> Option<&SuggestionRecord> {
        self.suggestions
            .get(self.selected)
            .filter(|suggestion| self.suggestion_matches(suggestion))
    }

    fn selected_suggestion_ids(&self) -> Vec<String> {
        if self.checked.is_empty() {
            return self
                .selected_suggestion()
                .map(|suggestion| vec![suggestion.id.clone()])
                .unwrap_or_default();
        }
        self.suggestions
            .iter()
            .filter(|suggestion| self.checked.contains(&suggestion.id))
            .map(|suggestion| suggestion.id.clone())
            .collect()
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.suggestions
            .iter()
            .enumerate()
            .filter_map(|(idx, suggestion)| self.suggestion_matches(suggestion).then_some(idx))
            .collect()
    }

    fn suggestion_matches(&self, suggestion: &SuggestionRecord) -> bool {
        fuzzy_match(&self.filter.query, &suggestion.id)
            || fuzzy_match(&self.filter.query, &suggestion.text)
            || fuzzy_match(&self.filter.query, &suggestion.status)
            || fuzzy_match(&self.filter.query, &suggestion.target)
            || fuzzy_match(&self.filter.query, &suggestion.rationale)
            || fuzzy_match(&self.filter.query, &suggestion.draft)
    }

    fn ensure_selection_visible(&mut self) {
        let visible = self.visible_indices();
        if let Some(first) = visible.first() {
            if selected_visible_position(self.selected, &visible).is_none() {
                self.selected = *first;
            }
        }
        self.preview_scroll = 0;
    }

    fn toggle_filter(&mut self) {
        self.filter.toggle();
        self.ensure_selection_visible();
    }

    fn filter_push(&mut self, ch: char) {
        self.filter.push(ch);
        self.ensure_selection_visible();
    }

    fn filter_backspace(&mut self) {
        self.filter.backspace();
        self.ensure_selection_visible();
    }

    fn toggle_selected(&mut self) {
        if let Some(id) = self
            .selected_suggestion()
            .map(|suggestion| suggestion.id.clone())
        {
            if !self.checked.insert(id.clone()) {
                self.checked.remove(&id);
            }
        }
    }

    fn toggle_all(&mut self) {
        let visible_ids = self
            .visible_indices()
            .iter()
            .map(|idx| self.suggestions[*idx].id.clone())
            .collect::<Vec<_>>();
        if visible_ids.is_empty() {
            return;
        }
        if visible_ids.iter().all(|id| self.checked.contains(id)) {
            self.checked.clear();
        } else {
            self.checked = visible_ids.into_iter().collect();
        }
    }

    fn remove_ids(&mut self, ids: &[String]) {
        let removed = ids.iter().cloned().collect::<HashSet<_>>();
        self.suggestions
            .retain(|suggestion| !removed.contains(&suggestion.id));
        self.checked.retain(|id| !removed.contains(id));
        if self.selected >= self.suggestions.len() {
            self.selected = self.suggestions.len().saturating_sub(1);
        }
        self.ensure_selection_visible();
    }

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);

        let visible = self.visible_indices();
        let items = if self.suggestions.is_empty() {
            vec![ListItem::new("No suggestions recorded").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No suggestions match filter").style(dim_style())]
        } else {
            visible
                .iter()
                .map(|idx| {
                    let suggestion = &self.suggestions[*idx];
                    let checked = if self.checked.contains(&suggestion.id) {
                        "[x]"
                    } else {
                        "[ ]"
                    };
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(
                                format!("{checked} "),
                                if checked == "[x]" {
                                    Style::default().fg(CTP_GREEN).bg(CTP_BASE)
                                } else {
                                    dim_style()
                                },
                            ),
                            Span::styled(suggestion.id.clone(), title_style()),
                        ]),
                        Line::from(Span::styled(
                            truncate_line(&suggestion.text, 96),
                            dim_style(),
                        )),
                    ])
                })
                .collect::<Vec<_>>()
        };

        let mut state = ListState::default();
        if !visible.is_empty() {
            state.select(selected_visible_position(self.selected, &visible));
        }
        let title = format!(
            "Suggestions ({} selected, {})",
            self.checked.len(),
            self.filter.label()
        );
        let list = List::new(items)
            .block(block(&title))
            .style(base_style())
            .highlight_style(highlight_style())
            .highlight_symbol("› ");
        frame.render_widget(Clear, body[0]);
        frame.render_stateful_widget(list, body[0], &mut state);

        let preview = self
            .selected_suggestion()
            .map(suggestion_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview_title = self
            .selected_suggestion()
            .map(|suggestion| compact_id(&suggestion.id))
            .unwrap_or_else(|| "Suggestion".to_string());
        let preview = Paragraph::new(preview)
            .block(block(&preview_title))
            .style(base_style())
            .scroll((self.preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);
    }
}

struct MemoriesApp {
    memories: Vec<MemoryRecord>,
    selected: usize,
    preview_scroll: u16,
    checked: HashSet<String>,
    filter: FilterState,
}

impl MemoriesApp {
    fn new(memories: Vec<MemoryRecord>) -> Self {
        Self {
            memories,
            selected: 0,
            preview_scroll: 0,
            checked: HashSet::new(),
            filter: FilterState::default(),
        }
    }

    fn next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[(pos + 1).min(visible.len() - 1)];
        self.preview_scroll = 0;
    }

    fn previous(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[pos.saturating_sub(1)];
        self.preview_scroll = 0;
    }

    fn scroll_down(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_add(8);
    }

    fn scroll_up(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_sub(8);
    }

    fn selected_memory(&self) -> Option<&MemoryRecord> {
        self.memories
            .get(self.selected)
            .filter(|memory| self.memory_matches(memory))
    }

    fn selected_memory_id(&self) -> Option<String> {
        self.selected_memory().map(|memory| memory.id.clone())
    }

    fn selected_memory_ids(&self) -> Vec<String> {
        if self.checked.is_empty() {
            return self
                .selected_memory()
                .map(|memory| vec![memory.id.clone()])
                .unwrap_or_default();
        }
        self.memories
            .iter()
            .filter(|memory| self.checked.contains(&memory.id))
            .map(|memory| memory.id.clone())
            .collect()
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.memories
            .iter()
            .enumerate()
            .filter_map(|(idx, memory)| self.memory_matches(memory).then_some(idx))
            .collect()
    }

    fn memory_matches(&self, memory: &MemoryRecord) -> bool {
        fuzzy_match(&self.filter.query, &memory.id)
            || fuzzy_match(&self.filter.query, &memory.text)
            || fuzzy_match(&self.filter.query, &memory.status)
            || fuzzy_match(&self.filter.query, &memory.scope)
            || fuzzy_match(&self.filter.query, &memory.kind)
            || fuzzy_match(&self.filter.query, &memory.confidence)
            || fuzzy_match(&self.filter.query, &memory.not_before)
    }

    fn ensure_selection_visible(&mut self) {
        let visible = self.visible_indices();
        if let Some(first) = visible.first() {
            if selected_visible_position(self.selected, &visible).is_none() {
                self.selected = *first;
            }
        }
        self.preview_scroll = 0;
    }

    fn toggle_filter(&mut self) {
        self.filter.toggle();
        self.ensure_selection_visible();
    }

    fn filter_push(&mut self, ch: char) {
        self.filter.push(ch);
        self.ensure_selection_visible();
    }

    fn filter_backspace(&mut self) {
        self.filter.backspace();
        self.ensure_selection_visible();
    }

    fn toggle_selected(&mut self) {
        if let Some(id) = self.selected_memory_id() {
            if !self.checked.insert(id.clone()) {
                self.checked.remove(&id);
            }
        }
    }

    fn toggle_all(&mut self) {
        let visible_ids = self
            .visible_indices()
            .iter()
            .map(|idx| self.memories[*idx].id.clone())
            .collect::<Vec<_>>();
        if visible_ids.is_empty() {
            return;
        }
        if visible_ids.iter().all(|id| self.checked.contains(id)) {
            self.checked.clear();
        } else {
            self.checked = visible_ids.into_iter().collect();
        }
    }

    fn remove_ids(&mut self, ids: &[String]) {
        let removed = ids.iter().cloned().collect::<HashSet<_>>();
        self.memories.retain(|memory| !removed.contains(&memory.id));
        self.checked.retain(|id| !removed.contains(id));
        if self.selected >= self.memories.len() {
            self.selected = self.memories.len().saturating_sub(1);
        }
        self.ensure_selection_visible();
    }

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);

        let visible = self.visible_indices();
        let items = if self.memories.is_empty() {
            vec![ListItem::new("No memories recorded").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No memories match filter").style(dim_style())]
        } else {
            visible
                .iter()
                .map(|idx| {
                    let memory = &self.memories[*idx];
                    let checked = if self.checked.contains(&memory.id) {
                        "[x]"
                    } else {
                        "[ ]"
                    };
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(
                                format!("{checked} "),
                                if checked == "[x]" {
                                    Style::default().fg(CTP_GREEN).bg(CTP_BASE)
                                } else {
                                    dim_style()
                                },
                            ),
                            Span::styled(memory.id.clone(), title_style()),
                        ]),
                        Line::from(Span::styled(truncate_line(&memory.text, 96), dim_style())),
                    ])
                })
                .collect::<Vec<_>>()
        };

        let mut state = ListState::default();
        if !visible.is_empty() {
            state.select(selected_visible_position(self.selected, &visible));
        }
        let title = format!(
            "Memories ({} selected, {})",
            self.checked.len(),
            self.filter.label()
        );
        let list = List::new(items)
            .block(block(&title))
            .style(base_style())
            .highlight_style(highlight_style())
            .highlight_symbol("› ");
        frame.render_widget(Clear, body[0]);
        frame.render_stateful_widget(list, body[0], &mut state);

        let preview = self
            .selected_memory()
            .map(memory_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview_title = self
            .selected_memory()
            .map(|memory| compact_id(&memory.id))
            .unwrap_or_else(|| "Memory".to_string());
        let preview = Paragraph::new(preview)
            .block(block(&preview_title))
            .style(base_style())
            .scroll((self.preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);
    }
}

struct SkillsApp {
    skills: Vec<SkillRecord>,
    selected: usize,
    preview_scroll: u16,
    filter: FilterState,
}

impl SkillsApp {
    fn new(skills: Vec<SkillRecord>) -> Self {
        Self {
            skills,
            selected: 0,
            preview_scroll: 0,
            filter: FilterState::default(),
        }
    }

    fn next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[(pos + 1).min(visible.len() - 1)];
        self.preview_scroll = 0;
    }

    fn previous(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = selected_visible_position(self.selected, &visible).unwrap_or(0);
        self.selected = visible[pos.saturating_sub(1)];
        self.preview_scroll = 0;
    }

    fn scroll_down(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_add(8);
    }

    fn scroll_up(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_sub(8);
    }

    fn selected_skill(&self) -> Option<&SkillRecord> {
        self.skills
            .get(self.selected)
            .filter(|skill| self.skill_matches(skill))
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.skills
            .iter()
            .enumerate()
            .filter_map(|(idx, skill)| self.skill_matches(skill).then_some(idx))
            .collect()
    }

    fn skill_matches(&self, skill: &SkillRecord) -> bool {
        fuzzy_match(&self.filter.query, &skill.name)
            || fuzzy_match(&self.filter.query, &skill.source)
            || fuzzy_match(&self.filter.query, &skill.description)
    }

    fn ensure_selection_visible(&mut self) {
        let visible = self.visible_indices();
        if let Some(first) = visible.first() {
            if selected_visible_position(self.selected, &visible).is_none() {
                self.selected = *first;
            }
        }
        self.preview_scroll = 0;
    }

    fn toggle_filter(&mut self) {
        self.filter.toggle();
        self.ensure_selection_visible();
    }

    fn filter_push(&mut self, ch: char) {
        self.filter.push(ch);
        self.ensure_selection_visible();
    }

    fn filter_backspace(&mut self) {
        self.filter.backspace();
        self.ensure_selection_visible();
    }

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);

        let visible = self.visible_indices();
        let items = if self.skills.is_empty() {
            vec![ListItem::new("No skills discovered").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No skills match filter").style(dim_style())]
        } else {
            visible
                .iter()
                .map(|idx| {
                    let skill = &self.skills[*idx];
                    let badge = if skill.managed {
                        format!("[{} managed] ", skill.source)
                    } else {
                        format!("[{}] ", skill.source)
                    };
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(badge, skill_source_style(skill)),
                            Span::styled(skill.name.clone(), title_style()),
                        ]),
                        Line::from(Span::styled(
                            if skill.description.is_empty() {
                                "No description".to_string()
                            } else {
                                truncate_line(&skill.description, 96)
                            },
                            dim_style(),
                        )),
                    ])
                })
                .collect::<Vec<_>>()
        };

        let mut state = ListState::default();
        if !visible.is_empty() {
            state.select(selected_visible_position(self.selected, &visible));
        }
        let title = format!("Skills ({})", self.filter.label());
        let list = List::new(items)
            .block(block(&title))
            .style(base_style())
            .highlight_style(highlight_style())
            .highlight_symbol("› ");
        frame.render_widget(Clear, body[0]);
        frame.render_stateful_widget(list, body[0], &mut state);

        let preview = self
            .selected_skill()
            .map(skill_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview_title = self
            .selected_skill()
            .map(|skill| compact_id(&skill.name))
            .unwrap_or_else(|| "Skill".to_string());
        let preview = Paragraph::new(preview)
            .block(block(&preview_title))
            .style(base_style())
            .scroll((self.preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);
    }
}

fn tool_preview(tool: &ToolEntry) -> String {
    format!(
        "{}\n{}:{}\n\n{}",
        tool.description,
        tool.path.display(),
        tool.line,
        sanitize_preview(&strip_tool_metadata_lines(&tool.preview))
    )
}

fn skill_source_style(skill: &SkillRecord) -> Style {
    if skill.managed {
        Style::default()
            .fg(CTP_GREEN)
            .bg(CTP_BASE)
            .add_modifier(Modifier::BOLD)
    } else {
        match skill.source.as_str() {
            "opencode" => Style::default().fg(CTP_LAVENDER).bg(CTP_BASE),
            "agents" => Style::default().fg(CTP_MAUVE).bg(CTP_BASE),
            "repo" => Style::default().fg(CTP_PEACH).bg(CTP_BASE),
            _ => dim_style(),
        }
    }
}

fn skill_preview(skill: &SkillRecord) -> String {
    let mut out = format!(
        "Name: {}\nSource: {}\nManaged: {}\nPath: {}\nRoot: {}\n",
        skill.name,
        skill.source,
        if skill.managed { "yes" } else { "no" },
        skill.path.display(),
        skill.root.display()
    );
    if !skill.description.trim().is_empty() {
        out.push_str(&format!("Description: {}\n", skill.description));
    }
    out.push_str("\n");
    match fs::read_to_string(&skill.path) {
        Ok(content) => out.push_str(&content),
        Err(error) => out.push_str(&format!("Unable to read skill file: {error}")),
    }
    sanitize_preview(&out)
}

fn suggestion_preview(suggestion: &SuggestionRecord) -> String {
    let mut out = format!(
        "ID: {}\nCreated: {}\nStatus: {}\n",
        suggestion.id, suggestion.created_at, suggestion.status
    );
    if !suggestion.target.trim().is_empty() {
        out.push_str(&format!("Target: {}\n", suggestion.target));
    }
    out.push_str("\nSuggestion:\n");
    out.push_str(&suggestion.text);
    if !suggestion.rationale.trim().is_empty() {
        out.push_str("\n\nRationale:\n");
        out.push_str(&suggestion.rationale);
    }
    if !suggestion.draft.trim().is_empty() {
        out.push_str("\n\nDraft:\n");
        out.push_str(&suggestion.draft);
    }
    if !suggestion.evidence.is_empty() {
        out.push_str("\n\nEvidence:\n");
        for evidence in &suggestion.evidence {
            out.push_str(&format!("- {}\n", evidence));
        }
    }
    if !suggestion.sources.is_empty() {
        out.push_str("\nSources:\n");
        for source in &suggestion.sources {
            let label = if !source.title.trim().is_empty() {
                source.title.as_str()
            } else if !source.chat_id.trim().is_empty() {
                source.chat_id.as_str()
            } else if !source.source_id.trim().is_empty() {
                source.source_id.as_str()
            } else {
                "unknown source"
            };
            out.push_str(&format!("- {}\n", label));
        }
    }
    sanitize_preview(&out)
}

fn strip_tool_metadata_lines(preview: &str) -> String {
    preview
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("# @name:")
                && !trimmed.starts_with("# @description:")
                && !trimmed.starts_with("// @name:")
                && !trimmed.starts_with("// @description:")
                && !trimmed.starts_with("-- @name:")
                && !trimmed.starts_with("-- @description:")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_id(id: &str) -> String {
    truncate_title(id.trim(), 64)
}

fn truncate_title(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else if truncated.is_empty() {
        "untitled".to_string()
    } else {
        truncated
    }
}

fn memory_preview(memory: &MemoryRecord) -> String {
    let mut out = format!(
        "ID: {}\nCreated: {}\nStatus: {}\n",
        memory.id, memory.created_at, memory.status
    );
    if !memory.scope.trim().is_empty() {
        out.push_str(&format!("Scope: {}\n", memory.scope));
    }
    if !memory.kind.trim().is_empty() {
        out.push_str(&format!("Kind: {}\n", memory.kind));
    }
    if !memory.confidence.trim().is_empty() {
        out.push_str(&format!("Confidence: {}\n", memory.confidence));
    }
    if !memory.not_before.trim().is_empty() {
        out.push_str(&format!("Not before: {}\n", memory.not_before));
    }
    out.push_str("\n");
    out.push_str(&memory.text);
    if !memory.evidence.is_empty() {
        out.push_str("\n\nEvidence:\n");
        for evidence in &memory.evidence {
            out.push_str(&format!("- {}\n", evidence));
        }
    }
    if !memory.sources.is_empty() {
        out.push_str("\nSources:\n");
        for source in &memory.sources {
            let label = if !source.title.trim().is_empty() {
                source.title.as_str()
            } else if !source.chat_id.trim().is_empty() {
                source.chat_id.as_str()
            } else if !source.source_id.trim().is_empty() {
                source.source_id.as_str()
            } else {
                "unknown source"
            };
            out.push_str(&format!("- {}", label));
            if !source.source.trim().is_empty() || !source.source_id.trim().is_empty() {
                out.push_str(&format!(" ({}/{})", source.source, source.source_id));
            }
            out.push('\n');
        }
    }
    out.push_str("\nActions: press `a` to review this memory, Space to select, `A` to select all visible, or `r`/`x`/Delete to remove selected/current memories.\n");
    sanitize_preview(&out)
}

fn truncate_line(value: &str, max_chars: usize) -> String {
    let line = value.lines().next().unwrap_or(value).trim();
    let mut chars = line.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn sanitize_preview(preview: &str) -> String {
    preview
        .chars()
        .filter_map(|ch| match ch {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            ch if ch.is_control() => None,
            ch => Some(ch),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    #[test]
    fn fuzzy_match_matches_subsequence_case_insensitive() {
        assert!(fuzzy_match("ocd", "OpenCode Debug Session"));
        assert!(fuzzy_match("tl", "tool-list"));
        assert!(!fuzzy_match("xyz", "tool-list"));
    }

    #[test]
    fn actionable_key_event_ignores_release_events() {
        let press = KeyEvent {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let repeat = KeyEvent {
            kind: KeyEventKind::Repeat,
            ..press
        };
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..press
        };

        assert!(actionable_key_event(&press));
        assert!(actionable_key_event(&repeat));
        assert!(!actionable_key_event(&release));
    }

    #[test]
    fn dashboard_palette_and_help_keys_use_control_shortcuts() {
        assert!(dashboard_palette_key(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL
        ));
        assert!(palette_previous_key(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL
        ));
        assert!(palette_next_key(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert!(dashboard_help_key(
            KeyCode::Char('/'),
            KeyModifiers::CONTROL
        ));
        assert!(dashboard_help_key(
            KeyCode::Char('_'),
            KeyModifiers::CONTROL
        ));

        assert!(!dashboard_palette_key(
            KeyCode::Char('p'),
            KeyModifiers::NONE
        ));
        assert!(!dashboard_help_key(KeyCode::Char('/'), KeyModifiers::NONE));
    }

    #[test]
    fn approval_preview_state_parses_and_navigates_files() {
        let mut state = ApprovalPreviewState::from_metadata(&serde_json::json!({
            "preview": [
                {
                    "operation": "update",
                    "relative_path": "src/lib.rs",
                    "lines_added": 1,
                    "lines_removed": 1,
                    "hunks": [
                        {
                            "lines": [
                                {"kind": "context", "content": "fn answer() -> i32 {"},
                                {"kind": "remove", "content": "    41"},
                                {"kind": "add", "content": "    42"},
                                {"kind": "context", "content": "}"}
                            ]
                        }
                    ]
                },
                {
                    "operation": "move",
                    "relative_path": "old.txt",
                    "relative_new_path": "new.txt",
                    "lines_added": 0,
                    "lines_removed": 0,
                    "hunks": []
                }
            ]
        }));

        assert!(!state.is_empty());
        assert_eq!(
            state.file_labels(),
            vec!["[ ] update src/lib.rs", "[ ] move old.txt -> new.txt"]
        );
        assert_eq!(state.selected_file().unwrap().path, "src/lib.rs");
        state.next_file();
        assert_eq!(state.selected_file_index(), 1);
        assert_eq!(
            state.selected_file().unwrap().new_path.as_deref(),
            Some("new.txt")
        );
        state.previous_file();
        assert_eq!(state.selected_file_index(), 0);
    }

    #[test]
    fn approval_preview_file_lines_render_hunk_prefixes() {
        let state = ApprovalPreviewState::from_metadata(&serde_json::json!({
            "preview": [
                {
                    "operation": "update",
                    "relative_path": "src/lib.rs",
                    "lines_added": 1,
                    "lines_removed": 1,
                    "hunks": [
                        {
                            "lines": [
                                {"kind": "context", "content": "fn answer() -> i32 {"},
                                {"kind": "remove", "content": "    41"},
                                {"kind": "add", "content": "    42"},
                                {"kind": "context", "content": "}"}
                            ]
                        }
                    ]
                }
            ]
        }));
        let rendered = state
            .selected_lines()
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(rendered
            .iter()
            .any(|line| line.contains("update src/lib.rs (+1/-1)")));
        assert!(rendered.iter().any(|line| line == "@@ hunk 1"));
        assert!(rendered.iter().any(|line| line == "  fn answer() -> i32 {"));
        assert!(rendered.iter().any(|line| line == "-     41"));
        assert!(rendered.iter().any(|line| line == "+     42"));
    }

    #[test]
    fn approval_preview_filter_limits_visible_hunk_lines() {
        let mut state = ApprovalPreviewState::from_metadata(&serde_json::json!({
            "preview": [
                {
                    "operation": "update",
                    "relative_path": "src/lib.rs",
                    "lines_added": 1,
                    "lines_removed": 1,
                    "hunks": [
                        {
                            "lines": [
                                {"kind": "context", "content": "fn answer() -> i32 {"},
                                {"kind": "remove", "content": "    41"},
                                {"kind": "add", "content": "    42"},
                                {"kind": "context", "content": "}"}
                            ]
                        }
                    ]
                }
            ]
        }));

        state.toggle_filter();
        for ch in "42".chars() {
            state.filter_push(ch);
        }
        state.finish_filter();
        let rendered = state
            .selected_lines()
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(state.filter_query(), "42");
        assert!(!state.filter_editing());
        assert!(rendered.iter().any(|line| line == "@@ hunk 1"));
        assert!(rendered.iter().any(|line| line == "+     42"));
        assert!(!rendered.iter().any(|line| line == "-     41"));
    }

    #[test]
    fn approval_preview_tracks_marked_file_resource_paths() {
        let mut state = ApprovalPreviewState::from_metadata(&serde_json::json!({
            "preview": [
                {
                    "operation": "update",
                    "path": "/tmp/work/src/lib.rs",
                    "relative_path": "src/lib.rs",
                    "lines_added": 1,
                    "lines_removed": 1,
                    "hunks": []
                },
                {
                    "operation": "move",
                    "path": "/tmp/work/old.txt",
                    "relative_path": "old.txt",
                    "new_path": "/tmp/work/new.txt",
                    "relative_new_path": "new.txt",
                    "lines_added": 0,
                    "lines_removed": 0,
                    "hunks": []
                }
            ]
        }));

        state.toggle_selected_file_approval();
        state.next_file();
        state.toggle_selected_file_approval();

        assert_eq!(state.approved_file_indices().len(), 2);
        assert_eq!(
            state.approved_paths(),
            vec![
                "/tmp/work/src/lib.rs".to_string(),
                "/tmp/work/old.txt".to_string(),
                "/tmp/work/new.txt".to_string(),
            ]
        );
        assert_eq!(
            state.file_labels(),
            vec!["[x] update src/lib.rs", "[x] move old.txt -> new.txt"]
        );
    }

    #[test]
    fn approval_dialog_app_navigates_files_and_scrolls() {
        let mut app = ApprovalDialogApp::new(serde_json::json!({
            "preview": [
                {"operation": "update", "relative_path": "a.txt", "lines_added": 1, "lines_removed": 0, "hunks": []},
                {"operation": "delete", "relative_path": "b.txt", "lines_added": 0, "lines_removed": 1, "hunks": []}
            ]
        }));

        assert_eq!(app.preview.selected_file().unwrap().path, "a.txt");
        app.next_file();
        assert_eq!(app.preview.selected_file().unwrap().path, "b.txt");
        app.scroll_down();
        assert_eq!(app.preview.scroll(), 1);
        app.previous_file();
        assert_eq!(app.preview.selected_file().unwrap().path, "a.txt");
        assert_eq!(app.preview.scroll(), 0);
    }

    #[test]
    fn approval_dialog_app_edits_filter_query() {
        let mut app = ApprovalDialogApp::new(serde_json::json!({
            "preview": [
                {"operation": "update", "relative_path": "a.txt", "lines_added": 1, "lines_removed": 0, "hunks": []}
            ]
        }));

        app.toggle_filter();
        app.filter_push('a');
        app.filter_push('b');
        app.filter_backspace();
        app.finish_filter();

        assert_eq!(app.preview.filter_query(), "a");
        assert!(!app.preview.filter_editing());
        app.toggle_filter();
        assert_eq!(app.preview.filter_query(), "");
    }

    #[test]
    fn approval_dialog_app_returns_marked_file_decision() {
        let mut app = ApprovalDialogApp::new(serde_json::json!({
            "preview": [
                {"operation": "update", "path": "/tmp/work/a.txt", "relative_path": "a.txt", "lines_added": 1, "lines_removed": 0, "hunks": []}
            ]
        }));

        assert!(app.approval_decision_for_marked_files().is_none());
        app.toggle_selected_file_approval();

        assert_eq!(
            app.approval_decision_for_marked_files(),
            Some(ApprovalDecision::ApprovePaths(vec![
                "/tmp/work/a.txt".to_string()
            ]))
        );
    }

    #[test]
    fn approval_dialog_app_returns_session_scope_decisions() {
        let mut app = ApprovalDialogApp::new(serde_json::json!({
            "preview": [
                {"operation": "update", "path": "/tmp/work/a.txt", "relative_path": "a.txt", "lines_added": 1, "lines_removed": 0, "hunks": []},
                {"operation": "move", "path": "/tmp/work/b.txt", "relative_path": "b.txt", "new_path": "/tmp/work/c.txt", "relative_new_path": "c.txt", "lines_added": 0, "lines_removed": 0, "hunks": []}
            ]
        }));

        assert_eq!(
            app.approval_decision_for_all_files_session(),
            ApprovalDecision::ApproveAllForSession(vec![
                "/tmp/work/a.txt".to_string(),
                "/tmp/work/b.txt".to_string(),
                "/tmp/work/c.txt".to_string(),
            ])
        );

        app.next_file();
        app.toggle_selected_file_approval();
        assert_eq!(
            app.approval_decision_for_marked_files_session(),
            Some(ApprovalDecision::ApprovePathsForSession(vec![
                "/tmp/work/b.txt".to_string(),
                "/tmp/work/c.txt".to_string(),
            ]))
        );
    }

    #[test]
    fn dashboard_tabs_follow_progression_order() {
        assert_eq!(
            DASHBOARD_TABS,
            ["Tools", "Sessions", "Memories", "Suggestions", "Skills"]
        );
        assert_eq!(DashboardTab::Tools.index(), 0);
        assert_eq!(DashboardTab::Sessions.index(), 1);
        assert_eq!(DashboardTab::Memories.index(), 2);
        assert_eq!(DashboardTab::Suggestions.index(), 3);
        assert_eq!(DashboardTab::Skills.index(), 4);
        assert_eq!(DashboardTab::from_index(5), DashboardTab::Tools);
    }

    #[test]
    fn folder_session_status_shortcuts_map_to_actions() {
        assert_eq!(
            folder_session_action_for_key(KeyCode::Char('r')),
            Some(FolderSessionAction::Run)
        );
        assert_eq!(
            folder_session_action_for_key(KeyCode::Char('b')),
            Some(FolderSessionAction::Buddy)
        );
        assert_eq!(
            folder_session_action_for_key(KeyCode::Char('w')),
            Some(FolderSessionAction::Watch)
        );
        assert_eq!(
            folder_session_action_for_key(KeyCode::Char('o')),
            Some(FolderSessionAction::OpenSummary)
        );
        assert_eq!(
            folder_session_action_for_key(KeyCode::Char('e')),
            Some(FolderSessionAction::EditRequest)
        );
        assert_eq!(
            folder_session_action_for_key(KeyCode::Char('c')),
            Some(FolderSessionAction::OpenContext)
        );
        assert_eq!(
            folder_session_action_for_key(KeyCode::Char('d')),
            Some(FolderSessionAction::DiscoverContext)
        );
        assert_eq!(folder_session_action_for_key(KeyCode::Char('x')), None);
    }

    #[test]
    fn folder_session_candidate_shortcuts_map_to_selected_candidate_actions() {
        let view = FolderSessionStatusView {
            title: "promotion".to_string(),
            state: "complete".to_string(),
            mode: Some("promotion".to_string()),
            promotion_type: Some("pattern".to_string()),
            session_dir: "/tmp/promotion".to_string(),
            summary_path: None,
            request_path: None,
            response_path: None,
            turn_count: 0,
            event_count: 0,
            candidate_status: Some("1 total, 0 accepted, 0 denied, 1 pending".to_string()),
            candidate_details: Vec::new(),
            candidate_entries: vec![PromotionCandidateRow {
                id: "todo-001".to_string(),
                candidate_type: Some("pattern".to_string()),
                status: "pending".to_string(),
                path: "/tmp/promotion/outputs/candidates/todo-001.toml".to_string(),
                text: Some("Polish promotion review workflow.".to_string()),
                rationale: Some("The session asks for a better review flow.".to_string()),
                evidence: vec!["/tmp/source/summary.md".to_string()],
                destination: Some("mindweaver".to_string()),
                writeback_path: None,
            }],
            next_action: None,
            note: None,
            message: Some("Accepted candidate todo-001".to_string()),
            latest_generation_response_path: Some(
                "/tmp/promotion/outputs/generation/latest-response.md".to_string(),
            ),
            latest_run_log_path: Some("/tmp/promotion/.djinn/runs/latest.log".to_string()),
            events_path: Some("/tmp/promotion/events.jsonl".to_string()),
            latest_event_rebuild_backup_path: Some(
                "/tmp/promotion/.djinn/backups/events-rebuild-latest".to_string(),
            ),
            candidates_dir: Some("/tmp/promotion/outputs/candidates".to_string()),
            source_packet_path: Some("/tmp/promotion/context/source-packet.md".to_string()),
            sources_manifest_path: Some("/tmp/promotion/context/sources.toml".to_string()),
        };

        assert_eq!(
            folder_session_candidate_action_for_key(KeyCode::Char('a'), &view, 0),
            Some(FolderSessionAction::AcceptCandidate("todo-001".to_string()))
        );
        assert_eq!(
            folder_session_candidate_action_for_key(KeyCode::Char('m'), &view, 0),
            Some(FolderSessionAction::AcceptCandidateAndSyncMindweaver(
                "todo-001".to_string()
            ))
        );
        assert_eq!(
            folder_session_candidate_action_for_key(KeyCode::Char('x'), &view, 0),
            Some(FolderSessionAction::DenyCandidate("todo-001".to_string()))
        );
        assert_eq!(
            folder_session_candidate_action_for_key(KeyCode::Enter, &view, 0),
            Some(FolderSessionAction::OpenCandidate(
                "/tmp/promotion/outputs/candidates/todo-001.toml".to_string()
            ))
        );

        let palette = folder_session_command_palette(&view, 0);
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Open Djinn UI chat"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Show keybindings"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Validate all candidates"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Validate selected candidate"));
        assert!(!palette
            .iter()
            .any(|entry| entry.label == "Show pattern export command"));
        assert!(!palette
            .iter()
            .any(|entry| entry.label == "Show selected pattern export command"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Accept selected candidate"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Open latest generation response"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Open latest run log"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Show validate-events command"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Show events preview command"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Show events rebuild command"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Show latest events restore command"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Open events ledger"));
        assert!(palette
            .iter()
            .any(|entry| entry.label == "Open source packet"));
        let mut palette_state = GroupedSelectState::default();
        palette_state.open();
        palette_state.query = "key".to_string();
        normalize_folder_session_palette_selection(&mut palette_state, &view, 0);
        assert_eq!(
            selected_folder_session_palette_command(&view, 0, &palette_state),
            Some(FolderSessionCommand::OpenHelp)
        );

        let detail = selected_promotion_candidate_detail_lines(&view.candidate_entries[0])
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(detail.contains("Selected candidate"));
        assert!(detail.contains("Id:      todo-001"));
        assert!(detail.contains("Dest:    mindweaver"));
        assert!(detail.contains("Text:    Polish promotion review workflow."));
        assert!(detail.contains("Why:     The session asks for a better review flow."));
        assert!(detail.contains("/tmp/source/summary.md"));
    }

    #[test]
    fn sessions_tab_filters_and_previews_folder_sessions() {
        let session = SessionRecord {
            name: "repo-review".to_string(),
            reference_name: "repo-review-1234567890".to_string(),
            path: "/tmp/repo-review".to_string(),
            state: "paused".to_string(),
            mode: Some("background".to_string()),
            updated_at: Some("2026-07-29T12:00:00Z".to_string()),
            repo_path: Some("/tmp/repo".to_string()),
            summary_preview: Some("Latest answer preview".to_string()),
            turn_count: 2,
            event_health: "ready:2/5".to_string(),
            candidate_status: Some("3 total, 1 accepted, 1 denied, 1 pending".to_string()),
            candidate_details: vec![
                "memory-001 [memory] accepted -> memory".to_string(),
                "todo-001 [todo] pending".to_string(),
            ],
            candidate_entries: vec![
                PromotionCandidateRow {
                    id: "memory-001".to_string(),
                    candidate_type: Some("memory".to_string()),
                    status: "accepted".to_string(),
                    path: "/tmp/repo-review/outputs/candidates/memory-001.toml".to_string(),
                    text: Some("Keep source sessions as promotion provenance.".to_string()),
                    rationale: None,
                    evidence: vec!["/tmp/repo-review/summary.md".to_string()],
                    destination: Some("memory".to_string()),
                    writeback_path: None,
                },
                PromotionCandidateRow {
                    id: "todo-001".to_string(),
                    candidate_type: Some("todo".to_string()),
                    status: "pending".to_string(),
                    path: "/tmp/repo-review/outputs/candidates/todo-001.toml".to_string(),
                    text: Some("Wire promotion todos into MindWeaver.".to_string()),
                    rationale: None,
                    evidence: vec!["/tmp/repo-review/turns/turn-1/response.md".to_string()],
                    destination: None,
                    writeback_path: None,
                },
            ],
            next_action: Some("edit request.md or run again".to_string()),
        };
        let mut app = SessionsApp::new(vec![session.clone()]);

        assert_eq!(app.visible_indices(), vec![0]);
        assert_eq!(
            app.visible_rows(),
            vec![
                SessionListRow::Header("Repo: repo".to_string()),
                SessionListRow::Session(0),
            ]
        );
        assert_eq!(
            session_state_badge(app.selected_session().unwrap()),
            "Ⅱ paused"
        );
        app.filter_push('r');
        app.filter_push('e');
        app.filter_push('p');
        app.filter_push('o');
        assert_eq!(app.selected_session().unwrap().name, "repo-review");

        let preview = session_preview(app.selected_session().unwrap());
        assert!(preview.contains("Name: repo-review"));
        assert!(preview.contains("Status: Ⅱ paused"));
        assert!(preview.contains("Events: ready:2/5"));
        assert!(preview.contains("Group: Repo: repo"));
        assert!(preview.contains("Next action: edit request.md or run again"));
        assert!(preview.contains("Focused shortcuts"));
        assert!(preview.contains("Candidates: 3 total"));
        assert!(preview.contains("memory-001 [memory] accepted"));
        assert!(preview.contains("evidence /tmp/repo-review/summary.md"));
        assert!(preview.contains("Latest answer preview"));
        assert!(
            session_list_metadata(app.selected_session().unwrap()).contains("candidates 3 total")
        );
        assert!(session_list_metadata(app.selected_session().unwrap()).contains("events ready:2/5"));
        assert!(app.selected_sessions().is_empty());
        app.toggle_selected();
        assert_eq!(app.selected_sessions().len(), 1);
        assert_eq!(app.selected_sessions()[0].name, "repo-review");

        let mut event_app = SessionsApp::new(vec![session]);
        event_app.filter_push('2');
        event_app.filter_push('/');
        event_app.filter_push('5');
        assert_eq!(event_app.visible_indices(), vec![0]);
    }

    #[test]
    fn dashboard_help_open_and_close() {
        let mut app = DashboardApp::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            DashboardTab::Sessions,
        );

        assert!(!app.help_open);
        app.open_help();
        assert!(app.help_open);
        app.close_help();
        assert!(!app.help_open);
    }

    #[test]
    fn dashboard_palette_scopes_commands_to_active_tab() {
        let sessions_app = DashboardApp::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            DashboardTab::Sessions,
        );
        let session_entries = sessions_app.dashboard_command_palette();

        assert!(session_entries.iter().any(|entry| {
            entry.section == "Sessions" && entry.command == DashboardCommand::OpenSelected
        }));
        assert!(session_entries.iter().any(|entry| {
            entry.section == "Sessions"
                && entry.command
                    == DashboardCommand::PromoteSelectedSessions(DashboardPromotionType::Memory)
        }));
        assert!(session_entries.iter().any(|entry| {
            entry.section == "Navigation"
                && entry.label == "Open Sessions"
                && entry.command == DashboardCommand::OpenTab(DashboardTab::Sessions)
        }));
        assert!(!session_entries.iter().any(|entry| {
            entry.section == "Skills" && entry.command == DashboardCommand::OpenSelected
        }));

        let skills_app = DashboardApp::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            DashboardTab::Skills,
        );
        let skill_entries = skills_app.dashboard_command_palette();
        assert!(skill_entries.iter().any(|entry| {
            entry.section == "Skills" && entry.command == DashboardCommand::OpenSelected
        }));
        assert!(!skill_entries.iter().any(|entry| {
            entry.section == "Sessions" && entry.command == DashboardCommand::OpenSelected
        }));
    }

    #[test]
    fn dashboard_palette_filters_and_selects_commands() {
        let mut app = DashboardApp::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            DashboardTab::Memories,
        );

        app.open_palette();
        for ch in "Review selected memory".chars() {
            app.push_palette_query(ch);
        }

        let visible = app.visible_palette_indices();
        assert!(!visible.is_empty());
        assert_eq!(
            app.selected_palette_command(),
            Some(DashboardCommand::AcceptSelected)
        );
    }

    #[test]
    fn memory_preview_includes_evidence_sources_and_actions() {
        let memory = MemoryRecord {
            id: "prefer-uv".to_string(),
            text: "Prefer uv in this repo".to_string(),
            created_at: "2026-07-09".to_string(),
            status: "active".to_string(),
            scope: "project".to_string(),
            kind: "tool-preference".to_string(),
            confidence: "high".to_string(),
            not_before: "2026-10-01".to_string(),
            evidence: vec!["User corrected pip to uv.".to_string()],
            sources: vec![djinn_memory::MemorySource {
                source_type: "chat".to_string(),
                source: "opencode".to_string(),
                source_id: "ses_123".to_string(),
                chat_id: "debugging-session".to_string(),
                title: "Debugging session".to_string(),
                captured_at: "2026-07-09".to_string(),
            }],
        };
        let preview = memory_preview(&memory);
        assert!(preview.contains("Status: active"));
        assert!(preview.contains("Not before: 2026-10-01"));
        assert!(preview.contains("User corrected pip to uv."));
        assert!(preview.contains("Debugging session"));
        assert!(preview.contains("review this memory"));
    }

    #[test]
    fn memories_app_lists_active_memories() {
        let first = MemoryRecord {
            id: "first-memory".to_string(),
            text: "Review this".to_string(),
            created_at: "2026-07-15".to_string(),
            status: "active".to_string(),
            scope: String::new(),
            kind: String::new(),
            confidence: String::new(),
            not_before: String::new(),
            evidence: Vec::new(),
            sources: Vec::new(),
        };
        let second = MemoryRecord {
            id: "second-memory".to_string(),
            text: "Also active".to_string(),
            ..first.clone()
        };

        let app = MemoriesApp::new(vec![first, second]);
        assert_eq!(app.memories.len(), 2);
        assert_eq!(app.memories[0].id, "first-memory");
    }

    #[test]
    fn suggestion_preview_shows_follow_up_fields_not_memory_metadata() {
        let suggestion = SuggestionRecord {
            id: "create-postgres-audit-note".to_string(),
            text: "Create a Postgres DDL audit cheatsheet.".to_string(),
            created_at: "2026-07-15".to_string(),
            status: "open".to_string(),
            target: "docs".to_string(),
            rationale: "The memory points to a reusable troubleshooting pattern.".to_string(),
            draft: "Include pg_stat_all_tables caveats and audit trigger examples.".to_string(),
            evidence: vec!["User clarified they wanted a Postgres query.".to_string()],
            sources: vec![djinn_memory::MemorySource {
                source_type: "memory".to_string(),
                source: "djinn".to_string(),
                source_id: "postgres-query-memory".to_string(),
                chat_id: String::new(),
                title: "Postgres query clarification".to_string(),
                captured_at: "2026-07-15".to_string(),
            }],
        };

        let preview = suggestion_preview(&suggestion);
        assert!(preview.contains("Target: docs"));
        assert!(preview.contains("Suggestion:\nCreate a Postgres DDL audit cheatsheet."));
        assert!(preview.contains("Rationale:"));
        assert!(preview.contains("Draft:"));
        assert!(preview.contains("Postgres query clarification"));
        assert!(!preview.contains("Kind: rule-proposal"));
        assert!(!preview.contains("Confidence:"));
    }

    #[test]
    fn skill_preview_includes_metadata_and_file_content() {
        let dir = std::env::temp_dir().join(format!("djinn-tui-skill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("SKILL.md");
        std::fs::write(&path, "# Skill: release\n\nSafe release workflow.\n").unwrap();
        let skill = SkillRecord {
            name: "release".to_string(),
            description: "Safe release workflow.".to_string(),
            source: "djinn".to_string(),
            path,
            root: dir.clone(),
            managed: true,
        };
        let preview = skill_preview(&skill);
        assert!(preview.contains("Name: release"));
        assert!(preview.contains("Managed: yes"));
        assert!(preview.contains("# Skill: release"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dashboard_header_shows_active_context() {
        let app = DashboardApp::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(ContextRecord {
                name: "djinn".to_string(),
                description: String::new(),
                roots: Vec::new(),
                skill_roots: Vec::new(),
                memory_scope: String::new(),
            }),
            DashboardTab::Tools,
        );
        assert_eq!(app.header_title(), "Djinn  ctx: djinn");
    }
}
