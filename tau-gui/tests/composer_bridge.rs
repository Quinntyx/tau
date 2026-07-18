use tau_gui::composer::{Composer, ComposerAction, ComposerMode};

#[test]
fn composer_keeps_input_adapter_and_typed_send_paths_reachable() {
    let mut composer = Composer::with_ids("session-1", "project-1");
    assert_eq!(composer.session_id(), Some("session-1"));
    assert_eq!(composer.project_id(), Some("project-1"));
    composer.sync_ime_text("please inspect @src/lib.rs");
    assert_eq!(composer.file_references(), vec!["@src/lib.rs"]);
    assert!(composer.state().send_enabled);

    composer.apply(ComposerAction::ChooseModel("gpt-test".into()));
    composer.apply(ComposerAction::ChooseAgent("reviewer".into()));
    composer.apply(ComposerAction::InsertFileReference("README.md".into()));
    assert_eq!(composer.model.as_deref(), Some("gpt-test"));
    assert_eq!(composer.agent.as_deref(), Some("reviewer"));

    let prompt = composer.apply(ComposerAction::Send);
    assert_eq!(
        prompt.as_deref(),
        Some("please inspect @src/lib.rs@README.md")
    );
    assert_eq!(composer.state().mode, ComposerMode::Streaming);
    assert!(composer.state().cancel_enabled);
    assert_eq!(composer.apply(ComposerAction::Cancel), None);
    assert_eq!(composer.state().mode, ComposerMode::Editing);
    assert!(!composer.state().disabled);
    assert!(!composer.state().cancel_enabled);
}

#[test]
fn ime_sync_and_completion_actions_share_canonical_buffer() {
    let mut composer = Composer::new();
    composer.sync_ime_text("/mo");
    composer.apply(ComposerAction::ChooseCompletion("/model".into()));
    assert_eq!(composer.text(), "/model");
    composer.apply(ComposerAction::ChooseSlashCommand("/models".into()));
    assert_eq!(composer.text(), "/models");
}

#[test]
fn gui_composer_file_refs_and_model_agent_are_adapter_state_not_requests() {
    let mut composer = Composer::with_ids("s", "p");
    composer.sync_ime_text("review @src/main.rs, and @README.md)");
    assert_eq!(
        composer.file_references(),
        vec!["@src/main.rs", "@README.md"]
    );
    composer.apply(ComposerAction::ChooseModel("model-a".into()));
    composer.apply(ComposerAction::ChooseAgent("agent-a".into()));
    assert_eq!(composer.model.as_deref(), Some("model-a"));
    assert_eq!(composer.agent.as_deref(), Some("agent-a"));
    assert_eq!(composer.session_id(), Some("s"));
    assert_eq!(composer.project_id(), Some("p"));
}

#[test]
fn gui_streaming_blocks_edits_but_cancel_remains_reachable() {
    let mut composer = Composer::new();
    composer.sync_ime_text("prompt");
    assert_eq!(
        composer.apply(ComposerAction::Send).as_deref(),
        Some("prompt")
    );
    assert_eq!(
        composer.apply(ComposerAction::InsertText(" changed".into())),
        None
    );
    assert_eq!(composer.text(), "prompt");
    assert_eq!(composer.apply(ComposerAction::Cancel), None);
    assert_eq!(composer.state().mode, ComposerMode::Editing);
    assert!(composer.state().send_enabled);
}
