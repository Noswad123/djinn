mod approval;
mod editor;
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
use editor::{edit_text_in_external_editor, normalize_editor_text};
use filter::{fuzzy_match, selected_visible_position, FilterState};
use grouped_select::{GroupedSelectItem, GroupedSelectState};
use keys::*;
use style::*;
use terminal::{enter_terminal, leave_terminal, resume_terminal, suspend_terminal, TuiTerminal};

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
            chats,
            memories,
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
    OpenAgentChat,
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

impl GroupedSelectItem for AgentChatCommandEntry {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentChatCommand {
    OpenHelp,
    ScrollHalfPageUp,
    ScrollHalfPageDown,
    JumpFirstMessage,
    JumpPreviousMessage,
    JumpNextMessage,
    JumpLastMessage,
    JumpLastUserMessage,
    NewSession,
    OpenSessions,
    AddCredential,
    OpenDashboardTab(DashboardTab),
    SwitchProfile(String),
    SwitchModel(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardCommand {
    OpenAgent,
    OpenTab(DashboardTab),
    OpenHelp,
    ToggleFilter,
    OpenSelected,
    ResumeSelectedChat,
    PromoteSessions,
    SetSessionScope(SessionFilterScope),
    ToggleSelected,
    ToggleAll,
    AcceptSelected,
    RejectSelected,
    DeleteSelected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionFilterScope {
    All,
    Promotable,
    DjinnAgent,
    ChildAgent,
}

impl SessionFilterScope {
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

    fn description(self) -> &'static str {
        match self {
            Self::All => "Show all session rows",
            Self::Promotable => "Show persisted rows that can be promoted",
            Self::DjinnAgent => "Show projected Djinn agent sessions",
            Self::ChildAgent => "Show projected child agent sessions with parent metadata",
        }
    }

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
enum AgentChatRenderMode {
    Markdown,
    Raw,
}

impl AgentChatRenderMode {
    fn toggle(self) -> Self {
        match self {
            Self::Markdown => Self::Raw,
            Self::Raw => Self::Markdown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Markdown => "rendered Markdown",
            Self::Raw => "raw Markdown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentToolDetailMode {
    Collapsed,
    Full,
}

impl AgentToolDetailMode {
    fn toggle(self) -> Self {
        match self {
            Self::Collapsed => Self::Full,
            Self::Full => Self::Collapsed,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Collapsed => "compact tools",
            Self::Full => "full tools",
        }
    }

    fn is_full(self) -> bool {
        self == Self::Full
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AgentChatMessageRenderContext<'a> {
    profile: Option<&'a str>,
    model: Option<&'a str>,
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

fn dashboard_tab_returns_to_agent(tab: DashboardTab) -> bool {
    tab == DashboardTab::Skills
}

fn dashboard_back_tab_returns_to_agent(tab: DashboardTab) -> bool {
    tab == DashboardTab::Tools
}

const DASHBOARD_TABS: [&str; 5] = ["Tools", "Sessions", "Memories", "Suggestions", "Skills"];
const APP_TABS: [&str; 6] = [
    "Agent",
    "Tools",
    "Sessions",
    "Memories",
    "Suggestions",
    "Skills",
];

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
                if !actionable_key_event(&key) {
                    continue;
                }
                if app.help_open {
                    match key.code {
                        _ if agent_chat_help_key(key.code, key.modifiers) => app.close_help(),
                        KeyCode::Esc | KeyCode::Enter => app.close_help(),
                        _ if agent_chat_quit_key(key.code, key.modifiers, app.input.is_empty()) => {
                            return Ok(None);
                        }
                        _ => {}
                    }
                    continue;
                }
                if app.palette.open {
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
                                let size = terminal.size()?;
                                if !app.handle_local_palette_command(
                                    &command,
                                    size.height,
                                    size.width,
                                ) {
                                    app.close_palette();
                                    app.status.notice =
                                        "That command is available in interactive session mode."
                                            .to_string();
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
                match key.code {
                    _ if agent_chat_quit_key(key.code, key.modifiers, app.input.is_empty()) => {
                        return Ok(None);
                    }
                    _ if agent_chat_slash_palette_key(
                        key.code,
                        key.modifiers,
                        app.input.is_empty(),
                    ) =>
                    {
                        app.open_palette();
                    }
                    _ if agent_chat_newline_key(key.code, key.modifiers) => {
                        app.insert_newline();
                    }
                    _ if agent_chat_editor_key(key.code, key.modifiers) => {
                        if let Err(error) = edit_agent_chat_input(terminal, &mut app) {
                            app.status.notice = format!("Editor failed: {error:#}");
                        }
                    }
                    _ if agent_chat_render_mode_key(key.code, key.modifiers) => {
                        app.toggle_render_mode();
                    }
                    _ if agent_chat_tool_detail_key(key.code, key.modifiers) => {
                        app.toggle_tool_detail_mode();
                    }
                    _ if agent_chat_half_page_up_key(key.code, key.modifiers) => {
                        app.scroll_half_page_up(terminal.size()?.height);
                    }
                    _ if agent_chat_half_page_down_key(key.code, key.modifiers) => {
                        app.scroll_half_page_down(terminal.size()?.height);
                    }
                    _ if agent_chat_first_message_key(key.code, key.modifiers) => {
                        app.jump_to_first_message();
                    }
                    _ if agent_chat_previous_message_key(key.code, key.modifiers) => {
                        let size = terminal.size()?;
                        app.jump_to_previous_message(size.height, size.width);
                    }
                    _ if agent_chat_next_message_key(key.code, key.modifiers) => {
                        let size = terminal.size()?;
                        app.jump_to_next_message(size.height, size.width);
                    }
                    _ if agent_chat_last_message_key(key.code, key.modifiers) => {
                        let size = terminal.size()?;
                        app.jump_to_last_message(size.height, size.width);
                    }
                    _ if agent_chat_last_user_message_key(key.code, key.modifiers) => {
                        let size = terminal.size()?;
                        app.jump_to_last_user_message(size.height, size.width);
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
                if !actionable_key_event(&key) {
                    continue;
                }
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
                if app.palette.open {
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
                                let size = terminal.size()?;
                                if app.handle_local_palette_command(
                                    &command,
                                    size.height,
                                    size.width,
                                ) {
                                    continue;
                                }
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
                    _ if agent_chat_slash_palette_key(
                        key.code,
                        key.modifiers,
                        app.input.is_empty(),
                    ) =>
                    {
                        app.open_palette();
                    }
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
                    _ if agent_chat_render_mode_key(key.code, key.modifiers) => {
                        app.toggle_render_mode();
                    }
                    _ if agent_chat_tool_detail_key(key.code, key.modifiers) => {
                        app.toggle_tool_detail_mode();
                    }
                    _ if agent_chat_half_page_up_key(key.code, key.modifiers) => {
                        app.scroll_half_page_up(terminal.size()?.height);
                    }
                    _ if agent_chat_half_page_down_key(key.code, key.modifiers) => {
                        app.scroll_half_page_down(terminal.size()?.height);
                    }
                    _ if agent_chat_first_message_key(key.code, key.modifiers) => {
                        app.jump_to_first_message();
                    }
                    _ if agent_chat_previous_message_key(key.code, key.modifiers) => {
                        let size = terminal.size()?;
                        app.jump_to_previous_message(size.height, size.width);
                    }
                    _ if agent_chat_next_message_key(key.code, key.modifiers) => {
                        let size = terminal.size()?;
                        app.jump_to_next_message(size.height, size.width);
                    }
                    _ if agent_chat_last_message_key(key.code, key.modifiers) => {
                        let size = terminal.size()?;
                        app.jump_to_last_message(size.height, size.width);
                    }
                    _ if agent_chat_last_user_message_key(key.code, key.modifiers) => {
                        let size = terminal.size()?;
                        app.jump_to_last_user_message(size.height, size.width);
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
    palette: GroupedSelectState,
    help_open: bool,
    render_mode: AgentChatRenderMode,
    tool_detail_mode: AgentToolDetailMode,
}

const AGENT_CHAT_HEADER_HEIGHT: u16 = 3;
const AGENT_CHAT_COMPOSER_HEIGHT: u16 = 6;
const AGENT_CHAT_FOOTER_HEIGHT: u16 = 1;
const AGENT_CHAT_COMPOSER_INPUT_MAX_LINES: usize = 4;

impl AgentChatComposerApp {
    fn new(messages: Vec<AgentChatMessage>, status: AgentChatStatus) -> Self {
        Self {
            messages,
            status,
            input: String::new(),
            transcript_scroll: 0,
            palette: GroupedSelectState::default(),
            help_open: false,
            render_mode: AgentChatRenderMode::Markdown,
            tool_detail_mode: AgentToolDetailMode::Collapsed,
        }
    }

    fn toggle_render_mode(&mut self) {
        self.render_mode = self.render_mode.toggle();
        self.status.notice = format!("Transcript display: {}.", self.render_mode.label());
    }

    fn toggle_tool_detail_mode(&mut self) {
        self.tool_detail_mode = self.tool_detail_mode.toggle();
        self.status.notice = format!("Tool output display: {}.", self.tool_detail_mode.label());
    }

    fn open_help(&mut self) {
        self.palette.close();
        self.help_open = true;
    }

    fn close_help(&mut self) {
        self.help_open = false;
    }

    fn open_palette(&mut self) {
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

    fn selected_palette_command(&self) -> Option<AgentChatCommand> {
        let entries = self.agent_chat_command_palette();
        let visible = self.visible_palette_indices();
        grouped_select::selected_item(&entries, &visible, self.palette.selected, |entry| {
            entry.command.clone()
        })
    }

    fn visible_palette_indices(&self) -> Vec<usize> {
        grouped_select::visible_indices(&self.agent_chat_command_palette(), &self.palette.query)
    }

    fn handle_local_palette_command(
        &mut self,
        command: &AgentChatCommand,
        terminal_height: u16,
        terminal_width: u16,
    ) -> bool {
        match command {
            AgentChatCommand::OpenHelp => {
                self.open_help();
                true
            }
            AgentChatCommand::ScrollHalfPageUp => {
                self.scroll_half_page_up(terminal_height);
                self.close_palette();
                true
            }
            AgentChatCommand::ScrollHalfPageDown => {
                self.scroll_half_page_down(terminal_height);
                self.close_palette();
                true
            }
            AgentChatCommand::JumpFirstMessage => {
                self.jump_to_first_message();
                self.close_palette();
                true
            }
            AgentChatCommand::JumpPreviousMessage => {
                self.jump_to_previous_message(terminal_height, terminal_width);
                self.close_palette();
                true
            }
            AgentChatCommand::JumpNextMessage => {
                self.jump_to_next_message(terminal_height, terminal_width);
                self.close_palette();
                true
            }
            AgentChatCommand::JumpLastMessage => {
                self.jump_to_last_message(terminal_height, terminal_width);
                self.close_palette();
                true
            }
            AgentChatCommand::JumpLastUserMessage => {
                self.jump_to_last_user_message(terminal_height, terminal_width);
                self.close_palette();
                true
            }
            _ => false,
        }
    }

    fn agent_chat_command_palette(&self) -> Vec<AgentChatCommandEntry> {
        let local_entries = agent_chat_local_command_entries();
        let mut entries = Vec::with_capacity(
            self.status
                .command_palette
                .len()
                .saturating_add(local_entries.len()),
        );
        entries.push(agent_chat_help_command_entry());
        entries.extend(local_entries);
        entries.extend(self.status.command_palette.clone());
        entries
    }

    fn normalize_palette_selection(&mut self) {
        let visible = self.visible_palette_indices();
        self.palette.normalize_selection(&visible);
    }

    fn palette_body_lines_and_selected_row(&self) -> (Vec<Line<'static>>, Option<usize>) {
        let entries = self.agent_chat_command_palette();
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

    fn scroll_down(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(8);
    }

    fn scroll_half_page_up(&mut self, terminal_height: u16) {
        let amount = self.half_page_scroll_amount(terminal_height);
        self.transcript_scroll = self.transcript_scroll.saturating_sub(amount);
    }

    fn scroll_half_page_down(&mut self, terminal_height: u16) {
        let amount = self.half_page_scroll_amount(terminal_height);
        let max_scroll = self.max_transcript_scroll_for_terminal(terminal_height);
        self.transcript_scroll = self
            .transcript_scroll
            .saturating_add(amount)
            .min(max_scroll);
    }

    fn half_page_scroll_amount(&self, terminal_height: u16) -> u16 {
        self.visible_transcript_lines_for_terminal(terminal_height)
            .saturating_div(2)
            .max(1)
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

    fn jump_to_first_message(&mut self) {
        self.transcript_scroll = self
            .message_start_offsets(None)
            .first()
            .map(|(_, offset)| *offset)
            .unwrap_or(0);
    }

    fn jump_to_end(&mut self, terminal_height: u16) {
        self.transcript_scroll = self.max_transcript_scroll_for_terminal(terminal_height);
    }

    fn jump_to_previous_message(&mut self, terminal_height: u16, terminal_width: u16) {
        if let Some((_, offset)) = self
            .message_start_offsets(Some(terminal_width as usize))
            .into_iter()
            .rev()
            .find(|(_, offset)| *offset < self.transcript_scroll)
        {
            self.set_transcript_scroll_to_message_offset(offset, terminal_height);
        }
    }

    fn jump_to_next_message(&mut self, terminal_height: u16, terminal_width: u16) {
        if let Some((_, offset)) = self
            .message_start_offsets(Some(terminal_width as usize))
            .into_iter()
            .find(|(_, offset)| *offset > self.transcript_scroll)
        {
            self.set_transcript_scroll_to_message_offset(offset, terminal_height);
        }
    }

    fn jump_to_last_message(&mut self, terminal_height: u16, terminal_width: u16) {
        if let Some((_, offset)) = self
            .message_start_offsets(Some(terminal_width as usize))
            .last()
            .copied()
        {
            self.set_transcript_scroll_to_message_offset(offset, terminal_height);
        }
    }

    fn jump_to_last_user_message(&mut self, terminal_height: u16, terminal_width: u16) {
        if let Some((_, offset)) = self
            .message_start_offsets(Some(terminal_width as usize))
            .into_iter()
            .rev()
            .find(|(role, _)| *role == AgentChatRole::User)
        {
            self.set_transcript_scroll_to_message_offset(offset, terminal_height);
        }
    }

    fn set_transcript_scroll_to_message_offset(&mut self, offset: u16, terminal_height: u16) {
        self.transcript_scroll =
            offset.min(self.max_transcript_scroll_for_terminal(terminal_height));
    }

    fn message_start_offsets(&self, code_block_width: Option<usize>) -> Vec<(AgentChatRole, u16)> {
        let mut offsets = Vec::new();
        let mut line_offset = 0usize;
        let context = self.message_render_context();
        for message in &self.messages {
            offsets.push((message.role, line_offset.min(u16::MAX as usize) as u16));
            line_offset = line_offset.saturating_add(
                agent_chat_message_lines_with_mode(
                    message,
                    self.render_mode,
                    self.tool_detail_mode,
                    code_block_width,
                    Some(context),
                )
                .len()
                .saturating_add(1),
            );
        }
        offsets
    }

    fn visible_transcript_lines_for_terminal(&self, terminal_height: u16) -> u16 {
        self.transcript_area_height_for_terminal(terminal_height)
            .saturating_sub(2)
            .max(1)
    }

    fn transcript_area_height_for_terminal(&self, terminal_height: u16) -> u16 {
        let reserved = AGENT_CHAT_HEADER_HEIGHT
            .saturating_add(AGENT_CHAT_COMPOSER_HEIGHT)
            .saturating_add(AGENT_CHAT_FOOTER_HEIGHT);
        terminal_height.saturating_sub(reserved).max(4)
    }

    fn max_transcript_scroll_for_terminal(&self, terminal_height: u16) -> u16 {
        self.max_transcript_scroll(self.transcript_area_height_for_terminal(terminal_height))
    }

    fn max_transcript_scroll(&self, transcript_area_height: u16) -> u16 {
        let visible_lines = transcript_area_height.saturating_sub(2).max(1) as usize;
        let context = self.message_render_context();
        agent_chat_transcript_lines_with_mode(
            &self.messages,
            &self.status.notice,
            self.render_mode,
            self.tool_detail_mode,
            None,
            Some(context),
        )
        .len()
        .saturating_sub(visible_lines)
        .min(u16::MAX as usize) as u16
    }

    fn at_transcript_end(&self, transcript_area_height: u16) -> bool {
        self.transcript_scroll >= self.max_transcript_scroll(transcript_area_height)
    }

    fn message_render_context(&self) -> AgentChatMessageRenderContext<'_> {
        AgentChatMessageRenderContext {
            profile: non_empty_context_value(&self.status.profile),
            model: non_empty_context_value(&self.status.model),
        }
    }

    fn cursor_position(&self, composer_area: Rect) -> Position {
        let inner_height = composer_area.height.saturating_sub(2).max(1);
        let input_lines = self.visible_composer_input_lines();
        let cursor_line = input_lines.len().saturating_sub(1) as u16;
        let cursor_col = input_lines
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
                Constraint::Length(AGENT_CHAT_HEADER_HEIGHT),
                Constraint::Min(4),
                Constraint::Length(AGENT_CHAT_COMPOSER_HEIGHT),
                Constraint::Length(AGENT_CHAT_FOOTER_HEIGHT),
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

        let transcript = agent_chat_transcript_lines_with_mode(
            &self.messages,
            &self.status.notice,
            self.render_mode,
            self.tool_detail_mode,
            Some(chunks[1].width as usize),
            Some(self.message_render_context()),
        );
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
        frame.render_widget(Clear, chunks[1]);
        frame.render_widget(transcript, chunks[1]);

        let input = self.composer_lines();
        let composer = Paragraph::new(input)
            .block(agent_chat_block("Composer"))
            .style(base_style())
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, chunks[2]);
        frame.render_widget(composer, chunks[2]);
        frame.set_cursor_position(self.cursor_position(chunks[2]));

        let footer = format!(
            "{} transcript • {} tool output • cwd {}",
            self.render_mode.label(),
            self.tool_detail_mode.label(),
            self.status.workspace
        );
        frame.render_widget(Clear, chunks[3]);
        frame.render_widget(Paragraph::new(footer).style(dim_style()), chunks[3]);

        if self.palette.open {
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
                Span::styled("Ctrl+R", selected_style()),
                Span::raw(" toggle rendered/raw Markdown transcript"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+T", selected_style()),
                Span::raw(" toggle compact/full tool output"),
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
                Span::styled("/", selected_style()),
                Span::raw(" open command palette when composer is empty"),
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
                Span::styled("Ctrl+U / Ctrl+D", selected_style()),
                Span::raw(" scroll transcript by half page"),
            ]),
            Line::from(vec![
                Span::styled("Alt+↑ / Alt+↓", selected_style()),
                Span::raw(" jump previous / next message"),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+Home / Ctrl+End", selected_style()),
                Span::raw(" jump first / last message"),
            ]),
            Line::from(vec![
                Span::styled("Alt+U", selected_style()),
                Span::raw(" jump to last user message"),
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

    fn composer_lines(&self) -> Vec<Line<'static>> {
        if self.input.is_empty() {
            return vec![Line::from(Span::styled(
                "Type a prompt and press Enter…",
                dim_style(),
            ))];
        }
        self.visible_composer_input_lines()
            .into_iter()
            .map(Line::from)
            .collect()
    }

    fn visible_composer_input_lines(&self) -> Vec<String> {
        let lines = self
            .input
            .split('\n')
            .map(str::to_string)
            .collect::<Vec<_>>();
        if lines.len() <= AGENT_CHAT_COMPOSER_INPUT_MAX_LINES {
            return lines;
        }
        let keep = AGENT_CHAT_COMPOSER_INPUT_MAX_LINES.saturating_sub(1).max(1);
        let hidden = lines.len().saturating_sub(keep);
        let mut visible = vec![format!("… {hidden} earlier lines")];
        visible.extend(lines[lines.len().saturating_sub(keep)..].iter().cloned());
        visible
    }
}

fn agent_chat_help_command_entry() -> AgentChatCommandEntry {
    AgentChatCommandEntry {
        section: "Help".to_string(),
        label: "Show keybindings".to_string(),
        description: "View agent chat shortcuts and navigation keys.".to_string(),
        command: AgentChatCommand::OpenHelp,
    }
}

fn agent_chat_local_command_entries() -> Vec<AgentChatCommandEntry> {
    vec![
        agent_chat_command_entry(
            "Navigation",
            "Scroll half page up",
            "Move the transcript up by half a visible page.",
            AgentChatCommand::ScrollHalfPageUp,
        ),
        agent_chat_command_entry(
            "Navigation",
            "Scroll half page down",
            "Move the transcript down by half a visible page.",
            AgentChatCommand::ScrollHalfPageDown,
        ),
        agent_chat_command_entry(
            "Navigation",
            "Jump to first message",
            "Move to the start of the transcript.",
            AgentChatCommand::JumpFirstMessage,
        ),
        agent_chat_command_entry(
            "Navigation",
            "Jump to previous message",
            "Move to the previous message boundary.",
            AgentChatCommand::JumpPreviousMessage,
        ),
        agent_chat_command_entry(
            "Navigation",
            "Jump to next message",
            "Move to the next message boundary.",
            AgentChatCommand::JumpNextMessage,
        ),
        agent_chat_command_entry(
            "Navigation",
            "Jump to last message",
            "Move to the final message boundary.",
            AgentChatCommand::JumpLastMessage,
        ),
        agent_chat_command_entry(
            "Navigation",
            "Jump to last user message",
            "Move to the most recent user turn.",
            AgentChatCommand::JumpLastUserMessage,
        ),
    ]
}

fn agent_chat_command_entry(
    section: &str,
    label: &str,
    description: &str,
    command: AgentChatCommand,
) -> AgentChatCommandEntry {
    AgentChatCommandEntry {
        section: section.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        command,
    }
}

fn non_empty_context_value(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
fn agent_chat_transcript_lines(messages: &[AgentChatMessage], notice: &str) -> Vec<Line<'static>> {
    agent_chat_transcript_lines_with_mode(
        messages,
        notice,
        AgentChatRenderMode::Markdown,
        AgentToolDetailMode::Collapsed,
        None,
        None,
    )
}

fn agent_chat_transcript_lines_with_mode(
    messages: &[AgentChatMessage],
    notice: &str,
    render_mode: AgentChatRenderMode,
    tool_detail_mode: AgentToolDetailMode,
    code_block_width: Option<usize>,
    context: Option<AgentChatMessageRenderContext<'_>>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if messages.is_empty() {
        lines.push(Line::from(Span::styled(
            "Start a new agent conversation below.",
            dim_style(),
        )));
        lines.push(Line::from(Span::styled(
            "This is the runtime chat surface; Sessions is the unified history and resume picker.",
            dim_style(),
        )));
    } else {
        for message in messages {
            lines.extend(agent_chat_message_lines_with_mode(
                message,
                render_mode,
                tool_detail_mode,
                code_block_width,
                context,
            ));
            lines.push(Line::from(""));
        }
    }
    if !notice.trim().is_empty() && !notice_duplicates_last_message(messages, notice) {
        lines.push(Line::from(Span::styled(notice.to_string(), dim_style())));
    }
    lines
}

fn notice_duplicates_last_message(messages: &[AgentChatMessage], notice: &str) -> bool {
    let Some(last) = messages.last() else {
        return false;
    };
    last.role == AgentChatRole::Thought
        && normalize_notice_text(&last.content) == normalize_notice_text(notice)
}

fn normalize_notice_text(value: &str) -> String {
    value.trim().trim_end_matches('.').to_string()
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

fn agent_chat_dashboard_target(code: KeyCode) -> Option<DashboardTab> {
    match code {
        KeyCode::Tab => Some(DashboardTab::Tools),
        KeyCode::BackTab => Some(DashboardTab::Skills),
        _ => None,
    }
}

fn edit_agent_chat_input(terminal: &mut TuiTerminal, app: &mut AgentChatComposerApp) -> Result<()> {
    let edited = edit_text_in_external_editor(terminal, &app.input)?;
    app.input = normalize_editor_text(&edited);
    app.status.notice = "Composer updated from editor.".to_string();
    Ok(())
}

#[cfg(test)]
fn agent_chat_message_lines(message: &AgentChatMessage) -> Vec<Line<'static>> {
    agent_chat_message_lines_with_mode(
        message,
        AgentChatRenderMode::Markdown,
        AgentToolDetailMode::Collapsed,
        None,
        None,
    )
}

fn agent_chat_message_lines_with_mode(
    message: &AgentChatMessage,
    render_mode: AgentChatRenderMode,
    tool_detail_mode: AgentToolDetailMode,
    code_block_width: Option<usize>,
    context: Option<AgentChatMessageRenderContext<'_>>,
) -> Vec<Line<'static>> {
    if matches!(
        message.role,
        AgentChatRole::Tool | AgentChatRole::ToolOutput
    ) {
        return agent_tool_message_lines(message.role, message.content.trim(), tool_detail_mode);
    }

    let content = message.content.trim();
    let (label, label_style, content_style) = match message.role {
        AgentChatRole::User => (
            "You",
            Style::default().fg(CTP_GREEN).bg(CTP_SURFACE0),
            Style::default().fg(CTP_TEXT).bg(CTP_SURFACE0),
        ),
        AgentChatRole::Assistant => (
            "Djinn",
            title_style().bg(CTP_BASE),
            Style::default().fg(CTP_TEXT).bg(CTP_BASE),
        ),
        AgentChatRole::Thought => (
            "Thought",
            Style::default().fg(CTP_MAUVE).bg(CTP_SURFACE0),
            Style::default().fg(CTP_SUBTEXT0).bg(CTP_SURFACE0),
        ),
        AgentChatRole::Tool => unreachable!("tool messages return before generic rendering"),
        AgentChatRole::ToolOutput => {
            unreachable!("tool output messages return before generic rendering")
        }
        AgentChatRole::Notice if notice_is_error(content) => (
            "Error",
            Style::default()
                .fg(CTP_RED)
                .bg(CTP_SURFACE0)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(CTP_RED).bg(CTP_SURFACE0),
        ),
        AgentChatRole::Notice => ("Notice", dim_style(), dim_style()),
    };
    let label = agent_chat_message_label(message.role, label, content);
    let mut lines = vec![Line::from(vec![
        Span::styled(" ", label_style),
        Span::styled(label, label_style.add_modifier(Modifier::BOLD)),
        Span::styled(" ", label_style),
    ])];
    if let Some(metadata) = agent_chat_assistant_metadata_line(message.role, context) {
        lines.push(metadata);
    }
    if content.is_empty() {
        lines.push(Line::from(Span::styled(" (empty) ", content_style)));
    } else {
        lines.extend(agent_chat_message_body_lines(
            message.role,
            content,
            content_style,
            render_mode,
            code_block_width,
        ));
    }
    lines
}

fn agent_chat_assistant_metadata_line(
    role: AgentChatRole,
    context: Option<AgentChatMessageRenderContext<'_>>,
) -> Option<Line<'static>> {
    if role != AgentChatRole::Assistant {
        return None;
    }
    let context = context?;
    let mut parts = Vec::new();
    if let Some(profile) = context.profile {
        parts.push(format!("profile {profile}"));
    }
    if let Some(model) = context.model {
        parts.push(format!("model {model}"));
    }
    if parts.is_empty() {
        return None;
    }
    Some(Line::from(Span::styled(
        format!("  {} ", parts.join(" · ")),
        dim_style(),
    )))
}

fn notice_is_error(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("failed") || lower.contains("error")
}

fn agent_tool_message_lines(
    role: AgentChatRole,
    content: &str,
    detail_mode: AgentToolDetailMode,
) -> Vec<Line<'static>> {
    if content.is_empty() {
        return vec![Line::from(Span::styled(" ⚙ Tool · empty ", dim_style()))];
    }
    match role {
        AgentChatRole::Tool => agent_tool_request_lines(content),
        AgentChatRole::ToolOutput => agent_tool_output_lines(content, detail_mode),
        AgentChatRole::User
        | AgentChatRole::Assistant
        | AgentChatRole::Thought
        | AgentChatRole::Notice => unreachable!("only tool roles use tool rendering"),
    }
}

fn agent_tool_request_lines(content: &str) -> Vec<Line<'static>> {
    if let Some((workdir, command)) = parse_shell_request(content) {
        return shell_tool_request_lines(workdir, command);
    }
    let Some((name, detail)) = parse_tool_request(content) else {
        return vec![inline_tool_line("⚙", "Tool", content, dim_style())];
    };
    vec![inline_tool_line(
        tool_icon(name),
        tool_display_name(name),
        detail,
        tool_request_style(),
    )]
}

fn agent_tool_output_lines(content: &str, detail_mode: AgentToolDetailMode) -> Vec<Line<'static>> {
    let mut lines = content.lines();
    let Some(first) = lines.next() else {
        return vec![inline_tool_line("⚙", "Tool", "empty result", dim_style())];
    };
    let body = lines.collect::<Vec<_>>();
    let Some((name, status)) = parse_tool_execution_status(first.trim()) else {
        return vec![inline_tool_line("⚙", "Tool", content, dim_style())];
    };
    if name == "shell" {
        return shell_tool_output_lines(status, &body, detail_mode);
    }
    if matches!(name, "apply_patch" | "write_file" | "edit_file") {
        return block_tool_output_lines(name, status, &body, AgentToolDetailMode::Full, None);
    }
    if !matches!(
        name,
        "read_file" | "list_dir" | "find_files" | "search_files" | "webfetch" | "websearch"
    ) && body.len() > GENERIC_TOOL_OUTPUT_COLLAPSED_LINES
    {
        return block_tool_output_lines(
            name,
            status,
            &body,
            detail_mode,
            Some(GENERIC_TOOL_OUTPUT_COLLAPSED_LINES),
        );
    }
    let mut rendered = vec![inline_tool_line(
        tool_status_icon(status),
        tool_display_name(name),
        &inline_tool_output_summary(name, status, &body),
        tool_status_style(status),
    )];
    rendered.extend(inline_tool_detail_lines(name, &body));
    rendered
}

fn inline_tool_line(icon: &str, label: &str, detail: &str, style: Style) -> Line<'static> {
    let detail = detail.trim();
    let text = if detail.is_empty() {
        format!(" {icon} {label} ")
    } else {
        format!(" {icon} {label} {detail} ")
    };
    Line::from(Span::styled(text, style))
}

fn inline_tool_detail_lines(name: &str, body: &[&str]) -> Vec<Line<'static>> {
    if !matches!(name, "list_dir" | "find_files" | "search_files") {
        return Vec::new();
    }
    body.iter()
        .filter_map(|line| line.trim().strip_prefix("- "))
        .take(3)
        .map(|detail| Line::from(Span::styled(format!("   ↳ {detail} "), dim_style())))
        .collect()
}

fn inline_tool_output_summary(name: &str, status: &str, body: &[&str]) -> String {
    let status_label = if status == "ok" { "" } else { "failed · " };
    match name {
        "read_file" => {
            let path = body_value(body, "path").unwrap_or("file");
            let size = body
                .iter()
                .map(|line| line.trim())
                .find(|line| line.contains("bytes") && line.contains("lines"))
                .unwrap_or_default();
            if size.is_empty() {
                format!("{status_label}{path}")
            } else {
                format!("{status_label}{path} · {size}")
            }
        }
        "list_dir" | "find_files" | "search_files" => {
            let path = body_value(body, "path").unwrap_or(".");
            let matches = body
                .iter()
                .map(|line| line.trim())
                .find(|line| line.ends_with("matches") || line.ends_with("match"))
                .unwrap_or("matches");
            format!("{status_label}{matches} in {path}")
        }
        _ => {
            let first = body
                .iter()
                .map(|line| line.trim())
                .find(|line| !line.is_empty());
            match first {
                Some(first) => format!("{status_label}{first}"),
                None => status_label.trim_end_matches(" · ").to_string(),
            }
        }
    }
}

fn body_value<'a>(body: &'a [&str], key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    body.iter()
        .map(|line| line.trim())
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
        .filter(|value| !value.is_empty())
}

const SHELL_OUTPUT_COLLAPSED_LINES: usize = 8;
const GENERIC_TOOL_OUTPUT_COLLAPSED_LINES: usize = 3;

fn shell_tool_request_lines(workdir: &str, command: &str) -> Vec<Line<'static>> {
    let mut lines = vec![block_tool_header_line(
        "$",
        "Shell",
        workdir,
        tool_request_style(),
    )];
    lines.push(Line::from(Span::styled(
        format!(" $ {command} "),
        shell_tool_block_style(),
    )));
    lines
}

fn shell_tool_output_lines(
    status: &str,
    body: &[&str],
    detail_mode: AgentToolDetailMode,
) -> Vec<Line<'static>> {
    let mut lines = vec![block_tool_header_line(
        tool_status_icon(status),
        "Shell",
        status,
        tool_status_style(status),
    )];
    let parsed = parse_shell_output_body(body);
    if let Some(command) = parsed.command {
        lines.push(Line::from(Span::styled(
            format!(" $ {command} "),
            shell_tool_block_style(),
        )));
    }
    lines.extend(parsed.meta.into_iter().map(|line| {
        Line::from(Span::styled(
            format!(" {line} "),
            dim_style().bg(CTP_SURFACE0),
        ))
    }));
    push_tool_output_section(
        &mut lines,
        "stdout",
        &parsed.stdout,
        detail_mode,
        SHELL_OUTPUT_COLLAPSED_LINES,
        shell_tool_block_style(),
    );
    push_tool_output_section(
        &mut lines,
        "stderr",
        &parsed.stderr,
        detail_mode,
        SHELL_OUTPUT_COLLAPSED_LINES,
        shell_tool_block_style(),
    );
    lines
}

#[derive(Debug, Default)]
struct ShellOutputBody<'a> {
    command: Option<String>,
    meta: Vec<&'a str>,
    stdout: Vec<&'a str>,
    stderr: Vec<&'a str>,
}

fn parse_shell_output_body<'a>(body: &'a [&str]) -> ShellOutputBody<'a> {
    let mut parsed = ShellOutputBody::default();
    let mut section: Option<&str> = None;
    for raw in body {
        let line = raw.trim_end();
        if line == "stdout:" {
            section = Some("stdout");
            continue;
        }
        if line == "stderr:" {
            section = Some("stderr");
            continue;
        }
        if let Some(command) = line.strip_prefix("command: ") {
            parsed.command = Some(command.trim().trim_matches('`').to_string());
            continue;
        }
        match section {
            Some("stdout") => parsed.stdout.push(line),
            Some("stderr") => parsed.stderr.push(line),
            _ => parsed.meta.push(line),
        }
    }
    parsed
}

fn push_tool_output_section(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    values: &[&str],
    detail_mode: AgentToolDetailMode,
    budget: usize,
    style: Style,
) {
    if values.is_empty() {
        return;
    }
    lines.push(Line::from(Span::styled(
        format!(" {label} "),
        dim_style().bg(CTP_SURFACE1),
    )));
    let limit = if detail_mode.is_full() {
        values.len()
    } else {
        budget.min(values.len())
    };
    lines.extend(
        values
            .iter()
            .take(limit)
            .map(|line| Line::from(Span::styled(format!(" {line} "), style))),
    );
    if limit < values.len() {
        lines.push(Line::from(Span::styled(
            format!(
                " … {} more lines (Ctrl+T full tool output) ",
                values.len() - limit
            ),
            dim_style().bg(CTP_SURFACE1),
        )));
    }
}

fn block_tool_output_lines(
    name: &str,
    status: &str,
    body: &[&str],
    detail_mode: AgentToolDetailMode,
    collapsed_budget: Option<usize>,
) -> Vec<Line<'static>> {
    let mut lines = vec![block_tool_header_line(
        tool_status_icon(status),
        tool_display_name(name),
        status,
        tool_status_style(status),
    )];
    let body = body
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let limit = collapsed_budget
        .filter(|_| !detail_mode.is_full())
        .map(|budget| budget.min(body.len()))
        .unwrap_or(body.len());
    lines.extend(body.iter().take(limit).map(|line| {
        Line::from(Span::styled(
            format!(" {line} "),
            Style::default().fg(CTP_TEXT).bg(CTP_SURFACE0),
        ))
    }));
    if limit < body.len() {
        lines.push(Line::from(Span::styled(
            format!(
                " … {} more lines (Ctrl+T full tool output) ",
                body.len() - limit
            ),
            dim_style().bg(CTP_SURFACE1),
        )));
    }
    lines
}

fn block_tool_header_line(icon: &str, label: &str, detail: &str, style: Style) -> Line<'static> {
    let detail = detail.trim();
    let text = if detail.is_empty() || detail == "." {
        format!(" {icon} {label} ")
    } else {
        format!(" {icon} {label} · {detail} ")
    };
    Line::from(Span::styled(text, style.add_modifier(Modifier::BOLD)))
}

fn parse_shell_request(content: &str) -> Option<(&str, &str)> {
    let mut lines = content.lines();
    let first = lines.next()?.trim();
    let second = lines.next()?.trim();
    let workdir = first.strip_prefix("# Running in ")?.trim();
    let command = second.strip_prefix("$ ")?.trim();
    Some((workdir, command))
}

fn parse_tool_request(content: &str) -> Option<(&str, &str)> {
    let first = content.lines().next()?.trim();
    let (name, detail) = first.split_once(':')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name, detail.trim()))
}

fn tool_icon(name: &str) -> &'static str {
    match name {
        "shell" => "$",
        "read_file" | "list_dir" => "→",
        "find_files" | "search_files" => "✱",
        "webfetch" => "%",
        "websearch" => "◈",
        "apply_patch" | "write_file" | "edit_file" => "←",
        _ => "⚙",
    }
}

fn tool_status_icon(status: &str) -> &'static str {
    match status {
        "ok" => "✓",
        "failed" => "✗",
        _ => "•",
    }
}

fn tool_display_name(name: &str) -> &'static str {
    match name {
        "shell" => "Shell",
        "read_file" => "Read",
        "list_dir" => "List",
        "find_files" => "Find",
        "search_files" => "Search",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        "apply_patch" => "Patch",
        "write_file" => "Write",
        "edit_file" => "Edit",
        _ => "Tool",
    }
}

fn tool_request_style() -> Style {
    Style::default().fg(CTP_YELLOW).bg(CTP_BASE)
}

fn tool_status_style(status: &str) -> Style {
    match status {
        "ok" => Style::default().fg(CTP_GREEN).bg(CTP_BASE),
        "failed" => Style::default().fg(CTP_RED).bg(CTP_BASE),
        _ => Style::default().fg(CTP_SKY).bg(CTP_BASE),
    }
}

fn shell_tool_block_style() -> Style {
    Style::default().fg(CTP_TEXT).bg(CTP_SURFACE1)
}

fn agent_chat_message_label(role: AgentChatRole, default_label: &str, _content: &str) -> String {
    match role {
        AgentChatRole::User
        | AgentChatRole::Assistant
        | AgentChatRole::Thought
        | AgentChatRole::Tool
        | AgentChatRole::ToolOutput
        | AgentChatRole::Notice => default_label.to_string(),
    }
}

fn agent_chat_message_body_lines(
    role: AgentChatRole,
    content: &str,
    content_style: Style,
    render_mode: AgentChatRenderMode,
    code_block_width: Option<usize>,
) -> Vec<Line<'static>> {
    let prefix = agent_chat_body_prefix(role);
    match role {
        AgentChatRole::Assistant if render_mode == AgentChatRenderMode::Markdown => {
            render_agent_markdown_body_lines(content, content_style, code_block_width, prefix)
        }
        AgentChatRole::User
        | AgentChatRole::Assistant
        | AgentChatRole::Thought
        | AgentChatRole::Tool
        | AgentChatRole::ToolOutput
        | AgentChatRole::Notice => plain_agent_chat_body_lines(
            content.lines().map(ToOwned::to_owned).collect(),
            content_style,
            prefix,
        ),
    }
}

fn agent_chat_body_prefix(role: AgentChatRole) -> &'static str {
    match role {
        AgentChatRole::Assistant => "  ",
        AgentChatRole::User
        | AgentChatRole::Thought
        | AgentChatRole::Tool
        | AgentChatRole::ToolOutput
        | AgentChatRole::Notice => " ",
    }
}

fn plain_agent_chat_body_lines(
    lines: Vec<String>,
    style: Style,
    prefix: &'static str,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| Line::from(Span::styled(format!("{prefix}{line} "), style)))
        .collect()
}

fn render_agent_markdown_body_lines(
    content: &str,
    base_style: Style,
    code_block_width: Option<usize>,
    prefix: &'static str,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut code_fence = "```".to_string();

    for raw_line in content.lines() {
        let trimmed_start = raw_line.trim_start();
        if !in_code_block && (trimmed_start.starts_with("```") || trimmed_start.starts_with("~~~"))
        {
            in_code_block = true;
            code_fence = trimmed_start[..3].to_string();
            continue;
        }
        if in_code_block {
            if raw_line.trim_start().starts_with(&code_fence) {
                in_code_block = false;
            } else {
                lines.push(Line::from(Span::styled(
                    padded_code_block_line(raw_line, code_block_width, prefix),
                    markdown_code_style(),
                )));
            }
            continue;
        }

        lines.push(render_agent_markdown_line(raw_line, base_style, prefix));
    }

    lines
}

fn padded_code_block_line(raw_line: &str, code_block_width: Option<usize>, prefix: &str) -> String {
    let mut line = format!("{prefix}{raw_line} ");
    let Some(width) = code_block_width else {
        return line;
    };
    let line_width = line.chars().count();
    if line_width < width {
        line.push_str(&" ".repeat(width - line_width));
    }
    line
}

fn render_agent_markdown_line(
    raw_line: &str,
    base_style: Style,
    prefix: &'static str,
) -> Line<'static> {
    let trimmed = raw_line.trim_start();
    if trimmed.is_empty() {
        return Line::from(Span::styled(prefix.to_string(), base_style));
    }
    if let Some((level, heading)) = markdown_heading(trimmed) {
        let marker = match level {
            1 => "▌ ",
            2 => "▸ ",
            _ => "• ",
        };
        return Line::from(Span::styled(
            format!("{prefix}{marker}{heading} "),
            title_style().bg(CTP_BASE),
        ));
    }
    if markdown_horizontal_rule(trimmed) {
        return Line::from(Span::styled(
            format!("{prefix}───────────────────────── "),
            dim_style(),
        ));
    }
    if let Some(item) = markdown_bullet_item(trimmed) {
        return markdown_prefixed_inline_line(&format!("{prefix}• "), item, base_style);
    }
    if let Some((number, item)) = markdown_numbered_item(trimmed) {
        return markdown_prefixed_inline_line(&format!("{prefix}{number}. "), item, base_style);
    }
    if let Some(quote) = trimmed.strip_prefix('>') {
        return markdown_prefixed_inline_line(
            &format!("{prefix}│ "),
            quote.trim_start(),
            dim_style(),
        );
    }
    markdown_prefixed_inline_line(prefix, trimmed, base_style)
}

fn markdown_prefixed_inline_line(prefix: &str, content: &str, style: Style) -> Line<'static> {
    let mut spans = vec![Span::styled(prefix.to_string(), style)];
    spans.extend(render_inline_markdown(content, style));
    spans.push(Span::styled(" ", style));
    Line::from(spans)
}

fn render_inline_markdown(content: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut index = 0;
    while index < content.len() {
        let rest = &content[index..];
        if let Some(stripped) = rest.strip_prefix("**") {
            if let Some(end) = stripped.find("**") {
                let value = &stripped[..end];
                if !value.is_empty() {
                    spans.push(Span::styled(
                        value.to_string(),
                        base_style.add_modifier(Modifier::BOLD),
                    ));
                    index += 4 + end;
                    continue;
                }
            }
        }
        if let Some(stripped) = rest.strip_prefix('`') {
            if let Some(end) = stripped.find('`') {
                let value = &stripped[..end];
                if !value.is_empty() {
                    spans.push(Span::styled(
                        format!(" {value} "),
                        markdown_inline_code_style(),
                    ));
                    index += 2 + end;
                    continue;
                }
            }
        }
        if rest.starts_with('[') {
            if let Some((text, url, consumed)) = markdown_link(rest) {
                spans.push(Span::styled(
                    text,
                    base_style.add_modifier(Modifier::UNDERLINED),
                ));
                spans.push(Span::styled(format!(" ({url})"), dim_style()));
                index += consumed;
                continue;
            }
        }
        if let Some(stripped) = rest.strip_prefix('*') {
            if let Some(end) = stripped.find('*') {
                let value = &stripped[..end];
                if !value.is_empty() {
                    spans.push(Span::styled(
                        value.to_string(),
                        base_style.add_modifier(Modifier::ITALIC),
                    ));
                    index += 2 + end;
                    continue;
                }
            }
        }

        let next = rest
            .char_indices()
            .skip(1)
            .find_map(|(offset, ch)| matches!(ch, '*' | '`' | '[').then_some(offset))
            .unwrap_or(rest.len());
        spans.push(Span::styled(rest[..next].to_string(), base_style));
        index += next;
    }
    spans
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&level) && line.chars().nth(level).is_some_and(char::is_whitespace) {
        let heading = line[level..].trim();
        if !heading.is_empty() {
            return Some((level, heading));
        }
    }
    None
}

fn markdown_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 3 && trimmed.chars().all(|ch| matches!(ch, '-' | '*' | '_'))
}

fn markdown_bullet_item(line: &str) -> Option<&str> {
    let mut chars = line.chars();
    let marker = chars.next()?;
    if matches!(marker, '-' | '*' | '+') && chars.next().is_some_and(char::is_whitespace) {
        return Some(line[2..].trim_start());
    }
    None
}

fn markdown_numbered_item(line: &str) -> Option<(&str, &str)> {
    let dot = line.find('.')?;
    let number = &line[..dot];
    let rest = &line[dot + 1..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    if !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()) {
        return Some((number, rest.trim_start()));
    }
    None
}

fn markdown_link(line: &str) -> Option<(String, String, usize)> {
    let text_end = line.find("](")?;
    let url_start = text_end + 2;
    let url_end = line[url_start..].find(')')? + url_start;
    let text = &line[1..text_end];
    let url = &line[url_start..url_end];
    if text.is_empty() || url.is_empty() {
        return None;
    }
    Some((text.to_string(), url.to_string(), url_end + 1))
}

fn markdown_code_style() -> Style {
    Style::default().fg(CTP_TEXT).bg(CTP_SURFACE1)
}

fn markdown_inline_code_style() -> Style {
    Style::default().fg(CTP_YELLOW).bg(CTP_SURFACE1)
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

fn run_dashboard_loop(
    terminal: &mut TuiTerminal,
    tools: Vec<ToolEntry>,
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
                        _ if agent_chat_help_key(key.code, key.modifiers) => app.close_help(),
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
                        _ if agent_chat_palette_next_key(key.code, key.modifiers) => {
                            app.next_palette_item()
                        }
                        _ if agent_chat_palette_previous_key(key.code, key.modifiers) => {
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

                if agent_chat_palette_key(key.code, key.modifiers) {
                    app.open_palette();
                    continue;
                }

                if agent_chat_help_key(key.code, key.modifiers) {
                    app.open_help();
                    continue;
                }

                if app.active_tab == DashboardTab::Sessions {
                    match &app.chats.mode {
                        ChatUiMode::Options => {
                            match key.code {
                                KeyCode::Char('q') => return Ok(None),
                                KeyCode::Esc | KeyCode::Backspace => {
                                    app.chats.mode = ChatUiMode::Selecting
                                }
                                KeyCode::Char('j') | KeyCode::Down => app.chats.next_option(),
                                KeyCode::Char('k') | KeyCode::Up => app.chats.previous_option(),
                                KeyCode::Enter => {
                                    return Ok(app
                                        .chats
                                        .promote_request()
                                        .map(TuiAction::PromoteSessions));
                                }
                                _ => {}
                            }
                            continue;
                        }
                        ChatUiMode::ConfirmDelete(_) => {
                            match key.code {
                                KeyCode::Enter | KeyCode::Char('y') => {
                                    if let Some(action) = app.chats.confirm_delete_action() {
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
                                KeyCode::Esc
                                | KeyCode::Backspace
                                | KeyCode::Char('n')
                                | KeyCode::Char('q') => app.chats.cancel_modal(),
                                _ => {}
                            }
                            continue;
                        }
                        ChatUiMode::Selecting => {}
                    }
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
                        DashboardTab::Memories | DashboardTab::Suggestions => {}
                    },
                    KeyCode::Char('r') => {
                        if app.active_tab == DashboardTab::Sessions {
                            if let Some(request) = app.chats.selected_chat_session_request() {
                                return Ok(Some(TuiAction::OpenChatSession(request)));
                            }
                        } else if app.active_tab == DashboardTab::Memories {
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
                    KeyCode::Char('f') => {
                        if app.active_tab == DashboardTab::Sessions {
                            app.chats.cycle_scope();
                        }
                    }
                    KeyCode::Char('s') => {
                        if app.active_tab == DashboardTab::Sessions {
                            app.chats.open_options();
                        }
                    }
                    KeyCode::Char('x') | KeyCode::Delete => match app.active_tab {
                        DashboardTab::Sessions => {
                            app.chats.open_delete_confirmation();
                        }
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

fn handle_dashboard_command(
    app: &mut DashboardApp,
    on_continue_action: &mut Option<&mut dyn FnMut(TuiAction) -> Result<()>>,
    command: DashboardCommand,
) -> Result<Option<TuiAction>> {
    match command {
        DashboardCommand::OpenAgent => return Ok(Some(TuiAction::OpenAgentChat)),
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
            DashboardTab::Memories | DashboardTab::Suggestions => {}
        },
        DashboardCommand::ResumeSelectedChat => {
            if let Some(request) = app.chats.selected_chat_session_request() {
                return Ok(Some(TuiAction::OpenChatSession(request)));
            }
        }
        DashboardCommand::PromoteSessions => app.chats.open_options(),
        DashboardCommand::SetSessionScope(scope) => app.chats.set_scope(scope),
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
            if app.active_tab == DashboardTab::Sessions {
                app.chats.open_delete_confirmation();
            } else if let Some(action) = app.delete_selected_action() {
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
    chats: ChatsApp,
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
        chats: Vec<ChatRecord>,
        memories: Vec<MemoryRecord>,
        suggestions: Vec<SuggestionRecord>,
        skills: Vec<SkillRecord>,
        active_context: Option<ContextRecord>,
        initial_tab: DashboardTab,
    ) -> Self {
        Self {
            active_tab: initial_tab,
            tools: ToolsApp::new(tools),
            chats: ChatsApp::new(chats),
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
        if self.active_tab == DashboardTab::Sessions {
            self.chats.mode = ChatUiMode::Selecting;
        }
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
                label: "Open Agent".to_string(),
                description: "Return to the active Agent chat".to_string(),
                command: DashboardCommand::OpenAgent,
            },
            DashboardCommandEntry {
                section: "Navigation".to_string(),
                label: "Open Tools".to_string(),
                description: "Jump to Tools".to_string(),
                command: DashboardCommand::OpenTab(DashboardTab::Tools),
            },
            DashboardCommandEntry {
                section: "Navigation".to_string(),
                label: "Open Sessions".to_string(),
                description: "Jump to the session picker".to_string(),
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
            DashboardTab::Sessions => {
                let mut entries = vec![
                    dashboard_command_entry(
                        "Sessions",
                        "Resume selected session",
                        "Resume Djinn session or convert+resume OpenCode session",
                        DashboardCommand::ResumeSelectedChat,
                    ),
                    dashboard_command_entry(
                        "Sessions",
                        "Promote selected sessions",
                        "Open promotion options for selected session rows",
                        DashboardCommand::PromoteSessions,
                    ),
                    dashboard_command_entry(
                        "Sessions",
                        "Toggle selected session",
                        "Select or unselect the highlighted session",
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
                        "Remove selected sessions",
                        "Remove selected persisted session rows or Djinn sessions",
                        DashboardCommand::DeleteSelected,
                    ),
                    dashboard_command_entry(
                        "Sessions",
                        "Filter sessions",
                        "Edit the session picker text filter",
                        DashboardCommand::ToggleFilter,
                    ),
                ];
                for scope in SessionFilterScope::ALL {
                    entries.push(dashboard_command_entry(
                        "Session filters",
                        &format!("Show {} sessions", scope.label()),
                        scope.description(),
                        DashboardCommand::SetSessionScope(scope),
                    ));
                }
                entries
            }
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
            DashboardTab::Sessions => self.chats.next(),
            DashboardTab::Memories => self.memories.next(),
            DashboardTab::Suggestions => self.suggestions.next(),
            DashboardTab::Skills => self.skills.next(),
        }
    }

    fn previous_item(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.previous(),
            DashboardTab::Sessions => self.chats.previous(),
            DashboardTab::Memories => self.memories.previous(),
            DashboardTab::Suggestions => self.suggestions.previous(),
            DashboardTab::Skills => self.skills.previous(),
        }
    }

    fn scroll_down(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.scroll_down(),
            DashboardTab::Sessions => self.chats.scroll_down(),
            DashboardTab::Memories => self.memories.scroll_down(),
            DashboardTab::Suggestions => self.suggestions.scroll_down(),
            DashboardTab::Skills => self.skills.scroll_down(),
        }
    }

    fn scroll_up(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.scroll_up(),
            DashboardTab::Sessions => self.chats.scroll_up(),
            DashboardTab::Memories => self.memories.scroll_up(),
            DashboardTab::Suggestions => self.suggestions.scroll_up(),
            DashboardTab::Skills => self.skills.scroll_up(),
        }
    }

    fn filter_editing(&self) -> bool {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter.editing,
            DashboardTab::Sessions => self.chats.filter.editing,
            DashboardTab::Memories => self.memories.filter.editing,
            DashboardTab::Suggestions => self.suggestions.filter.editing,
            DashboardTab::Skills => self.skills.filter.editing,
        }
    }

    fn toggle_filter(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.toggle_filter(),
            DashboardTab::Sessions => self.chats.toggle_filter(),
            DashboardTab::Memories => self.memories.toggle_filter(),
            DashboardTab::Suggestions => self.suggestions.toggle_filter(),
            DashboardTab::Skills => self.skills.toggle_filter(),
        }
    }

    fn filter_push(&mut self, ch: char) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter_push(ch),
            DashboardTab::Sessions => self.chats.filter_push(ch),
            DashboardTab::Memories => self.memories.filter_push(ch),
            DashboardTab::Suggestions => self.suggestions.filter_push(ch),
            DashboardTab::Skills => self.skills.filter_push(ch),
        }
    }

    fn filter_backspace(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter_backspace(),
            DashboardTab::Sessions => self.chats.filter_backspace(),
            DashboardTab::Memories => self.memories.filter_backspace(),
            DashboardTab::Suggestions => self.suggestions.filter_backspace(),
            DashboardTab::Skills => self.skills.filter_backspace(),
        }
    }

    fn finish_filter_edit(&mut self) {
        match self.active_tab {
            DashboardTab::Tools => self.tools.filter.editing = false,
            DashboardTab::Sessions => self.chats.filter.editing = false,
            DashboardTab::Memories => self.memories.filter.editing = false,
            DashboardTab::Suggestions => self.suggestions.filter.editing = false,
            DashboardTab::Skills => self.skills.filter.editing = false,
        }
    }

    fn toggle_selected(&mut self) {
        match self.active_tab {
            DashboardTab::Sessions => self.chats.toggle_selected(),
            DashboardTab::Memories => self.memories.toggle_selected(),
            DashboardTab::Suggestions => self.suggestions.toggle_selected(),
            DashboardTab::Tools | DashboardTab::Skills => {}
        }
    }

    fn toggle_all(&mut self) {
        match self.active_tab {
            DashboardTab::Sessions => self.chats.toggle_all(),
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
            DashboardTab::Sessions => self.chats.delete_request().map(TuiAction::DeleteChatRows),
            DashboardTab::Memories => self.reject_selected_action(),
            DashboardTab::Suggestions => {
                let ids = self.suggestions.selected_suggestion_ids();
                (!ids.is_empty()).then_some(TuiAction::DeleteSuggestions(ids))
            }
            DashboardTab::Tools | DashboardTab::Skills => None,
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
            TuiAction::DeleteChatRows(request) => self.chats.remove_deleted_rows(request),
            TuiAction::DeleteMemories(ids) => self.memories.remove_ids(ids),
            TuiAction::DeleteSuggestions(ids) => self.suggestions.remove_ids(ids),
            TuiAction::OpenTool(_)
            | TuiAction::OpenAgentChat
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
            DashboardTab::Sessions => self.chats.draw_body(frame, chunks[1]),
            DashboardTab::Memories => self.memories.draw_body(frame, chunks[1]),
            DashboardTab::Suggestions => self.suggestions.draw_body(frame, chunks[1]),
            DashboardTab::Skills => self.skills.draw_body(frame, chunks[1]),
        }

        frame.render_widget(Clear, chunks[2]);
        frame.render_widget(
            Paragraph::new("Ctrl+P commands • Ctrl+/ help • q quit").style(dim_style()),
            chunks[2],
        );

        if self.active_tab == DashboardTab::Sessions {
            match &self.chats.mode {
                ChatUiMode::Options => self.chats.draw_options(frame),
                ChatUiMode::ConfirmDelete(request) => {
                    self.chats.draw_delete_confirmation(frame, request)
                }
                ChatUiMode::Selecting => {}
            }
        }
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
                Span::styled("Enter / r", selected_style()),
                Span::raw(" resume Djinn session or convert+resume OpenCode session"),
            ]),
            Line::from(vec![
                Span::styled("s", selected_style()),
                Span::raw(" open promotion options"),
            ]),
            Line::from(vec![
                Span::styled("Space / A", selected_style()),
                Span::raw(" select one / all visible"),
            ]),
            Line::from(vec![
                Span::styled("f", selected_style()),
                Span::raw(" cycle session scope: all, promotable, djinn-agent, child-agent"),
            ]),
            Line::from(vec![
                Span::styled("x / Delete", selected_style()),
                Span::raw(" confirm removal of selected sessions"),
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

    fn set_scope(&mut self, scope: SessionFilterScope) {
        self.scope = scope;
        self.ensure_selection_visible();
    }

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

    fn open_delete_confirmation(&mut self) {
        if let Some(request) = self.delete_request() {
            self.mode = ChatUiMode::ConfirmDelete(request);
        }
    }

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
    use ratatui::widgets::Borders;

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

    fn rendered_agent_chat_message_lines_with_mode(
        message: AgentChatMessage,
        render_mode: AgentChatRenderMode,
    ) -> Vec<String> {
        rendered_agent_chat_message_lines_with_modes(
            message,
            render_mode,
            AgentToolDetailMode::Collapsed,
            None,
        )
    }

    fn rendered_agent_chat_message_lines_with_mode_and_width(
        message: AgentChatMessage,
        render_mode: AgentChatRenderMode,
        code_block_width: Option<usize>,
    ) -> Vec<String> {
        rendered_agent_chat_message_lines_with_modes(
            message,
            render_mode,
            AgentToolDetailMode::Collapsed,
            code_block_width,
        )
    }

    fn rendered_agent_chat_message_lines_with_modes(
        message: AgentChatMessage,
        render_mode: AgentChatRenderMode,
        tool_detail_mode: AgentToolDetailMode,
        code_block_width: Option<usize>,
    ) -> Vec<String> {
        agent_chat_message_lines_with_mode(
            &message,
            render_mode,
            tool_detail_mode,
            code_block_width,
            None,
        )
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
    fn agent_chat_transcript_does_not_repeat_notice_matching_last_thought() {
        let lines = agent_chat_transcript_lines(
            &[AgentChatMessage {
                role: AgentChatRole::Thought,
                content: "Planning next step…".to_string(),
            }],
            "Planning next step…",
        )
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("Planning next step"))
                .count(),
            1
        );
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

        assert_eq!(lines, vec![" Djinn ", "  Hello ", "  world "]);
    }

    #[test]
    fn agent_chat_transcript_adds_muted_assistant_metadata_when_available() {
        let lines = agent_chat_transcript_lines_with_mode(
            &[AgentChatMessage {
                role: AgentChatRole::Assistant,
                content: "Hello".to_string(),
            }],
            "",
            AgentChatRenderMode::Markdown,
            AgentToolDetailMode::Collapsed,
            None,
            Some(AgentChatMessageRenderContext {
                profile: Some("architect"),
                model: Some("openai/gpt-5.5"),
            }),
        )
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                " Djinn ",
                "  profile architect · model openai/gpt-5.5 ",
                "  Hello ",
                "",
            ]
        );
    }

    #[test]
    fn agent_chat_user_turn_uses_subtle_panel_background() {
        let lines = agent_chat_message_lines(&AgentChatMessage {
            role: AgentChatRole::User,
            content: "hello".to_string(),
        });

        assert_eq!(lines[1].spans[0].style.bg, Some(CTP_SURFACE0));
    }

    #[test]
    fn agent_chat_notice_errors_are_visually_distinct() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::Notice,
            content: "Agent turn failed: nope".to_string(),
        });

        assert_eq!(lines, vec![" Error ", " Agent turn failed: nope "]);
    }

    #[test]
    fn agent_chat_assistant_markdown_is_rendered_visually() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::Assistant,
            content: "# Plan\n\n- **Build** `thing`\n```rust\nfn main() {}\n```".to_string(),
        });

        assert_eq!(
            lines,
            vec![
                " Djinn ",
                "  ▌ Plan ",
                "  ",
                "  • Build  thing  ",
                "  fn main() {} ",
            ]
        );
    }

    #[test]
    fn agent_chat_code_blocks_pad_to_transcript_width() {
        let lines = rendered_agent_chat_message_lines_with_mode_and_width(
            AgentChatMessage {
                role: AgentChatRole::Assistant,
                content: "```text\nx\n\ny\n```".to_string(),
            },
            AgentChatRenderMode::Markdown,
            Some(10),
        );

        assert_eq!(
            lines,
            vec![" Djinn ", "  x       ", "          ", "  y       "]
        );
        assert!(lines.iter().all(|line| !line.contains('│')));
        assert!(lines.iter().all(|line| !line.contains('┌')));
        assert!(lines.iter().all(|line| !line.contains('└')));
        assert_eq!(lines[1].chars().count(), 10);
        assert_eq!(lines[2].chars().count(), 10);
        assert_eq!(lines[3].chars().count(), 10);
    }

    #[test]
    fn agent_chat_raw_markdown_mode_preserves_markers_for_copying() {
        let lines = rendered_agent_chat_message_lines_with_mode(
            AgentChatMessage {
                role: AgentChatRole::Assistant,
                content: "# Plan\n- **Build** `thing`".to_string(),
            },
            AgentChatRenderMode::Raw,
        );

        assert_eq!(
            lines,
            vec![" Djinn ", "  # Plan ", "  - **Build** `thing` "]
        );
    }

    #[test]
    fn agent_chat_tool_read_uses_compact_inline_rows() {
        let request_lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::Tool,
            content: "read_file: Cargo.toml".to_string(),
        });
        let execution_lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::ToolOutput,
            content: "read_file result: ok\npath: Cargo.toml\n123 bytes, 7 lines".to_string(),
        });

        assert_eq!(request_lines, vec![" → Read Cargo.toml "]);
        assert_eq!(
            execution_lines,
            vec![" ✓ Read Cargo.toml · 123 bytes, 7 lines "]
        );
    }

    #[test]
    fn agent_chat_search_tools_keep_compact_match_details() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::ToolOutput,
            content:
                "find_files result: ok\npath: .\n3 matches\n- Cargo.toml\n- src/lib.rs\n- README.md"
                    .to_string(),
        });

        assert_eq!(
            lines,
            vec![
                " ✓ Find 3 matches in . ",
                "   ↳ Cargo.toml ",
                "   ↳ src/lib.rs ",
                "   ↳ README.md ",
            ]
        );
    }

    #[test]
    fn agent_chat_failed_execution_uses_failure_glyph() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::ToolOutput,
            content: "shell result: failed\nexit 1".to_string(),
        });

        assert_eq!(lines, vec![" ✗ Shell · failed ", " exit 1 "]);
    }

    #[test]
    fn agent_chat_shell_request_renders_as_block_tool() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::Tool,
            content: "# Running in .\n$ cargo test".to_string(),
        });

        assert_eq!(lines, vec![" $ Shell ", " $ cargo test "]);
    }

    #[test]
    fn agent_chat_shell_output_renders_as_block_tool() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::ToolOutput,
            content: "shell result: ok\ncommand: `cargo test`\nexit 0 • 42ms\nstdout:\nok"
                .to_string(),
        });

        assert_eq!(
            lines,
            vec![
                " ✓ Shell · ok ",
                " $ cargo test ",
                " exit 0 • 42ms ",
                " stdout ",
                " ok ",
            ]
        );
    }

    #[test]
    fn agent_chat_shell_output_collapses_long_sections_by_default() {
        let stdout = (1..=10)
            .map(|idx| format!("line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::ToolOutput,
            content: format!("shell result: ok\nstdout:\n{stdout}"),
        });

        assert_eq!(lines[0], " ✓ Shell · ok ");
        assert_eq!(lines[1], " stdout ");
        assert!(lines.contains(&" line 8 ".to_string()));
        assert!(!lines.contains(&" line 9 ".to_string()));
        assert!(lines.contains(&" … 2 more lines (Ctrl+T full tool output) ".to_string()));
    }

    #[test]
    fn agent_chat_full_tool_mode_shows_long_shell_sections() {
        let stdout = (1..=10)
            .map(|idx| format!("line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = rendered_agent_chat_message_lines_with_modes(
            AgentChatMessage {
                role: AgentChatRole::ToolOutput,
                content: format!("shell result: ok\nstdout:\n{stdout}"),
            },
            AgentChatRenderMode::Markdown,
            AgentToolDetailMode::Full,
            None,
        );

        assert!(lines.contains(&" line 9 ".to_string()));
        assert!(lines.contains(&" line 10 ".to_string()));
        assert!(!lines.iter().any(|line| line.contains("more lines (Ctrl+T")));
    }

    #[test]
    fn agent_chat_generic_tool_output_collapses_when_long() {
        let lines = rendered_agent_chat_message_lines(AgentChatMessage {
            role: AgentChatRole::ToolOutput,
            content: "custom_tool result: ok\none\ntwo\nthree\nfour".to_string(),
        });

        assert_eq!(
            lines,
            vec![
                " ✓ Tool · ok ",
                " one ",
                " two ",
                " three ",
                " … 1 more lines (Ctrl+T full tool output) ",
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
    fn agent_chat_half_page_scrolls_use_visible_transcript_height() {
        let messages = (0..20)
            .map(|idx| AgentChatMessage {
                role: AgentChatRole::Assistant,
                content: format!("message {idx}"),
            })
            .collect::<Vec<_>>();
        let mut app = AgentChatComposerApp::new(messages, test_agent_chat_status("Ready."));

        app.scroll_half_page_down(31);
        assert_eq!(app.transcript_scroll, 9);

        app.scroll_half_page_up(31);
        assert_eq!(app.transcript_scroll, 0);
    }

    #[test]
    fn agent_chat_message_navigation_jumps_between_message_offsets() {
        let mut app = AgentChatComposerApp::new(
            vec![
                AgentChatMessage {
                    role: AgentChatRole::User,
                    content: "first".to_string(),
                },
                AgentChatMessage {
                    role: AgentChatRole::Assistant,
                    content: "reply".to_string(),
                },
                AgentChatMessage {
                    role: AgentChatRole::User,
                    content: "second".to_string(),
                },
            ],
            test_agent_chat_status("Ready."),
        );

        assert_eq!(
            app.message_start_offsets(None),
            vec![
                (AgentChatRole::User, 0),
                (AgentChatRole::Assistant, 3),
                (AgentChatRole::User, 7)
            ]
        );

        app.jump_to_next_message(16, 80);
        assert_eq!(app.transcript_scroll, 3);
        app.jump_to_next_message(16, 80);
        assert_eq!(app.transcript_scroll, 7);
        app.jump_to_previous_message(16, 80);
        assert_eq!(app.transcript_scroll, 3);
        app.jump_to_last_user_message(16, 80);
        assert_eq!(app.transcript_scroll, 7);
        app.jump_to_first_message();
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
    fn agent_chat_composer_dock_bounds_input_without_repeating_status() {
        let mut app = AgentChatComposerApp::new(Vec::new(), test_agent_chat_status("Thinking…"));
        app.input = "one\ntwo\nthree\nfour\nfive".to_string();

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

        assert_eq!(
            rendered,
            vec!["… 2 earlier lines", "three", "four", "five",]
        );
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
    fn agent_chat_render_mode_key_uses_ctrl_r() {
        assert!(agent_chat_render_mode_key(
            KeyCode::Char('r'),
            KeyModifiers::CONTROL
        ));
        assert!(!agent_chat_render_mode_key(
            KeyCode::Char('r'),
            KeyModifiers::NONE
        ));
        assert!(!agent_chat_render_mode_key(
            KeyCode::Char('e'),
            KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn agent_chat_tool_detail_key_uses_ctrl_t() {
        assert!(agent_chat_tool_detail_key(
            KeyCode::Char('t'),
            KeyModifiers::CONTROL
        ));
        assert!(!agent_chat_tool_detail_key(
            KeyCode::Char('t'),
            KeyModifiers::NONE
        ));
        assert!(!agent_chat_tool_detail_key(
            KeyCode::Char('r'),
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

        assert!(!app.palette.open);
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
    fn agent_chat_navigation_keys_use_modified_shortcuts() {
        assert!(agent_chat_half_page_up_key(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL
        ));
        assert!(agent_chat_half_page_down_key(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL
        ));
        assert!(agent_chat_previous_message_key(
            KeyCode::Up,
            KeyModifiers::ALT
        ));
        assert!(agent_chat_next_message_key(
            KeyCode::Down,
            KeyModifiers::ALT
        ));
        assert!(agent_chat_first_message_key(
            KeyCode::Home,
            KeyModifiers::CONTROL
        ));
        assert!(agent_chat_last_message_key(
            KeyCode::End,
            KeyModifiers::CONTROL
        ));
        assert!(agent_chat_last_user_message_key(
            KeyCode::Char('u'),
            KeyModifiers::ALT
        ));
        assert!(!agent_chat_last_user_message_key(
            KeyCode::Char('u'),
            KeyModifiers::NONE
        ));
    }

    #[test]
    fn agent_chat_slash_palette_key_only_opens_on_empty_composer() {
        assert!(agent_chat_slash_palette_key(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
            true
        ));
        assert!(!agent_chat_slash_palette_key(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
            false
        ));
        assert!(!agent_chat_slash_palette_key(
            KeyCode::Char('/'),
            KeyModifiers::CONTROL,
            true
        ));
        assert!(!agent_chat_slash_palette_key(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            true
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
        for ch in "model".chars() {
            app.push_palette_query(ch);
        }

        assert!(app.palette.open);
        assert_eq!(
            app.selected_palette_command(),
            Some(AgentChatCommand::SwitchModel("test".to_string()))
        );
    }

    #[test]
    fn agent_chat_palette_includes_local_keybindings_command() {
        let status = test_agent_chat_status("Ready.");
        let mut app = AgentChatComposerApp::new(Vec::new(), status);

        app.open_palette();

        assert!(app.palette.open);
        assert_eq!(
            app.selected_palette_command(),
            Some(AgentChatCommand::OpenHelp)
        );

        let (lines, _) = app.palette_body_lines_and_selected_row();
        assert!(lines.iter().any(|line| line
            .spans
            .iter()
            .any(|span| span.content.contains("Show keybindings"))));
    }

    #[test]
    fn agent_chat_palette_filters_commands_with_fuzzy_query() {
        let mut status = test_agent_chat_status("Ready.");
        status.command_palette = vec![
            AgentChatCommandEntry {
                section: "Session".to_string(),
                label: "Resume session…".to_string(),
                description: "Open the Sessions picker".to_string(),
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
        for ch in "5.5".chars() {
            app.push_palette_query(ch);
        }

        assert_eq!(app.visible_palette_indices(), vec![10]);
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
        assert!(app.palette.scroll > 0);
        assert!(selected_row >= app.palette.scroll);
        assert!(selected_row < app.palette.scroll + 5);

        for _ in 0..10 {
            app.previous_palette_item();
        }
        let (lines, selected_row) = app.palette_body_lines_and_selected_row();
        app.ensure_palette_selection_visible(5, selected_row, lines.len());

        assert_eq!(app.palette.scroll, 0);
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
        assert!(dashboard_tab_returns_to_agent(DashboardTab::Skills));
        assert!(!dashboard_tab_returns_to_agent(DashboardTab::Tools));
        assert!(dashboard_back_tab_returns_to_agent(DashboardTab::Tools));
        assert!(!dashboard_back_tab_returns_to_agent(DashboardTab::Skills));
        assert_eq!(
            APP_TABS,
            [
                "Agent",
                "Tools",
                "Sessions",
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
        let chats_app = DashboardApp::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            DashboardTab::Sessions,
        );
        let chat_entries = chats_app.dashboard_command_palette();

        assert!(chat_entries.iter().any(|entry| {
            entry.section == "Navigation" && entry.command == DashboardCommand::OpenAgent
        }));
        assert!(chat_entries.iter().any(|entry| {
            entry.section == "Sessions" && entry.command == DashboardCommand::ResumeSelectedChat
        }));
        assert!(chat_entries.iter().any(|entry| {
            entry.section == "Session filters"
                && entry.command
                    == DashboardCommand::SetSessionScope(SessionFilterScope::ChildAgent)
        }));
        assert!(!chat_entries.iter().any(|entry| {
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
            entry.section == "Sessions" && entry.command == DashboardCommand::ResumeSelectedChat
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
            DashboardTab::Sessions,
        );

        app.open_palette();
        for ch in "Promote selected sessions".chars() {
            app.push_palette_query(ch);
        }

        let visible = app.visible_palette_indices();
        assert!(!visible.is_empty());
        assert_eq!(
            app.selected_palette_command(),
            Some(DashboardCommand::PromoteSessions)
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
