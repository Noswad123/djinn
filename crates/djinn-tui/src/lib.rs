mod approval;
mod filter;
mod grouped_select;
mod keys;
mod style;
mod terminal;

use std::collections::HashSet;
use std::fs;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use djinn_chats::ChatRecord;
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
        workspaces: Vec<WorkspaceRecord>,
        chats: Vec<ChatRecord>,
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
            workspaces,
            chats,
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
    pub session_dir: String,
    pub summary_path: Option<String>,
    pub request_path: Option<String>,
    pub response_path: Option<String>,
    pub turn_count: usize,
    pub next_action: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub name: String,
    pub reference_name: String,
    pub path: String,
    pub state: String,
    pub mode: Option<String>,
    pub updated_at: Option<String>,
    pub repo_path: Option<String>,
    pub summary_preview: Option<String>,
    pub turn_count: usize,
    pub next_action: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderSessionAction {
    Run,
    Watch,
    OpenSummary,
    EditRequest,
    OpenContext,
    DiscoverContext,
}

pub fn run_tools(tools: Vec<ToolEntry>) -> Result<()> {
    let mut terminal = enter_terminal()?;
    let result = run_tools_loop(&mut terminal, tools);
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_chats(chats: Vec<ChatRecord>) -> Result<Option<SessionPromoteRequest>> {
    let mut terminal = enter_terminal()?;
    let result = run_chats_loop(&mut terminal, chats);
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_dashboard(
    tools: Vec<ToolEntry>,
    workspaces: Vec<WorkspaceRecord>,
    chats: Vec<ChatRecord>,
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
        workspaces,
        chats,
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
    workspaces: Vec<WorkspaceRecord>,
    chats: Vec<ChatRecord>,
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
        workspaces,
        chats,
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
    OpenWorkspace(WorkspaceRecord),
    OpenChatSession(ChatSessionRequest),
    OpenTool(ToolEntry),
    OpenSkill(SkillRecord),
    PromoteSessions(SessionPromoteRequest),
    ReviewMemory(String),
    DeleteMemories(Vec<String>),
    DeleteChatRows(ChatDeleteRequest),
    DeleteSuggestions(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatDeleteRequest {
    pub chat_ids: Vec<String>,
    pub agent_session_ids: Vec<String>,
}

impl ChatDeleteRequest {
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.chat_ids.is_empty() && self.agent_session_ids.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSessionRequest {
    pub kind: ChatSessionKind,
    pub session_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatSessionKind {
    DjinnAgent,
    OpenCode,
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
    ToggleSelected,
    ToggleAll,
    AcceptSelected,
    RejectSelected,
    DeleteSelected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum SessionFilterScope {
    All,
    Promotable,
    DjinnAgent,
    ChildAgent,
}

impl SessionFilterScope {
    #[allow(dead_code)]
    const ALL: [Self; 4] = [
        Self::All,
        Self::Promotable,
        Self::DjinnAgent,
        Self::ChildAgent,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Promotable => "promotable",
            Self::DjinnAgent => "djinn-agent",
            Self::ChildAgent => "child-agent",
        }
    }

    #[allow(dead_code)]
    fn description(self) -> &'static str {
        match self {
            Self::All => "Show all session rows",
            Self::Promotable => "Show persisted rows that can be promoted",
            Self::DjinnAgent => "Show projected Djinn agent sessions",
            Self::ChildAgent => "Show projected child agent sessions with parent metadata",
        }
    }

    #[allow(dead_code)]
    fn next(self) -> Self {
        match self {
            Self::All => Self::Promotable,
            Self::Promotable => Self::DjinnAgent,
            Self::DjinnAgent => Self::ChildAgent,
            Self::ChildAgent => Self::All,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardTab {
    Tools,
    Workspaces,
    Memories,
    Suggestions,
    Skills,
}

impl DashboardTab {
    fn index(self) -> usize {
        match self {
            DashboardTab::Tools => 0,
            DashboardTab::Workspaces => 1,
            DashboardTab::Memories => 2,
            DashboardTab::Suggestions => 3,
            DashboardTab::Skills => 4,
        }
    }

    fn from_index(index: usize) -> Self {
        match index % DASHBOARD_TABS.len() {
            0 => DashboardTab::Tools,
            1 => DashboardTab::Workspaces,
            2 => DashboardTab::Memories,
            3 => DashboardTab::Suggestions,
            _ => DashboardTab::Skills,
        }
    }
}

const DASHBOARD_TABS: [&str; 5] = ["Tools", "Workspaces", "Memories", "Suggestions", "Skills"];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPromoteRequest {
    pub chat_ids: Vec<String>,
    pub mode: SessionPromoteMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPromoteMode {
    Summary,
    Pattern,
    Memories,
}

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
    loop {
        let view = load()?;
        terminal.draw(|frame| draw_folder_session_status(frame, &view))?;
        if event::poll(Duration::from_millis(1000))? {
            if let Event::Key(key) = event::read()? {
                if !actionable_key_event(&key) {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                    code => {
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
        KeyCode::Char('w') => Some(FolderSessionAction::Watch),
        KeyCode::Char('o') => Some(FolderSessionAction::OpenSummary),
        KeyCode::Char('e') => Some(FolderSessionAction::EditRequest),
        KeyCode::Char('c') => Some(FolderSessionAction::OpenContext),
        KeyCode::Char('d') => Some(FolderSessionAction::DiscoverContext),
        _ => None,
    }
}

fn draw_folder_session_status(frame: &mut ratatui::Frame<'_>, view: &FolderSessionStatusView) {
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

    let footer = Paragraph::new(
        "r run · w watch · o summary · e request · c context · d discover · q/Esc quit",
    )
    .style(dim_style());
    frame.render_widget(footer, chunks[2]);
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
    workspaces: Vec<WorkspaceRecord>,
    chats: Vec<ChatRecord>,
    memories: Vec<MemoryRecord>,
    suggestions: Vec<SuggestionRecord>,
    skills: Vec<SkillRecord>,
    active_context: Option<ContextRecord>,
    initial_tab: DashboardTab,
    mut on_continue_action: Option<&mut dyn FnMut(TuiAction) -> Result<()>>,
) -> Result<Option<TuiAction>> {
    let mut app = DashboardApp::new(
        tools,
        workspaces,
        chats,
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
                        DashboardTab::Workspaces => {
                            if let Some(workspace) = app.workspaces.selected_workspace().cloned() {
                                return Ok(Some(TuiAction::OpenWorkspace(workspace)));
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
                        DashboardTab::Tools | DashboardTab::Workspaces | DashboardTab::Skills => {}
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
            DashboardTab::Workspaces => {
                if let Some(workspace) = app.workspaces.selected_workspace().cloned() {
                    return Ok(Some(TuiAction::OpenWorkspace(workspace)));
                }
            }
            DashboardTab::Skills => {
                if let Some(skill) = app.skills.selected_skill().cloned() {
                    return Ok(Some(TuiAction::OpenSkill(skill)));
                }
            }
            DashboardTab::Memories | DashboardTab::Suggestions => {}
        },
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
    workspaces: WorkspacesApp,
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
        workspaces: Vec<WorkspaceRecord>,
        _chats: Vec<ChatRecord>,
        memories: Vec<MemoryRecord>,
        suggestions: Vec<SuggestionRecord>,
        skills: Vec<SkillRecord>,
        active_context: Option<ContextRecord>,
        initial_tab: DashboardTab,
    ) -> Self {
        Self {
            active_tab: initial_tab,
            tools: ToolsApp::new(tools),
            workspaces: WorkspacesApp::new(workspaces),
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
                label: "Open Workspaces".to_string(),
                description: "Jump to folder-backed workspaces".to_string(),
                command: DashboardCommand::OpenTab(DashboardTab::Workspaces),
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
            DashboardTab::Workspaces => vec![
                dashboard_command_entry(
                    "Workspaces",
                    "Open selected workspace",
                    "Open the highlighted folder-backed workspace",
                    DashboardCommand::OpenSelected,
                ),
                dashboard_command_entry(
                    "Workspaces",
                    "Filter workspaces",
                    "Edit the Workspaces filter",
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
            DashboardTab::Workspaces => self.workspaces.next(),
            DashboardTab::Memories => self.memories.next(),
            DashboardTab::Suggestions => self.suggestions.next(),
            DashboardTab::Skills => self.skills.next(),
        }
    }

    fn previous_item(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.previous(),
            DashboardTab::Workspaces => self.workspaces.previous(),
            DashboardTab::Memories => self.memories.previous(),
            DashboardTab::Suggestions => self.suggestions.previous(),
            DashboardTab::Skills => self.skills.previous(),
        }
    }

    fn scroll_down(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.scroll_down(),
            DashboardTab::Workspaces => self.workspaces.scroll_down(),
            DashboardTab::Memories => self.memories.scroll_down(),
            DashboardTab::Suggestions => self.suggestions.scroll_down(),
            DashboardTab::Skills => self.skills.scroll_down(),
        }
    }

    fn scroll_up(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.scroll_up(),
            DashboardTab::Workspaces => self.workspaces.scroll_up(),
            DashboardTab::Memories => self.memories.scroll_up(),
            DashboardTab::Suggestions => self.suggestions.scroll_up(),
            DashboardTab::Skills => self.skills.scroll_up(),
        }
    }

    fn filter_editing(&self) -> bool {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter.editing,
            DashboardTab::Workspaces => self.workspaces.filter.editing,
            DashboardTab::Memories => self.memories.filter.editing,
            DashboardTab::Suggestions => self.suggestions.filter.editing,
            DashboardTab::Skills => self.skills.filter.editing,
        }
    }

    fn toggle_filter(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.toggle_filter(),
            DashboardTab::Workspaces => self.workspaces.toggle_filter(),
            DashboardTab::Memories => self.memories.toggle_filter(),
            DashboardTab::Suggestions => self.suggestions.toggle_filter(),
            DashboardTab::Skills => self.skills.toggle_filter(),
        }
    }

    fn filter_push(&mut self, ch: char) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter_push(ch),
            DashboardTab::Workspaces => self.workspaces.filter_push(ch),
            DashboardTab::Memories => self.memories.filter_push(ch),
            DashboardTab::Suggestions => self.suggestions.filter_push(ch),
            DashboardTab::Skills => self.skills.filter_push(ch),
        }
    }

    fn filter_backspace(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter_backspace(),
            DashboardTab::Workspaces => self.workspaces.filter_backspace(),
            DashboardTab::Memories => self.memories.filter_backspace(),
            DashboardTab::Suggestions => self.suggestions.filter_backspace(),
            DashboardTab::Skills => self.skills.filter_backspace(),
        }
    }

    fn finish_filter_edit(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter.editing = false,
            DashboardTab::Workspaces => self.workspaces.filter.editing = false,
            DashboardTab::Memories => self.memories.filter.editing = false,
            DashboardTab::Suggestions => self.suggestions.filter.editing = false,
            DashboardTab::Skills => self.skills.filter.editing = false,
        }
    }

    fn toggle_selected(&mut self) {
        match self.active_tab {
            DashboardTab::Memories => self.memories.toggle_selected(),
            DashboardTab::Suggestions => self.suggestions.toggle_selected(),
            DashboardTab::Tools | DashboardTab::Workspaces | DashboardTab::Skills => {}
        }
    }

    fn toggle_all(&mut self) {
        match self.active_tab {
            DashboardTab::Memories => self.memories.toggle_all(),
            DashboardTab::Suggestions => self.suggestions.toggle_all(),
            DashboardTab::Tools | DashboardTab::Workspaces | DashboardTab::Skills => {}
        }
    }

    fn reject_selected_action(&self) -> Option<TuiAction> {
        match self.active_tab {
            DashboardTab::Memories => {
                let ids = self.memories.selected_memory_ids();
                (!ids.is_empty()).then_some(TuiAction::DeleteMemories(ids))
            }
            DashboardTab::Tools
            | DashboardTab::Workspaces
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
            DashboardTab::Tools | DashboardTab::Workspaces | DashboardTab::Skills => None,
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
            TuiAction::OpenWorkspace(_)
            | TuiAction::DeleteChatRows(_)
            | TuiAction::OpenTool(_)
            | TuiAction::OpenChatSession(_)
            | TuiAction::OpenSkill(_)
            | TuiAction::PromoteSessions(_)
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
            DashboardTab::Workspaces => self.workspaces.draw_body(frame, chunks[1]),
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
            Line::from(Span::styled("Workspaces", title_style())),
            Line::from(vec![
                Span::styled("Enter", selected_style()),
                Span::raw(" open focused folder-backed workspace"),
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

struct WorkspacesApp {
    workspaces: Vec<WorkspaceRecord>,
    selected: usize,
    preview_scroll: u16,
    filter: FilterState,
}

impl WorkspacesApp {
    fn new(workspaces: Vec<WorkspaceRecord>) -> Self {
        Self {
            workspaces,
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

    fn selected_workspace(&self) -> Option<&WorkspaceRecord> {
        self.workspaces
            .get(self.selected)
            .filter(|workspace| self.workspace_matches(workspace))
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.workspaces
            .iter()
            .enumerate()
            .filter_map(|(idx, workspace)| self.workspace_matches(workspace).then_some(idx))
            .collect()
    }

    fn workspace_matches(&self, workspace: &WorkspaceRecord) -> bool {
        fuzzy_match(&self.filter.query, &workspace.name)
            || fuzzy_match(&self.filter.query, &workspace.reference_name)
            || fuzzy_match(&self.filter.query, &workspace.path)
            || fuzzy_match(&self.filter.query, &workspace.state)
            || workspace
                .repo_path
                .as_ref()
                .is_some_and(|repo| fuzzy_match(&self.filter.query, repo))
            || workspace
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

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(area);

        let visible = self.visible_indices();
        let items = if self.workspaces.is_empty() {
            vec![ListItem::new("No workspaces found").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No workspaces match filter").style(dim_style())]
        } else {
            visible
                .iter()
                .map(|idx| {
                    let workspace = &self.workspaces[*idx];
                    ListItem::new(vec![
                        Line::from(Span::styled(workspace.name.clone(), title_style())),
                        Line::from(Span::styled(
                            workspace_list_metadata(workspace),
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
            "Workspaces ({} / {} visible, {})",
            visible.len(),
            self.workspaces.len(),
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
            .selected_workspace()
            .map(workspace_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview_title = self
            .selected_workspace()
            .map(|workspace| workspace.name.as_str())
            .unwrap_or("Workspace");
        let preview = Paragraph::new(preview)
            .block(block(preview_title))
            .style(base_style())
            .scroll((self.preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);
    }
}

fn workspace_list_metadata(workspace: &WorkspaceRecord) -> String {
    let mode = workspace.mode.as_deref().unwrap_or("-");
    let updated = workspace.updated_at.as_deref().unwrap_or("unknown");
    let next = workspace.next_action.as_deref().unwrap_or("-");
    format!(
        "{} / {} · {} turns · updated {} · next {}",
        workspace.state, mode, workspace.turn_count, updated, next
    )
}

fn workspace_preview(workspace: &WorkspaceRecord) -> String {
    let mut lines = vec![
        format!("Name: {}", workspace.name),
        format!("Reference: {}", workspace.reference_name),
        format!("Path: {}", workspace.path),
        format!("State: {}", workspace.state),
        format!("Mode: {}", workspace.mode.as_deref().unwrap_or("-")),
        format!("Turns: {}", workspace.turn_count),
        format!(
            "Updated: {}",
            workspace.updated_at.as_deref().unwrap_or("unknown")
        ),
    ];
    if let Some(repo) = &workspace.repo_path {
        lines.push(format!("Repo: {repo}"));
    }
    if let Some(next) = &workspace.next_action {
        lines.push(format!("Next: {next}"));
    }
    lines.push(String::new());
    lines.push("Enter opens the focused workspace view.".to_string());
    lines.push(
        "Focused shortcuts: r run, w watch, o summary, e request, c context, d discover."
            .to_string(),
    );
    if let Some(summary) = &workspace.summary_preview {
        lines.push(String::new());
        lines.push("Summary".to_string());
        lines.push(summary.clone());
    }
    lines.join("\n")
}

fn run_chats_loop(
    terminal: &mut TuiTerminal,
    chats: Vec<ChatRecord>,
) -> Result<Option<SessionPromoteRequest>> {
    let mut app = ChatsApp::new(chats);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if !actionable_key_event(&key) {
                    continue;
                }
                match &app.mode {
                    ChatUiMode::Selecting => match key.code {
                        _ if app.filter.editing => match key.code {
                            KeyCode::Char('/') => app.toggle_filter(),
                            KeyCode::Backspace => app.filter_backspace(),
                            KeyCode::Enter | KeyCode::Esc => app.filter.editing = false,
                            KeyCode::Char(ch) => app.filter_push(ch),
                            _ => {}
                        },
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                        KeyCode::Char('/') => app.toggle_filter(),
                        KeyCode::Char('j') | KeyCode::Down => app.next(),
                        KeyCode::Char('k') | KeyCode::Up => app.previous(),
                        KeyCode::Char('d') | KeyCode::PageDown => app.scroll_down(),
                        KeyCode::Char('u') | KeyCode::PageUp => app.scroll_up(),
                        KeyCode::Char(' ') => app.toggle_selected(),
                        KeyCode::Char('a') => app.toggle_all(),
                        KeyCode::Enter => app.open_options(),
                        _ => {}
                    },
                    ChatUiMode::Options => match key.code {
                        KeyCode::Char('q') => return Ok(None),
                        KeyCode::Esc | KeyCode::Backspace => app.mode = ChatUiMode::Selecting,
                        KeyCode::Char('j') | KeyCode::Down => app.next_option(),
                        KeyCode::Char('k') | KeyCode::Up => app.previous_option(),
                        KeyCode::Enter => return Ok(app.promote_request()),
                        _ => {}
                    },
                    ChatUiMode::ConfirmDelete(_) => match key.code {
                        KeyCode::Esc
                        | KeyCode::Backspace
                        | KeyCode::Char('n')
                        | KeyCode::Char('q') => app.cancel_modal(),
                        _ => {}
                    },
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
enum ChatUiMode {
    Selecting,
    Options,
    ConfirmDelete(ChatDeleteRequest),
}

struct ChatsApp {
    chats: Vec<ChatRecord>,
    selected: usize,
    preview_scroll: u16,
    checked: HashSet<String>,
    mode: ChatUiMode,
    option_selected: usize,
    scope: SessionFilterScope,
    filter: FilterState,
}

impl ChatsApp {
    fn new(chats: Vec<ChatRecord>) -> Self {
        Self {
            chats,
            selected: 0,
            preview_scroll: 0,
            checked: HashSet::new(),
            mode: ChatUiMode::Selecting,
            option_selected: 0,
            scope: SessionFilterScope::All,
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

    fn selected_chat(&self) -> Option<&ChatRecord> {
        self.chats
            .get(self.selected)
            .filter(|chat| self.chat_matches(chat))
    }

    #[allow(dead_code)]
    fn selected_chat_session_request(&self) -> Option<ChatSessionRequest> {
        self.selected_chat().and_then(chat_session_request)
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.chats
            .iter()
            .enumerate()
            .filter_map(|(idx, chat)| self.chat_matches(chat).then_some(idx))
            .collect()
    }

    fn chat_matches(&self, chat: &ChatRecord) -> bool {
        self.scope_matches(chat)
            && (fuzzy_match(&self.filter.query, &chat.title)
                || fuzzy_match(&self.filter.query, &chat.id)
                || fuzzy_match(&self.filter.query, &chat.source)
                || fuzzy_match(&self.filter.query, &chat.source_id)
                || fuzzy_match(&self.filter.query, &chat.source_path)
                || fuzzy_match(&self.filter.query, &chat.content_path)
                || fuzzy_match(&self.filter.query, &chat.content))
    }

    fn scope_matches(&self, chat: &ChatRecord) -> bool {
        match self.scope {
            SessionFilterScope::All => true,
            SessionFilterScope::Promotable => chat.source.trim() != "djinn-agent",
            SessionFilterScope::DjinnAgent => chat.source.trim() == "djinn-agent",
            SessionFilterScope::ChildAgent => {
                chat.source.trim() == "djinn-agent"
                    && chat_content_metadata_value(chat, "Parent session").is_some()
            }
        }
    }

    #[allow(dead_code)]
    fn set_scope(&mut self, scope: SessionFilterScope) {
        self.scope = scope;
        self.ensure_selection_visible();
    }

    #[allow(dead_code)]
    fn cycle_scope(&mut self) {
        self.set_scope(self.scope.next());
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

    fn selected_promotable_session_ids(&self) -> Vec<String> {
        self.selected_chats()
            .into_iter()
            .filter(|chat| chat.source != "djinn-agent")
            .map(|chat| chat.id.clone())
            .collect()
    }

    fn selected_chats(&self) -> Vec<&ChatRecord> {
        if self.checked.is_empty() {
            return self
                .selected_chat()
                .map(|chat| vec![chat])
                .unwrap_or_default();
        }
        self.chats
            .iter()
            .filter(|chat| self.checked.contains(&chat.id))
            .collect()
    }

    #[allow(dead_code)]
    fn delete_request(&self) -> Option<ChatDeleteRequest> {
        let mut request = ChatDeleteRequest {
            chat_ids: Vec::new(),
            agent_session_ids: Vec::new(),
        };
        for chat in self.selected_chats() {
            if chat.source == "djinn-agent" {
                let session_id = chat.source_id.trim();
                if !session_id.is_empty()
                    && !request.agent_session_ids.iter().any(|id| id == session_id)
                {
                    request.agent_session_ids.push(session_id.to_string());
                }
            } else if !request.chat_ids.iter().any(|id| id == &chat.id) {
                request.chat_ids.push(chat.id.clone());
            }
        }
        (!request.is_empty()).then_some(request)
    }

    #[allow(dead_code)]
    fn open_delete_confirmation(&mut self) {
        if let Some(request) = self.delete_request() {
            self.mode = ChatUiMode::ConfirmDelete(request);
        }
    }

    #[allow(dead_code)]
    fn confirm_delete_action(&mut self) -> Option<TuiAction> {
        let ChatUiMode::ConfirmDelete(request) = self.mode.clone() else {
            return None;
        };
        self.mode = ChatUiMode::Selecting;
        Some(TuiAction::DeleteChatRows(request))
    }

    fn cancel_modal(&mut self) {
        self.mode = ChatUiMode::Selecting;
    }

    fn toggle_selected(&mut self) {
        if let Some(id) = self.selected_chat().map(|chat| chat.id.clone()) {
            if !self.checked.insert(id.clone()) {
                self.checked.remove(&id);
            }
        }
    }

    fn toggle_all(&mut self) {
        let visible = self.visible_indices();
        let visible_ids = visible
            .iter()
            .map(|idx| self.chats[*idx].id.clone())
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

    #[allow(dead_code)]
    fn remove_deleted_rows(&mut self, request: &ChatDeleteRequest) {
        let removed_chats = request.chat_ids.iter().cloned().collect::<HashSet<_>>();
        let removed_sessions = request
            .agent_session_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        self.chats.retain(|chat| {
            !removed_chats.contains(&chat.id)
                && !(chat.source == "djinn-agent" && removed_sessions.contains(&chat.source_id))
        });
        self.checked.retain(|id| !removed_chats.contains(id));
        self.checked.retain(|id| {
            self.chats
                .iter()
                .any(|chat| chat.id == *id || chat.source_id == *id)
        });
        if self.selected >= self.chats.len() {
            self.selected = self.chats.len().saturating_sub(1);
        }
        self.mode = ChatUiMode::Selecting;
        self.ensure_selection_visible();
    }

    fn open_options(&mut self) {
        if !self.selected_promotable_session_ids().is_empty() {
            self.mode = ChatUiMode::Options;
        }
    }

    fn next_option(&mut self) {
        self.option_selected = (self.option_selected + 1).min(2);
    }

    fn previous_option(&mut self) {
        self.option_selected = self.option_selected.saturating_sub(1);
    }

    fn selected_promote_mode(&self) -> SessionPromoteMode {
        match self.option_selected {
            0 => SessionPromoteMode::Summary,
            1 => SessionPromoteMode::Pattern,
            _ => SessionPromoteMode::Memories,
        }
    }

    fn promote_request(&self) -> Option<SessionPromoteRequest> {
        let chat_ids = self.selected_promotable_session_ids();
        if chat_ids.is_empty() {
            return None;
        }
        Some(SessionPromoteRequest {
            chat_ids,
            mode: self.selected_promote_mode(),
        })
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(frame.area());

        self.draw_body(frame, chunks[0]);

        let help = Paragraph::new(
            "↑/k ↓/j move • Space select • a all visible • f scope • / search • Enter resume/promote • x/Delete remove • q/Esc quit",
        )
        .style(dim_style());
        frame.render_widget(Clear, chunks[1]);
        frame.render_widget(help, chunks[1]);

        if self.mode == ChatUiMode::Options {
            self.draw_options(frame);
        }
        if let ChatUiMode::ConfirmDelete(request) = &self.mode {
            self.draw_delete_confirmation(frame, request);
        }
    }

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);

        let visible = self.visible_indices();
        let items = if self.chats.is_empty() {
            vec![ListItem::new("No sessions recorded").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No sessions match filter").style(dim_style())]
        } else {
            visible
                .iter()
                .map(|idx| {
                    let chat = &self.chats[*idx];
                    let checked = if self.checked.contains(&chat.id) {
                        "[x]"
                    } else {
                        "[ ]"
                    };
                    let metadata = chat_list_metadata(chat);
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
                            Span::styled(chat.title.clone(), title_style()),
                        ]),
                        Line::from(Span::styled(metadata, dim_style())),
                    ])
                })
                .collect::<Vec<_>>()
        };

        let mut state = ListState::default();
        if !visible.is_empty() {
            state.select(selected_visible_position(self.selected, &visible));
        }
        let title = format!(
            "Sessions ({} / {} visible, {} selected, scope: {}, {})",
            visible.len(),
            self.chats.len(),
            self.checked.len(),
            self.scope.label(),
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
            .selected_chat()
            .map(chat_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview_title = self
            .selected_chat()
            .map(|chat| chat_preview_title(chat, &self.chats))
            .unwrap_or_else(|| "Chat".to_string());
        let preview = Paragraph::new(preview)
            .block(block(&preview_title))
            .style(base_style())
            .scroll((self.preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);
    }

    fn draw_options(&self, frame: &mut ratatui::Frame) {
        let area = centered_rect(58, 42, frame.area());
        let mode_names = ["summary", "pattern", "memories"];
        let mut lines = vec![
            Line::from(Span::styled("Promote selected sessions", title_style())),
            Line::from(Span::styled(
                format!(
                    "Promotable sessions: {}",
                    self.selected_promotable_session_ids().len()
                ),
                dim_style(),
            )),
            Line::from(""),
        ];
        for (idx, name) in mode_names.iter().enumerate() {
            let marker = if idx == self.option_selected {
                "›"
            } else {
                " "
            };
            let style = if idx == self.option_selected {
                selected_style()
            } else {
                base_style()
            };
            lines.push(Line::from(Span::styled(format!("{marker} {name}"), style)));
        }
        lines.push(Line::from(Span::styled(
            "Enter promote • Esc back",
            dim_style(),
        )));

        let modal = Paragraph::new(lines)
            .block(block("Promote Options"))
            .style(base_style())
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, area);
        frame.render_widget(modal, area);
    }

    fn draw_delete_confirmation(&self, frame: &mut ratatui::Frame, request: &ChatDeleteRequest) {
        let area = centered_rect(58, 34, frame.area());
        let total = request.chat_ids.len() + request.agent_session_ids.len();
        let mut lines = vec![
            Line::from(Span::styled("Confirm removal", title_style())),
            Line::from(""),
            Line::from(format!("Selected items: {total}")),
        ];
        if !request.chat_ids.is_empty() {
            lines.push(Line::from(format!(
                "Session rows: {}",
                request.chat_ids.len()
            )));
            for id in request.chat_ids.iter().take(3) {
                lines.push(Line::from(Span::styled(
                    format!("  - {}", truncate_line(id, 52)),
                    dim_style(),
                )));
            }
        }
        if !request.agent_session_ids.is_empty() {
            lines.push(Line::from(format!(
                "Djinn sessions: {}",
                request.agent_session_ids.len()
            )));
            for id in request.agent_session_ids.iter().take(3) {
                lines.push(Line::from(Span::styled(
                    format!("  - {}", truncate_line(id, 52)),
                    dim_style(),
                )));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "This removes selected session rows and deletes selected Djinn session JSONL files.",
            dim_style(),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Enter / y", selected_style()),
            Span::raw(" delete  •  "),
            Span::styled("Esc / n", selected_style()),
            Span::raw(" cancel"),
        ]));

        let modal = Paragraph::new(lines)
            .block(block("Confirm Delete"))
            .style(base_style())
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, area);
        frame.render_widget(modal, area);
    }
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

fn chat_preview(chat: &ChatRecord) -> String {
    let mut out = format!(
        "ID: {}\nTitle: {}\nCreated: {}\n",
        chat.id, chat.title, chat.created_at
    );
    out.push_str(&format!("Actions: {}\n", chat_picker_action_hint(chat)));
    if chat.source.trim() == "djinn-agent" {
        if let Some(role) = chat_content_metadata_value(chat, "Agent role") {
            out.push_str(&format!("Agent role: {role}\n"));
        }
        if let Some(parent) = chat_content_metadata_value(chat, "Parent session") {
            out.push_str(&format!("Parent session: {parent}\n"));
        }
        if let Some(profile) = chat_content_metadata_value(chat, "Profile") {
            out.push_str(&format!("Profile: {profile}\n"));
        }
    }
    if !chat.source.trim().is_empty() {
        out.push_str(&format!("Source: {}\n", chat.source));
    }
    if !chat.source_id.trim().is_empty() {
        out.push_str(&format!("Source ID: {}\n", chat.source_id));
    }
    if !chat.source_path.trim().is_empty() {
        out.push_str(&format!("Source path: {}\n", chat.source_path));
    }
    out.push_str("\n");
    out.push_str(&sanitize_preview(&chat.content));
    out
}

fn chat_picker_action_hint(chat: &ChatRecord) -> &'static str {
    match chat.source.trim() {
        "djinn-agent" => "Enter/r resume session • x delete session (confirm)",
        "opencode" if !chat.source_id.trim().is_empty() => {
            "Enter/r convert+resume in Djinn • s promote • x remove (confirm)"
        }
        _ => "Enter/s promote options • x remove (confirm)",
    }
}

fn chat_source_label(chat: &ChatRecord) -> String {
    if chat.source.trim() == "djinn-agent" {
        let mut parts = vec!["Djinn agent".to_string()];
        if let Some(role) = chat_content_metadata_value(chat, "Agent role") {
            parts.push(format!("role: {role}"));
        }
        if let Some(parent) = chat_content_metadata_value(chat, "Parent session") {
            parts.push(format!("parent: {parent}"));
        }
        if parts.len() == 1 && !chat.source_id.trim().is_empty() {
            parts.push(chat.source_id.trim().to_string());
        }
        return format!(" • {}", parts.join(" • "));
    }
    if !chat.source.trim().is_empty() && !chat.source_id.trim().is_empty() {
        format!(" • {}:{}", chat.source, chat.source_id)
    } else if !chat.source.trim().is_empty() {
        format!(" • {}", chat.source)
    } else if !chat.source_id.trim().is_empty() {
        format!(" • {}", chat.source_id)
    } else {
        String::new()
    }
}

fn chat_list_metadata(chat: &ChatRecord) -> String {
    if chat.source.trim() == "djinn-agent" {
        let mut parts = Vec::new();
        if let Some(role) = chat_content_metadata_value(chat, "Agent role") {
            parts.push(format!("role: {role}"));
        }
        if let Some(parent) = chat_content_metadata_value(chat, "Parent session") {
            parts.push(format!("parent: {parent}"));
        }
        if let Some(profile) = chat_content_metadata_value(chat, "Profile") {
            parts.push(format!("profile: {profile}"));
        }
        if let Some(events) = chat_content_metadata_value(chat, "Events") {
            parts.push(format!("{events} events"));
        }
        if parts.is_empty() {
            let id = chat.source_id.trim();
            if id.is_empty() {
                "Djinn agent".to_string()
            } else {
                format!("Djinn agent • {id}")
            }
        } else {
            format!("Djinn agent • {}", parts.join(" • "))
        }
    } else {
        format!(
            "{} chars{}",
            chat.content.chars().count(),
            chat_source_label(chat)
        )
    }
}

fn chat_content_metadata_value<'a>(chat: &'a ChatRecord, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    chat.content.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

#[allow(dead_code)]
fn chat_session_request(chat: &ChatRecord) -> Option<ChatSessionRequest> {
    let source = chat.source.trim();
    let source_id = chat.source_id.trim();
    if source_id.is_empty() {
        return None;
    }
    let kind = match source {
        "djinn-agent" => ChatSessionKind::DjinnAgent,
        "opencode" => ChatSessionKind::OpenCode,
        _ => return None,
    };
    Some(ChatSessionRequest {
        kind,
        session_id: source_id.to_string(),
        title: chat.title.clone(),
    })
}

fn chat_preview_title(chat: &ChatRecord, chats: &[ChatRecord]) -> String {
    let title = chat.title.trim();
    if !title.is_empty()
        && chats
            .iter()
            .filter(|candidate| candidate.title.trim() == title)
            .count()
            == 1
    {
        truncate_title(title, 64)
    } else {
        compact_id(&chat.id)
    }
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

    fn test_chat_record(id: &str, title: &str, source: &str, source_id: &str) -> ChatRecord {
        ChatRecord {
            id: id.to_string(),
            title: title.to_string(),
            content: String::new(),
            source: source.to_string(),
            source_id: source_id.to_string(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        }
    }

    #[test]
    fn strip_tool_metadata_lines_removes_name_and_description_tags() {
        let preview =
            "# @name: foo\n# @description: does foo\necho foo\n-- @name: luafoo\nprint('foo')";
        let stripped = strip_tool_metadata_lines(preview);
        assert!(!stripped.contains("@name"));
        assert!(!stripped.contains("@description"));
        assert!(stripped.contains("echo foo"));
        assert!(stripped.contains("print('foo')"));
    }

    #[test]
    fn chat_preview_title_uses_unique_title_else_id() {
        let unique = ChatRecord {
            id: "chat-one-id".to_string(),
            title: "Unique title".to_string(),
            content: String::new(),
            source: String::new(),
            source_id: String::new(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        };
        let duplicate_a = ChatRecord {
            id: "chat-two-id".to_string(),
            title: "Duplicate".to_string(),
            content: String::new(),
            source: String::new(),
            source_id: String::new(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        };
        let duplicate_b = ChatRecord {
            id: "chat-three-id".to_string(),
            title: "Duplicate".to_string(),
            content: String::new(),
            source: String::new(),
            source_id: String::new(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        };
        let chats = vec![unique.clone(), duplicate_a.clone(), duplicate_b];
        assert_eq!(chat_preview_title(&unique, &chats), "Unique title");
        assert_eq!(chat_preview_title(&duplicate_a, &chats), "chat-two-id");
    }

    #[test]
    fn chats_can_request_djinn_or_opencode_session_resume() {
        let djinn = test_chat_record("agent:agt_1", "Djinn", "djinn-agent", "agt_1");
        let opencode = test_chat_record("chat", "OpenCode", "opencode", "ses_1");

        assert_eq!(
            chat_session_request(&djinn).map(|request| (request.kind, request.session_id)),
            Some((ChatSessionKind::DjinnAgent, "agt_1".to_string()))
        );
        assert_eq!(
            chat_session_request(&opencode).map(|request| (request.kind, request.session_id)),
            Some((ChatSessionKind::OpenCode, "ses_1".to_string()))
        );
    }

    #[test]
    fn chats_filter_matches_source_paths_and_content() {
        let mut app = ChatsApp::new(vec![
            ChatRecord {
                id: "chat-one".to_string(),
                title: "Architecture notes".to_string(),
                content: "Discussed command palettes".to_string(),
                source: "opencode".to_string(),
                source_id: "ses_alpha".to_string(),
                source_path: "/tmp/opencode/ses_alpha.json".to_string(),
                content_path: "/tmp/cache/chat-one.md".to_string(),
                created_at: String::new(),
            },
            ChatRecord {
                id: "chat-two".to_string(),
                title: "Other".to_string(),
                content: "Unrelated".to_string(),
                source: "manual".to_string(),
                source_id: String::new(),
                source_path: String::new(),
                content_path: String::new(),
                created_at: String::new(),
            },
        ]);

        app.filter.query = "ocsa".to_string();
        assert_eq!(app.visible_indices(), vec![0]);

        app.filter.query = "cmdpal".to_string();
        assert_eq!(app.visible_indices(), vec![0]);
    }

    #[test]
    fn sessions_filter_matches_agent_role_and_parent_metadata() {
        let mut agent =
            test_chat_record("agent:agt_child", "Review diff", "djinn-agent", "agt_child");
        agent.content = "Djinn agent session\n\nID: agt_child\nProfile: default\nEvents: 7\nAgent role: reviewer\nParent session: agt_parent".to_string();
        let mut app = ChatsApp::new(vec![
            agent,
            test_chat_record("chat-one", "Manual", "manual", ""),
        ]);

        app.filter.query = "reviewer".to_string();
        assert_eq!(app.visible_indices(), vec![0]);

        app.filter.query = "agt_parent".to_string();
        assert_eq!(app.visible_indices(), vec![0]);
    }

    #[test]
    fn sessions_scope_filter_uses_projected_session_metadata() {
        let mut child_agent =
            test_chat_record("agent:agt_child", "Child", "djinn-agent", "agt_child");
        child_agent.content = "Djinn agent session\n\nID: agt_child\nProfile: default\nEvents: 7\nAgent role: reviewer\nParent session: agt_parent".to_string();
        let parent_agent =
            test_chat_record("agent:agt_parent", "Parent", "djinn-agent", "agt_parent");
        let manual = test_chat_record("chat-one", "Manual", "manual", "");
        let mut app = ChatsApp::new(vec![child_agent, parent_agent, manual]);

        assert_eq!(app.visible_indices(), vec![0, 1, 2]);

        app.set_scope(SessionFilterScope::Promotable);
        assert_eq!(app.visible_indices(), vec![2]);

        app.set_scope(SessionFilterScope::DjinnAgent);
        assert_eq!(app.visible_indices(), vec![0, 1]);

        app.set_scope(SessionFilterScope::ChildAgent);
        assert_eq!(app.visible_indices(), vec![0]);

        app.cycle_scope();
        assert_eq!(app.scope, SessionFilterScope::All);
        assert_eq!(app.visible_indices(), vec![0, 1, 2]);
    }

    #[test]
    fn chats_delete_request_separates_saved_chats_and_agent_sessions() {
        let mut app = ChatsApp::new(vec![
            test_chat_record("chat-one", "Saved", "manual", ""),
            test_chat_record("agent:agt_1", "Agent", "djinn-agent", "agt_1"),
            test_chat_record("chat-two", "Other", "opencode", "ses_2"),
        ]);
        app.checked.insert("chat-one".to_string());
        app.checked.insert("agent:agt_1".to_string());

        let request = app.delete_request().unwrap();

        assert_eq!(request.chat_ids, vec!["chat-one"]);
        assert_eq!(request.agent_session_ids, vec!["agt_1"]);

        app.remove_deleted_rows(&request);

        assert_eq!(app.chats.len(), 1);
        assert_eq!(app.chats[0].id, "chat-two");
        assert!(app.checked.is_empty());
    }

    #[test]
    fn chats_delete_request_defaults_to_selected_agent_session() {
        let app = ChatsApp::new(vec![test_chat_record(
            "agent:agt_1",
            "Agent",
            "djinn-agent",
            "agt_1",
        )]);

        let request = app.delete_request().unwrap();

        assert!(request.chat_ids.is_empty());
        assert_eq!(request.agent_session_ids, vec!["agt_1"]);
    }

    #[test]
    fn sessions_promote_options_do_not_open_for_agent_session_only() {
        let mut app = ChatsApp::new(vec![test_chat_record(
            "agent:agt_1",
            "Agent",
            "djinn-agent",
            "agt_1",
        )]);

        app.open_options();

        assert_eq!(app.mode, ChatUiMode::Selecting);
        assert!(app.promote_request().is_none());
    }

    #[test]
    fn sessions_promote_request_uses_only_promotable_session_rows() {
        let mut app = ChatsApp::new(vec![
            test_chat_record("chat-one", "Saved", "manual", ""),
            test_chat_record("agent:agt_1", "Agent", "djinn-agent", "agt_1"),
            test_chat_record("chat-two", "OpenCode", "opencode", "ses_2"),
        ]);
        app.checked.insert("chat-one".to_string());
        app.checked.insert("agent:agt_1".to_string());
        app.checked.insert("chat-two".to_string());

        app.open_options();
        let request = app.promote_request().unwrap();

        assert_eq!(app.mode, ChatUiMode::Options);
        assert_eq!(request.chat_ids, vec!["chat-one", "chat-two"]);
    }

    #[test]
    fn chats_delete_confirmation_requires_explicit_confirm() {
        let mut app = ChatsApp::new(vec![test_chat_record(
            "agent:agt_1",
            "Agent",
            "djinn-agent",
            "agt_1",
        )]);

        app.open_delete_confirmation();

        assert_eq!(
            app.mode,
            ChatUiMode::ConfirmDelete(ChatDeleteRequest {
                chat_ids: Vec::new(),
                agent_session_ids: vec!["agt_1".to_string()],
            })
        );

        let action = app.confirm_delete_action().unwrap();
        assert_eq!(
            action,
            TuiAction::DeleteChatRows(ChatDeleteRequest {
                chat_ids: Vec::new(),
                agent_session_ids: vec!["agt_1".to_string()],
            })
        );
        assert_eq!(app.mode, ChatUiMode::Selecting);
    }

    #[test]
    fn chats_delete_confirmation_can_cancel_without_action() {
        let mut app = ChatsApp::new(vec![test_chat_record("chat-one", "Saved", "manual", "")]);

        app.open_delete_confirmation();
        app.cancel_modal();

        assert_eq!(app.mode, ChatUiMode::Selecting);
        assert_eq!(app.chats.len(), 1);
        assert!(app.confirm_delete_action().is_none());
    }

    #[test]
    fn chat_preview_surfaces_session_picker_actions() {
        let djinn = test_chat_record("agent:agt_1", "Djinn", "djinn-agent", "agt_1");
        let opencode = test_chat_record("chat", "OpenCode", "opencode", "ses_1");

        assert!(chat_preview(&djinn)
            .contains("Actions: Enter/r resume session • x delete session (confirm)"));
        assert!(chat_preview(&opencode).contains("Actions: Enter/r convert+resume in Djinn"));
    }

    #[test]
    fn chat_preview_and_list_metadata_surface_agent_role_and_parent() {
        let mut djinn = test_chat_record("agent:agt_child", "Djinn", "djinn-agent", "agt_child");
        djinn.content = "Djinn agent session\n\nID: agt_child\nProfile: default\nEvents: 7\nAgent role: reviewer\nParent session: agt_parent".to_string();

        let preview = chat_preview(&djinn);

        assert!(preview.contains("Agent role: reviewer"));
        assert!(preview.contains("Parent session: agt_parent"));
        assert_eq!(
            chat_source_label(&djinn),
            " • Djinn agent • role: reviewer • parent: agt_parent"
        );
        assert_eq!(
            chat_list_metadata(&djinn),
            "Djinn agent • role: reviewer • parent: agt_parent • profile: default • 7 events"
        );
    }

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
            ["Tools", "Workspaces", "Memories", "Suggestions", "Skills"]
        );
        assert_eq!(DashboardTab::Tools.index(), 0);
        assert_eq!(DashboardTab::Workspaces.index(), 1);
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
    fn workspaces_tab_filters_and_previews_folder_sessions() {
        let workspace = WorkspaceRecord {
            name: "repo-review".to_string(),
            reference_name: "repo-review-1234567890".to_string(),
            path: "/tmp/repo-review".to_string(),
            state: "paused".to_string(),
            mode: Some("background".to_string()),
            updated_at: Some("2026-07-29T12:00:00Z".to_string()),
            repo_path: Some("/tmp/repo".to_string()),
            summary_preview: Some("Latest answer preview".to_string()),
            turn_count: 2,
            next_action: Some("edit request.md or run again".to_string()),
        };
        let mut app = WorkspacesApp::new(vec![workspace]);

        assert_eq!(app.visible_indices(), vec![0]);
        app.filter_push('r');
        app.filter_push('e');
        app.filter_push('p');
        app.filter_push('o');
        assert_eq!(app.selected_workspace().unwrap().name, "repo-review");

        let preview = workspace_preview(app.selected_workspace().unwrap());
        assert!(preview.contains("Name: repo-review"));
        assert!(preview.contains("Focused shortcuts"));
        assert!(preview.contains("Latest answer preview"));
        assert!(workspace_list_metadata(app.selected_workspace().unwrap())
            .contains("paused / background"));
    }

    #[test]
    fn dashboard_help_open_and_close() {
        let mut app = DashboardApp::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            DashboardTab::Workspaces,
        );

        assert!(!app.help_open);
        app.open_help();
        assert!(app.help_open);
        app.close_help();
        assert!(!app.help_open);
    }

    #[test]
    fn dashboard_palette_scopes_commands_to_active_tab() {
        let workspaces_app = DashboardApp::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            DashboardTab::Workspaces,
        );
        let workspace_entries = workspaces_app.dashboard_command_palette();

        assert!(workspace_entries.iter().any(|entry| {
            entry.section == "Workspaces" && entry.command == DashboardCommand::OpenSelected
        }));
        assert!(!workspace_entries
            .iter()
            .any(|entry| { entry.section == "Sessions" || entry.label == "Open Sessions" }));
        assert!(!workspace_entries.iter().any(|entry| {
            entry.section == "Skills" && entry.command == DashboardCommand::OpenSelected
        }));

        let skills_app = DashboardApp::new(
            Vec::new(),
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
            entry.section == "Workspaces" && entry.command == DashboardCommand::OpenSelected
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
