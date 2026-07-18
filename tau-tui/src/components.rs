//! Small composable widgets. Keeping these pure makes ratatui snapshots cheap.
use crate::operations::OperationsState;
use crate::{reducer::filtered_models, state::*};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

pub fn render(frame: &mut Frame, s: &AppState) {
    let root = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(38)])
        .split(frame.area());
    let shell = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(5)])
        .split(root[0]);
    let mut transcript = s.transcript.join("\n");
    for tool in &s.tools {
        transcript.push('\n');
        transcript.push_str(&format_tool(tool));
    }
    if !s.hunks.is_empty() {
        transcript.push_str("\n\n");
        transcript.push_str(&format_diff(s));
    }
    frame.render_widget(
        Paragraph::new(transcript)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" tau ")),
        shell[0],
    );
    let footer = Line::from(vec![
        Span::styled(format!(" {} ", s.model), Style::default().fg(Color::Cyan)),
        Span::raw(format!(
            "  agent:{} tier:{} {}",
            s.agent,
            s.task_tier,
            if s.autonomous { "AUTO" } else { "ASK" }
        )),
    ]);
    frame.render_widget(
        Paragraph::new(Text::from(vec![footer, Line::from(s.input.as_str())])).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" prompt (Shift+Enter newline) "),
        ),
        shell[1],
    );
    // Keep the terminal cursor at the UTF-8 byte cursor's visual column so
    // multiline editing remains usable instead of merely storing text.
    let line_start = s.input[..s.cursor].rfind('\n').map_or(0, |i| i + 1);
    let line = s.input[..s.cursor].matches('\n').count() as u16;
    let column = s.input[line_start..s.cursor].chars().count() as u16;
    frame.set_cursor_position((shell[1].x + 1 + column, shell[1].y + 2 + line));
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
    if let Some(p) = &s.permission {
        permission(frame, p);
    }
    if let Some(question) = &s.question {
        question_modal(frame, question);
    }
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
    frame.render_widget(list, area);
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

fn format_tool(t: &ToolCard) -> String {
    let status = match t.status {
        ToolStatus::Running => "…",
        ToolStatus::Complete => "✓",
        ToolStatus::Failed => "!",
        ToolStatus::Denied => "×",
        ToolStatus::Cancelled => "−",
    };
    if t.expanded {
        format!(
            "{status} {}\n  input: {}\n  output: {}",
            t.name, t.input, t.result
        )
    } else {
        format!(
            "{status} {} — {}",
            t.name,
            t.result.lines().next().unwrap_or("")
        )
    }
}

fn format_diff(s: &AppState) -> String {
    let mut out = format!(
        "DIFF REVIEW  hunk {}/{}  [Enter] accept [Backspace] reject [u/U] undo/redo [Ctrl-A] file\n",
        s.hunk_index + 1,
        s.hunks.len()
    );
    if let Some(h) = s.hunks.get(s.hunk_index) {
        out.push_str(&format!("{}\n", h.header));
        for line in &h.lines {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("--- split ---\n");
        for line in &h.before {
            out.push_str(&format!("< {line}\n"));
        }
        for line in &h.after {
            out.push_str(&format!("> {line}\n"));
        }
    }
    out
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
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn model_picker_snapshot_contains_favorite() {
        let s = AppState {
            picker: Picker::Models,
            ..AppState::default()
        };
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| render(f, &s)).unwrap();
        let x = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(x.contains("Select model") && x.contains("gpt-4o"));
    }
}
