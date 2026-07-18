//! Responsive project shell.  This module is intentionally stateless: the
//! reducer remains the source of truth and this widget only projects it.
use crate::state::{AppState, Connection};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget, Wrap},
};

const ACCENT: Color = Color::Cyan;

/// Draw the shell, collapsing side regions as the terminal gets narrower.
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    let top = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(area);
    frame.render_widget(top_bar(state), top[0]);

    let (rail, projects, content) = if area.width < 60 {
        (
            Constraint::Length(0),
            Constraint::Length(0),
            Constraint::Min(1),
        )
    } else if area.width < 90 {
        (
            Constraint::Length(4),
            Constraint::Length(0),
            Constraint::Min(1),
        )
    } else {
        (
            Constraint::Length(5),
            Constraint::Length(24),
            Constraint::Min(1),
        )
    };
    let columns = Layout::horizontal([rail, projects, content]).split(top[1]);
    if columns[0].width > 0 {
        frame.render_widget(icon_rail(), columns[0]);
    }
    if columns[1].width > 0 {
        frame.render_widget(project_list(state), columns[1]);
    }
    frame.render_widget(content_panel(state), columns[2]);
}

fn top_bar(state: &AppState) -> impl Widget + '_ {
    let status = match state.connection {
        Connection::Connected => "● connected",
        Connection::Reconnecting => "◌ reconnecting",
        Connection::Disconnected => "× offline",
    };
    Paragraph::new(Line::from(vec![
        Span::styled(
            " τ ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("project / "),
        Span::styled(
            state.session_id.as_deref().unwrap_or("new conversation"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("                                      "),
        Span::styled(
            status,
            Style::default().fg(if state.connection == Connection::Connected {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
    ]))
    .block(Block::default().borders(Borders::BOTTOM))
}

fn icon_rail() -> impl Widget {
    List::new(["⌂", "▣", "◈", "⚙"].into_iter().map(ListItem::new))
        .block(Block::default().borders(Borders::RIGHT).title(" τ "))
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
}

fn project_list(state: &AppState) -> impl Widget + '_ {
    let mut items = vec![ListItem::new(Span::styled(
        "PROJECTS",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if state.servers.is_empty() {
        items.push(ListItem::new(Span::styled(
            "Loading projects…",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (index, project) in state.servers.iter().enumerate() {
            let marker = if index == state.server_index {
                "› "
            } else {
                "  "
            };
            items.push(ListItem::new(format!("{marker}{project}")));
        }
    }
    items.push(ListItem::new(""));
    items.push(ListItem::new(Span::styled(
        "+ New project",
        Style::default().fg(ACCENT),
    )));
    List::new(items).block(Block::default().borders(Borders::RIGHT).title(" Projects "))
}

fn content_panel(state: &AppState) -> impl Widget + '_ {
    let mut lines = Vec::new();
    if state.connection == Connection::Disconnected {
        lines.push(Line::from(Span::styled(
            "Unable to connect to daemon",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from("Retry with R, or start `tau serve`."));
    } else if state.transcript.is_empty() {
        lines.push(Line::from(Span::styled(
            "No conversation yet",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from("Start a new conversation below."));
    } else {
        lines.extend(
            state
                .transcript
                .iter()
                .map(|line| Line::from(line.as_str())),
        );
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("▸ ", Style::default().fg(ACCENT)),
        Span::raw(state.input.as_str()),
    ]));
    Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Conversation "),
    )
}

/// Geometry used by callers that need to place a floating widget consistently.
pub fn content_area(area: Rect) -> Rect {
    let top = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(area);
    let widths = if area.width < 60 {
        [
            Constraint::Length(0),
            Constraint::Length(0),
            Constraint::Min(1),
        ]
    } else if area.width < 90 {
        [
            Constraint::Length(4),
            Constraint::Length(0),
            Constraint::Min(1),
        ]
    } else {
        [
            Constraint::Length(5),
            Constraint::Length(24),
            Constraint::Min(1),
        ]
    };
    Layout::horizontal(widths).split(top[1])[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    fn text(width: u16, height: u16, state: &AppState) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, state)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }
    #[test]
    fn wide_shell_has_projects_and_content() {
        let out = text(120, 30, &AppState::default());
        assert!(out.contains("Projects") && out.contains("Conversation"));
    }
    #[test]
    fn narrow_shell_collapses_project_list() {
        let out = text(50, 20, &AppState::default());
        assert!(!out.contains("PROJECTS"));
    }
    #[test]
    fn error_state_is_visible() {
        let s = AppState {
            connection: Connection::Disconnected,
            ..AppState::default()
        };
        assert!(text(80, 20, &s).contains("Unable to connect"));
    }
}
