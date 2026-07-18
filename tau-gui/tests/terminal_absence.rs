use tau_gui::chat::{ChatAction, ChatState, ChatStatus};
use tau_gui::view::describe_chat;

#[test]
fn composer_status_and_send_cancel_surfaces_are_public() {
    let mut chat = ChatState::default();
    assert!(matches!(chat.status, ChatStatus::Ready));
    assert!(chat.reduce(ChatAction::Submit("hello".into())));
    let description = describe_chat(&chat, "default", "model", "/tmp");
    assert_eq!(description.status, "Thinking...");

    let view_source = include_str!("../src/view.rs");
    assert!(view_source.contains("Send"));
    assert!(view_source.contains("Cancel"));
}

#[test]
fn gui_surface_has_no_embedded_terminal_or_command_runner_labels() {
    let view_source = include_str!("../src/view.rs").to_ascii_lowercase();
    for forbidden in ["Terminal", "PTY", "CommandRunner", "terminal tab"] {
        assert!(
            !view_source.contains(forbidden),
            "unexpected label/API: {forbidden}"
        );
    }
}
