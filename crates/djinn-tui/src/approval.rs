use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};
use serde_json::Value;

use crate::filter::fuzzy_match;
use crate::style::*;
use crate::TuiTerminal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    ApproveAll,
    ApprovePaths(Vec<String>),
    ApproveAllForSession(Vec<String>),
    ApprovePathsForSession(Vec<String>),
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPreviewState {
    files: Vec<ApprovalPreviewFile>,
    selected_file: usize,
    scroll: u16,
    filter_query: String,
    filter_editing: bool,
    approved_files: HashSet<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPreviewFile {
    pub operation: String,
    pub path: String,
    pub resource_path: String,
    pub new_path: Option<String>,
    pub new_resource_path: Option<String>,
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
            filter_query: String::new(),
            filter_editing: false,
            approved_files: HashSet::new(),
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

    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    pub fn filter_editing(&self) -> bool {
        self.filter_editing
    }

    pub fn approved_file_indices(&self) -> &HashSet<usize> {
        &self.approved_files
    }

    pub fn toggle_selected_file_approval(&mut self) {
        if self.files.is_empty() {
            return;
        }
        if !self.approved_files.insert(self.selected_file) {
            self.approved_files.remove(&self.selected_file);
        }
    }

    pub fn approved_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        let mut indices = self.approved_files.iter().copied().collect::<Vec<_>>();
        indices.sort_unstable();
        for index in indices {
            let Some(file) = self.files.get(index) else {
                continue;
            };
            push_unique_path(&mut paths, file.resource_path.clone());
            if let Some(new_resource_path) = &file.new_resource_path {
                push_unique_path(&mut paths, new_resource_path.clone());
            }
        }
        paths
    }

    pub fn all_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for file in &self.files {
            push_unique_path(&mut paths, file.resource_path.clone());
            if let Some(new_resource_path) = &file.new_resource_path {
                push_unique_path(&mut paths, new_resource_path.clone());
            }
        }
        paths
    }

    pub fn has_approved_files(&self) -> bool {
        !self.approved_files.is_empty()
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

    pub fn toggle_filter(&mut self) {
        if self.filter_query.is_empty() {
            self.filter_editing = true;
        } else {
            self.filter_query.clear();
            self.filter_editing = false;
            self.scroll = 0;
        }
    }

    pub fn finish_filter(&mut self) {
        self.filter_editing = false;
    }

    pub fn filter_push(&mut self, ch: char) {
        self.filter_query.push(ch);
        self.scroll = 0;
    }

    pub fn filter_backspace(&mut self) {
        self.filter_query.pop();
        self.scroll = 0;
    }

    pub fn filter_label(&self) -> String {
        if self.filter_query.is_empty() && self.filter_editing {
            "filter: ".to_string()
        } else if self.filter_query.is_empty() {
            "filter: off".to_string()
        } else if self.filter_editing {
            format!("filter: {}", self.filter_query)
        } else {
            format!("filter: {} (/ clears)", self.filter_query)
        }
    }

    pub fn file_labels(&self) -> Vec<String> {
        self.files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let marker = if self.approved_files.contains(&index) {
                    "[x]"
                } else {
                    "[ ]"
                };
                format!("{marker} {}", file.label())
            })
            .collect()
    }

    pub fn selected_lines(&self) -> Vec<Line<'static>> {
        self.selected_file()
            .map(|file| approval_preview_file_lines_with_filter(file, &self.filter_query))
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

    pub(crate) fn toggle_filter(&mut self) {
        self.preview.toggle_filter();
    }

    pub(crate) fn finish_filter(&mut self) {
        self.preview.finish_filter();
    }

    pub(crate) fn filter_push(&mut self, ch: char) {
        self.preview.filter_push(ch);
    }

    pub(crate) fn filter_backspace(&mut self) {
        self.preview.filter_backspace();
    }

    pub(crate) fn toggle_selected_file_approval(&mut self) {
        self.preview.toggle_selected_file_approval();
    }

    pub(crate) fn approval_decision_for_marked_files(&self) -> Option<ApprovalDecision> {
        self.preview
            .has_approved_files()
            .then(|| ApprovalDecision::ApprovePaths(self.preview.approved_paths()))
    }

    pub(crate) fn approval_decision_for_marked_files_session(&self) -> Option<ApprovalDecision> {
        self.preview
            .has_approved_files()
            .then(|| ApprovalDecision::ApprovePathsForSession(self.preview.approved_paths()))
    }

    pub(crate) fn approval_decision_for_all_files_session(&self) -> ApprovalDecision {
        ApprovalDecision::ApproveAllForSession(self.preview.all_paths())
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

        let preview_title = format!("Patch preview ({})", self.preview.filter_label());
        let preview = Paragraph::new(self.preview.selected_lines())
            .block(block(&preview_title))
            .style(base_style())
            .scroll((self.preview.scroll(), 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);

        let help = "a/Enter approve all  A remember all  Space mark  p approve marked  P remember marked  d/q/Esc deny  j/k file  J/K scroll  / filter";
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
                if app.preview.filter_editing() {
                    match key.code {
                        KeyCode::Char('/') => app.toggle_filter(),
                        KeyCode::Backspace => app.filter_backspace(),
                        KeyCode::Enter | KeyCode::Esc => app.finish_filter(),
                        KeyCode::Char(ch) => app.filter_push(ch),
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('a') | KeyCode::Enter => return Ok(ApprovalDecision::ApproveAll),
                    KeyCode::Char('A') => {
                        return Ok(app.approval_decision_for_all_files_session());
                    }
                    KeyCode::Char(' ') => app.toggle_selected_file_approval(),
                    KeyCode::Char('p') => {
                        if let Some(decision) = app.approval_decision_for_marked_files() {
                            return Ok(decision);
                        }
                    }
                    KeyCode::Char('P') => {
                        if let Some(decision) = app.approval_decision_for_marked_files_session() {
                            return Ok(decision);
                        }
                    }
                    KeyCode::Char('q') | KeyCode::Char('d') | KeyCode::Esc => {
                        return Ok(ApprovalDecision::Deny);
                    }
                    KeyCode::Char('/') => app.toggle_filter(),
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
            resource_path: value["path"]
                .as_str()
                .or_else(|| value["relative_path"].as_str())
                .unwrap_or("<unknown>")
                .to_string(),
            new_path: value["relative_new_path"]
                .as_str()
                .or_else(|| value["new_path"].as_str())
                .map(str::to_string),
            new_resource_path: value["new_path"]
                .as_str()
                .or_else(|| value["relative_new_path"].as_str())
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

fn push_unique_path(paths: &mut Vec<String>, path: String) {
    if path.trim().is_empty() || paths.iter().any(|existing| existing == &path) {
        return;
    }
    paths.push(path);
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
    approval_preview_file_lines_with_filter(file, "")
}

fn approval_preview_file_lines_with_filter(
    file: &ApprovalPreviewFile,
    filter_query: &str,
) -> Vec<Line<'static>> {
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
    let filter_query = filter_query.trim();
    let mut matched_lines = 0usize;
    for (index, hunk) in file.hunks.iter().enumerate() {
        let hunk_lines = hunk
            .lines
            .iter()
            .filter(|line| approval_preview_line_matches(line, filter_query))
            .collect::<Vec<_>>();
        if !filter_query.is_empty() && hunk_lines.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(
            format!("@@ hunk {}", index + 1),
            dim_style(),
        )));
        for line in hunk_lines {
            matched_lines += 1;
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
    if !filter_query.is_empty() && matched_lines == 0 {
        lines.push(Line::from(Span::styled(
            format!("No hunk lines match `{filter_query}`."),
            dim_style(),
        )));
    }
    lines
}

fn approval_preview_line_matches(line: &ApprovalPreviewLine, filter_query: &str) -> bool {
    if filter_query.trim().is_empty() {
        return true;
    }
    let prefixed = match line.kind {
        ApprovalPreviewLineKind::Context => format!("  {}", line.content),
        ApprovalPreviewLineKind::Add => format!("+ {}", line.content),
        ApprovalPreviewLineKind::Remove => format!("- {}", line.content),
    };
    prefixed
        .to_lowercase()
        .contains(&filter_query.to_lowercase())
        || fuzzy_match(filter_query, &prefixed)
}
