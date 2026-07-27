use ratatui::text::{Line, Span};

use crate::filter::{fuzzy_match, selected_visible_position};
use crate::{base_style, dim_style, selected_style, title_style};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GroupedSelectState {
    pub(crate) open: bool,
    pub(crate) selected: usize,
    pub(crate) query: String,
    pub(crate) scroll: usize,
}

impl GroupedSelectState {
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

pub(crate) trait GroupedSelectItem {
    fn section(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
}

pub(crate) fn item_matches_query(item: &impl GroupedSelectItem, query: &str) -> bool {
    fuzzy_match(
        query,
        &format!("{} {} {}", item.section(), item.label(), item.description()),
    )
}

pub(crate) fn visible_indices<T: GroupedSelectItem>(items: &[T], query: &str) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| item_matches_query(item, query).then_some(idx))
        .collect()
}

pub(crate) fn selected_item<T, C>(
    items: &[T],
    visible: &[usize],
    selected: usize,
    item: impl FnOnce(&T) -> C,
) -> Option<C> {
    if !visible.contains(&selected) {
        return None;
    }
    items.get(selected).map(item)
}

pub(crate) fn body_lines_and_selected_row<T: GroupedSelectItem>(
    items: &[T],
    visible: &[usize],
    selected: usize,
) -> (Vec<Line<'static>>, Option<usize>) {
    let mut lines = Vec::new();
    let mut selected_row = None;
    let mut previous_section = None::<String>;
    for idx in visible.iter().copied() {
        let Some(item) = items.get(idx) else {
            continue;
        };
        if previous_section.as_deref() != Some(item.section()) {
            if previous_section.is_some() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                item.section().to_string(),
                title_style(),
            )));
            previous_section = Some(item.section().to_string());
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
            format!("{marker} {}", item.label()),
            style,
        )));
        if !item.description().trim().is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  {}", item.description()),
                dim_style(),
            )));
        }
    }
    if visible.is_empty() {
        lines.push(Line::from(Span::styled(
            "No items match your search.",
            dim_style(),
        )));
    }
    (lines, selected_row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestItem {
        section: &'static str,
        label: &'static str,
        description: &'static str,
    }

    impl GroupedSelectItem for TestItem {
        fn section(&self) -> &str {
            self.section
        }

        fn label(&self) -> &str {
            self.label
        }

        fn description(&self) -> &str {
            self.description
        }
    }

    #[test]
    fn grouped_select_filters_across_section_label_and_description() {
        let items = vec![
            TestItem {
                section: "Session",
                label: "Resume chat",
                description: "Open history",
            },
            TestItem {
                section: "Model",
                label: "Switch model",
                description: "openai/gpt-5.5",
            },
        ];

        assert_eq!(visible_indices(&items, "hist"), vec![0]);
        assert_eq!(visible_indices(&items, "gpt55"), vec![1]);
        assert_eq!(visible_indices(&items, "model"), vec![1]);
    }

    #[test]
    fn grouped_select_state_tracks_selection_and_query() {
        let mut state = GroupedSelectState::default();

        state.open();
        state.push_query('m');
        state.normalize_selection(&[2, 4]);
        assert!(state.open);
        assert_eq!(state.query, "m");
        assert_eq!(state.selected, 2);

        state.next(&[2, 4]);
        assert_eq!(state.selected, 4);
        state.previous(&[2, 4]);
        assert_eq!(state.selected, 2);

        state.backspace_query_or_close();
        assert_eq!(state.query, "");
        state.backspace_query_or_close();
        assert!(!state.open);
    }

    #[test]
    fn grouped_select_renders_grouped_rows_and_selected_row() {
        let items = vec![
            TestItem {
                section: "A",
                label: "One",
                description: "First",
            },
            TestItem {
                section: "B",
                label: "Two",
                description: "",
            },
        ];

        let (lines, selected_row) = body_lines_and_selected_row(&items, &[0, 1], 1);
        let rendered = lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(rendered, vec!["A", "  One", "  First", "", "B", "› Two"]);
        assert_eq!(selected_row, Some(5));
    }
}
