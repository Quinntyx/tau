use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tau_proto::prelude::{BoundedOutput, SequencedEvent, TurnEvent};
use tau_tui::{AppState, ClientEvent, ScriptedClient, reducer};

fn event(sequence: u64, event: TurnEvent) -> SequencedEvent {
    SequencedEvent {
        event_id: format!("event-{sequence}"),
        session_id: "session-1".into(),
        sequence,
        occurred_at: sequence as i64,
        request_id: None,
        event,
    }
}

#[test]
fn concurrent_stream_events_are_ordered_and_duplicate_replay_is_ignored() {
    let mut state = AppState {
        project_id: Some("project-1".into()),
        ..AppState::default()
    };
    state.transcript.push(String::new());
    ScriptedClient {
        events: vec![
            ClientEvent::Turn(event(
                1,
                TurnEvent::TurnStarted {
                    turn_id: "turn-1".into(),
                },
            )),
            ClientEvent::Turn(event(
                2,
                TurnEvent::TextDelta {
                    turn_id: "turn-1".into(),
                    text: "hello ".into(),
                },
            )),
            ClientEvent::Disconnected,
            ClientEvent::Turn(event(
                2,
                TurnEvent::TextDelta {
                    turn_id: "turn-1".into(),
                    text: "stale".into(),
                },
            )),
            ClientEvent::Turn(event(
                3,
                TurnEvent::TextDelta {
                    turn_id: "turn-1".into(),
                    text: "world".into(),
                },
            )),
            ClientEvent::Reconnected,
            ClientEvent::Complete {
                session_id: "session-1".into(),
                turn_id: "turn-1".into(),
            },
        ],
    }
    .drive(&mut state);

    assert_eq!(state.sequence, 3);
    assert_eq!(state.transcript.last().unwrap(), "hello world");
    assert_eq!(state.connection, tau_tui::state::Connection::Connected);
    assert_eq!(state.session_id.as_deref(), Some("session-1"));
}

#[test]
fn tool_permission_and_input_cancel_remain_available_during_arrival() {
    let mut state = AppState::default();
    state.transcript.push(String::new());
    ScriptedClient {
        events: vec![
            ClientEvent::Tool {
                name: "shell".into(),
                output: BoundedOutput {
                    inline: "ok".into(),
                    truncated: false,
                    artifacts: vec![],
                },
            },
            ClientEvent::Permission {
                tool: "shell".into(),
                summary: "run command".into(),
            },
        ],
    }
    .drive(&mut state);
    assert_eq!(state.tools[0].result, "ok");
    assert!(state.permission.is_some());

    assert!(matches!(
        reducer::key_action(
            &state,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)
        ),
        Some(reducer::Action::Permission(
            tau_tui::state::PermissionChoice::DenyOnce
        ))
    ));
    reducer::apply(&mut state, reducer::Action::Cancel);
    assert!(state.cancelling);
    assert!(!state.replaying);
}

#[test]
fn autonomy_tier_prompt_and_diff_replies_are_scriptable() {
    let mut state = AppState::default();
    reducer::apply(&mut state, reducer::Action::ToggleAutonomy);
    reducer::apply(&mut state, reducer::Action::Tier(2));
    assert!(state.autonomous);
    assert_eq!(state.task_tier, 3);
    state.project_id = Some("project-1".into());
    let params = reducer::params(&state, "inspect this".into(), Some("/tmp".into())).unwrap();
    assert_eq!(params.task_tier, Some(3));
    assert_eq!(params.autonomous, Some(true));

    state.diff_reply = Some(tau_tui::state::DiffReply { accepted: None });
    assert_eq!(
        reducer::apply(&mut state, reducer::Action::DiffReply(true)),
        Some("diff:accept".into())
    );
    assert_eq!(state.diff_reply.as_ref().unwrap().accepted, Some(true));
}

#[test]
fn turn_params_reject_missing_project_selection() {
    let state = AppState::default();
    let error = reducer::params(&state, "inspect this".into(), None)
        .expect_err("turns must require an explicitly selected project");
    assert!(error.to_string().contains("select an active project first"));
}

#[tokio::test]
async fn daemon_down_error_contains_exact_help() {
    let path = std::env::temp_dir().join(format!("tau-tui-missing-{}", std::process::id()));
    let error = tau_tui::run(path.clone()).await.unwrap_err().to_string();
    assert!(error.contains(&format!(
        "daemon unavailable at {}\nhelp: to fix this, run `tau serve` before running the tui",
        path.display()
    )));
}
