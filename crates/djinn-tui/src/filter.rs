#[derive(Debug, Clone, Default)]
pub(crate) struct FilterState {
    pub(crate) query: String,
    pub(crate) editing: bool,
}

impl FilterState {
    pub(crate) fn toggle(&mut self) {
        if self.query.is_empty() {
            self.editing = true;
        } else {
            self.query.clear();
            self.editing = false;
        }
    }

    pub(crate) fn push(&mut self, ch: char) {
        self.query.push(ch);
    }

    pub(crate) fn backspace(&mut self) {
        self.query.pop();
    }

    pub(crate) fn label(&self) -> String {
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

pub(crate) fn fuzzy_match(query: &str, candidate: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let candidate = candidate.to_lowercase();
    let mut chars = candidate.chars();
    query.chars().all(|needle| chars.any(|ch| ch == needle))
}

pub(crate) fn selected_visible_position(selected: usize, visible: &[usize]) -> Option<usize> {
    visible.iter().position(|idx| *idx == selected)
}
