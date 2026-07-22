use std::collections::HashSet;
use std::fs;
use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use djinn_chats::ChatRecord;
use djinn_contexts::ContextRecord;
use djinn_memory::{MemoryCandidate, SuggestionRecord};
use djinn_skills::SkillRecord;
use djinn_tools::ToolEntry;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::Terminal;
use serde_json::Value;

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

pub fn run_tools(tools: Vec<ToolEntry>) -> Result<()> {
    let mut terminal = enter_terminal()?;
    let result = run_tools_loop(&mut terminal, tools);
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_chats(chats: Vec<ChatRecord>) -> Result<Option<ChatShareRequest>> {
    let mut terminal = enter_terminal()?;
    let result = run_chats_loop(&mut terminal, chats);
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_dashboard(
    tools: Vec<ToolEntry>,
    chats: Vec<ChatRecord>,
    candidates: Vec<MemoryCandidate>,
    suggestions: Vec<SuggestionRecord>,
    skills: Vec<SkillRecord>,
    active_context: Option<ContextRecord>,
    initial_tab: DashboardTab,
) -> Result<Option<TuiAction>> {
    let mut terminal = enter_terminal()?;
    let result = run_dashboard_loop(
        &mut terminal,
        tools,
        chats,
        candidates,
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
    chats: Vec<ChatRecord>,
    candidates: Vec<MemoryCandidate>,
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
        chats,
        candidates,
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
    let result = run_approval_dialog_loop(&mut terminal, metadata);
    leave_terminal(&mut terminal)?;
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiAction {
    OpenTool(ToolEntry),
    OpenSkill(SkillRecord),
    ShareChats(ChatShareRequest),
    AcceptCandidate(String),
    RejectCandidates(Vec<String>),
    DeleteChats(Vec<String>),
    DeleteSuggestions(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardTab {
    Tools,
    Chats,
    Candidates,
    Memories,
    Skills,
}

impl DashboardTab {
    fn index(self) -> usize {
        match self {
            DashboardTab::Tools => 0,
            DashboardTab::Chats => 1,
            DashboardTab::Candidates => 2,
            DashboardTab::Memories => 3,
            DashboardTab::Skills => 4,
        }
    }

    fn from_index(index: usize) -> Self {
        match index % DASHBOARD_TABS.len() {
            0 => DashboardTab::Tools,
            1 => DashboardTab::Chats,
            2 => DashboardTab::Candidates,
            3 => DashboardTab::Memories,
            _ => DashboardTab::Skills,
        }
    }
}

const DASHBOARD_TABS: [&str; 5] = ["Tools", "Chats", "Memories", "Suggestions", "Skills"];

// Catppuccin Mocha palette.
const CTP_BASE: Color = Color::Rgb(30, 30, 46);
const CTP_MANTLE: Color = Color::Rgb(24, 24, 37);
const CTP_SURFACE0: Color = Color::Rgb(49, 50, 68);
const CTP_SURFACE1: Color = Color::Rgb(69, 71, 90);
const CTP_TEXT: Color = Color::Rgb(205, 214, 244);
const CTP_SUBTEXT0: Color = Color::Rgb(166, 173, 200);
const CTP_LAVENDER: Color = Color::Rgb(180, 190, 254);
const CTP_MAUVE: Color = Color::Rgb(203, 166, 247);
const CTP_GREEN: Color = Color::Rgb(166, 227, 161);
const CTP_PEACH: Color = Color::Rgb(250, 179, 135);

fn base_style() -> Style {
    Style::default().fg(CTP_TEXT).bg(CTP_BASE)
}

fn dim_style() -> Style {
    Style::default().fg(CTP_SUBTEXT0).bg(CTP_BASE)
}

fn title_style() -> Style {
    Style::default()
        .fg(CTP_LAVENDER)
        .bg(CTP_BASE)
        .add_modifier(Modifier::BOLD)
}

fn highlight_style() -> Style {
    Style::default()
        .fg(CTP_MAUVE)
        .bg(CTP_SURFACE1)
        .add_modifier(Modifier::BOLD)
}

fn selected_style() -> Style {
    Style::default()
        .fg(CTP_PEACH)
        .bg(CTP_BASE)
        .add_modifier(Modifier::BOLD)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPreviewState {
    files: Vec<ApprovalPreviewFile>,
    selected_file: usize,
    scroll: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPreviewFile {
    pub operation: String,
    pub path: String,
    pub new_path: Option<String>,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub hunks: Vec<ApprovalPreviewHunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPreviewHunk {
    pub lines: Vec<ApprovalPreviewLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPreviewLine {
    pub kind: ApprovalPreviewLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPreviewLineKind {
    Context,
    Add,
    Remove,
}

impl ApprovalPreviewState {
    pub fn from_metadata(metadata: &Value) -> Self {
        let files = metadata
            .get("preview")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(ApprovalPreviewFile::from_value).collect())
            .unwrap_or_default();
        Self {
            files,
            selected_file: 0,
            scroll: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn files(&self) -> &[ApprovalPreviewFile] {
        &self.files
    }

    pub fn selected_file_index(&self) -> usize {
        self.selected_file
    }

    pub fn scroll(&self) -> u16 {
        self.scroll
    }

    pub fn selected_file(&self) -> Option<&ApprovalPreviewFile> {
        self.files.get(self.selected_file)
    }

    pub fn next_file(&mut self) {
        if !self.files.is_empty() {
            self.selected_file = (self.selected_file + 1) % self.files.len();
            self.scroll = 0;
        }
    }

    pub fn previous_file(&mut self) {
        if !self.files.is_empty() {
            self.selected_file = if self.selected_file == 0 {
                self.files.len() - 1
            } else {
                self.selected_file - 1
            };
            self.scroll = 0;
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn file_labels(&self) -> Vec<String> {
        self.files.iter().map(ApprovalPreviewFile::label).collect()
    }

    pub fn selected_lines(&self) -> Vec<Line<'static>> {
        self.selected_file()
            .map(approval_preview_file_lines)
            .unwrap_or_else(|| vec![Line::from(Span::styled("No patch preview.", dim_style()))])
    }
}

struct ApprovalDialogApp {
    preview: ApprovalPreviewState,
}

impl ApprovalDialogApp {
    fn new(metadata: Value) -> Self {
        Self {
            preview: ApprovalPreviewState::from_metadata(&metadata),
        }
    }

    fn next_file(&mut self) {
        self.preview.next_file();
    }

    fn previous_file(&mut self) {
        self.preview.previous_file();
    }

    fn scroll_down(&mut self) {
        self.preview.scroll_down();
    }

    fn scroll_up(&mut self) {
        self.preview.scroll_up();
    }

    fn draw(&self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();
        frame.render_widget(Clear, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(area);
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
            .split(chunks[0]);

        let items = self
            .preview
            .file_labels()
            .into_iter()
            .map(ListItem::new)
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        if !self.preview.is_empty() {
            state.select(Some(self.preview.selected_file_index()));
        }
        let list = List::new(items)
            .block(block("Approval files"))
            .style(base_style())
            .highlight_style(highlight_style())
            .highlight_symbol("› ");
        frame.render_widget(Clear, body[0]);
        frame.render_stateful_widget(list, body[0], &mut state);

        let preview = Paragraph::new(self.preview.selected_lines())
            .block(block("Patch preview"))
            .style(base_style())
            .scroll((self.preview.scroll(), 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);

        let help = "a/Enter approve  d/q/Esc deny  j/k file  J/K or PgDn/PgUp scroll";
        frame.render_widget(Paragraph::new(help).style(dim_style()), chunks[1]);
    }
}

fn run_approval_dialog_loop(
    terminal: &mut TuiTerminal,
    metadata: Value,
) -> Result<ApprovalDecision> {
    let mut app = ApprovalDialogApp::new(metadata);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('a') | KeyCode::Enter => return Ok(ApprovalDecision::Approve),
                    KeyCode::Char('q') | KeyCode::Char('d') | KeyCode::Esc => {
                        return Ok(ApprovalDecision::Deny)
                    }
                    KeyCode::Char('j') | KeyCode::Down => app.next_file(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous_file(),
                    KeyCode::Char('J') | KeyCode::PageDown => app.scroll_down(),
                    KeyCode::Char('K') | KeyCode::PageUp => app.scroll_up(),
                    _ => {}
                }
            }
        }
    }
}

impl ApprovalPreviewFile {
    fn from_value(value: &Value) -> Self {
        Self {
            operation: value["operation"]
                .as_str()
                .unwrap_or("operation")
                .to_string(),
            path: value["relative_path"]
                .as_str()
                .or_else(|| value["path"].as_str())
                .unwrap_or("<unknown>")
                .to_string(),
            new_path: value["relative_new_path"]
                .as_str()
                .or_else(|| value["new_path"].as_str())
                .map(str::to_string),
            lines_added: value["lines_added"].as_u64().unwrap_or_default(),
            lines_removed: value["lines_removed"].as_u64().unwrap_or_default(),
            hunks: value["hunks"]
                .as_array()
                .map(|hunks| hunks.iter().map(ApprovalPreviewHunk::from_value).collect())
                .unwrap_or_default(),
        }
    }

    pub fn label(&self) -> String {
        match &self.new_path {
            Some(new_path) => format!("{} {} -> {}", self.operation, self.path, new_path),
            None => format!("{} {}", self.operation, self.path),
        }
    }
}

impl ApprovalPreviewHunk {
    fn from_value(value: &Value) -> Self {
        Self {
            lines: value["lines"]
                .as_array()
                .map(|lines| lines.iter().map(ApprovalPreviewLine::from_value).collect())
                .unwrap_or_default(),
        }
    }
}

impl ApprovalPreviewLine {
    fn from_value(value: &Value) -> Self {
        Self {
            kind: match value["kind"].as_str().unwrap_or("context") {
                "add" => ApprovalPreviewLineKind::Add,
                "remove" => ApprovalPreviewLineKind::Remove,
                _ => ApprovalPreviewLineKind::Context,
            },
            content: value["content"].as_str().unwrap_or_default().to_string(),
        }
    }
}

pub fn approval_preview_file_lines(file: &ApprovalPreviewFile) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(file.operation.clone(), selected_style()),
        Span::raw(" "),
        Span::styled(file.path.clone(), title_style()),
        Span::raw(format!(" (+{}/-{})", file.lines_added, file.lines_removed)),
    ])];
    if let Some(new_path) = &file.new_path {
        lines.push(Line::from(Span::styled(
            format!("→ {new_path}"),
            dim_style(),
        )));
    }
    if file.hunks.is_empty() {
        lines.push(Line::from(Span::styled("No hunks.", dim_style())));
        return lines;
    }
    for (index, hunk) in file.hunks.iter().enumerate() {
        lines.push(Line::from(Span::styled(
            format!("@@ hunk {}", index + 1),
            dim_style(),
        )));
        for line in &hunk.lines {
            let (prefix, style) = match line.kind {
                ApprovalPreviewLineKind::Context => (' ', base_style()),
                ApprovalPreviewLineKind::Add => ('+', Style::default().fg(CTP_GREEN).bg(CTP_BASE)),
                ApprovalPreviewLineKind::Remove => {
                    ('-', Style::default().fg(CTP_PEACH).bg(CTP_BASE))
                }
            };
            lines.push(Line::from(Span::styled(
                format!("{prefix} {}", line.content),
                style,
            )));
        }
    }
    lines
}

fn block<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(title_style())
        .border_style(Style::default().fg(CTP_SURFACE0).bg(CTP_MANTLE))
        .style(Style::default().fg(CTP_TEXT).bg(CTP_MANTLE))
}

#[derive(Debug, Clone, Default)]
struct FilterState {
    query: String,
    editing: bool,
}

impl FilterState {
    fn toggle(&mut self) {
        if self.query.is_empty() {
            self.editing = true;
        } else {
            self.query.clear();
            self.editing = false;
        }
    }

    fn push(&mut self, ch: char) {
        self.query.push(ch);
    }

    fn backspace(&mut self) {
        self.query.pop();
    }

    fn label(&self) -> String {
        if self.query.is_empty() && self.editing {
            "filter: ".to_string()
        } else if self.query.is_empty() {
            "filter: off".to_string()
        } else if self.editing {
            format!("filter: {}", self.query)
        } else {
            format!("filter: {} (/ clears)", self.query)
        }
    }
}

fn fuzzy_match(query: &str, candidate: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let candidate = candidate.to_lowercase();
    let mut chars = candidate.chars();
    query.chars().all(|needle| chars.any(|ch| ch == needle))
}

fn selected_visible_position(selected: usize, visible: &[usize]) -> Option<usize> {
    visible.iter().position(|idx| *idx == selected)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatShareRequest {
    pub chat_ids: Vec<String>,
    pub mode: ChatShareMode,
    pub context_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatShareMode {
    Summary,
    Patterns,
    Memories,
}

fn enter_terminal() -> Result<TuiTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn leave_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_tools_loop(terminal: &mut TuiTerminal, tools: Vec<ToolEntry>) -> Result<()> {
    let mut app = ToolsApp::new(tools);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
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

fn run_dashboard_loop(
    terminal: &mut TuiTerminal,
    tools: Vec<ToolEntry>,
    chats: Vec<ChatRecord>,
    candidates: Vec<MemoryCandidate>,
    suggestions: Vec<SuggestionRecord>,
    skills: Vec<SkillRecord>,
    active_context: Option<ContextRecord>,
    initial_tab: DashboardTab,
    mut on_continue_action: Option<&mut dyn FnMut(TuiAction) -> Result<()>>,
) -> Result<Option<TuiAction>> {
    let mut app = DashboardApp::new(
        tools,
        chats,
        candidates,
        suggestions,
        skills,
        active_context,
        initial_tab,
    );
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if app.chats.mode == ChatUiMode::Options && app.active_tab == DashboardTab::Chats {
                    match key.code {
                        KeyCode::Char('q') => return Ok(None),
                        KeyCode::Esc | KeyCode::Backspace => app.chats.mode = ChatUiMode::Selecting,
                        KeyCode::Char('j') | KeyCode::Down => app.chats.next_option(),
                        KeyCode::Char('k') | KeyCode::Up => app.chats.previous_option(),
                        KeyCode::Char('c') => app.chats.context_only = !app.chats.context_only,
                        KeyCode::Enter => {
                            return Ok(app.chats.share_request().map(TuiAction::ShareChats));
                        }
                        _ => {}
                    }
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
                        if app.active_tab == DashboardTab::Candidates {
                            if let Some(id) = app.candidates.selected_candidate_id() {
                                return Ok(Some(TuiAction::AcceptCandidate(id)));
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
                        DashboardTab::Chats => app.chats.open_options(),
                        DashboardTab::Skills => {
                            if let Some(skill) = app.skills.selected_skill().cloned() {
                                return Ok(Some(TuiAction::OpenSkill(skill)));
                            }
                        }
                        DashboardTab::Candidates | DashboardTab::Memories => {}
                    },
                    KeyCode::Char('r') => {
                        if app.active_tab == DashboardTab::Candidates {
                            let ids = app.candidates.selected_candidate_ids();
                            if !ids.is_empty() {
                                let action = TuiAction::RejectCandidates(ids);
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
                        DashboardTab::Chats => {
                            let ids = app.chats.selected_chat_ids();
                            if !ids.is_empty() {
                                let action = TuiAction::DeleteChats(ids);
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
                        DashboardTab::Candidates => {
                            let ids = app.candidates.selected_candidate_ids();
                            if !ids.is_empty() {
                                let action = TuiAction::RejectCandidates(ids);
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
                        DashboardTab::Memories => {
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
                        DashboardTab::Tools | DashboardTab::Skills => {}
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

struct DashboardApp {
    active_tab: DashboardTab,
    tools: ToolsApp,
    chats: ChatsApp,
    candidates: CandidatesApp,
    suggestions: SuggestionsApp,
    skills: SkillsApp,
    active_context: Option<ContextRecord>,
}

impl DashboardApp {
    fn new(
        tools: Vec<ToolEntry>,
        chats: Vec<ChatRecord>,
        candidates: Vec<MemoryCandidate>,
        suggestions: Vec<SuggestionRecord>,
        skills: Vec<SkillRecord>,
        active_context: Option<ContextRecord>,
        initial_tab: DashboardTab,
    ) -> Self {
        Self {
            active_tab: initial_tab,
            tools: ToolsApp::new(tools),
            chats: ChatsApp::new(chats),
            candidates: CandidatesApp::new(candidates),
            suggestions: SuggestionsApp::new(suggestions),
            skills: SkillsApp::new(skills),
            active_context,
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
            DashboardTab::Chats => self.chats.next(),
            DashboardTab::Candidates => self.candidates.next(),
            DashboardTab::Memories => self.suggestions.next(),
            DashboardTab::Skills => self.skills.next(),
        }
    }

    fn previous_item(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.previous(),
            DashboardTab::Chats => self.chats.previous(),
            DashboardTab::Candidates => self.candidates.previous(),
            DashboardTab::Memories => self.suggestions.previous(),
            DashboardTab::Skills => self.skills.previous(),
        }
    }

    fn scroll_down(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.scroll_down(),
            DashboardTab::Chats => self.chats.scroll_down(),
            DashboardTab::Candidates => self.candidates.scroll_down(),
            DashboardTab::Memories => self.suggestions.scroll_down(),
            DashboardTab::Skills => self.skills.scroll_down(),
        }
    }

    fn scroll_up(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.scroll_up(),
            DashboardTab::Chats => self.chats.scroll_up(),
            DashboardTab::Candidates => self.candidates.scroll_up(),
            DashboardTab::Memories => self.suggestions.scroll_up(),
            DashboardTab::Skills => self.skills.scroll_up(),
        }
    }

    fn filter_editing(&self) -> bool {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter.editing,
            DashboardTab::Chats => self.chats.filter.editing,
            DashboardTab::Candidates => self.candidates.filter.editing,
            DashboardTab::Memories => self.suggestions.filter.editing,
            DashboardTab::Skills => self.skills.filter.editing,
        }
    }

    fn toggle_filter(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.toggle_filter(),
            DashboardTab::Chats => self.chats.toggle_filter(),
            DashboardTab::Candidates => self.candidates.toggle_filter(),
            DashboardTab::Memories => self.suggestions.toggle_filter(),
            DashboardTab::Skills => self.skills.toggle_filter(),
        }
    }

    fn filter_push(&mut self, ch: char) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter_push(ch),
            DashboardTab::Chats => self.chats.filter_push(ch),
            DashboardTab::Candidates => self.candidates.filter_push(ch),
            DashboardTab::Memories => self.suggestions.filter_push(ch),
            DashboardTab::Skills => self.skills.filter_push(ch),
        }
    }

    fn filter_backspace(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter_backspace(),
            DashboardTab::Chats => self.chats.filter_backspace(),
            DashboardTab::Candidates => self.candidates.filter_backspace(),
            DashboardTab::Memories => self.suggestions.filter_backspace(),
            DashboardTab::Skills => self.skills.filter_backspace(),
        }
    }

    fn finish_filter_edit(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter.editing = false,
            DashboardTab::Chats => self.chats.filter.editing = false,
            DashboardTab::Candidates => self.candidates.filter.editing = false,
            DashboardTab::Memories => self.suggestions.filter.editing = false,
            DashboardTab::Skills => self.skills.filter.editing = false,
        }
    }

    fn toggle_selected(&mut self) {
        match self.active_tab {
            DashboardTab::Chats => self.chats.toggle_selected(),
            DashboardTab::Candidates => self.candidates.toggle_selected(),
            DashboardTab::Memories => self.suggestions.toggle_selected(),
            DashboardTab::Tools | DashboardTab::Skills => {}
        }
    }

    fn toggle_all(&mut self) {
        match self.active_tab {
            DashboardTab::Chats => self.chats.toggle_all(),
            DashboardTab::Candidates => self.candidates.toggle_all(),
            DashboardTab::Memories => self.suggestions.toggle_all(),
            DashboardTab::Tools | DashboardTab::Skills => {}
        }
    }

    fn apply_completed_action(&mut self, action: &TuiAction) {
        match action {
            TuiAction::DeleteChats(ids) => self.chats.remove_ids(ids),
            TuiAction::RejectCandidates(ids) => self.candidates.remove_ids(ids),
            TuiAction::DeleteSuggestions(ids) => self.suggestions.remove_ids(ids),
            TuiAction::OpenTool(_)
            | TuiAction::OpenSkill(_)
            | TuiAction::ShareChats(_)
            | TuiAction::AcceptCandidate(_) => {}
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
            DashboardTab::Chats => self.chats.draw_body(frame, chunks[1]),
            DashboardTab::Candidates => self.candidates.draw_body(frame, chunks[1]),
            DashboardTab::Memories => self.suggestions.draw_body(frame, chunks[1]),
            DashboardTab::Skills => self.skills.draw_body(frame, chunks[1]),
        }

        let help = match self.active_tab {
            DashboardTab::Tools => "Tab/Shift+Tab tabs • / filter/clear • ↑/↓ move • Enter open • PgUp/PgDn scroll preview • q quit",
            DashboardTab::Chats => "Tab/Shift+Tab tabs • / filter/clear • ↑/↓ move • Space select • a all visible • Enter share • x/Delete remove • q quit",
            DashboardTab::Candidates => "Tab/Shift+Tab tabs • / filter/clear • ↑/↓ move • Space select • a review memory • A all visible • r/x reject+remove • q quit",
            DashboardTab::Memories => "Tab/Shift+Tab tabs • / filter/clear • ↑/↓ move • Space select • a all visible • x/Delete remove suggestion • q quit",
            DashboardTab::Skills => "Tab/Shift+Tab tabs • / filter/clear • ↑/↓ move • Enter open • PgUp/PgDn scroll preview • q quit",
        };
        frame.render_widget(Clear, chunks[2]);
        frame.render_widget(Paragraph::new(help).style(dim_style()), chunks[2]);

        if self.active_tab == DashboardTab::Chats && self.chats.mode == ChatUiMode::Options {
            self.chats.draw_options(frame);
        }
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

fn run_chats_loop(
    terminal: &mut TuiTerminal,
    chats: Vec<ChatRecord>,
) -> Result<Option<ChatShareRequest>> {
    let mut app = ChatsApp::new(chats);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                match app.mode {
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
                        KeyCode::Char('c') => app.context_only = !app.context_only,
                        KeyCode::Enter => return Ok(app.share_request()),
                        _ => {}
                    },
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatUiMode {
    Selecting,
    Options,
}

struct ChatsApp {
    chats: Vec<ChatRecord>,
    selected: usize,
    preview_scroll: u16,
    checked: HashSet<String>,
    mode: ChatUiMode,
    option_selected: usize,
    context_only: bool,
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
            option_selected: 1,
            context_only: false,
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

    fn visible_indices(&self) -> Vec<usize> {
        self.chats
            .iter()
            .enumerate()
            .filter_map(|(idx, chat)| self.chat_matches(chat).then_some(idx))
            .collect()
    }

    fn chat_matches(&self, chat: &ChatRecord) -> bool {
        fuzzy_match(&self.filter.query, &chat.title) || fuzzy_match(&self.filter.query, &chat.id)
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

    fn selected_chat_ids(&self) -> Vec<String> {
        if self.checked.is_empty() {
            return self
                .selected_chat()
                .map(|chat| vec![chat.id.clone()])
                .unwrap_or_default();
        }
        self.chats
            .iter()
            .filter(|chat| self.checked.contains(&chat.id))
            .map(|chat| chat.id.clone())
            .collect()
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

    fn remove_ids(&mut self, ids: &[String]) {
        let removed = ids.iter().cloned().collect::<HashSet<_>>();
        self.chats.retain(|chat| !removed.contains(&chat.id));
        self.checked.retain(|id| !removed.contains(id));
        if self.selected >= self.chats.len() {
            self.selected = self.chats.len().saturating_sub(1);
        }
        self.mode = ChatUiMode::Selecting;
        self.ensure_selection_visible();
    }

    fn open_options(&mut self) {
        if !self.chats.is_empty() {
            self.mode = ChatUiMode::Options;
        }
    }

    fn next_option(&mut self) {
        self.option_selected = (self.option_selected + 1).min(2);
    }

    fn previous_option(&mut self) {
        self.option_selected = self.option_selected.saturating_sub(1);
    }

    fn selected_share_mode(&self) -> ChatShareMode {
        match self.option_selected {
            0 => ChatShareMode::Summary,
            1 => ChatShareMode::Patterns,
            _ => ChatShareMode::Memories,
        }
    }

    fn share_request(&self) -> Option<ChatShareRequest> {
        let chat_ids = self.selected_chat_ids();
        if chat_ids.is_empty() {
            return None;
        }
        Some(ChatShareRequest {
            chat_ids,
            mode: self.selected_share_mode(),
            context_only: self.context_only,
        })
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(frame.area());

        self.draw_body(frame, chunks[0]);

        let help = Paragraph::new(
            "↑/k ↓/j move • Space select • a all visible • Enter share options • x/Delete remove • PgUp/u PgDn/d scroll • q/Esc quit",
        )
        .style(dim_style());
        frame.render_widget(Clear, chunks[1]);
        frame.render_widget(help, chunks[1]);

        if self.mode == ChatUiMode::Options {
            self.draw_options(frame);
        }
    }

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);

        let visible = self.visible_indices();
        let items = if self.chats.is_empty() {
            vec![ListItem::new("No chats recorded").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No chats match filter").style(dim_style())]
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
                    let source = chat_source_label(chat);
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
                        Line::from(Span::styled(
                            format!("{} chars{}", chat.content.chars().count(), source),
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
            "Chats ({} selected, {})",
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
        let mode_names = ["summary", "patterns", "memories"];
        let mut lines = vec![
            Line::from(Span::styled("Share selected chats", title_style())),
            Line::from(Span::styled(
                format!("Chats: {}", self.selected_chat_ids().len()),
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
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "[{}] context only",
                if self.context_only { "x" } else { " " }
            ),
            if self.context_only {
                Style::default().fg(CTP_GREEN).bg(CTP_BASE)
            } else {
                dim_style()
            },
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Enter share • c toggle context • Esc back",
            dim_style(),
        )));

        let modal = Paragraph::new(lines)
            .block(block("Share Options"))
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

struct CandidatesApp {
    candidates: Vec<MemoryCandidate>,
    selected: usize,
    preview_scroll: u16,
    checked: HashSet<String>,
    filter: FilterState,
}

impl CandidatesApp {
    fn new(candidates: Vec<MemoryCandidate>) -> Self {
        Self {
            candidates: candidates
                .into_iter()
                .filter(is_pending_memory)
                .collect::<Vec<_>>(),
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

    fn selected_candidate(&self) -> Option<&MemoryCandidate> {
        self.candidates
            .get(self.selected)
            .filter(|candidate| self.candidate_matches(candidate))
    }

    fn selected_candidate_id(&self) -> Option<String> {
        self.selected_candidate()
            .map(|candidate| candidate.id.clone())
    }

    fn selected_candidate_ids(&self) -> Vec<String> {
        if self.checked.is_empty() {
            return self
                .selected_candidate()
                .map(|candidate| vec![candidate.id.clone()])
                .unwrap_or_default();
        }
        self.candidates
            .iter()
            .filter(|candidate| self.checked.contains(&candidate.id))
            .map(|candidate| candidate.id.clone())
            .collect()
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.candidates
            .iter()
            .enumerate()
            .filter_map(|(idx, candidate)| self.candidate_matches(candidate).then_some(idx))
            .collect()
    }

    fn candidate_matches(&self, candidate: &MemoryCandidate) -> bool {
        fuzzy_match(&self.filter.query, &candidate.id)
            || fuzzy_match(&self.filter.query, &candidate.text)
            || fuzzy_match(&self.filter.query, &candidate.scope)
            || fuzzy_match(&self.filter.query, &candidate.kind)
            || fuzzy_match(&self.filter.query, &candidate.confidence)
            || fuzzy_match(&self.filter.query, &candidate.not_before)
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
        if let Some(id) = self.selected_candidate_id() {
            if !self.checked.insert(id.clone()) {
                self.checked.remove(&id);
            }
        }
    }

    fn toggle_all(&mut self) {
        let visible_ids = self
            .visible_indices()
            .iter()
            .map(|idx| self.candidates[*idx].id.clone())
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
        self.candidates
            .retain(|candidate| !removed.contains(&candidate.id));
        self.checked.retain(|id| !removed.contains(id));
        if self.selected >= self.candidates.len() {
            self.selected = self.candidates.len().saturating_sub(1);
        }
        self.ensure_selection_visible();
    }

    fn draw_body(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);

        let visible = self.visible_indices();
        let items = if self.candidates.is_empty() {
            vec![ListItem::new("No memories recorded").style(dim_style())]
        } else if visible.is_empty() {
            vec![ListItem::new("No memories match filter").style(dim_style())]
        } else {
            visible
                .iter()
                .map(|idx| {
                    let candidate = &self.candidates[*idx];
                    let checked = if self.checked.contains(&candidate.id) {
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
                            Span::styled(candidate.id.clone(), title_style()),
                        ]),
                        Line::from(Span::styled(
                            truncate_line(&candidate.text, 96),
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
            .selected_candidate()
            .map(candidate_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview_title = self
            .selected_candidate()
            .map(|candidate| compact_id(&candidate.id))
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

fn is_pending_memory(candidate: &MemoryCandidate) -> bool {
    candidate.status.trim().is_empty() || candidate.status.eq_ignore_ascii_case("pending")
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

fn chat_source_label(chat: &ChatRecord) -> String {
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

fn candidate_preview(candidate: &MemoryCandidate) -> String {
    let mut out = format!("ID: {}\nCreated: {}\n", candidate.id, candidate.created_at);
    if !candidate.scope.trim().is_empty() {
        out.push_str(&format!("Scope: {}\n", candidate.scope));
    }
    if !candidate.kind.trim().is_empty() {
        out.push_str(&format!("Kind: {}\n", candidate.kind));
    }
    if !candidate.confidence.trim().is_empty() {
        out.push_str(&format!("Confidence: {}\n", candidate.confidence));
    }
    if !candidate.not_before.trim().is_empty() {
        out.push_str(&format!("Not before: {}\n", candidate.not_before));
    }
    out.push_str("\n");
    out.push_str(&candidate.text);
    if !candidate.evidence.is_empty() {
        out.push_str("\n\nEvidence:\n");
        for evidence in &candidate.evidence {
            out.push_str(&format!("- {}\n", evidence));
        }
    }
    if !candidate.sources.is_empty() {
        out.push_str("\nSources:\n");
        for source in &candidate.sources {
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
    out.push_str("\nActions: press `a` to review this memory, Space to select, `A` to select all visible, or `r`/`x`/Delete to reject and remove selected/current memories.\n");
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
    fn fuzzy_match_matches_subsequence_case_insensitive() {
        assert!(fuzzy_match("ocd", "OpenCode Debug Session"));
        assert!(fuzzy_match("tl", "tool-list"));
        assert!(!fuzzy_match("xyz", "tool-list"));
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
            vec!["update src/lib.rs", "move old.txt -> new.txt"]
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
    fn dashboard_tabs_follow_progression_order() {
        assert_eq!(
            DASHBOARD_TABS,
            ["Tools", "Chats", "Memories", "Suggestions", "Skills"]
        );
        assert_eq!(DashboardTab::Tools.index(), 0);
        assert_eq!(DashboardTab::Chats.index(), 1);
        assert_eq!(DashboardTab::Candidates.index(), 2);
        assert_eq!(DashboardTab::Memories.index(), 3);
        assert_eq!(DashboardTab::Skills.index(), 4);
        assert_eq!(DashboardTab::from_index(5), DashboardTab::Tools);
    }

    #[test]
    fn candidate_preview_includes_evidence_sources_and_actions() {
        let candidate = MemoryCandidate {
            id: "prefer-uv".to_string(),
            text: "Prefer uv in this repo".to_string(),
            created_at: "2026-07-09".to_string(),
            status: "pending".to_string(),
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
            reinforcement_count: 1,
        };
        let preview = candidate_preview(&candidate);
        assert!(!preview.contains("Status:"));
        assert!(preview.contains("Not before: 2026-10-01"));
        assert!(preview.contains("User corrected pip to uv."));
        assert!(preview.contains("Debugging session"));
        assert!(preview.contains("review this memory"));
    }

    #[test]
    fn memories_app_only_lists_pending_memories() {
        let pending = MemoryCandidate {
            id: "pending-memory".to_string(),
            text: "Review this".to_string(),
            created_at: "2026-07-15".to_string(),
            status: "pending".to_string(),
            scope: String::new(),
            kind: String::new(),
            confidence: String::new(),
            not_before: String::new(),
            evidence: Vec::new(),
            sources: Vec::new(),
            reinforcement_count: 1,
        };
        let accepted = MemoryCandidate {
            id: "accepted-memory".to_string(),
            text: "Already reviewed".to_string(),
            status: "accepted".to_string(),
            ..pending.clone()
        };

        let app = CandidatesApp::new(vec![pending, accepted]);
        assert_eq!(app.candidates.len(), 1);
        assert_eq!(app.candidates[0].id, "pending-memory");
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
