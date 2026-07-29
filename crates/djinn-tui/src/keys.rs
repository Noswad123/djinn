use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub(crate) fn actionable_key_event(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub(crate) fn dashboard_help_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('/') | KeyCode::Char('_'))
}

pub(crate) fn dashboard_palette_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('p'))
}

pub(crate) fn palette_next_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Down)
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('n')))
}

pub(crate) fn palette_previous_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Up)
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('p')))
}

pub(crate) fn palette_text_key(modifiers: KeyModifiers) -> bool {
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}
