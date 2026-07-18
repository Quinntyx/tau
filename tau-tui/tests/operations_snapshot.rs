use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use tau_proto::git::{GitFileResult, GitFileStatus};
use tau_tui::operations::{OperationsState, render};
use tau_tui::{Action, AppState, reduce};

#[test]
fn operations_panel_snapshot_contains_status_and_diff() {
    let mut state = OperationsState::new("/workspace/project");
    state.branch = "feature".into();
    state.files = vec![GitFileStatus {
        path: "tracked.txt".into(),
        staged: true,
        modified: true,
        untracked: false,
    }];
    state.content = Some(GitFileResult {
        path: "tracked.txt".into(),
        content: "changed".into(),
        diff: "@@ -1 +1 @@\n-base\n+changed".into(),
    });
    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render(frame, frame.area(), &state))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text = buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("Operations [feature]"));
    assert!(text.contains("tracked.txt"));
    assert!(text.contains("+changed"));
}

#[test]
fn operations_key_listener_reduces_typed_actions() {
    let mut state = AppState {
        operations_focused: true,
        ..AppState::default()
    };
    assert!(matches!(
        tau_tui::reducer::key_action(
            &state,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        ),
        Some(Action::OperationsStage)
    ));
    reduce(
        &mut state,
        Action::OperationsTab(tau_tui::state::OperationsTab::Git),
    );
    assert_eq!(state.operations_tab, tau_tui::state::OperationsTab::Git);
    assert!(matches!(
        tau_tui::reducer::key_action(
            &state,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        ),
        Some(Action::OperationsAcknowledge)
    ));
}

#[test]
fn operations_acknowledgement_is_reduced_as_typed_state() {
    let mut state = AppState {
        operations_focused: true,
        ..AppState::default()
    };

    let action = tau_tui::reducer::key_action(
        &state,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
    )
    .expect("operations acknowledgement key should produce an action");
    assert!(matches!(action, Action::OperationsAcknowledge));

    reduce(&mut state, action);
    assert!(state.operations_loading);

    tau_tui::operations::reduce(
        &mut state.operations,
        tau_tui::operations::Action::Acknowledge("operations".into(), true),
    );
    assert_eq!(state.operations.acknowledged.get("operations"), Some(&true));
}
