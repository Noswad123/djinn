use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Stdout};
use std::process::Command as ProcessCommand;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossterm::event::{
    self, Event, KeyCode, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
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
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::Terminal;
use serde_json::Value;

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;
pub type AgentChatProgressHandler<'a> = dyn FnMut(Vec<AgentChatMessage>, String) -> Result<()> + 'a;

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

    pub fn run_agent_chat_with_handler<F>(
        &mut self,
        messages: Vec<AgentChatMessage>,
        status: AgentChatStatus,
        mut on_prompt: F,
    ) -> Result<AgentChatExit>
    where
        F: FnMut(String) -> Result<Vec<AgentChatMessage>>,
    {
        run_agent_chat_session_loop(
            &mut self.terminal,
            messages,
            status,
            &mut |prompt, _progress| on_prompt(prompt),
        )
    }

    pub fn run_agent_chat_with_progress_handler<F>(
        &mut self,
        messages: Vec<AgentChatMessage>,
        status: AgentChatStatus,
        mut on_prompt: F,
    ) -> Result<AgentChatExit>
    where
        F: FnMut(String, &mut AgentChatProgressHandler<'_>) -> Result<Vec<AgentChatMessage>>,
    {
        run_agent_chat_session_loop(&mut self.terminal, messages, status, &mut on_prompt)
    }

    pub fn run_dashboard_with_handler<F>(
        &mut self,
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
        run_dashboard_loop(
            &mut self.terminal,
            tools,
            chats,
            candidates,
            suggestions,
            skills,
            active_context,
            initial_tab,
            Some(&mut on_continue_action),
        )
    }

    pub fn finish(mut self) -> Result<()> {
        if self.active {
            leave_terminal(&mut self.terminal)?;
            self.active = false;
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

pub fn run_agent_chat(
    messages: Vec<AgentChatMessage>,
    status: AgentChatStatus,
) -> Result<Option<String>> {
    let mut terminal = enter_terminal()?;
    let result = run_agent_chat_prompt_loop(&mut terminal, messages, status);
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_agent_chat_with_handler<F>(
    messages: Vec<AgentChatMessage>,
    status: AgentChatStatus,
    mut on_prompt: F,
) -> Result<AgentChatExit>
where
    F: FnMut(String) -> Result<Vec<AgentChatMessage>>,
{
    let mut terminal = enter_terminal()?;
    let result =
        run_agent_chat_session_loop(&mut terminal, messages, status, &mut |prompt, _progress| {
            on_prompt(prompt)
        });
    leave_terminal(&mut terminal)?;
    result
}

pub fn run_agent_chat_with_progress_handler<F>(
    messages: Vec<AgentChatMessage>,
    status: AgentChatStatus,
    mut on_prompt: F,
) -> Result<AgentChatExit>
where
    F: FnMut(String, &mut AgentChatProgressHandler<'_>) -> Result<Vec<AgentChatMessage>>,
{
    let mut terminal = enter_terminal()?;
    let result = run_agent_chat_session_loop(&mut terminal, messages, status, &mut on_prompt);
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
    OpenAgentChat,
    OpenChatSession(ChatSessionRequest),
    OpenTool(ToolEntry),
    OpenSkill(SkillRecord),
    ShareChats(ChatShareRequest),
    AcceptCandidate(String),
    RejectCandidates(Vec<String>),
    DeleteChats(Vec<String>),
    DeleteSuggestions(Vec<String>),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChatStatus {
    pub session_id: String,
    pub workspace: String,
    pub profile: String,
    pub model: String,
    pub notice: String,
    #[allow(dead_code)]
    pub command_palette: Vec<AgentChatCommandEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChatCommandEntry {
    pub section: String,
    pub label: String,
    pub description: String,
    pub command: AgentChatCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentChatCommand {
    OpenSessions,
    SwitchProfile(String),
    SwitchModel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChatMessage {
    pub role: AgentChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentChatRole {
    User,
    Assistant,
    Thought,
    Tool,
    ToolOutput,
    Notice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentChatExit {
    Quit,
    Dashboard { initial_tab: DashboardTab },
    Command(AgentChatCommand),
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

fn dashboard_tab_returns_to_agent(tab: DashboardTab) -> bool {
    tab == DashboardTab::Skills
}

fn dashboard_back_tab_returns_to_agent(tab: DashboardTab) -> bool {
    tab == DashboardTab::Tools
}

const DASHBOARD_TABS: [&str; 5] = ["Tools", "Chats", "Memories", "Suggestions", "Skills"];
const APP_TABS: [&str; 6] = [
    "Agent",
    "Tools",
    "Chats",
    "Memories",
    "Suggestions",
    "Skills",
];

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
const CTP_RED: Color = Color::Rgb(243, 139, 168);
const CTP_SKY: Color = Color::Rgb(137, 220, 235);
const CTP_YELLOW: Color = Color::Rgb(249, 226, 175);

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

fn agent_chat_block<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .borders(agent_chat_borders())
        .title(title)
        .title_style(title_style())
        .border_style(Style::default().fg(CTP_SURFACE0).bg(CTP_MANTLE))
        .style(Style::default().fg(CTP_TEXT).bg(CTP_MANTLE))
}

fn agent_chat_borders() -> Borders {
    Borders::TOP | Borders::BOTTOM
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

fn run_agent_chat_prompt_loop(
    terminal: &mut TuiTerminal,
    messages: Vec<AgentChatMessage>,
    status: AgentChatStatus,
) -> Result<Option<String>> {
    let mut app = AgentChatComposerApp::new(messages, status);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    _ if agent_chat_quit_key(key.code, key.modifiers, app.input.is_empty()) => {
                        return Ok(None);
                    }
                    _ if agent_chat_newline_key(key.code, key.modifiers) => {
                        app.insert_newline();
                    }
                    _ if agent_chat_editor_key(key.code, key.modifiers) => {
                        if let Err(error) = edit_agent_chat_input(terminal, &mut app) {
                            app.status.notice = format!("Editor failed: {error:#}");
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(prompt) = app.submit_prompt() {
                            return Ok(Some(prompt));
                        }
                    }
                    KeyCode::Backspace => {
                        app.backspace();
                    }
                    KeyCode::Char(ch) => app.push_char(ch),
                    KeyCode::End => app.jump_to_end(terminal.size()?.height),
                    KeyCode::Home => app.jump_to_top(),
                    KeyCode::PageDown => app.scroll_down(),
                    KeyCode::PageUp => app.scroll_up(),
                    KeyCode::Down => app.scroll_down(),
                    KeyCode::Up => app.scroll_up(),
                    _ => {}
                }
            }
        }
    }
}

fn run_agent_chat_session_loop<F>(
    terminal: &mut TuiTerminal,
    messages: Vec<AgentChatMessage>,
    status: AgentChatStatus,
    on_prompt: &mut F,
) -> Result<AgentChatExit>
where
    F: FnMut(String, &mut AgentChatProgressHandler<'_>) -> Result<Vec<AgentChatMessage>>,
{
    let mut app = AgentChatComposerApp::new(messages, status);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if app.help_open {
                    match key.code {
                        _ if agent_chat_help_key(key.code, key.modifiers) => app.close_help(),
                        KeyCode::Esc | KeyCode::Enter => app.close_help(),
                        _ if agent_chat_quit_key(key.code, key.modifiers, app.input.is_empty()) => {
                            return Ok(AgentChatExit::Quit);
                        }
                        _ => {}
                    }
                    continue;
                }
                if app.palette_open {
                    match key.code {
                        _ if agent_chat_help_key(key.code, key.modifiers) => app.open_help(),
                        KeyCode::Esc => app.close_palette(),
                        KeyCode::Backspace => app.backspace_palette_query_or_close(),
                        _ if agent_chat_palette_next_key(key.code, key.modifiers) => {
                            app.next_palette_item()
                        }
                        _ if agent_chat_palette_previous_key(key.code, key.modifiers) => {
                            app.previous_palette_item()
                        }
                        KeyCode::Enter => {
                            if let Some(command) = app.selected_palette_command() {
                                return Ok(AgentChatExit::Command(command));
                            }
                            app.close_palette();
                        }
                        KeyCode::Char(ch) if palette_text_key(key.modifiers) => {
                            app.push_palette_query(ch)
                        }
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    _ if agent_chat_help_key(key.code, key.modifiers) => app.open_help(),
                    _ if agent_chat_palette_key(key.code, key.modifiers) => app.open_palette(),
                    _ if agent_chat_dashboard_target(key.code).is_some() => {
                        return Ok(AgentChatExit::Dashboard {
                            initial_tab: agent_chat_dashboard_target(key.code).unwrap(),
                        });
                    }
                    _ if agent_chat_quit_key(key.code, key.modifiers, app.input.is_empty()) => {
                        return Ok(AgentChatExit::Quit);
                    }
                    _ if agent_chat_newline_key(key.code, key.modifiers) => {
                        app.insert_newline();
                    }
                    _ if agent_chat_editor_key(key.code, key.modifiers) => {
                        if let Err(error) = edit_agent_chat_input(terminal, &mut app) {
                            app.status.notice = format!("Editor failed: {error:#}");
                        }
                    }
                    KeyCode::Enter => {
                        let Some(prompt) = app.submit_prompt() else {
                            continue;
                        };
                        app.messages.push(AgentChatMessage {
                            role: AgentChatRole::User,
                            content: prompt.clone(),
                        });
                        app.status.notice = "Djinn is thinking…".to_string();
                        app.messages.push(AgentChatMessage {
                            role: AgentChatRole::Thought,
                            content: "Waiting for model response…".to_string(),
                        });
                        terminal.draw(|frame| app.draw(frame))?;

                        let mut progress = |messages: Vec<AgentChatMessage>, notice: String| {
                            app.messages = messages;
                            app.status.notice = notice;
                            terminal.draw(|frame| app.draw(frame))?;
                            Ok(())
                        };

                        match on_prompt(prompt, &mut progress) {
                            Ok(messages) => {
                                app.messages = messages;
                                app.status.notice = "Ready.".to_string();
                            }
                            Err(error) => {
                                app.messages.push(AgentChatMessage {
                                    role: AgentChatRole::Notice,
                                    content: format!("Agent turn failed: {error:#}"),
                                });
                                app.status.notice = "Agent turn failed.".to_string();
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        app.backspace();
                    }
                    KeyCode::Char(ch) => app.push_char(ch),
                    KeyCode::End => app.jump_to_end(terminal.size()?.height),
                    KeyCode::Home => app.jump_to_top(),
                    KeyCode::PageDown => app.scroll_down(),
                    KeyCode::PageUp => app.scroll_up(),
                    KeyCode::Down => app.scroll_down(),
                    KeyCode::Up => app.scroll_up(),
                    _ => {}
                }
            }
        }
    }
}

struct AgentChatComposerApp {
    messages: Vec<AgentChatMessage>,
    status: AgentChatStatus,
    input: String,
    transcript_scroll: u16,
    palette_open: bool,
    palette_selected: usize,
    palette_query: String,
    palette_scroll: usize,
    help_open: bool,
}

impl AgentChatComposerApp {
    fn new(messages: Vec<AgentChatMessage>, status: AgentChatStatus) -> Self {
        Self {
            messages,
            status,
            input: String::new(),
            transcript_scroll: 0,
            palette_open: false,
            palette_selected: 0,
            palette_query: String::new(),
            palette_scroll: 0,
            help_open: false,
        }
    }

    fn open_help(&mut self) {
        self.palette_open = false;
        self.help_open = true;
    }

    fn close_help(&mut self) {
        self.help_open = false;
    }

    fn open_palette(&mut self) {
        if self.status.command_palette.is_empty() {
            self.status.notice = "No command palette actions available.".to_string();
            return;
        }
        self.palette_open = true;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.palette_scroll = 0;
        self.normalize_palette_selection();
    }

    fn close_palette(&mut self) {
        self.palette_open = false;
    }

    fn push_palette_query(&mut self, ch: char) {
        self.palette_query.push(ch);
        self.palette_scroll = 0;
        self.normalize_palette_selection();
    }

    fn backspace_palette_query_or_close(&mut self) {
        if self.palette_query.is_empty() {
            self.close_palette();
        } else {
            self.palette_query.pop();
            self.palette_scroll = 0;
            self.normalize_palette_selection();
        }
    }

    fn next_palette_item(&mut self) {
        let visible = self.visible_palette_indices();
        if visible.is_empty() {
            return;
        }
        let position = selected_visible_position(self.palette_selected, &visible).unwrap_or(0);
        self.palette_selected = visible[(position + 1).min(visible.len() - 1)];
    }

    fn previous_palette_item(&mut self) {
        let visible = self.visible_palette_indices();
        if visible.is_empty() {
            return;
        }
        let position = selected_visible_position(self.palette_selected, &visible).unwrap_or(0);
        self.palette_selected = visible[position.saturating_sub(1)];
    }

    fn selected_palette_command(&self) -> Option<AgentChatCommand> {
        let visible = self.visible_palette_indices();
        if !visible.contains(&self.palette_selected) {
            return None;
        }
        self.status
            .command_palette
            .get(self.palette_selected)
            .map(|entry| entry.command.clone())
    }

    fn visible_palette_indices(&self) -> Vec<usize> {
        self.status
            .command_palette
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                palette_entry_matches_query(entry, &self.palette_query).then_some(idx)
            })
            .collect()
    }

    fn normalize_palette_selection(&mut self) {
        let visible = self.visible_palette_indices();
        if visible.is_empty() {
            self.palette_selected = 0;
            self.palette_scroll = 0;
        } else if !visible.contains(&self.palette_selected) {
            self.palette_selected = visible[0];
        }
    }

    fn palette_body_lines_and_selected_row(&self) -> (Vec<Line<'static>>, Option<usize>) {
        let visible = self.visible_palette_indices();
        let mut lines = Vec::new();
        let mut selected_row = None;
        let mut previous_section = None::<String>;
        for idx in visible.iter().copied() {
            let Some(entry) = self.status.command_palette.get(idx) else {
                continue;
            };
            if previous_section.as_deref() != Some(entry.section.as_str()) {
                if previous_section.is_some() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    entry.section.clone(),
                    title_style(),
                )));
                previous_section = Some(entry.section.clone());
            }
            let marker = if idx == self.palette_selected {
                "›"
            } else {
                " "
            };
            let style = if idx == self.palette_selected {
                selected_style()
            } else {
                base_style()
            };
            if idx == self.palette_selected {
                selected_row = Some(lines.len());
            }
            lines.push(Line::from(Span::styled(
                format!("{marker} {}", entry.label),
                style,
            )));
            if !entry.description.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", entry.description),
                    dim_style(),
                )));
            }
        }
        if visible.is_empty() {
            lines.push(Line::from(Span::styled(
                "No commands match your search.",
                dim_style(),
            )));
        }
        (lines, selected_row)
    }

    fn ensure_palette_selection_visible(
        &mut self,
        body_height: usize,
        selected_row: Option<usize>,
        total_lines: usize,
    ) {
        if body_height == 0 || total_lines <= body_height {
            self.palette_scroll = 0;
            return;
        }
        let max_scroll = total_lines.saturating_sub(body_height);
        if let Some(selected_row) = selected_row {
            if selected_row < body_height {
                self.palette_scroll = 0;
            } else if selected_row < self.palette_scroll {
                self.palette_scroll = selected_row;
            } else if selected_row >= self.palette_scroll.saturating_add(body_height) {
                self.palette_scroll = selected_row.saturating_add(1).saturating_sub(body_height);
            }
        }
        self.palette_scroll = self.palette_scroll.min(max_scroll);
    }

    fn scroll_down(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(8);
    }

    fn push_char(&mut self, ch: char) {
        self.input.push(ch);
    }

    fn insert_newline(&mut self) {
        self.input.push('\n');
    }

    fn backspace(&mut self) {
        self.input.pop();
    }

    fn submit_prompt(&mut self) -> Option<String> {
        let prompt = self.input.trim().to_string();
        if prompt.is_empty() {
            return None;
        }
        self.input.clear();
        Some(prompt)
    }

    fn scroll_up(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(8);
    }

    fn jump_to_top(&mut self) {
        self.transcript_scroll = 0;
    }

    fn jump_to_end(&mut self, terminal_height: u16) {
        self.transcript_scroll = self.max_transcript_scroll_for_terminal(terminal_height);
    }

    fn max_transcript_scroll_for_terminal(&self, terminal_height: u16) -> u16 {
        let reserved = 3 + 7 + 1;
        let transcript_area_height = terminal_height.saturating_sub(reserved).max(4);
        self.max_transcript_scroll(transcript_area_height)
    }

    fn max_transcript_scroll(&self, transcript_area_height: u16) -> u16 {
        let visible_lines = transcript_area_height.saturating_sub(2).max(1) as usize;
        agent_chat_transcript_lines(&self.messages, &self.status.notice)
            .len()
            .saturating_sub(visible_lines)
            .min(u16::MAX as usize) as u16
    }

    fn at_transcript_end(&self, transcript_area_height: u16) -> bool {
        self.transcript_scroll >= self.max_transcript_scroll(transcript_area_height)
    }

    fn cursor_position(&self, composer_area: Rect) -> Position {
        let inner_height = composer_area.height.saturating_sub(2).max(1);
        let lines = self.input.split('\n').collect::<Vec<_>>();
        let cursor_line = lines.len().saturating_sub(1) as u16;
        let cursor_col = lines
            .last()
            .map(|line| line.chars().count())
            .unwrap_or_default() as u16;
        Position::new(
            composer_area.x + cursor_col.min(composer_area.width.saturating_sub(1)),
            composer_area.y + 1 + cursor_line.min(inner_height.saturating_sub(1)),
        )
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(4),
                Constraint::Length(7),
                Constraint::Length(1),
            ])
            .split(frame.area());

        let header_title = format!(
            "session {} • profile {} • model {}",
            self.status.session_id, self.status.profile, self.status.model
        );
        let tabs = Tabs::new(
            APP_TABS
                .iter()
                .map(|tab| Line::from(Span::styled(*tab, dim_style())))
                .collect::<Vec<_>>(),
        )
        .block(block(&header_title))
        .select(0)
        .style(dim_style())
        .highlight_style(selected_style());
        frame.render_widget(Clear, chunks[0]);
        frame.render_widget(tabs, chunks[0]);

        let transcript_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(chunks[1]);
        let transcript = agent_chat_transcript_lines(&self.messages, &self.status.notice);
        let transcript_title = if self.at_transcript_end(chunks[1].height) {
            "Transcript"
        } else {
            "Transcript  ↓ End"
        };
        let transcript = Paragraph::new(transcript)
            .block(agent_chat_block(transcript_title))
            .style(base_style())
            .scroll((self.transcript_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, transcript_layout[0]);
        frame.render_widget(transcript, transcript_layout[0]);
        let scrollbar = Paragraph::new(transcript_scrollbar_lines(
            self.transcript_scroll,
            self.max_transcript_scroll(chunks[1].height),
            chunks[1].height,
        ))
        .style(dim_style());
        frame.render_widget(Clear, transcript_layout[1]);
        frame.render_widget(scrollbar, transcript_layout[1]);

        let input = self.composer_lines();
        let composer = Paragraph::new(input)
            .block(agent_chat_block("Composer"))
            .style(base_style())
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, chunks[2]);
        frame.render_widget(composer, chunks[2]);
        frame.set_cursor_position(self.cursor_position(chunks[2]));

        let footer = format!("Ctrl+/ help • cwd {}", self.status.workspace);
        frame.render_widget(Clear, chunks[3]);
        frame.render_widget(Paragraph::new(footer).style(dim_style()), chunks[3]);

        if self.palette_open {
            self.draw_palette(frame);
        }
        if self.help_open {
            self.draw_help(frame);
        }
    }

    fn draw_help(&self, frame: &mut ratatui::Frame) {
        let area = centered_rect(66, 58, frame.area());
        let lines = vec![
            Line::from(Span::styled("Agent chat", title_style())),
            Line::from(""),
            Line::from(vec![
                Span::styled("Enter", selected_style()),
                Span::raw(" send prompt"),
            ]),
            Line::from(vec![
                Span::styled("Shift+Enter", selected_style()),
                Span::raw(" insert newline"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+E", selected_style()),
                Span::raw(" edit prompt in $VISUAL/$EDITOR/nvim"),
            ]),
            Line::from(vec![
                Span::styled("Esc", selected_style()),
                Span::raw(" quit when composer is empty"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+C", selected_style()),
                Span::raw(" quit"),
            ]),
            Line::from(""),
            Line::from(Span::styled("Navigation", title_style())),
            Line::from(""),
            Line::from(vec![
                Span::styled("Ctrl+P", selected_style()),
                Span::raw(" open command palette"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+/", selected_style()),
                Span::raw(" open or close this help"),
            ]),
            Line::from(vec![
                Span::styled("Tab / Shift+Tab", selected_style()),
                Span::raw(" jump to Tools / Skills"),
            ]),
            Line::from(vec![
                Span::styled("↑/↓ or PgUp/PgDn", selected_style()),
                Span::raw(" scroll transcript"),
            ]),
            Line::from(vec![
                Span::styled("Home / End", selected_style()),
                Span::raw(" jump to transcript top / latest"),
            ]),
            Line::from(""),
            Line::from(Span::styled("Command palette", title_style())),
            Line::from(""),
            Line::from(vec![
                Span::styled("type", selected_style()),
                Span::raw(" fuzzy-search actions"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+N / Ctrl+P", selected_style()),
                Span::raw(" move selection"),
            ]),
            Line::from(vec![
                Span::styled("Enter", selected_style()),
                Span::raw(" run selected action"),
            ]),
            Line::from(vec![
                Span::styled("Esc", selected_style()),
                Span::raw(" close palette"),
            ]),
        ];
        let help = Paragraph::new(lines)
            .block(block("Help"))
            .style(base_style())
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, area);
        frame.render_widget(help, area);
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
            if self.palette_query.is_empty() {
                Span::styled("find action…", dim_style())
            } else {
                Span::raw(self.palette_query.clone())
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
            .scroll((self.palette_scroll.min(u16::MAX as usize) as u16, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, area);
        frame.render_widget(block("Command palette"), area);
        frame.render_widget(Paragraph::new(search_line).style(base_style()), chunks[0]);
        frame.render_widget(body, chunks[2]);
        let cursor_x = area
            .x
            .saturating_add(1)
            .saturating_add("Search: ".len() as u16)
            .saturating_add(self.palette_query.chars().count() as u16)
            .min(area.right().saturating_sub(2));
        frame.set_cursor_position(Position::new(cursor_x, area.y.saturating_add(1)));
    }

    fn composer_lines(&self) -> Vec<Line<'static>> {
        if self.input.is_empty() {
            return vec![Line::from(Span::styled(
                "Type a prompt and press Enter…",
                dim_style(),
            ))];
        }
        self.input
            .split('\n')
            .map(|line| Line::from(line.to_string()))
            .collect()
    }
}

fn agent_chat_transcript_lines(messages: &[AgentChatMessage], notice: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if messages.is_empty() {
        lines.push(Line::from(Span::styled(
            "Start a new agent conversation below.",
            dim_style(),
        )));
        lines.push(Line::from(Span::styled(
            "This is the runtime chat surface; saved Chats remains a separate history/memory browser.",
            dim_style(),
        )));
    } else {
        for message in messages {
            lines.extend(agent_chat_message_lines(message));
            lines.push(Line::from(""));
        }
    }
    if !notice.trim().is_empty() {
        lines.push(Line::from(Span::styled(notice.to_string(), dim_style())));
    }
    lines
}

fn agent_chat_newline_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::SHIFT) && matches!(code, KeyCode::Enter)
}

fn agent_chat_editor_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('e'))
}

fn agent_chat_help_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('/') | KeyCode::Char('_'))
}

fn agent_chat_palette_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('p'))
}

fn agent_chat_palette_next_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Down)
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('n')))
}

fn agent_chat_palette_previous_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Up)
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('p')))
}

fn palette_text_key(modifiers: KeyModifiers) -> bool {
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

fn palette_entry_matches_query(entry: &AgentChatCommandEntry, query: &str) -> bool {
    fuzzy_match(
        query,
        &format!("{} {} {}", entry.section, entry.label, entry.description),
    )
}

fn agent_chat_dashboard_target(code: KeyCode) -> Option<DashboardTab> {
    match code {
        KeyCode::Tab => Some(DashboardTab::Tools),
        KeyCode::BackTab => Some(DashboardTab::Skills),
        _ => None,
    }
}

fn agent_chat_quit_key(code: KeyCode, modifiers: KeyModifiers, input_empty: bool) -> bool {
    (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')))
        || (input_empty && matches!(code, KeyCode::Esc))
}

fn edit_agent_chat_input(terminal: &mut TuiTerminal, app: &mut AgentChatComposerApp) -> Result<()> {
    let edited = edit_text_in_external_editor(terminal, &app.input)?;
    app.input = normalize_editor_text(&edited);
    app.status.notice = "Composer updated from editor.".to_string();
    Ok(())
}

fn edit_text_in_external_editor(terminal: &mut TuiTerminal, current: &str) -> Result<String> {
    let path = env::temp_dir().join(format!(
        "djinn-agent-composer-{}-{}.md",
        std::process::id(),
        timestamp_nanos()
    ));
    fs::write(&path, current).with_context(|| format!("writing {}", path.display()))?;

    suspend_terminal(terminal)?;
    let editor_result = run_editor_for_path(&path);
    let resume_result = resume_terminal(terminal);
    let read_result =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()));
    let _ = fs::remove_file(&path);

    resume_result?;
    editor_result?;
    read_result
}

fn run_editor_for_path(path: &std::path::Path) -> Result<()> {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "nvim".to_string());
    let mut parts = editor.split_whitespace();
    let command = parts.next().unwrap_or("nvim");
    let status = ProcessCommand::new(command)
        .args(parts)
        .arg(path)
        .status()
        .with_context(|| format!("running editor `{editor}`"))?;
    if !status.success() {
        bail!("editor exited with status {status}");
    }
    Ok(())
}

fn normalize_editor_text(value: &str) -> String {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
        .to_string()
}

fn timestamp_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn transcript_scrollbar_lines(scroll: u16, max_scroll: u16, height: u16) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    if max_scroll == 0 {
        return (0..height).map(|_| Line::from(" ")).collect();
    }

    let mut cells = vec!["│"; height as usize];
    if scroll > 0 {
        cells[0] = "↑";
    }
    if scroll < max_scroll {
        cells[height.saturating_sub(1) as usize] = "↓";
    }

    let available = height.saturating_sub(2).max(1) as usize;
    let thumb = 1 + ((scroll as usize * available) / max_scroll.max(1) as usize);
    let thumb_max = if scroll < max_scroll {
        height.saturating_sub(2)
    } else {
        height.saturating_sub(1)
    } as usize;
    let thumb = thumb.min(thumb_max);
    cells[thumb] = "█";

    cells
        .into_iter()
        .map(|cell| Line::from(Span::styled(cell, dim_style())))
        .collect()
}

fn agent_chat_message_lines(message: &AgentChatMessage) -> Vec<Line<'static>> {
    let (label, label_style, content_style) = match message.role {
        AgentChatRole::User => (
            "You",
            Style::default().fg(CTP_GREEN).bg(CTP_SURFACE0),
            Style::default().fg(CTP_TEXT).bg(CTP_BASE),
        ),
        AgentChatRole::Assistant => (
            "Djinn",
            title_style().bg(CTP_SURFACE0),
            Style::default().fg(CTP_TEXT).bg(CTP_BASE),
        ),
        AgentChatRole::Thought => (
            "Thought",
            Style::default().fg(CTP_MAUVE).bg(CTP_SURFACE0),
            Style::default().fg(CTP_SUBTEXT0).bg(CTP_SURFACE0),
        ),
        AgentChatRole::Tool => (
            "Tool Request",
            Style::default().fg(CTP_PEACH).bg(CTP_SURFACE1),
            Style::default().fg(CTP_YELLOW).bg(CTP_SURFACE1),
        ),
        AgentChatRole::ToolOutput => (
            "Tool Execution",
            Style::default().fg(CTP_SKY).bg(CTP_SURFACE0),
            Style::default().fg(CTP_TEXT).bg(CTP_SURFACE0),
        ),
        AgentChatRole::Notice => ("Notice", dim_style(), dim_style()),
    };
    let content = message.content.trim();
    let label = agent_chat_message_label(message.role, label, content);
    let label_style = agent_chat_message_label_style(message.role, label_style, content);
    let mut lines = vec![Line::from(vec![
        Span::styled(" ", label_style),
        Span::styled(label, label_style.add_modifier(Modifier::BOLD)),
        Span::styled(" ", label_style),
    ])];
    if content.is_empty() {
        lines.push(Line::from(Span::styled(" (empty) ", content_style)));
    } else {
        for line in agent_chat_message_body_lines(message.role, content) {
            lines.push(Line::from(Span::styled(format!(" {line} "), content_style)));
        }
    }
    lines
}

fn agent_chat_message_label(role: AgentChatRole, default_label: &str, content: &str) -> String {
    match role {
        AgentChatRole::Tool => {
            tool_request_label(content).unwrap_or_else(|| default_label.to_string())
        }
        AgentChatRole::ToolOutput => {
            tool_execution_label(content).unwrap_or_else(|| default_label.to_string())
        }
        AgentChatRole::User
        | AgentChatRole::Assistant
        | AgentChatRole::Thought
        | AgentChatRole::Notice => default_label.to_string(),
    }
}

fn agent_chat_message_label_style(
    role: AgentChatRole,
    default_style: Style,
    content: &str,
) -> Style {
    match role {
        AgentChatRole::ToolOutput => {
            let status = content.lines().next().and_then(|line| {
                parse_tool_execution_status(line.trim()).map(|(_, status)| status)
            });
            match status {
                Some("ok") => default_style.fg(CTP_GREEN),
                Some("failed") => default_style.fg(CTP_RED),
                _ => default_style,
            }
        }
        AgentChatRole::User
        | AgentChatRole::Assistant
        | AgentChatRole::Thought
        | AgentChatRole::Tool
        | AgentChatRole::Notice => default_style,
    }
}

fn agent_chat_message_body_lines(role: AgentChatRole, content: &str) -> Vec<String> {
    match role {
        AgentChatRole::Tool => tool_request_body_lines(content),
        AgentChatRole::ToolOutput => tool_execution_body_lines(content),
        AgentChatRole::User
        | AgentChatRole::Assistant
        | AgentChatRole::Thought
        | AgentChatRole::Notice => content.lines().map(ToOwned::to_owned).collect(),
    }
}

fn tool_request_label(content: &str) -> Option<String> {
    let first = content.lines().next()?.trim();
    if first.starts_with("# Running in ") {
        return Some("▶ Tool Request · shell".to_string());
    }
    let (name, _) = first.split_once(':')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(format!("▶ Tool Request · {name}"))
}

fn tool_execution_label(content: &str) -> Option<String> {
    let (name, status) = parse_tool_execution_status(content.lines().next()?.trim())?;
    Some(format!(
        "{} Tool Execution · {name} · {status}",
        tool_execution_status_glyph(status)
    ))
}

fn tool_execution_status_glyph(status: &str) -> &'static str {
    match status {
        "ok" => "✓",
        "failed" => "✗",
        _ => "•",
    }
}

fn parse_tool_execution_status(line: &str) -> Option<(&str, &str)> {
    let (name, status) = line.split_once(" result: ")?;
    let name = name.trim();
    let status = status.trim();
    if name.is_empty() || status.is_empty() {
        return None;
    }
    Some((name, status))
}

fn tool_request_body_lines(content: &str) -> Vec<String> {
    let mut lines = content.lines();
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    if first.trim_start().starts_with("# Running in ") {
        return std::iter::once(first)
            .chain(lines)
            .map(ToOwned::to_owned)
            .collect();
    }
    let mut rendered = Vec::new();
    if let Some((_, rest)) = first.split_once(':') {
        let rest = rest.trim();
        if !rest.is_empty() {
            rendered.push(rest.to_string());
        }
    } else {
        rendered.push(first.to_string());
    }
    rendered.extend(lines.map(ToOwned::to_owned));
    rendered
}

fn tool_execution_body_lines(content: &str) -> Vec<String> {
    let mut lines = content.lines();
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    if parse_tool_execution_status(first.trim()).is_some() {
        return lines.map(ToOwned::to_owned).collect();
    }
    std::iter::once(first)
        .chain(lines)
        .map(ToOwned::to_owned)
        .collect()
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
    execute!(stdout, EnterAlternateScreen, push_keyboard_enhancement())?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn leave_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    suspend_terminal(terminal)
}

fn suspend_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        PopKeyboardEnhancementFlags,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        push_keyboard_enhancement()
    )?;
    terminal.clear()?;
    Ok(())
}

fn push_keyboard_enhancement() -> PushKeyboardEnhancementFlags {
    PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
    )
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
                if app.help_open {
                    match key.code {
                        _ if agent_chat_help_key(key.code, key.modifiers) => app.close_help(),
                        KeyCode::Esc | KeyCode::Enter => app.close_help(),
                        KeyCode::Char('q') => return Ok(None),
                        _ => {}
                    }
                    continue;
                }

                if agent_chat_help_key(key.code, key.modifiers) {
                    app.open_help();
                    continue;
                }

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
                    KeyCode::Tab if dashboard_tab_returns_to_agent(app.active_tab) => {
                        return Ok(Some(TuiAction::OpenAgentChat));
                    }
                    KeyCode::Tab => app.next_tab(),
                    KeyCode::BackTab if dashboard_back_tab_returns_to_agent(app.active_tab) => {
                        return Ok(Some(TuiAction::OpenAgentChat));
                    }
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
                        DashboardTab::Chats => {
                            if let Some(request) = app.chats.selected_chat_session_request() {
                                return Ok(Some(TuiAction::OpenChatSession(request)));
                            }
                            app.chats.open_options();
                        }
                        DashboardTab::Skills => {
                            if let Some(skill) = app.skills.selected_skill().cloned() {
                                return Ok(Some(TuiAction::OpenSkill(skill)));
                            }
                        }
                        DashboardTab::Candidates | DashboardTab::Memories => {}
                    },
                    KeyCode::Char('r') => {
                        if app.active_tab == DashboardTab::Chats {
                            if let Some(request) = app.chats.selected_chat_session_request() {
                                return Ok(Some(TuiAction::OpenChatSession(request)));
                            }
                        } else if app.active_tab == DashboardTab::Candidates {
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
                    KeyCode::Char('s') => {
                        if app.active_tab == DashboardTab::Chats {
                            app.chats.open_options();
                        }
                    }
                    KeyCode::Char('x') | KeyCode::Delete => match app.active_tab {
                        DashboardTab::Chats => {
                            let ids = app.chats.selected_persisted_chat_ids();
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
    help_open: bool,
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
            help_open: false,
        }
    }

    fn open_help(&mut self) {
        self.help_open = true;
    }

    fn close_help(&mut self) {
        self.help_open = false;
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
            | TuiAction::OpenAgentChat
            | TuiAction::OpenChatSession(_)
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
            APP_TABS
                .iter()
                .map(|tab| Line::from(Span::styled(*tab, dim_style())))
                .collect::<Vec<_>>(),
        )
        .block(block(&header_title))
        .select(self.active_tab.index() + 1)
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

        frame.render_widget(Clear, chunks[2]);
        frame.render_widget(
            Paragraph::new("Ctrl+/ help • q quit").style(dim_style()),
            chunks[2],
        );

        if self.active_tab == DashboardTab::Chats && self.chats.mode == ChatUiMode::Options {
            self.chats.draw_options(frame);
        }
        if self.help_open {
            self.draw_help(frame);
        }
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
            Line::from(Span::styled("Chats / session picker", title_style())),
            Line::from(vec![
                Span::styled("Enter / r", selected_style()),
                Span::raw(" resume Djinn session or convert+resume OpenCode session"),
            ]),
            Line::from(vec![
                Span::styled("s", selected_style()),
                Span::raw(" open share options"),
            ]),
            Line::from(vec![
                Span::styled("Space / A", selected_style()),
                Span::raw(" select one / all visible"),
            ]),
            Line::from(vec![
                Span::styled("x / Delete", selected_style()),
                Span::raw(" remove persisted chat rows"),
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
        fuzzy_match(&self.filter.query, &chat.title)
            || fuzzy_match(&self.filter.query, &chat.id)
            || fuzzy_match(&self.filter.query, &chat.source)
            || fuzzy_match(&self.filter.query, &chat.source_id)
            || fuzzy_match(&self.filter.query, &chat.source_path)
            || fuzzy_match(&self.filter.query, &chat.content_path)
            || fuzzy_match(&self.filter.query, &chat.content)
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

    fn selected_persisted_chat_ids(&self) -> Vec<String> {
        self.selected_chat_ids()
            .into_iter()
            .filter(|id| {
                self.chats
                    .iter()
                    .find(|chat| chat.id == *id)
                    .map(|chat| chat.source != "djinn-agent")
                    .unwrap_or(false)
            })
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
        let chat_ids = chat_ids
            .into_iter()
            .filter(|id| {
                self.chats
                    .iter()
                    .find(|chat| chat.id == *id)
                    .map(|chat| chat.source != "djinn-agent")
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();
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
            "Chats ({} / {} visible, {} selected, {})",
            visible.len(),
            self.chats.len(),
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
    out.push_str(&format!("Actions: {}\n", chat_picker_action_hint(chat)));
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
        "djinn-agent" => "Enter/r resume session",
        "opencode" if !chat.source_id.trim().is_empty() => {
            "Enter/r convert+resume in Djinn • s share • x remove"
        }
        _ => "Enter/s share options • x remove",
    }
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

    fn rendered_agent_chat_message_lines(message: AgentChatMessage) -> Vec<String> {
        agent_chat_message_lines(&message)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect()
    }

    fn test_agent_chat_status(notice: impl Into<String>) -> AgentChatStatus {
        AgentChatStatus {
            session_id: "agt_test".to_string(),
            workspace: "/tmp/project".to_string(),
            profile: "default".to_string(),
            model: "openai/gpt-5.5".to_string(),
            notice: notice.into(),
            command_palette: Vec::new(),
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
        let djinn = ChatRecord {
            id: "agent:agt_1".to_string(),
            title: "Djinn".to_string(),
            content: String::new(),
            source: "djinn-agent".to_string(),
            source_id: "agt_1".to_string(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        };
        let opencode = ChatRecord {
            id: "chat".to_string(),
            title: "OpenCode".to_string(),
            content: String::new(),
            source: "opencode".to_string(),
            source_id: "ses_1".to_string(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        };

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
    fn chat_preview_surfaces_session_picker_actions() {
        let djinn = ChatRecord {
            id: "agent:agt_1".to_string(),
            title: "Djinn".to_string(),
            content: String::new(),
            source: "djinn-agent".to_string(),
            source_id: "agt_1".to_string(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        };
        let opencode = ChatRecord {
            id: "chat".to_string(),
            title: "OpenCode".to_string(),
            content: String::new(),
            source: "opencode".to_string(),
            source_id: "ses_1".to_string(),
            source_path: String::new(),
            content_path: String::new(),
            created_at: String::new(),
        };

        assert!(chat_preview(&djinn).contains("Actions: Enter/r resume session"));
        assert!(chat_preview(&opencode).contains("Actions: Enter/r convert+resume in Djinn"));
    }

    #[test]
    fn fuzzy_match_matches_subsequence_case_insensitive() {
        assert!(fuzzy_match("ocd", "OpenCode Debug Session"));
        assert!(fuzzy_match("tl", "tool-list"));
        assert!(!fuzzy_match("xyz", "tool-list"));
    }

    #[test]
    fn agent_chat_blocks_avoid_left_and_right_borders_for_copying() {
        let borders = agent_chat_borders();
        assert!(borders.contains(Borders::TOP));
        assert!(borders.contains(Borders::BOTTOM));
        assert!(!borders.contains(Borders::LEFT));
        assert!(!borders.contains(Borders::RIGHT));
    }

    #[test]
    fn agent_chat_transcript_starts_with_runtime_guidance() {
        let lines = agent_chat_transcript_lines(&[], "ready")
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(lines
            .iter()
            .any(|line| line.contains("Start a new agent conversation")));
        assert!(lines
            .iter()
            .any(|line| line.contains("runtime chat surface")));
        assert!(lines.iter().any(|line| line == "ready"));
    }

    #[test]
    fn agent_chat_message_lines_render_roles() {
        let lines = agent_chat_message_lines(&AgentChatMessage {
            role: AgentChatRole::Assistant,
            content: "Hello\nworld".to_string(),
        })
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

        assert_eq!(lines, vec![" Djinn ", " Hello ", " world "]);
    }

    #[test]
    fn agent_chat_tool_blocks_use_request_and_execution_labels() {
        let request_lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::Tool,
            content: "read_file: Cargo.toml".to_string(),
        });
        let execution_lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::ToolOutput,
            content: "read_file result: ok".to_string(),
        });

        assert_eq!(
            request_lines,
            vec![" ▶ Tool Request · read_file ", " Cargo.toml "]
        );
        assert_eq!(execution_lines, vec![" ✓ Tool Execution · read_file · ok "]);
    }

    #[test]
    fn agent_chat_failed_execution_uses_failure_glyph() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::ToolOutput,
            content: "shell result: failed\nexit 1".to_string(),
        });

        assert_eq!(
            lines,
            vec![" ✗ Tool Execution · shell · failed ", " exit 1 "]
        );
    }

    #[test]
    fn agent_chat_shell_request_label_preserves_command_block() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::Tool,
            content: "# Running in .\n$ cargo test".to_string(),
        });

        assert_eq!(
            lines,
            vec![
                " ▶ Tool Request · shell ",
                " # Running in . ",
                " $ cargo test "
            ]
        );
    }

    #[test]
    fn agent_chat_composer_keeps_status_and_scroll_state() {
        let mut app = AgentChatComposerApp::new(
            vec![AgentChatMessage {
                role: AgentChatRole::User,
                content: "hello".to_string(),
            }],
            test_agent_chat_status("Djinn is thinking…"),
        );

        assert_eq!(app.status.notice, "Djinn is thinking…");
        assert_eq!(app.messages.len(), 1);
        app.scroll_down();
        assert_eq!(app.transcript_scroll, 8);
        app.scroll_up();
        assert_eq!(app.transcript_scroll, 0);
    }

    #[test]
    fn agent_chat_composer_uses_shift_enter_newline_and_enter_submit_model() {
        let mut app = AgentChatComposerApp::new(Vec::new(), test_agent_chat_status(String::new()));

        app.push_char('h');
        app.push_char('i');
        app.insert_newline();
        app.push_char('t');
        app.push_char('h');
        app.push_char('e');
        app.push_char('r');
        app.push_char('e');

        assert_eq!(app.input, "hi\nthere");
        let rendered = app
            .composer_lines()
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered, vec!["hi", "there"]);
        assert_eq!(app.submit_prompt().as_deref(), Some("hi\nthere"));
        assert!(app.input.is_empty());
    }

    #[test]
    fn agent_chat_newline_key_uses_shift_enter_without_ctrl_j_fallback() {
        assert!(agent_chat_newline_key(KeyCode::Enter, KeyModifiers::SHIFT));
        assert!(!agent_chat_newline_key(
            KeyCode::Enter,
            KeyModifiers::CONTROL
        ));
        assert!(!agent_chat_newline_key(
            KeyCode::Char('j'),
            KeyModifiers::CONTROL
        ));
        assert!(!agent_chat_newline_key(KeyCode::Enter, KeyModifiers::NONE));
    }

    #[test]
    fn agent_chat_editor_key_uses_ctrl_e() {
        assert!(agent_chat_editor_key(
            KeyCode::Char('e'),
            KeyModifiers::CONTROL
        ));
        assert!(!agent_chat_editor_key(
            KeyCode::Char('e'),
            KeyModifiers::NONE
        ));
        assert!(!agent_chat_editor_key(
            KeyCode::Char('j'),
            KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn agent_chat_help_key_uses_ctrl_slash() {
        assert!(agent_chat_help_key(
            KeyCode::Char('/'),
            KeyModifiers::CONTROL
        ));
        assert!(agent_chat_help_key(
            KeyCode::Char('_'),
            KeyModifiers::CONTROL
        ));
        assert!(!agent_chat_help_key(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(!agent_chat_help_key(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn agent_chat_help_open_closes_palette() {
        let mut status = test_agent_chat_status("Ready.");
        status.command_palette = vec![AgentChatCommandEntry {
            section: "Session".to_string(),
            label: "Resume session…".to_string(),
            description: String::new(),
            command: AgentChatCommand::OpenSessions,
        }];
        let mut app = AgentChatComposerApp::new(Vec::new(), status);

        app.open_palette();
        app.open_help();

        assert!(!app.palette_open);
        assert!(app.help_open);

        app.close_help();
        assert!(!app.help_open);
    }

    #[test]
    fn agent_chat_palette_key_uses_ctrl_p() {
        assert!(agent_chat_palette_key(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL
        ));
        assert!(agent_chat_palette_previous_key(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL
        ));
        assert!(agent_chat_palette_next_key(
            KeyCode::Char('n'),
            KeyModifiers::CONTROL
        ));
        assert!(!agent_chat_palette_key(
            KeyCode::Char('p'),
            KeyModifiers::NONE
        ));
        assert!(!agent_chat_palette_key(
            KeyCode::Char('e'),
            KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn agent_chat_palette_selects_commands() {
        let mut status = test_agent_chat_status("Ready.");
        status.command_palette = vec![
            AgentChatCommandEntry {
                section: "Session".to_string(),
                label: "Resume session…".to_string(),
                description: String::new(),
                command: AgentChatCommand::OpenSessions,
            },
            AgentChatCommandEntry {
                section: "Model".to_string(),
                label: "Switch model · test".to_string(),
                description: String::new(),
                command: AgentChatCommand::SwitchModel("test".to_string()),
            },
        ];
        let mut app = AgentChatComposerApp::new(Vec::new(), status);

        app.open_palette();
        app.next_palette_item();

        assert!(app.palette_open);
        assert_eq!(
            app.selected_palette_command(),
            Some(AgentChatCommand::SwitchModel("test".to_string()))
        );
    }

    #[test]
    fn agent_chat_palette_filters_commands_with_fuzzy_query() {
        let mut status = test_agent_chat_status("Ready.");
        status.command_palette = vec![
            AgentChatCommandEntry {
                section: "Session".to_string(),
                label: "Resume session…".to_string(),
                description: "Open the Chats/session picker".to_string(),
                command: AgentChatCommand::OpenSessions,
            },
            AgentChatCommandEntry {
                section: "Profile".to_string(),
                label: "Switch profile · architect".to_string(),
                description: String::new(),
                command: AgentChatCommand::SwitchProfile("architect".to_string()),
            },
            AgentChatCommandEntry {
                section: "Model".to_string(),
                label: "Switch model · openai/gpt-5.5".to_string(),
                description: String::new(),
                command: AgentChatCommand::SwitchModel("openai/gpt-5.5".to_string()),
            },
        ];
        let mut app = AgentChatComposerApp::new(Vec::new(), status);

        app.open_palette();
        for ch in "mdl".chars() {
            app.push_palette_query(ch);
        }

        assert_eq!(app.visible_palette_indices(), vec![2]);
        assert_eq!(
            app.selected_palette_command(),
            Some(AgentChatCommand::SwitchModel("openai/gpt-5.5".to_string()))
        );
    }

    #[test]
    fn agent_chat_palette_scroll_keeps_selected_action_visible() {
        let mut status = test_agent_chat_status("Ready.");
        status.command_palette = (0..12)
            .map(|idx| AgentChatCommandEntry {
                section: "Model".to_string(),
                label: format!("Switch model · model-{idx}"),
                description: String::new(),
                command: AgentChatCommand::SwitchModel(format!("model-{idx}")),
            })
            .collect();
        let mut app = AgentChatComposerApp::new(Vec::new(), status);

        app.open_palette();
        for _ in 0..10 {
            app.next_palette_item();
        }
        let (lines, selected_row) = app.palette_body_lines_and_selected_row();
        app.ensure_palette_selection_visible(5, selected_row, lines.len());

        let selected_row = selected_row.unwrap();
        assert!(app.palette_scroll > 0);
        assert!(selected_row >= app.palette_scroll);
        assert!(selected_row < app.palette_scroll + 5);

        for _ in 0..10 {
            app.previous_palette_item();
        }
        let (lines, selected_row) = app.palette_body_lines_and_selected_row();
        app.ensure_palette_selection_visible(5, selected_row, lines.len());

        assert_eq!(app.palette_scroll, 0);
    }

    #[test]
    fn agent_chat_dashboard_target_uses_tab_direction() {
        assert_eq!(
            agent_chat_dashboard_target(KeyCode::Tab),
            Some(DashboardTab::Tools)
        );
        assert_eq!(
            agent_chat_dashboard_target(KeyCode::BackTab),
            Some(DashboardTab::Skills)
        );
        assert_eq!(agent_chat_dashboard_target(KeyCode::Char('t')), None);
    }

    #[test]
    fn normalize_editor_text_removes_one_final_editor_newline() {
        assert_eq!(normalize_editor_text("hello\n"), "hello");
        assert_eq!(normalize_editor_text("hello\r\n"), "hello");
        assert_eq!(normalize_editor_text("hello\n\n"), "hello\n");
        assert_eq!(normalize_editor_text("hello"), "hello");
    }

    #[test]
    fn agent_chat_quit_key_does_not_treat_q_as_quit() {
        assert!(agent_chat_quit_key(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            false
        ));
        assert!(agent_chat_quit_key(KeyCode::Esc, KeyModifiers::NONE, true));
        assert!(!agent_chat_quit_key(
            KeyCode::Esc,
            KeyModifiers::NONE,
            false
        ));
        assert!(!agent_chat_quit_key(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            true
        ));
        assert!(!agent_chat_quit_key(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            false
        ));
    }

    #[test]
    fn agent_chat_composer_cursor_tracks_multiline_input() {
        let mut app = AgentChatComposerApp::new(Vec::new(), test_agent_chat_status(String::new()));
        app.input = "hello\nworld".to_string();

        assert_eq!(
            app.cursor_position(Rect::new(10, 20, 40, 7)),
            Position::new(15, 22)
        );
    }

    #[test]
    fn agent_chat_composer_jumps_to_transcript_end_without_autoscroll() {
        let messages = (0..12)
            .map(|idx| AgentChatMessage {
                role: AgentChatRole::User,
                content: format!("message {idx}"),
            })
            .collect::<Vec<_>>();
        let mut app = AgentChatComposerApp::new(messages, test_agent_chat_status("Ready."));

        assert_eq!(app.transcript_scroll, 0);
        let max_scroll = app.max_transcript_scroll_for_terminal(17);
        assert!(max_scroll > 0);
        app.messages.push(AgentChatMessage {
            role: AgentChatRole::Assistant,
            content: "new answer".to_string(),
        });
        assert_eq!(app.transcript_scroll, 0);
        app.jump_to_end(17);
        assert_eq!(
            app.transcript_scroll,
            app.max_transcript_scroll_for_terminal(17)
        );
        app.jump_to_top();
        assert_eq!(app.transcript_scroll, 0);
    }

    #[test]
    fn transcript_scrollbar_shows_bottom_arrow_until_end() {
        let rendered = transcript_scrollbar_lines(0, 10, 5)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered.last().map(String::as_str), Some("↓"));

        let rendered = transcript_scrollbar_lines(10, 10, 5)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_ne!(rendered.last().map(String::as_str), Some("↓"));
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
        assert!(dashboard_tab_returns_to_agent(DashboardTab::Skills));
        assert!(!dashboard_tab_returns_to_agent(DashboardTab::Tools));
        assert!(dashboard_back_tab_returns_to_agent(DashboardTab::Tools));
        assert!(!dashboard_back_tab_returns_to_agent(DashboardTab::Skills));
        assert_eq!(
            APP_TABS,
            [
                "Agent",
                "Tools",
                "Chats",
                "Memories",
                "Suggestions",
                "Skills"
            ]
        );
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
            DashboardTab::Chats,
        );

        assert!(!app.help_open);
        app.open_help();
        assert!(app.help_open);
        app.close_help();
        assert!(!app.help_open);
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
