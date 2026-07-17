//! Minimal power-user terminal client. Unlike the GUI, it never auto-starts tau.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tau_client::CompletionEvent;
use tau_proto::prelude::CompletionStreamParams;
use tokio::time::sleep;

pub async fn run(socket: PathBuf) -> Result<()> {
    let mut client = tau_client::Client::connect(&socket)
        .await
        .with_context(|| {
            format!(
                "daemon unavailable at {}\nhelp: run `tau serve` first",
                socket.display()
            )
        })?;
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = session(&mut terminal, &mut client).await;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn session(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    client: &mut tau_client::Client,
) -> Result<()> {
    let mut input = String::new();
    let mut transcript = String::new();
    let mut session_id = None;
    loop {
        draw(terminal, &input, &transcript)?;
        if !event::poll(Duration::from_millis(50))? {
            sleep(Duration::from_millis(10)).await;
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Esc => break,
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Enter if !input.trim().is_empty() => {
                transcript.push_str(&format!("\nYou: {input}\ntau: "));
                let prompt = std::mem::take(&mut input);
                let params = CompletionStreamParams {
                    model: "openai/gpt-4o".into(),
                    prompt,
                    session_id: session_id.clone(),
                    cwd: Some(std::env::current_dir()?.to_string_lossy().into_owned()),
                };
                let mut stream = client.completion_stream(params).await?;
                while let Some(event) = stream.next().await {
                    match event? {
                        CompletionEvent::Delta(delta) => transcript.push_str(&delta.text),
                        CompletionEvent::Complete(result) => session_id = Some(result.session_id),
                    }
                    draw(terminal, &input, &transcript)?;
                }
                transcript.push('\n');
            }
            KeyCode::Char(character) => input.push(character),
            _ => {}
        }
    }
    Ok(())
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    input: &str,
    transcript: &str,
) -> Result<()> {
    terminal.draw(|frame| {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(frame.area());
        frame.render_widget(
            Paragraph::new(transcript)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" tau ")),
            areas[0],
        );
        frame.render_widget(
            Paragraph::new(input).block(Block::default().borders(Borders::ALL).title(" prompt ")),
            areas[1],
        );
    })?;
    Ok(())
}
