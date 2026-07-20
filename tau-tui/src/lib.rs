//! Functional, reducer-driven terminal client.  The terminal shell is deliberately
//! thin; all interesting behaviour lives in typed state and renderable components.
mod adapter;
pub mod components;
pub mod composer;
pub mod feed;
pub mod operations;
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
use tau_client::{Client, CreateSession, ProjectId as ClientProjectId, SessionRef, SessionSummary};
use tau_proto::prelude::{
    Capability, ClientResponse, ProjectCreateParams, ProjectIdParams, ProjectListParams,
    ProjectNewIdParams, ProjectRenameParams, ProtocolNegotiateParams, ProtocolVersion,
    QuestionAnswer, TurnPermissionChoice,
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
    let mut state = AppState {
        project_root: std::env::current_dir()?.to_string_lossy().into_owned(),
        ..AppState::default()
    };
    state.operations.project = state.project_root.clone();
    // Project records are a projection of daemon state.  The daemon, never the
    // client cwd, is authoritative for IDs and lifecycle.
    let mut projects = ProjectState::default();
    initialize_projects(&client, &mut projects, &mut state).await?;
    if state.project_id.is_some() {
        handle_operations(&mut state, &client, reducer::Action::OperationsRefresh).await;
    }
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
                        if matches!(key.code, KeyCode::Esc)
                            && !state.sessions.open
                            && state.picker == state::Picker::None
                        {
                            break;
                        }
                        reducer::key_action(&state, key)
                    }
                    Event::Paste(text) => Some(reducer::Action::Paste(text)),
                    Event::Mouse(mouse) => reducer::mouse_action(&state, mouse),
                    Event::Resize(_, _) => Some(reducer::Action::Reconnect),
                    _ => None,
                };
                    if let Some(action) = action {
                        let operation_action = action.clone();
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
                    // Project/session mutations are daemon-owned.  Keep the reducer as
                    // the immediate UI projection, but issue the typed RPC alongside it
                    // and only surface failures through the normal event channel.
                    let rpc_action = action.clone();
                    let refresh_operations = matches!(
                        &rpc_action,
                        reducer::Action::ProjectSelect(_)
                            | reducer::Action::ProjectSelectWithRoot { .. }
                            | reducer::Action::ProjectSelectCurrent
                            | reducer::Action::ProjectCreateCurrent
                            | reducer::Action::ProjectReactivateCurrent
                            | reducer::Action::ProjectNewIdCurrent
                            | reducer::Action::ProjectChooseInactive(_)
                    );
                    let prompt = reducer::apply(&mut state, action);
                    dispatch_navigation_rpc(
                        &client,
                        &mut state,
                        &mut projects,
                        rpc_action,
                        events_tx.clone(),
                    );
                     if let Some(prompt) = prompt {
                         if is_submit {
                             adapter::complete(&mut state, &client, prompt, events_tx.clone()).await?;
                         }
                     }
                     handle_operations(&mut state, &client, operation_action).await;
                     if refresh_operations && state.project_id.is_some() {
                         state.operations_error = None;
                         handle_operations(&mut state, &client, reducer::Action::OperationsRefresh).await;
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
                    adapter::ClientEvent::ProjectsLoaded(records) => {
                        projects.replace_projects(records);
                        // Every refresh is authoritative.  Do not retain a
                        // locally selected project that the daemon removed or
                        // made inactive, and update the root used by turns and
                        // operations when the daemon's record changes.
                        let selected = state
                            .project_id
                            .as_ref()
                            .map(|id| crate::projects::ProjectId(id.clone()))
                            .or_else(|| projects.selected.clone());
                        if let Some(id) = selected {
                            if let Some(project) = projects.projects.get(&id).filter(|p| p.active) {
                                projects.selected = Some(id.clone());
                                activate_project(&mut state, project);
                            } else {
                                projects.selected = None;
                                let root = std::env::current_dir()
                                    .map(|path| path.to_string_lossy().into_owned())
                                    .unwrap_or_else(|_| state.project_root.clone());
                                state.set_project(None, root.clone());
                                state.operations.project = root;
                                state.sessions.project = None;
                                state.session_id = None;
                                state.project_focus = true;
                            }
                        }
                    }
                    adapter::ClientEvent::ProjectReady(id) => {
                        if let Some(project) = projects
                            .projects
                            .get(&crate::projects::ProjectId(id.clone()))
                            .cloned()
                        {
                            activate_project(&mut state, &project);
                            request_sessions(&client, &state, events_tx.clone());
                            state.project_error = None;
                            persist_selection(&state);
                            handle_operations(
                                &mut state,
                                &client,
                                reducer::Action::OperationsRefresh,
                            )
                            .await;
                        }
                    }
                    adapter::ClientEvent::Complete { session_id, turn_id } => {
                        state.session_id = Some(session_id);
                        let _ = turn_id;
                        state.turn_id = None;
                        // A completed stream is terminal from the composer’s
                        // perspective, even if the daemon did not emit a
                        // separate terminal event before closing it.
                        state.cancelling = false;
                        let _ = state
                            .composer
                            .apply(crate::composer::ComposerAction::SetSending(false));
                        reducer::sync_composer(&mut state);
                    }
                    adapter::ClientEvent::Turn(event) => {
                        let finished = matches!(
                            &event.event,
                            tau_proto::prelude::TurnEvent::TurnCompleted { .. }
                                | tau_proto::prelude::TurnEvent::TurnCancelled { .. }
                                | tau_proto::prelude::TurnEvent::TurnFailed { .. }
                        );
                        adapter::reduce_event(&mut state, event);
                        if finished {
                            let _ = state
                                .composer
                                .apply(crate::composer::ComposerAction::SetSending(false));
                        }
                        reducer::sync_composer(&mut state);
                    }
                    event @ adapter::ClientEvent::SessionHistory(_) |
                    event @ adapter::ClientEvent::SessionsLoaded { .. } |
                    event @ adapter::ClientEvent::SessionCreated(_) => {
                        let failed = false;
                        adapter::ScriptedClient { events: vec![event] }.drive(&mut state);
                        if failed { state.cancelling = false; }
                        reducer::sync_composer(&mut state);
                        persist_selection(&state);
                    }
                    adapter::ClientEvent::Disconnected => {
                        state.connection = state::Connection::Disconnected;
                        state.cancelling = false;
                        let _ = state
                            .composer
                            .apply(crate::composer::ComposerAction::SetSending(false));
                        reducer::sync_composer(&mut state);
                    }
                    other => {
                        let failed = matches!(&other, adapter::ClientEvent::Error(_));
                        adapter::ScriptedClient { events: vec![other] }.drive(&mut state);
                        if failed {
                            state.cancelling = false;
                            let _ = state
                                .composer
                                .apply(crate::composer::ComposerAction::SetSending(false));
                        }
                        reducer::sync_composer(&mut state);
                    }
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
async fn initialize_projects(
    client: &Client,
    projects: &mut ProjectState,
    state: &mut AppState,
) -> Result<()> {
    let result = client
        .project_list(ProjectListParams {
            include_inactive: Some(true),
        })
        .await?;
    projects.replace_projects(result.projects);
    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    let saved = load_selection().unwrap_or_else(|| state.last_selection());
    let saved_project = saved
        .project_id
        .as_ref()
        .map(|id| crate::projects::ProjectId(id.clone()));
    if let Some(project) = projects
        .restore_selection(saved_project.as_ref(), Some(&cwd))
        .cloned()
    {
        state.session_id = saved.session_id.clone();
        activate_project(state, &project);
        state.sessions.project = Some(crate::sessions::ProjectId::new(project.id.0.clone()));
        let sessions = client
            .session_list(ClientProjectId::new(project.id.0.clone()))
            .await?;
        install_sessions(state, sessions);
        if let Some(session_id) = saved.session_id {
            if state
                .sessions
                .sessions
                .iter()
                .any(|s| s.id.as_str() == session_id)
            {
                let history = client
                    .session_history(SessionRef {
                        project_id: ClientProjectId::new(project.id.0.clone()),
                        session_id: tau_client::SessionId::new(session_id),
                    })
                    .await?;
                adapter::ScriptedClient {
                    events: vec![adapter::ClientEvent::SessionHistory(history)],
                }
                .drive(state);
            } else {
                state.session_id = None;
            }
        }
    } else if let Some(project) = projects.cwd_record(&cwd) {
        if !project.active {
            projects.inactive_conflict = Some(crate::projects::InactiveConflict {
                project: project.id.clone(),
                name: project.name.clone(),
                root: project.root.clone(),
            });
        }
        state.project_focus = true;
    } else {
        // The left panel explicitly offers create/reactivate/new-ID actions;
        // no local project record is created here.
        state.project_focus = true;
    }
    state.operations.project = state.project_root.clone();
    Ok(())
}

fn activate_project(state: &mut AppState, project: &crate::projects::Project) {
    state.set_project(Some(project.id.0.clone()), project.root.clone());
    state.operations.project = project.root.clone();
    state.operations.branch.clear();
    state.operations.files.clear();
    state.operations.branches.clear();
    state.operations.content = None;
    state.operations.selected = 0;
    state.operations_error = None;
    state.operations_ack = None;
    state.sessions.project = Some(crate::sessions::ProjectId::new(project.id.0.clone()));
    state.project_focus = false;
}

fn install_sessions(state: &mut AppState, sessions: Vec<SessionSummary>) {
    state.sessions.sessions = sessions
        .iter()
        .map(crate::sessions::entry_from_summary)
        .collect();
    state
        .sessions
        .restart_selection(state.session_id.as_deref());
}

fn request_sessions(
    client: &Client,
    state: &AppState,
    tx: tokio::sync::mpsc::UnboundedSender<adapter::ClientEvent>,
) {
    let Some(project_id) = state.project_id.clone() else {
        return;
    };
    let client = client.clone();
    tokio::spawn(async move {
        match client
            .session_list(ClientProjectId::new(project_id.clone()))
            .await
        {
            Ok(sessions) => {
                let _ = tx.send(adapter::ClientEvent::SessionsLoaded {
                    project_id,
                    sessions,
                });
            }
            Err(error) => {
                let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
            }
        }
    });
}

fn persist_selection(state: &AppState) {
    let Some(path) = selection_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let selection = state.last_selection();
    let value = selection.to_json();
    if let Ok(value) = serde_json::to_vec_pretty(&value) {
        let _ = std::fs::write(path, value);
    }
}

fn load_selection() -> Option<state::LastSelection> {
    let path = selection_path()?;
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    state::LastSelection::from_json(&value)
}

fn selection_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("TAU_TUI_STATE_PATH") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/tau/tui-selection.json"))
}

fn dispatch_navigation_rpc(
    client: &Client,
    state: &mut AppState,
    projects: &mut ProjectState,
    action: reducer::Action,
    tx: tokio::sync::mpsc::UnboundedSender<adapter::ClientEvent>,
) {
    use reducer::Action;
    match action {
        Action::FocusProjects => {
            state.project_focus = !state.project_focus;
        }
        Action::ProjectMove(delta) => {
            let ids = projects.ordered_ids();
            if !ids.is_empty() {
                let current = state.project_index.min(ids.len() - 1);
                state.project_index = if delta < 0 {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(ids.len() - 1)
                };
                projects.selected = Some(ids[state.project_index].clone());
            }
        }
        Action::ProjectSelectCurrent => {
            let id = projects.ordered_ids().get(state.project_index).cloned();
            if let Some(id) = id {
                if projects.projects.get(&id).is_some_and(|p| p.active) {
                    select_project(client, state, projects, id, tx);
                } else if let Some(project) = projects.projects.get(&id) {
                    projects.inactive_conflict = Some(crate::projects::InactiveConflict {
                        project: id,
                        name: project.name.clone(),
                        root: project.root.clone(),
                    });
                }
            } else {
                create_current_project(client, state, tx);
            }
        }
        Action::ProjectSelect(id) => {
            select_project(client, state, projects, id, tx);
        }
        Action::ProjectSelectWithRoot { id, root } => {
            // The daemon record remains authoritative, but retain the root
            // carried by the typed intent when selecting an already-known
            // active record.  This makes selection synchronous for callers
            // that already have the acknowledged project payload.
            if let Some(project) = projects.projects.get(&id).cloned() {
                if project.active {
                    let mut project = project;
                    project.root = root;
                    state.session_id = None;
                    projects.selected = Some(id);
                    activate_project(state, &project);
                    state.project_error = None;
                    request_sessions(client, state, tx);
                    persist_selection(state);
                }
            }
        }
        Action::ProjectCreateCurrent => create_current_project(client, state, tx),
        Action::ProjectCreate { name, root } => create_project(client, name, root, tx),
        Action::ProjectReactivate(id) => {
            mutate_project(client, id, ProjectMutation::Reactivate, tx)
        }
        Action::ProjectChooseInactive(choice) => {
            let conflict = projects.inactive_conflict.take();
            match choice {
                crate::projects::InactiveConflictChoice::Reactivate => {
                    if let Some(conflict) = conflict {
                        mutate_project(client, conflict.project, ProjectMutation::Reactivate, tx);
                    }
                }
                crate::projects::InactiveConflictChoice::CreateNew => {
                    let root = conflict
                        .map(|conflict| conflict.root)
                        .unwrap_or_else(|| state.project_root.clone());
                    let name = std::path::Path::new(&root)
                        .file_name()
                        .and_then(|part| part.to_str())
                        .filter(|name| !name.is_empty())
                        .unwrap_or("project")
                        .to_owned();
                    create_project(client, name, root, tx);
                }
                crate::projects::InactiveConflictChoice::Cancel => {}
            }
        }
        Action::ProjectRename { id, name } => {
            let client = client.clone();
            tokio::spawn(async move {
                match client
                    .project_rename(ProjectRenameParams {
                        project_id: id.0,
                        name,
                    })
                    .await
                {
                    Ok(result) => {
                        let refreshed = client
                            .project_list(ProjectListParams {
                                include_inactive: Some(true),
                            })
                            .await;
                        match refreshed {
                            Ok(list) => {
                                let _ =
                                    tx.send(adapter::ClientEvent::ProjectsLoaded(list.projects));
                                let _ =
                                    tx.send(adapter::ClientEvent::ProjectReady(result.project.id));
                            }
                            Err(error) => {
                                let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
                            }
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
                    }
                }
            });
        }
        Action::ProjectReactivateCurrent => {
            let id = projects.selected.clone().or_else(|| {
                projects
                    .cwd_record(&state.project_root)
                    .map(|p| p.id.clone())
            });
            if let Some(id) = id {
                mutate_project(client, id, ProjectMutation::Reactivate, tx);
            }
        }
        Action::ProjectNewIdCurrent => {
            let id = projects.selected.clone().or_else(|| {
                projects
                    .cwd_record(&state.project_root)
                    .map(|p| p.id.clone())
            });
            if let Some(id) = id {
                let c = client.clone();
                tokio::spawn(async move {
                    match c
                        .project_new_id(ProjectNewIdParams {
                            project_id: Some(id.0.clone()),
                        })
                        .await
                    {
                        Ok(result) => {
                            if let Ok(list) = c
                                .project_list(ProjectListParams {
                                    include_inactive: Some(true),
                                })
                                .await
                            {
                                let _ =
                                    tx.send(adapter::ClientEvent::ProjectsLoaded(list.projects));
                                let _ =
                                    tx.send(adapter::ClientEvent::ProjectReady(result.project_id));
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
                        }
                    }
                });
            }
        }
        Action::NewChat => {
            if let Some(pid) = state.project_id.clone() {
                let c = client.clone();
                let cwd = state.project_root.clone();
                let tx2 = tx.clone();
                tokio::spawn(async move {
                    match c
                        .session_create(CreateSession {
                            project_id: ClientProjectId::new(pid),
                            cwd,
                        })
                        .await
                    {
                        Ok(session) => {
                            let _ = tx2.send(adapter::ClientEvent::SessionCreated(session));
                        }
                        Err(error) => {
                            let _ = tx2.send(adapter::ClientEvent::Error(error.to_string()));
                        }
                    }
                });
            }
        }
        Action::SessionSelect => {
            if let (Some(pid), Some(sid)) = (
                state.project_id.clone(),
                state
                    .sessions
                    .selected_id()
                    .map(|id| id.as_str().to_owned()),
            ) {
                let c = client.clone();
                tokio::spawn(async move {
                    match c
                        .session_history(SessionRef {
                            project_id: ClientProjectId::new(pid),
                            session_id: tau_client::SessionId::new(sid),
                        })
                        .await
                    {
                        Ok(history) => {
                            let _ = tx.send(adapter::ClientEvent::SessionHistory(history));
                        }
                        Err(error) => {
                            let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
                        }
                    }
                });
            }
        }
        Action::SessionRename(title) => {
            if let (Some(pid), Some(sid)) = (
                state.project_id.clone(),
                state.sessions.selected_id().map(|s| s.as_str().to_owned()),
            ) {
                let c = client.clone();
                tokio::spawn(async move {
                    let result = c
                        .session_rename(tau_client::SessionRename {
                            project_id: ClientProjectId::new(pid.clone()),
                            session_id: tau_client::SessionId::new(sid),
                            title,
                        })
                        .await;
                    match result {
                        Ok(_) => match c
                            .session_list_with_archived(ClientProjectId::new(pid.clone()), true)
                            .await
                        {
                            Ok(sessions) => {
                                let _ = tx.send(adapter::ClientEvent::SessionsLoaded {
                                    project_id: pid,
                                    sessions,
                                });
                            }
                            Err(error) => {
                                let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
                            }
                        },
                        Err(error) => {
                            let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
                        }
                    }
                });
            }
        }
        Action::SessionArchive | Action::SessionRestore => {
            if let (Some(pid), Some(sid)) = (
                state.project_id.clone(),
                state.sessions.selected_id().map(|s| s.as_str().to_owned()),
            ) {
                let c = client.clone();
                let tx2 = tx.clone();
                let archive = matches!(action, Action::SessionArchive);
                tokio::spawn(async move {
                    let project_id = pid.clone();
                    let r = SessionRef {
                        project_id: ClientProjectId::new(pid),
                        session_id: tau_client::SessionId::new(sid),
                    };
                    let result = if archive {
                        c.session_archive(r.clone()).await.map(|_| ())
                    } else {
                        c.session_restore(r).await.map(|_| ())
                    };
                    match result {
                        Ok(_) => match c
                            .session_list_with_archived(
                                ClientProjectId::new(project_id.clone()),
                                true,
                            )
                            .await
                        {
                            Ok(sessions) => {
                                let _ = tx2.send(adapter::ClientEvent::SessionsLoaded {
                                    project_id,
                                    sessions,
                                });
                            }
                            Err(error) => {
                                let _ = tx2.send(adapter::ClientEvent::Error(error.to_string()));
                            }
                        },
                        Err(error) => {
                            let _ = tx2.send(adapter::ClientEvent::Error(error.to_string()));
                        }
                    }
                });
            }
        }
        _ => {}
    }
}

fn select_project(
    _client: &Client,
    state: &mut AppState,
    projects: &mut ProjectState,
    id: crate::projects::ProjectId,
    tx: tokio::sync::mpsc::UnboundedSender<adapter::ClientEvent>,
) {
    let Some(project) = projects.projects.get(&id).cloned() else {
        return;
    };
    if !project.active {
        return;
    }
    state.session_id = None;
    state.turn_id = None;
    state.sequence = 0;
    state.raw_events.clear();
    state.transcript = vec!["New chat".into()];
    state.human_messages.clear();
    state.tools.clear();
    state.tool_call_ids.clear();
    state.assistant_index = None;
    state.sessions.sessions.clear();
    projects.selected = Some(id.clone());
    activate_project(state, &project);
    state.project_error = None;
    request_sessions(_client, state, tx);
    persist_selection(state);
}

fn create_current_project(
    client: &Client,
    state: &mut AppState,
    tx: tokio::sync::mpsc::UnboundedSender<adapter::ClientEvent>,
) {
    let root = state.project_root.clone();
    let name = std::path::Path::new(&root)
        .file_name()
        .and_then(|part| part.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project")
        .to_owned();
    create_project(client, name, root, tx);
}

fn create_project(
    client: &Client,
    name: String,
    root: String,
    tx: tokio::sync::mpsc::UnboundedSender<adapter::ClientEvent>,
) {
    let client = client.clone();
    tokio::spawn(async move {
        match client
            .project_create(ProjectCreateParams { name, root })
            .await
        {
            Ok(result) => {
                if let Ok(list) = client
                    .project_list(ProjectListParams {
                        include_inactive: Some(true),
                    })
                    .await
                {
                    let _ = tx.send(adapter::ClientEvent::ProjectsLoaded(list.projects));
                    let _ = tx.send(adapter::ClientEvent::ProjectReady(result.project.id));
                }
            }
            Err(error) => {
                let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
            }
        }
    });
}

enum ProjectMutation {
    Reactivate,
}

fn mutate_project(
    client: &Client,
    id: crate::projects::ProjectId,
    mutation: ProjectMutation,
    tx: tokio::sync::mpsc::UnboundedSender<adapter::ClientEvent>,
) {
    let client = client.clone();
    tokio::spawn(async move {
        let result = match mutation {
            ProjectMutation::Reactivate => client
                .project_reactivate(ProjectIdParams {
                    project_id: id.0.clone(),
                })
                .await
                .map(|result| result.project.id),
        };
        match result {
            Ok(id) => match client
                .project_list(ProjectListParams {
                    include_inactive: Some(true),
                })
                .await
            {
                Ok(list) => {
                    let _ = tx.send(adapter::ClientEvent::ProjectsLoaded(list.projects));
                    let _ = tx.send(adapter::ClientEvent::ProjectReady(id));
                }
                Err(error) => {
                    let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
                }
            },
            Err(error) => {
                let _ = tx.send(adapter::ClientEvent::Error(error.to_string()));
            }
        }
    });
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

async fn handle_operations(state: &mut AppState, client: &Client, action: reducer::Action) {
    use reducer::Action;
    let is_acknowledgement = matches!(&action, Action::OperationsAcknowledge);
    let result = match action {
        Action::OperationsRefresh => {
            match operations::status(client, &mut state.operations).await {
                Ok(()) => operations::branches(client, &mut state.operations).await,
                Err(error) => Err(error),
            }
        }
        Action::OperationsOpen => {
            if let Some(p) = state.operations.path().map(str::to_owned) {
                operations::file(client, &mut state.operations, p).await
            } else {
                Ok(())
            }
        }
        Action::OperationsStage => {
            if let Some(p) = state.operations.path().map(str::to_owned) {
                operations::stage(client, &state.operations, p).await
            } else {
                Ok(())
            }
        }
        Action::OperationsUnstage => {
            if let Some(p) = state.operations.path().map(str::to_owned) {
                operations::unstage(client, &state.operations, p).await
            } else {
                Ok(())
            }
        }
        Action::OperationsRevertConfirmed => {
            if let Some(p) = state.operations.path().map(str::to_owned) {
                operations::revert(client, &state.operations, p).await
            } else {
                Ok(())
            }
        }
        Action::OperationsAcknowledge => {
            operations::acknowledge(client, &state.operations, "operations".into(), true)
                .await
                .map(|value| {
                    state.operations_ack = Some(
                        if value {
                            "acknowledged"
                        } else {
                            "not acknowledged"
                        }
                        .into(),
                    );
                })
        }
        Action::OperationsCreateBranch(name) => {
            operations::create_branch(client, &state.operations, name).await
        }
        Action::OperationsSwitchBranch(name) => {
            operations::switch_branch(client, &state.operations, name).await
        }
        _ => return,
    };
    state.operations_loading = false;
    match result {
        Ok(()) if !is_acknowledgement => state.operations_ack = Some("operation complete".into()),
        Err(e) => state.operations_error = Some(e.to_string()),
        Ok(()) => {}
    }
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
        let projects = ProjectState::default();
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
