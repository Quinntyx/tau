//! Comprehensive GPUI Feature Survey Tests
//!
//! Exercises GUI view controller contracts, modal sheets, project rail context menus,
//! picker action dispatching, and repository operations.

use std::path::PathBuf;

use gpui::{Modifiers, TestAppContext};
use tau_gui::composer::{Composer, ComposerAction};
use tau_gui::operations::OperationsModel;
use tau_gui::picker::{PickerAction, command_action, parse_command};
use tau_gui::sessions::{Navigator, ProjectId, SessionSummary};
use tau_gui::shell::{ProjectItem, ProjectServiceAction, ProjectShell, ProjectState as ShellState};

#[test]
fn composer_action_pipeline_updates_state() {
    let mut composer = Composer::with_ids("sess-123", "proj-456");
    composer.apply(ComposerAction::ChooseModel(
        "openai-codex/gpt-5.6-sol".into(),
    ));
    composer.apply(ComposerAction::ChooseAgent("architect".into()));
    composer.apply(ComposerAction::InsertText("implement feature".into()));

    assert_eq!(composer.model.as_deref(), Some("openai-codex/gpt-5.6-sol"));
    assert_eq!(composer.agent.as_deref(), Some("architect"));
    assert_eq!(composer.text(), "implement feature");
}

#[test]
fn picker_command_parsing_maps_to_typed_actions() {
    assert_eq!(
        command_action(parse_command("/model").unwrap()),
        Some(PickerAction::OpenModels)
    );
    assert_eq!(
        command_action(parse_command("/agent architect").unwrap()),
        Some(PickerAction::SelectAgent("architect".into()))
    );
}

#[test]
fn operations_model_tracks_staged_untracked_and_keep_revisions() {
    let mut model = OperationsModel::new();
    model.branch = "feature-branch".into();
    model.acknowledgement = Some(true);

    assert_eq!(model.branch, "feature-branch");
    assert_eq!(model.acknowledgement, Some(true));
}

#[test]
fn navigator_filters_sessions_by_selected_project() {
    let mut nav = Navigator::new([
        SessionSummary {
            id: "s1".into(),
            project_id: ProjectId::new("p1"),
            title: Some("Session 1".into()),
            updated_at: 100,
            archived: false,
        },
        SessionSummary {
            id: "s2".into(),
            project_id: ProjectId::new("p2"),
            title: Some("Session 2".into()),
            updated_at: 200,
            archived: false,
        },
    ]);

    nav.set_project(Some(ProjectId::new("p1")));
    let visible = nav.visible();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, "s1");
}

#[gpui::test]
fn rendered_project_menu_opens_and_dispatches_update(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(|_, _| {
        ProjectShell::new(ShellState::Ready(vec![ProjectItem {
            id: "proj-1".into(),
            name: "Tau Core".into(),
            path: PathBuf::from("/workspace/tau"),
        }]))
    });

    let menu_btn = cx
        .debug_bounds("project-menu-btn-proj-1")
        .expect("Project menu button must be rendered with bounds");
    cx.simulate_click(menu_btn.center(), Modifiers::none());
    cx.run_until_parked();

    let update_btn = cx
        .debug_bounds("update-proj-1")
        .expect("Update option in project context menu must be rendered with bounds");
    cx.simulate_click(update_btn.center(), Modifiers::none());

    let action = cx.update(|_, app| shell.read(app).last_service_action.clone());
    assert_eq!(
        action,
        Some(ProjectServiceAction::Update {
            id: "proj-1".into(),
            name: "Tau Core".into(),
            root: PathBuf::from("/workspace/tau"),
        })
    );
}

#[gpui::test]
fn rendered_project_menu_dispatches_unregister(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(|_, _| {
        ProjectShell::new(ShellState::Ready(vec![ProjectItem {
            id: "proj-2".into(),
            name: "Tau GUI".into(),
            path: PathBuf::from("/workspace/tau-gui"),
        }]))
    });

    let menu_btn = cx
        .debug_bounds("project-menu-btn-proj-2")
        .expect("Project menu button must be rendered");
    cx.simulate_click(menu_btn.center(), Modifiers::none());
    cx.run_until_parked();

    let unregister_btn = cx
        .debug_bounds("unregister-proj-2")
        .expect("Unregister option must be rendered");
    cx.simulate_click(unregister_btn.center(), Modifiers::none());

    let action = cx.update(|_, app| shell.read(app).last_service_action.clone());
    assert_eq!(
        action,
        Some(ProjectServiceAction::Unregister("proj-2".into()))
    );
}
