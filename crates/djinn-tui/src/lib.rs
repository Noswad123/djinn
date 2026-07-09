use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use djinn_tools::ToolEntry;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

pub fn run_tools(tools: Vec<ToolEntry>) -> Result<()> {
    let mut terminal = enter_terminal()?;
    let result = run_loop(&mut terminal, tools);
    leave_terminal(&mut terminal)?;
    result
}

fn enter_terminal() -> Result<TuiTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn leave_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(terminal: &mut TuiTerminal, tools: Vec<ToolEntry>) -> Result<()> {
    let mut app = ToolsApp::new(tools);
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('j') | KeyCode::Down => app.next(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous(),
                    KeyCode::Char('d') | KeyCode::PageDown => app.scroll_down(),
                    KeyCode::Char('u') | KeyCode::PageUp => app.scroll_up(),
                    KeyCode::Home => app.first(),
                    KeyCode::End => app.last(),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

struct ToolsApp {
    tools: Vec<ToolEntry>,
    selected: usize,
    preview_scroll: u16,
}

impl ToolsApp {
    fn new(tools: Vec<ToolEntry>) -> Self {
        Self {
            tools,
            selected: 0,
            preview_scroll: 0,
        }
    }

    fn next(&mut self) {
        if self.tools.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.tools.len() - 1);
        self.preview_scroll = 0;
    }

    fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.preview_scroll = 0;
    }

    fn first(&mut self) {
        self.selected = 0;
        self.preview_scroll = 0;
    }

    fn last(&mut self) {
        if !self.tools.is_empty() {
            self.selected = self.tools.len() - 1;
            self.preview_scroll = 0;
        }
    }

    fn scroll_down(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_add(8);
    }

    fn scroll_up(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_sub(8);
    }

    fn selected_tool(&self) -> Option<&ToolEntry> {
        self.tools.get(self.selected)
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(frame.area());

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
            .split(chunks[0]);

        let items = if self.tools.is_empty() {
            vec![ListItem::new("No tools discovered")]
        } else {
            self.tools
                .iter()
                .map(|tool| {
                    ListItem::new(vec![
                        Line::from(Span::styled(
                            tool.name.clone(),
                            Style::default().add_modifier(Modifier::BOLD),
                        )),
                        Line::from(tool.description.clone()),
                    ])
                })
                .collect::<Vec<_>>()
        };

        let mut state = ListState::default();
        if !self.tools.is_empty() {
            state.select(Some(self.selected));
        }
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Tools"))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("› ");
        frame.render_widget(Clear, body[0]);
        frame.render_stateful_widget(list, body[0], &mut state);

        let preview = self
            .selected_tool()
            .map(tool_preview)
            .unwrap_or_else(|| "No preview available.".to_string());
        let preview = Paragraph::new(preview)
            .block(Block::default().borders(Borders::ALL).title("Preview"))
            .scroll((self.preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(Clear, body[1]);
        frame.render_widget(preview, body[1]);

        let help = Paragraph::new(
            "↑/k ↓/j move • PgUp/u PgDn/d scroll preview • Home/End jump • q/Esc quit",
        )
        .style(Style::default().add_modifier(Modifier::DIM));
        frame.render_widget(Clear, chunks[1]);
        frame.render_widget(help, chunks[1]);
    }
}

fn tool_preview(tool: &ToolEntry) -> String {
    format!(
        "{}\n{}:{}\n\n{}",
        tool.description,
        tool.path.display(),
        tool.line,
        sanitize_preview(&tool.preview)
    )
}

fn sanitize_preview(preview: &str) -> String {
    preview
        .chars()
        .filter_map(|ch| match ch {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            ch if ch.is_control() => None,
            ch => Some(ch),
        })
        .collect()
}
