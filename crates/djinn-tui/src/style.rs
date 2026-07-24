use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};

// Catppuccin Mocha palette.
pub(crate) const CTP_BASE: Color = Color::Rgb(30, 30, 46);
pub(crate) const CTP_MANTLE: Color = Color::Rgb(24, 24, 37);
pub(crate) const CTP_SURFACE0: Color = Color::Rgb(49, 50, 68);
pub(crate) const CTP_SURFACE1: Color = Color::Rgb(69, 71, 90);
pub(crate) const CTP_TEXT: Color = Color::Rgb(205, 214, 244);
pub(crate) const CTP_SUBTEXT0: Color = Color::Rgb(166, 173, 200);
pub(crate) const CTP_LAVENDER: Color = Color::Rgb(180, 190, 254);
pub(crate) const CTP_MAUVE: Color = Color::Rgb(203, 166, 247);
pub(crate) const CTP_GREEN: Color = Color::Rgb(166, 227, 161);
pub(crate) const CTP_PEACH: Color = Color::Rgb(250, 179, 135);
pub(crate) const CTP_RED: Color = Color::Rgb(243, 139, 168);
pub(crate) const CTP_SKY: Color = Color::Rgb(137, 220, 235);
pub(crate) const CTP_YELLOW: Color = Color::Rgb(249, 226, 175);

pub(crate) fn base_style() -> Style {
    Style::default().fg(CTP_TEXT).bg(CTP_BASE)
}

pub(crate) fn dim_style() -> Style {
    Style::default().fg(CTP_SUBTEXT0).bg(CTP_BASE)
}

pub(crate) fn title_style() -> Style {
    Style::default()
        .fg(CTP_LAVENDER)
        .bg(CTP_BASE)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn highlight_style() -> Style {
    Style::default()
        .fg(CTP_MAUVE)
        .bg(CTP_SURFACE1)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn selected_style() -> Style {
    Style::default()
        .fg(CTP_PEACH)
        .bg(CTP_BASE)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn block<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(title_style())
        .border_style(Style::default().fg(CTP_SURFACE0).bg(CTP_MANTLE))
        .style(Style::default().fg(CTP_TEXT).bg(CTP_MANTLE))
}

pub(crate) fn agent_chat_block<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .borders(agent_chat_borders())
        .title(title)
        .title_style(title_style())
        .border_style(Style::default().fg(CTP_SURFACE0).bg(CTP_MANTLE))
        .style(Style::default().fg(CTP_TEXT).bg(CTP_MANTLE))
}

pub(crate) fn agent_chat_borders() -> Borders {
    Borders::TOP | Borders::BOTTOM
}
