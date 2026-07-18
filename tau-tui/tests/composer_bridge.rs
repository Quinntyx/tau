use tau_tui::reducer::{params, sync_composer};
use tau_tui::{Action, AppState, reduce};

#[test]
fn reducer_synchronizes_canonical_composer_and_legacy_projection() {
    let mut state = AppState::with_ids("session-1", "project-1");
    reduce(&mut state, Action::Insert('@'));
    reduce(&mut state, Action::Paste("src/lib.rs".into()));
    assert_eq!(state.composer.text(), "@src/lib.rs");
    assert_eq!(state.input, state.composer.text());
    assert_eq!(state.cursor, state.composer.selection().cursor);
    assert_eq!(state.composer.file_references(), vec!["@src/lib.rs"]);

    let prompt = reduce(&mut state, Action::Submit).expect("send path returns prompt");
    assert_eq!(prompt, "@src/lib.rs");
    assert_eq!(state.input, "");
    assert_eq!(state.composer.text(), "");
    assert_eq!(
        state.transcript.last().map(String::as_str),
        Some("You: @src/lib.rs")
    );
}

#[test]
fn typed_model_agent_and_command_paths_remain_reachable_without_terminal_services() {
    let mut state = AppState::with_ids("session-1", "project-1");
    reduce(&mut state, Action::Open(tau_tui::state::Picker::Models));
    assert_eq!(
        state.composer.state().picker,
        Some(tau_tui::composer::PickerKind::Model)
    );
    reduce(&mut state, Action::ClosePicker);
    reduce(&mut state, Action::Open(tau_tui::state::Picker::Agents));
    assert_eq!(
        state.composer.state().picker,
        Some(tau_tui::composer::PickerKind::Agent)
    );

    state.model = "model-test".into();
    state.agent = "agent-test".into();
    let request = params(&state, "/help".into(), Some("/tmp/project".into()));
    assert_eq!(request.model, "model-test");
    assert_eq!(request.agent.as_deref(), Some("agent-test"));
    assert!(matches!(
        request.action,
        Some(tau_proto::prelude::RequestAction::Command { .. })
    ));
    assert_eq!(request.session_id.as_deref(), Some("session-1"));
    assert_eq!(request.cwd.as_deref(), Some("/tmp/project"));
}

#[test]
fn reducer_picker_query_and_pick_update_canonical_composer() {
    let mut state = AppState::with_ids("s", "p");
    state.models.push(tau_tui::state::Model {
        id: "model-a".into(),
        provider: "provider".into(),
        favorite: false,
    });
    reduce(&mut state, Action::Open(tau_tui::state::Picker::Models));
    for character in "model-a".chars() {
        reduce(&mut state, Action::Query(character));
    }
    reduce(&mut state, Action::Pick);
    assert_eq!(state.composer.state().model.as_deref(), Some("model-a"));
    assert_eq!(state.model, "model-a");
    assert_eq!(state.picker, tau_tui::state::Picker::None);
}

#[test]
fn reducer_send_cancel_lifecycle_has_no_terminal_or_runner_dependency() {
    let mut state = AppState::with_ids("s", "p");
    reduce(&mut state, Action::Paste("hello".into()));
    assert_eq!(reduce(&mut state, Action::Submit).as_deref(), Some("hello"));
    assert!(state.composer.state().text.is_empty());
    assert!(!state.composer.state().sending);
    reduce(&mut state, Action::Paste("next".into()));
    reduce(&mut state, Action::Cancel);
    assert!(state.cancelling);
    assert!(!state.composer.state().sending);
    assert_eq!(state.composer.text(), "next");
}

#[test]
fn active_turn_projection_disables_canonical_composer_until_terminal_event() {
    let mut state = AppState::with_ids("s", "p");
    state.turn_id = Some("turn-1".into());
    sync_composer(&mut state);
    assert!(state.composer.state().sending);
    assert!(state.composer.state().disabled);
    reduce(&mut state, Action::Insert('x'));
    assert_eq!(state.composer.text(), "");

    state.turn_id = None;
    sync_composer(&mut state);
    assert!(!state.composer.state().sending);
    assert!(!state.composer.state().disabled);
}
