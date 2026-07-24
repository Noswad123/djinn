use ratatui::text::{Line, Span};

use crate::filter::{fuzzy_match, selected_visible_position};
use crate::{base_style, dim_style, selected_style, title_style};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommandPaletteState {
    pub(crate) open: bool,
    pub(crate) selected: usize,
    pub(crate) query: String,
    pub(crate) scroll: usize,
}

impl CommandPaletteState {
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.selected = 0;
        self.query.clear();
        self.scroll = 0;
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    pub(crate) fn push_query(&mut self, ch: char) {
        self.query.push(ch);
        self.scroll = 0;
    }

    pub(crate) fn backspace_query_or_close(&mut self) {
        if self.query.is_empty() {
            self.close();
        } else {
            self.query.pop();
            self.scroll = 0;
        }
    }

    pub(crate) fn next(&mut self, visible: &[usize]) {
        if visible.is_empty() {
            return;
        }
        let position = selected_visible_position(self.selected, visible).unwrap_or(0);
        self.selected = visible[(position + 1).min(visible.len() - 1)];
    }

    pub(crate) fn previous(&mut self, visible: &[usize]) {
        if visible.is_empty() {
            return;
        }
        let position = selected_visible_position(self.selected, visible).unwrap_or(0);
        self.selected = visible[position.saturating_sub(1)];
    }

    pub(crate) fn normalize_selection(&mut self, visible: &[usize]) {
        if visible.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else if !visible.contains(&self.selected) {
            self.selected = visible[0];
        }
    }

    pub(crate) fn ensure_selection_visible(
        &mut self,
        body_height: usize,
        selected_row: Option<usize>,
        total_lines: usize,
    ) {
        if body_height == 0 || total_lines <= body_height {
            self.scroll = 0;
            return;
        }
        let max_scroll = total_lines.saturating_sub(body_height);
        if let Some(selected_row) = selected_row {
            if selected_row < body_height {
                self.scroll = 0;
            } else if selected_row < self.scroll {
                self.scroll = selected_row;
            } else if selected_row >= self.scroll.saturating_add(body_height) {
                self.scroll = selected_row.saturating_add(1).saturating_sub(body_height);
            }
        }
        self.scroll = self.scroll.min(max_scroll);
    }
}

pub(crate) trait CommandPaletteItem {
    fn section(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
}

pub(crate) fn entry_matches_query(entry: &impl CommandPaletteItem, query: &str) -> bool {
    fuzzy_match(
        query,
        &format!(
            "{} {} {}",
            entry.section(),
            entry.label(),
            entry.description()
        ),
    )
}

pub(crate) fn visible_indices<T: CommandPaletteItem>(entries: &[T], query: &str) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| entry_matches_query(entry, query).then_some(idx))
        .collect()
}

pub(crate) fn selected_command<T, C>(
    entries: &[T],
    visible: &[usize],
    selected: usize,
    command: impl FnOnce(&T) -> C,
) -> Option<C> {
    if !visible.contains(&selected) {
        return None;
    }
    entries.get(selected).map(command)
}

pub(crate) fn body_lines_and_selected_row<T: CommandPaletteItem>(
    entries: &[T],
    visible: &[usize],
    selected: usize,
) -> (Vec<Line<'static>>, Option<usize>) {
    let mut lines = Vec::new();
    let mut selected_row = None;
    let mut previous_section = None::<String>;
    for idx in visible.iter().copied() {
        let Some(entry) = entries.get(idx) else {
            continue;
        };
        if previous_section.as_deref() != Some(entry.section()) {
            if previous_section.is_some() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                entry.section().to_string(),
                title_style(),
            )));
            previous_section = Some(entry.section().to_string());
        }
        let marker = if idx == selected { "›" } else { " " };
        let style = if idx == selected {
            selected_style()
        } else {
            base_style()
        };
        if idx == selected {
            selected_row = Some(lines.len());
        }
        lines.push(Line::from(Span::styled(
            format!("{marker} {}", entry.label()),
            style,
        )));
        if !entry.description().trim().is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  {}", entry.description()),
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
