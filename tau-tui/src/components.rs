//! Small composable widgets. Keeping these pure makes ratatui snapshots cheap.
use crate::operations::OperationsState;
use crate::{feed, projects::ProjectState, reducer::filtered_models, shell, state::*};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

pub fn render(frame: &mut Frame, s: &AppState) {
    render_inner(frame, s, None);
}

/// Production render path with the client-local typed project projection.
/// The legacy `render` wrapper remains useful for callers that only have chat
/// state and intentionally displays the shell's loading projection.
pub fn render_with_projects(frame: &mut Frame, s: &AppState, projects: &ProjectState) {
    render_inner(frame, s, Some(projects));
}

fn render_inner(frame: &mut Frame, s: &AppState, projects: Option<&ProjectState>) {
    if let Some(projects) = projects {
        shell::render_with_projects(frame, s, projects);
    } else {
        shell::render(frame, s);
    }
    let content = shell::content_area(frame.area());
    let root = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(38)])
        .split(content);
    let conversation = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(7)])
        .split(root[0]);
    let transcript = s.transcript.join("\n");
    if !s.raw_events.is_empty() {
        let mut projection =
            feed::project_with_humans(&s.raw_events, &s.human_messages, s.connection, s.following);
        for item in &mut projection.items {
            if s.expanded_feed.contains(&item.event.sequence) {
                item.collapsed = false;
            }
        }
        feed::render(frame, conversation[0], &projection);
    } else {
        frame.render_widget(
            Paragraph::new(transcript).wrap(Wrap { trim: false }).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Conversation "),
            ),
            conversation[0],
        );
    }
    let footer = Line::from(vec![
        Span::styled(format!(" {} ", s.model), Style::default().fg(Color::Cyan)),
        Span::raw(format!(
            "  agent:{} tier:{} {}",
            s.agent,
            s.task_tier,
            if s.autonomous { "AUTO" } else { "ASK" }
        )),
    ]);
    let status = format!(
        "  {}  {}  {} chars  {}",
        if s.connection == Connection::Connected {
            "● connected"
        } else {
            "○ disconnected"
        },
        if s.cancelling {
            "cancelling"
        } else if s.turn_id.is_some() {
            "active"
        } else {
            "ready"
        },
        s.input.chars().count(),
        if s.turn_id.is_some() {
            "Ctrl-C cancel"
        } else {
            "Enter send"
        }
    );
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            footer,
            Line::from(s.input.as_str()),
            Line::from(status),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" prompt (Shift+Enter newline) "),
        ),
        conversation[1],
    );
    // Keep the terminal cursor at the UTF-8 byte cursor's visual column so
    // multiline editing remains usable instead of merely storing text.
    let line_start = s.input[..s.cursor].rfind('\n').map_or(0, |i| i + 1);
    let line = s.input[..s.cursor].matches('\n').count() as u16;
    let column = s.input[line_start..s.cursor].chars().count() as u16;
    frame.set_cursor_position((conversation[1].x + 1 + column, conversation[1].y + 2 + line));
    render_operations_panel(
        frame,
        root[1],
        &s.operations,
        s.operations_tab,
        s.operations_loading,
        s.operations_error.as_deref(),
        s.operations_ack.as_deref(),
    );
    if s.picker != Picker::None {
        picker(frame, s);
    }
    if s.sessions.open {
        session_navigator(frame, s);
    }
    if let Some(p) = &s.permission {
        permission(frame, p);
    }
    if let Some(question) = &s.question {
        question_modal(frame, question);
    }
    if s.diff_reply.is_some() {
        diff_modal(frame, s);
    }
}
fn session_navigator(frame: &mut Frame, s: &AppState) {
    let area = center(frame.area(), 70, 16);
    frame.render_widget(Clear, area);
    let mut items = s
        .sessions
        .visible()
        .into_iter()
        .map(|entry| ListItem::new(format!("{}  {}", entry.title, entry.id.as_str())))
        .collect::<Vec<_>>();
    if items.is_empty() {
        items.push(ListItem::new(if s.sessions.query.is_empty() {
            "No sessions for this project"
        } else {
            "No sessions match the search"
        }));
    }
    let item_count = items.len();
    let title = if s.sessions.show_archived {
        " Sessions · archived · ↑↓ select · Enter open · n new chat · Ctrl-S close · search "
    } else {
        " Sessions · ↑↓ select · Enter open · n new chat · Ctrl-S close · Ctrl-A archived · search "
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ratatui::widgets::ListState::default();
    if item_count != 0 {
        state.select(Some(s.sessions.selected.min(item_count - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

/// Render the daemon-backed operations projection. The caller owns loading,
/// error, selected-tab, and acknowledgement state; this widget only reads it.
/// `OperationsState` currently carries the durable portion of that projection.
pub fn render_operations_panel(
    frame: &mut Frame,
    area: Rect,
    state: &OperationsState,
    tab: OperationsTab,
    loading: bool,
    error: Option<&str>,
    acknowledgement: Option<&str>,
) {
    let tab_label = |name, selected| {
        if selected {
            Span::styled(
                format!(" {name} "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw(format!(" {name} "))
        }
    };
    let tabs = Line::from(vec![
        tab_label("Status", tab == OperationsTab::Status),
        tab_label("Git", tab == OperationsTab::Git),
        tab_label("Changes", tab == OperationsTab::Changes),
    ]);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
        .split(area);
    let files = state
        .files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let marker = if file.staged { "●" } else { "○" };
            let selected = if i == state.selected { ">" } else { " " };
            ListItem::new(format!("{selected}{marker} {}", file.path))
        })
        .collect::<Vec<_>>();
    let list = List::new(files).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Operations [{}]", state.branch)),
    );
    frame.render_widget(list, columns[0]);
    let mut body = vec![tabs];
    body.push(Line::from(if loading {
        " Loading: yes"
    } else {
        " Loading: no"
    }));
    body.push(Line::from(match error {
        Some(error) => format!(" Error: {error}"),
        None => " Error: —".to_owned(),
    }));
    if let Some(acknowledgement) = acknowledgement {
        body.push(Line::from(format!(" Acknowledgement: {acknowledgement}")));
    }
    if let Some(content) = &state.content {
        body.push(Line::from(content.path.to_string()));
        if tab == OperationsTab::Status {
            body.push(Line::from("Status: selected project file"));
        }
        if tab == OperationsTab::Changes {
            body.push(Line::from("Changes: full content and diff"));
            body.extend(
                content
                    .content
                    .lines()
                    .map(|line| Line::from(format!("  {line}"))),
            );
        }
        body.push(Line::from(" Actions: [Enter/o] open  [R] refresh"));
        body.push(Line::from(" [s] stage  [u] unstage  [v] revert"));
        body.push(Line::from(" [K] keep  [a] acknowledge"));
        body.push(Line::from(" [b] switch branch  [c] create branch"));
        if tab != OperationsTab::Status {
            body.extend(content.diff.lines().map(Line::from));
        }
        if state
            .acknowledged
            .get(&content.path)
            .copied()
            .unwrap_or(false)
        {
            body.insert(2, Line::from(" acknowledged ✓ "));
        }
    } else {
        body.push(Line::from("Select a file to inspect full content/diff."));
        body.push(Line::from(
            "Actions: [R] refresh  [↑/↓] select  [Enter/o] open",
        ));
        body.push(Line::from("[b] switch branch  [c] create branch"));
    }
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Git / Changes "),
        ),
        columns[1],
    );
}
fn picker(frame: &mut Frame, s: &AppState) {
    let area = center(frame.area(), 60, 14);
    frame.render_widget(Clear, area);
    let title = match s.picker {
        Picker::Models => " Select model (favorites · recents · search) ",
        Picker::Agents => " Select agent ",
        Picker::Commands => " Commands ",
        Picker::None => "",
    };
    let items: Vec<ListItem> = if s.picker == Picker::Models {
        filtered_models(s)
            .into_iter()
            .map(|m| ListItem::new(format!("{} {}", if m.favorite { "★" } else { " " }, m.id)))
            .collect()
    } else if s.picker == Picker::Commands {
        [
            "/agent <name>",
            "/agents",
            "/model <id>",
            "/help",
            "/replay",
        ]
        .iter()
        .filter(|command| s.picker_query.is_empty() || command.contains(&s.picker_query))
        .map(|command| ListItem::new(*command))
        .collect()
    } else {
        s.agents
            .iter()
            .filter(|a| s.picker_query.is_empty() || a.contains(&s.picker_query))
            .map(|a| ListItem::new(a.as_str()))
            .collect()
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(s.picker_index));
    frame.render_stateful_widget(list, area, &mut state);
}
fn permission(frame: &mut Frame, p: &Permission) {
    let area = center(frame.area(), 64, 10);
    frame.render_widget(Clear, area);
    let body = vec![
        Line::from(Span::styled(
            " Permission required ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(" {}: {}", p.tool, p.summary)),
        Line::from(""),
        Line::from("[Enter] once  [→] always  [s] session  [d] deny  [Backspace] reject"),
        Line::from(format!("stage: {:?}", p.stage)),
    ];
    frame.render_widget(
        Paragraph::new(body)
            .block(Block::default().borders(Borders::ALL).title(" Permission "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn question_modal(frame: &mut Frame, question: &Question) {
    let area = center(frame.area(), 64, 9);
    frame.render_widget(Clear, area);
    let body = vec![
        Line::from(Span::styled(
            " Question ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(question.prompt.as_str()),
        Line::from(""),
        Line::from("Type an answer in the prompt and press Enter; Esc rejects."),
    ];
    frame.render_widget(
        Paragraph::new(body)
            .block(Block::default().borders(Borders::ALL).title(" Question "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn diff_modal(frame: &mut Frame, s: &AppState) {
    let area = center(frame.area(), 64, 8);
    frame.render_widget(Clear, area);
    let path = s.diff_path.as_deref().unwrap_or("requested changes");
    let body = vec![
        Line::from(Span::styled(
            " Diff review required ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(" {path}")),
        Line::from(""),
        Line::from("[Enter/y] accept  [Backspace/n] reject"),
    ];
    frame.render_widget(
        Paragraph::new(body)
            .block(Block::default().borders(Borders::ALL).title(" Diff "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub fn tool_card(frame: &mut Frame, area: Rect, t: &ToolCard) {
    let status = match t.status {
        ToolStatus::Running => "…",
        ToolStatus::Complete => "✓",
        ToolStatus::Failed => "!",
        ToolStatus::Denied => "×",
        ToolStatus::Cancelled => "−",
    };
    let mut lines = vec![Line::from(format!(
        "{} {} — {}",
        status,
        t.name,
        t.result.lines().next().unwrap_or("")
    ))];
    if t.expanded {
        lines.push(Line::from(format!("input: {}", t.input)));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}
fn center(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::ProjectAction;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn model_picker_snapshot_contains_favorite() {
        let s = AppState {
            picker: Picker::Models,
            ..AppState::default()
        };
        let mut t = Terminal::new(TestBackend::new(120, 24)).unwrap();
        t.draw(|f| render(f, &s)).unwrap();
        let x = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(x.contains("Projects") && x.contains("Conversation"));
        assert!(x.contains("Select model") && x.contains("gpt-4o"));
    }

    #[test]
    fn production_render_keeps_prompt_overlay_and_cursor() {
        let s = AppState {
            input: "hello".into(),
            cursor: 5,
            permission: Some(Permission {
                tool: "shell".into(),
                summary: "run command".into(),
                choice: PermissionChoice::AllowOnce,
                stage: PermissionStage::Choose,
            }),
            ..AppState::default()
        };
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| render(f, &s)).unwrap();
        let x = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(x.contains("Projects") && x.contains("Conversation"));
        assert!(x.contains("Permission required") && x.contains("run command"));
        // The prompt is rendered inside the daemon-backed content column.  Keep
        // this assertion tied to the current layout rather than the old
        // pre-project-shell coordinates.
        assert_eq!(t.get_cursor_position().unwrap(), (35, 25).into());
    }

    #[test]
    fn production_project_render_uses_typed_selection() {
        let mut projects = ProjectState::default();
        projects
            .apply(ProjectAction::Register {
                name: "demo".into(),
                root: "/tmp/demo".into(),
            })
            .unwrap();
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        t.draw(|f| render_with_projects(f, &AppState::default(), &projects))
            .unwrap();
        let x = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(x.contains("demo") && x.contains("Selected project"));
    }
}
