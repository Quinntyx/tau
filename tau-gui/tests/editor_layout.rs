use tau_gui::chat::{Card, ChatAction, ChatState, ChatStatus, Role};
use tau_gui::input::EditorBuffer;

#[test]
fn editor_selection_handles_unicode_and_multiline_ranges() {
    let mut editor = EditorBuffer::new("one\nnaïve 😀\nthree");
    editor.move_home(false);
    assert_eq!(editor.cursor(), 16);

    editor.move_end(true);
    assert_eq!(editor.selected_text(), Some("three"));
    let mut unicode = EditorBuffer::new("naïve 😀");
    unicode.move_home(false);
    unicode.move_end(true);
    assert_eq!(unicode.selected_text(), Some("naïve 😀"));
    unicode.insert("replacement");
    assert_eq!(unicode.text(), "replacement");
}

#[test]
fn submit_starts_one_turn_and_rejects_empty_or_overlapping_submissions() {
    let mut state = ChatState::default();

    assert!(!state.reduce(ChatAction::Submit("   \n".into())));
    assert!(state.reduce(ChatAction::Submit("first prompt".into())));
    assert_eq!(state.status, ChatStatus::Streaming);
    assert_eq!(state.active_assistant, Some(1));
    assert!(matches!(
        &state.cards[0],
        Card::Message {
            role: Role::User,
            text
        } if text == "first prompt"
    ));
    assert!(!state.reduce(ChatAction::Submit("second prompt".into())));
    assert_eq!(state.cards.len(), 2);
}

#[test]
fn completed_turn_returns_to_ready_for_the_next_submit() {
    let mut state = ChatState::default();
    assert!(state.reduce(ChatAction::Submit("first".into())));
    assert!(state.reduce(ChatAction::Error("failed".into())));
    assert_eq!(state.status, ChatStatus::Failed("failed".into()));
    assert_eq!(state.active_assistant, None);

    assert!(state.reduce(ChatAction::Submit("second".into())));
    assert_eq!(state.status, ChatStatus::Streaming);
    assert_eq!(state.cards.len(), 4);
}
