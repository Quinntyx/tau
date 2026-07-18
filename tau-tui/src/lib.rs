//! Functional, reducer-driven terminal client.  The terminal shell is deliberately
//! thin; all interesting behaviour lives in typed state and renderable components.
mod adapter;
pub mod components;
pub mod projects;
pub mod reducer;
pub mod sessions;
pub mod shell;
pub mod state;

use crate::projects::{ProjectAction, ProjectState};
use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io, path::PathBuf, time::Duration};
use tau_client::Client;
use tau_proto::prelude::{
    Capability, ClientResponse, ProtocolNegotiateParams, ProtocolVersion, QuestionAnswer,
    TurnPermissionChoice,
};

pub use adapter::{ClientEvent, ScriptedClient};
pub use reducer::{Action, apply as reduce};
pub use state::AppState;

/// Start the client without ever starting a daemon for the user.
pub async fn run(socket: PathBuf) -> Result<()> {
    let client = connect_to_daemon(&socket).await.with_context(|| {
        format!(
            "daemon unavailable at {}\nhelp: to fix this, run `tau serve` before running the tui",
            socket.display()
        )
    })?;
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = session(&mut terminal, client, socket).await;
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

async fn connect_to_daemon(socket: &PathBuf) -> Result<Client> {
    let client = Client::connect(socket).await?;
    let report = client
        .negotiate_checked(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![
                Capability::TurnStreaming,
                Capability::TurnCancellation,
                Capability::EventReplay,
                Capability::Idempotency,
            ],
        })
        .await?;
    anyhow::ensure!(
        report.is_compatible(),
        "daemon protocol incompatible; missing capabilities: {:?}",
        report.missing_capabilities
    );
    Ok(client)
}

async fn session(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut client: Client,
    socket: PathBuf,
) -> Result<()> {
    let mut state = AppState::default();
    // Projects are a client-local projection.  Keep their lifecycle entirely
    // on this side of the daemon adapter: turns continue to use AppState and
    // no project wire protocol is implied by the shell.
    let mut projects = ProjectState::default();
    initialize_projects(&mut projects);
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut event_task = tokio::spawn(adapter::persistent_events(
        client.clone(),
        events_tx.clone(),
    ));
    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel();
    let input_task = tokio::spawn(async move {
        loop {
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                match event::read() {
                    Ok(input) => {
                        if input_tx.send(input).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            } else {
                tokio::task::yield_now().await;
            }
        }
    });
    loop {
        terminal.draw(|frame| components::render_with_projects(frame, &state, &projects))?;
        tokio::select! {
            Some(input) = input_rx.recv() => {
                let action = match input {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // Esc closes the navigator through `key_action`; only
                        // exit the application when no modal navigator is open.
                        if matches!(key.code, KeyCode::Esc) && !state.sessions.open { break; }
                        reducer::key_action(&state, key)
                    }
                    Event::Mouse(mouse) => reducer::mouse_action(&state, mouse),
                    Event::Resize(_, _) => Some(reducer::Action::Reconnect),
                    _ => None,
                };
                if let Some(action) = action {
                    let is_cancel = matches!(action, reducer::Action::Cancel);
                    let is_submit = matches!(action, reducer::Action::Submit);
                    let reconnect_requested = matches!(action, reducer::Action::Reconnect);
                    let replay_requested = matches!(action, reducer::Action::Replay);
                    let reconnect = reconnect_requested
                        || (replay_requested && state.connection == state::Connection::Disconnected);
                    let response = match &action {
                        reducer::Action::Permission(choice)
                        | reducer::Action::PermissionReply(choice) => {
                            Some(ClientResponse::Permission {
                                request_id: state.permission_request_id.clone().unwrap_or_default(),
                                choice: permission_choice(*choice),
                            })
                        }
                        reducer::Action::QuestionReply(answer) => {
                            Some(ClientResponse::Question {
                                question_id: state.question_id.clone().unwrap_or_default(),
                                answer: QuestionAnswer(answer.clone()),
                            })
                        }
                        reducer::Action::DiffReply(approved) => Some(ClientResponse::DiffHunk {
                            request_id: state.diff_request_id.clone().unwrap_or_default(),
                            path: state.diff_path.clone().unwrap_or_default(),
                            index: state.hunk_index as u32,
                            approved: *approved,
                        }),
                        _ => None,
                    };
                    if let Some(prompt) = reducer::apply(&mut state, action) {
                        if is_submit {
                            adapter::complete(&mut state, &client, prompt, events_tx.clone()).await?;
                        }
                    }
                    if let Some(response) = response {
                        let snapshot = state.clone();
                        let c = client.clone();
                        let tx = events_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = adapter::respond(&snapshot, &c, response).await {
                                let _ = tx.send(adapter::ClientEvent::Error(e.to_string()));
                            }
                        });
                    }
                    // These requests are independent of the active turn stream.
                    // In particular, Ctrl-C is not queued behind generation.
                    if is_cancel {
                        let c = client.clone();
                        let mut snapshot = state.clone();
                        let tx = events_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = adapter::cancel(&mut snapshot, &c).await { let _ = tx.send(adapter::ClientEvent::Error(e.to_string())); }
                        });
                    }
                    if reconnect {
                        event_task.abort();
                        match connect_to_daemon(&socket).await {
                            Ok(new_client) => {
                                client = new_client;
                                event_task = tokio::spawn(adapter::persistent_events(
                                    client.clone(),
                                    events_tx.clone(),
                                ));
                                state.connection = state::Connection::Reconnecting;
                            }
                            Err(error) => {
                                state.connection = state::Connection::Disconnected;
                                let _ = events_tx.send(adapter::ClientEvent::Error(
                                    format!("reconnect failed: {error}"),
                                ));
                            }
                        }
                    }
                    if replay_requested || reconnect {
                        if let Some(session_id) = state.session_id.clone() {
                            tokio::spawn(adapter::replay_task(
                                client.clone(),
                                session_id,
                                state.sequence,
                                events_tx.clone(),
                            ));
                        }
                    }
                }
            }
            Some(message) = events_rx.recv() => {
                match message {
                    adapter::ClientEvent::Complete { session_id, turn_id } => {
                        state.session_id = Some(session_id);
                        state.turn_id = Some(turn_id);
                    }
                    adapter::ClientEvent::Turn(event) => adapter::reduce_event(&mut state, event),
                    adapter::ClientEvent::Disconnected => state.connection = state::Connection::Disconnected,
                    other => adapter::ScriptedClient { events: vec![other] }.drive(&mut state),
                }
            }
            else => break,
        }
    }
    input_task.abort();
    event_task.abort();
    Ok(())
}

/// Seed and restore the local project projection for a new client session.
/// The working directory is deliberately only a UI-local registration; it is
/// not sent to the daemon as a protocol request.
fn initialize_projects(projects: &mut ProjectState) {
    if let Ok(root) = std::env::current_dir() {
        let root = root.to_string_lossy().into_owned();
        let name = root
            .rsplit(std::path::MAIN_SEPARATOR)
            .find(|part| !part.is_empty())
            .unwrap_or("project")
            .to_owned();
        let _ = apply_project_action(projects, ProjectAction::Register { name, root });
    }
    let saved = projects.selected.clone();
    projects.restore_active(saved.as_ref());
}

/// Narrow adapter seam for project intents.  Project actions remain typed and
/// client-local until a future adapter explicitly gains server support.
/// Dispatch a typed project intent through the client-local adapter seam.
/// S1 can replace this function's implementation with acknowledged service
/// calls without changing the Ratatui shell or reducer.
pub fn apply_project_action(
    projects: &mut ProjectState,
    action: ProjectAction,
) -> Result<(), projects::ProjectError> {
    projects.apply(action)
}

fn permission_choice(choice: state::PermissionChoice) -> TurnPermissionChoice {
    match choice {
        state::PermissionChoice::AllowOnce => TurnPermissionChoice::AllowOnce,
        state::PermissionChoice::AllowAlways => TurnPermissionChoice::AllowAlways,
        state::PermissionChoice::AllowSession => TurnPermissionChoice::AllowSession,
        state::PermissionChoice::DenyOnce => TurnPermissionChoice::DenyOnce,
        state::PermissionChoice::Reject => TurnPermissionChoice::Reject,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn initial_buffer_has_client_chrome() {
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut projects = ProjectState::default();
        initialize_projects(&mut projects);
        t.draw(|f| shell::render_with_projects(f, &AppState::default(), &projects))
            .unwrap();
        let text: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("tau") && text.contains("Conversation"));
    }
}
