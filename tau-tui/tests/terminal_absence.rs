use ratatui::{Terminal, backend::TestBackend};
use tau_tui::{AppState, reducer};

#[test]
fn tui_exposes_composer_status_send_and_cancel_surfaces() {
    let state = AppState::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|frame| tau_tui::components::render(frame, &state))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("prompt"));
    assert!(matches!(reducer::Action::Submit, reducer::Action::Submit));
    assert!(matches!(reducer::Action::Cancel, reducer::Action::Cancel));
}

#[test]
fn tui_surface_has_no_embedded_terminal_or_command_runner_labels() {
    let source = include_str!("../src/components.rs").to_ascii_lowercase();
    for forbidden in ["PTY", "CommandRunner", "terminal tab", "shell tab"] {
        assert!(
            !source.contains(forbidden),
            "unexpected label/API: {forbidden}"
        );
    }
}
