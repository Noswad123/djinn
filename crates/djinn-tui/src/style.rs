use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};

// Catppuccin-inspired palette.
pub(crate) const CTP_BASE: Color = Color::Rgb(36, 39, 58);
pub(crate) const CTP_MANTLE: Color = Color::Rgb(24, 25, 38);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThemeTokens {
    pub(crate) app_bg: Color,
    pub(crate) panel_bg: Color,
    pub(crate) composer_bg: Color,
    pub(crate) elevated_bg: Color,
    pub(crate) text: Color,
    pub(crate) muted_text: Color,
    pub(crate) title: Color,
    pub(crate) selected: Color,
    pub(crate) highlight: Color,
    pub(crate) border: Color,
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) error: Color,
    pub(crate) info: Color,
    pub(crate) accent: Color,
    pub(crate) code_bg: Color,
    pub(crate) tool_bg: Color,
}

impl ThemeTokens {
    pub(crate) const CATPPUCCIN_MOCHA: Self = Self {
        app_bg: CTP_BASE,
        panel_bg: CTP_MANTLE,
        composer_bg: CTP_MANTLE,
        elevated_bg: CTP_SURFACE0,
        text: CTP_TEXT,
        muted_text: CTP_SUBTEXT0,
        title: CTP_LAVENDER,
        selected: CTP_PEACH,
        highlight: CTP_MAUVE,
        border: CTP_SURFACE0,
        success: CTP_GREEN,
        warning: CTP_YELLOW,
        error: CTP_RED,
        info: CTP_SKY,
        accent: CTP_MAUVE,
        code_bg: CTP_SURFACE1,
        tool_bg: CTP_SURFACE1,
    };
}

pub(crate) fn theme_tokens() -> ThemeTokens {
    ThemeTokens::CATPPUCCIN_MOCHA
}

pub(crate) fn base_style() -> Style {
    let theme = theme_tokens();
    Style::default().fg(theme.text).bg(theme.app_bg)
}

pub(crate) fn dim_style() -> Style {
    let theme = theme_tokens();
    Style::default().fg(theme.muted_text).bg(theme.app_bg)
}

pub(crate) fn title_style() -> Style {
    let theme = theme_tokens();
    Style::default()
        .fg(theme.title)
        .bg(theme.app_bg)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn highlight_style() -> Style {
    let theme = theme_tokens();
    Style::default()
        .fg(theme.highlight)
        .bg(theme.code_bg)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn selected_style() -> Style {
    let theme = theme_tokens();
    Style::default()
        .fg(theme.selected)
        .bg(theme.app_bg)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn success_style() -> Style {
    let theme = theme_tokens();
    Style::default().fg(theme.success).bg(theme.app_bg)
}

pub(crate) fn warning_style() -> Style {
    let theme = theme_tokens();
    Style::default().fg(theme.warning).bg(theme.app_bg)
}

pub(crate) fn error_style() -> Style {
    let theme = theme_tokens();
    Style::default().fg(theme.error).bg(theme.app_bg)
}

pub(crate) fn info_style() -> Style {
    let theme = theme_tokens();
    Style::default().fg(theme.info).bg(theme.app_bg)
}

pub(crate) fn block<'a>(title: &'a str) -> Block<'a> {
    let theme = theme_tokens();
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(title_style())
        .border_style(Style::default().fg(theme.border).bg(theme.panel_bg))
        .style(Style::default().fg(theme.text).bg(theme.panel_bg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_tokens_map_semantic_roles() {
        let theme = theme_tokens();

        assert_eq!(theme.app_bg, Color::Rgb(36, 39, 58));
        assert_eq!(theme.composer_bg, Color::Rgb(24, 25, 38));
        assert_eq!(theme.app_bg, CTP_BASE);
        assert_eq!(theme.panel_bg, CTP_MANTLE);
        assert_eq!(theme.composer_bg, CTP_MANTLE);
        assert_eq!(theme.text, CTP_TEXT);
        assert_eq!(theme.muted_text, CTP_SUBTEXT0);
        assert_eq!(theme.success, CTP_GREEN);
        assert_eq!(theme.error, CTP_RED);
        assert_eq!(theme.code_bg, CTP_SURFACE1);
    }

    #[test]
    fn shared_styles_use_semantic_theme_tokens() {
        let theme = theme_tokens();

        assert_eq!(base_style().fg, Some(theme.text));
        assert_eq!(base_style().bg, Some(theme.app_bg));
        assert_eq!(dim_style().fg, Some(theme.muted_text));
        assert_eq!(selected_style().fg, Some(theme.selected));
    }
}
