use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub(crate) fn actionable_key_event(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub(crate) fn agent_chat_newline_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::SHIFT) && matches!(code, KeyCode::Enter)
}

pub(crate) fn agent_chat_editor_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('e'))
}

pub(crate) fn agent_chat_render_mode_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('r'))
}

pub(crate) fn agent_chat_help_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('/') | KeyCode::Char('_'))
}

pub(crate) fn agent_chat_palette_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('p'))
}

pub(crate) fn agent_chat_slash_palette_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    input_empty: bool,
) -> bool {
    input_empty
        && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && matches!(code, KeyCode::Char('/'))
}

pub(crate) fn agent_chat_palette_next_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Down)
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('n')))
}

pub(crate) fn agent_chat_palette_previous_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Up)
        || (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('p')))
}

pub(crate) fn palette_text_key(modifiers: KeyModifiers) -> bool {
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

pub(crate) fn agent_chat_quit_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    input_empty: bool,
) -> bool {
    (modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')))
        || (input_empty && matches!(code, KeyCode::Esc))
}
