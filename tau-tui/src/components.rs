//! Small composable widgets. Keeping these pure makes ratatui snapshots cheap.
use crate::{projects::ProjectState, reducer::filtered_models, shell, state::*};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
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
    /*
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(5)])
        .split(frame.area());
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
        root[0],
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
        root[1],
    );
    // Keep the terminal cursor at the UTF-8 byte cursor's visual column so
    // multiline editing remains usable instead of merely storing text.
    let line_start = s.input[..s.cursor].rfind('\n').map_or(0, |i| i + 1);
    let line = s.input[..s.cursor].matches('\n').count() as u16;
    let column = s.input[line_start..s.cursor].chars().count() as u16;
    */
    // Keep the terminal cursor at the UTF-8 byte cursor's visual column.
    let line_start = s.input[..s.cursor].rfind('\n').map_or(0, |i| i + 1);
    let line = s.input[..s.cursor].matches('\n').count() as u16;
    let column = s.input[line_start..s.cursor].chars().count() as u16;
    frame.set_cursor_position((
        content.x + 3 + column,
        content.y + content.height.saturating_sub(2) + line,
    ));
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
        assert_eq!(t.get_cursor_position().unwrap(), (37, 28).into());
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
