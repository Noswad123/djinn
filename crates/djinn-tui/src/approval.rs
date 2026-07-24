use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};
use serde_json::Value;

use crate::style::*;
use crate::TuiTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
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

pub(crate) struct ApprovalDialogApp {
    pub(crate) preview: ApprovalPreviewState,
}

impl ApprovalDialogApp {
    pub(crate) fn new(metadata: Value) -> Self {
        Self {
            preview: ApprovalPreviewState::from_metadata(&metadata),
        }
    }

    pub(crate) fn next_file(&mut self) {
        self.preview.next_file();
    }

    pub(crate) fn previous_file(&mut self) {
        self.preview.previous_file();
    }

    pub(crate) fn scroll_down(&mut self) {
        self.preview.scroll_down();
    }

    pub(crate) fn scroll_up(&mut self) {
        self.preview.scroll_up();
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame<'_>) {
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

pub(crate) fn run_approval_dialog_loop(
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
                        return Ok(ApprovalDecision::Deny);
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
