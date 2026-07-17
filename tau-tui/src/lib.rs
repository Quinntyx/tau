//! Functional, reducer-driven terminal client.  The terminal shell is deliberately
//! thin; all interesting behaviour lives in typed state and renderable components.
mod adapter;
pub mod components;
pub mod reducer;
pub mod state;

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io, path::PathBuf, time::Duration};
use tau_client::Client;

pub use adapter::{ClientEvent, ScriptedClient};
pub use reducer::{Action, apply as reduce};
pub use state::AppState;

/// Start the client without ever starting a daemon for the user.
pub async fn run(socket: PathBuf) -> Result<()> {
    let mut client = Client::connect(&socket).await.with_context(|| {
        format!(
            "daemon unavailable at {}\nhelp: run `tau serve` first",
            socket.display()
        )
    })?;
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = session(&mut terminal, &mut client).await;
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn session(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: &mut Client,
) -> Result<()> {
    let mut state = AppState::default();
    loop {
        terminal.draw(|frame| components::render(frame, &state))?;
        if !event::poll(Duration::from_millis(50))? {
            tokio::task::yield_now().await;
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if matches!(key.code, KeyCode::Esc) {
            break;
        }
        if let Some(action) = reducer::key_action(&state, key) {
            if let Some(prompt) = reducer::apply(&mut state, action) {
                adapter::complete(&mut state, client, prompt).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn initial_buffer_has_client_chrome() {
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| components::render(f, &AppState::default()))
            .unwrap();
        let text: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("tau") && text.contains("prompt"));
    }
}
