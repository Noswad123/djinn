use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

pub(crate) type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

// Ask xterm-compatible terminals to translate wheel motion in the alternate
// screen into cursor up/down keys. This keeps native terminal text selection
// available because we deliberately do not enable mouse capture/reporting.
const ENABLE_ALTERNATE_SCROLL: &str = "\x1b[?1007h";
const DISABLE_ALTERNATE_SCROLL: &str = "\x1b[?1007l";
const ENABLE_APPLICATION_CURSOR_KEYS: &str = "\x1b[?1h";
const DISABLE_APPLICATION_CURSOR_KEYS: &str = "\x1b[?1l";

pub(crate) fn enter_terminal() -> Result<TuiTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        Print(ENABLE_ALTERNATE_SCROLL),
        Print(ENABLE_APPLICATION_CURSOR_KEYS),
        EnableBracketedPaste,
        push_keyboard_enhancement()
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

pub(crate) fn leave_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    suspend_terminal(terminal)
}

pub(crate) fn suspend_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        Print(DISABLE_APPLICATION_CURSOR_KEYS),
        Print(DISABLE_ALTERNATE_SCROLL),
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

pub(crate) fn resume_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        Print(ENABLE_ALTERNATE_SCROLL),
        Print(ENABLE_APPLICATION_CURSOR_KEYS),
        EnableBracketedPaste,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternate_scroll_mode_uses_xterm_private_mode_without_mouse_capture() {
        assert_eq!(ENABLE_ALTERNATE_SCROLL, "\x1b[?1007h");
        assert_eq!(DISABLE_ALTERNATE_SCROLL, "\x1b[?1007l");
        assert_eq!(ENABLE_APPLICATION_CURSOR_KEYS, "\x1b[?1h");
        assert_eq!(DISABLE_APPLICATION_CURSOR_KEYS, "\x1b[?1l");
    }
}
