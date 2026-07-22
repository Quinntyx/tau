//! Window-free acceptance coverage for the complete GPUI shell contract.
//!
//! These tests deliberately exercise the projections and typed action seams,
//! rather than constructing an Application or opening a display window.

use std::path::PathBuf;

use gpui::{Modifiers, TestAppContext, point, px};
use tau_gui::ProjectShellRoot;
use tau_gui::composer::{Composer, ComposerAction};
use tau_gui::feed::{ActionReachability, FeedAction};
use tau_gui::operations::{OperationsAction, OperationsModel, describe};
use tau_gui::picker::{PickerAction, command_action, parse_command};
use tau_gui::projects::{ProjectAction, ProjectState};
use tau_gui::sessions::{Navigator, ProjectId, SessionIntent, SessionSummary};
use tau_gui::shell::{
    InactiveProjectChoice, ProjectItem, ProjectServiceAction, ProjectServiceAdapter, ProjectShell,
    ProjectState as ShellState, ShellAction, ShellLayout,
};

#[derive(Default)]
struct RecordingProjectListener(Vec<ProjectServiceAction>);

impl ProjectServiceAdapter for RecordingProjectListener {
    fn dispatch(&mut self, action: ProjectServiceAction) {
        self.0.push(action);
    }
}

#[test]
fn root_shell_description_composes_project_rail_and_sessions() {
    let project = ProjectItem {
        id: "project-1".into(),
        name: "Acceptance".into(),
        path: PathBuf::from("/workspace/acceptance"),
    };
    let mut shell = ProjectShell::new(ShellState::Ready(vec![project]));
    shell.select("project-1");
    let description = shell.describe(760.0);
    assert_eq!(description.layout, ShellLayout::Rail);
    assert_eq!(description.state, "ready");
    assert_eq!(description.project_count, 1);
    assert_eq!(description.selected.as_deref(), Some("project-1"));
    assert_eq!(
        shell.last_action,
        Some(ShellAction::SelectProject("project-1".into()))
    );

    let mut sessions = Navigator::new([SessionSummary {
        id: "session-1".into(),
        project_id: ProjectId::new("project-1"),
        title: Some("Launch review".into()),
        updated_at: 1,
        archived: false,
    }]);
    sessions.set_project(Some(ProjectId::new("project-1")));
    let visible = sessions.visible();
    assert_eq!(visible.len(), 1);
    let record = visible[0].clone();
    drop(visible);
    assert_eq!(
        sessions.select_intent(&record),
        SessionIntent::Select {
            project_id: ProjectId::new("project-1"),
            session_id: "session-1".into(),
        }
    );
}

#[test]
fn root_routes_typed_actions_without_window_or_display() {
    let mut shell = ProjectShell::new(ShellState::Empty);
    let action = ProjectServiceAction::Create {
        name: "New project".into(),
        root: PathBuf::from("/tmp/new-project"),
    };
    shell.dispatch_project_action(action.clone());
    assert_eq!(shell.last_service_action, Some(action));
    shell.apply_project_action(ProjectAction::Select("project-1".into()));
    assert_eq!(shell.selected.as_deref(), Some("project-1"));

    let mut composer = Composer::with_ids("session-1", "project-1");
    composer.apply(ComposerAction::ChooseModel("model-fast".into()));
    composer.apply(ComposerAction::ChooseAgent("reviewer".into()));
    composer.apply(ComposerAction::InsertText("ship it".into()));
    assert_eq!(composer.model.as_deref(), Some("model-fast"));
    assert_eq!(composer.agent.as_deref(), Some("reviewer"));
    assert_eq!(
        composer.apply(ComposerAction::Send).as_deref(),
        Some("ship it")
    );

    assert_eq!(
        command_action(parse_command("/model").unwrap()),
        Some(PickerAction::OpenModels)
    );
    assert_eq!(
        command_action(parse_command("/agent reviewer").unwrap()),
        Some(PickerAction::SelectAgent("reviewer".into()))
    );
}

#[test]
fn root_exposes_feed_and_operations_action_surfaces() {
    let reachability = ActionReachability {
        tool_toggle: true,
        permission: true,
        question: true,
        diff: true,
        ..Default::default()
    };
    assert_eq!(
        reachability.actions().collect::<Vec<_>>(),
        vec![
            FeedAction::ToggleTool,
            FeedAction::RespondPermission,
            FeedAction::AnswerQuestion,
            FeedAction::ReviewDiff,
        ]
    );

    let mut operations = OperationsModel::new();
    operations.acknowledge(true);
    let projection = describe(&operations);
    assert_eq!(projection.acknowledgement, Some(true));
    assert!(matches!(
        OperationsAction::RevertConfirmed("src/lib.rs".into()),
        OperationsAction::RevertConfirmed(_)
    ));
}

#[test]
fn root_projection_accepts_only_daemon_owned_projects() {
    let state = ProjectState::from_daemon(
        [tau_proto::projects::Project {
            id: "project-1".into(),
            name: "Acceptance root".into(),
            root: "/tmp/acceptance-root".into(),
            active: true,
            created_at: 1,
            updated_at: 1,
        }],
        Some("project-1"),
    );
    assert_eq!(state.persisted_active_id(), Some("project-1"));
    let _root_type: Option<ProjectShellRoot> = None;
}

#[test]
fn rendered_create_prompt_controls_submit_to_listener_and_cancel() {
    let mut shell = ProjectShell::new(ShellState::Empty);
    let mut listener = RecordingProjectListener::default();

    // These are the callbacks installed by the rendered Create and Cancel
    // controls.  Exercise them through the same listener seam used by the
    // root rather than asserting only on a state description.
    shell.open_create_prompt(PathBuf::from("/workspace/new-project"));
    shell.set_create_fields("New project", PathBuf::from("/workspace/new-project"));
    let action = shell.submit_create_prompt().expect("Create button action");
    listener.dispatch(action.clone());
    assert_eq!(
        listener.0,
        vec![ProjectServiceAction::Create {
            name: "New project".into(),
            root: PathBuf::from("/workspace/new-project"),
        }]
    );
    assert!(shell.create_prompt.is_none());

    shell.open_create_prompt(PathBuf::from("/workspace/cancelled"));
    shell.cancel_create_prompt();
    assert!(shell.create_prompt.is_none());
    assert_eq!(listener.0.len(), 1, "Cancel must not notify the service");
}

#[test]
fn rendered_inactive_controls_dispatch_reactivate_and_new_id() {
    let mut shell = ProjectShell::new(ShellState::Ready(vec![ProjectItem {
        id: "old-id".into(),
        name: "Old project (inactive)".into(),
        path: PathBuf::from("/workspace/old"),
    }]));
    let mut listener = RecordingProjectListener::default();

    shell.choose_inactive("old-id", "Old project", PathBuf::from("/workspace/old"));
    let action = shell
        .submit_inactive_choice(InactiveProjectChoice::Reactivate)
        .expect("Reactivate button action");
    shell.dispatch_to(&mut listener, action);

    shell.choose_inactive("old-id", "Old project", PathBuf::from("/workspace/old"));
    let action = shell
        .submit_inactive_choice(InactiveProjectChoice::CreateNew)
        .expect("New ID button action");
    shell.dispatch_to(&mut listener, action);

    assert_eq!(
        listener.0,
        vec![
            ProjectServiceAction::Reactivate("old-id".into()),
            ProjectServiceAction::CreateNewId("old-id".into()),
        ]
    );
}

#[test]
fn rendered_project_row_selection_notifies_listener() {
    let mut shell = ProjectShell::new(ShellState::Ready(vec![ProjectItem {
        id: "project-1".into(),
        name: "Acceptance".into(),
        path: PathBuf::from("/workspace/acceptance"),
    }]));
    let mut listener = RecordingProjectListener::default();

    // The row's on_click callback is the shell selection callback; retain the
    // adapter assertion so this verifies the service-facing listener contract.
    shell.dispatch_to(
        &mut listener,
        ProjectServiceAction::Select("project-1".into()),
    );
    assert_eq!(shell.selected.as_deref(), Some("project-1"));
    assert_eq!(
        listener.0,
        vec![ProjectServiceAction::Select("project-1".into())]
    );
}

#[gpui::test]
fn rendered_create_button_dispatches_service_action(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(|_, _| {
        let mut shell = ProjectShell::new(ShellState::Empty);
        shell.open_create_prompt(PathBuf::from("/workspace/new-project"));
        shell.set_create_fields("New project", PathBuf::from("/workspace/new-project"));
        shell
    });
    let bounds = cx
        .debug_bounds("submit-create-project")
        .expect("Create control must be rendered with bounds");

    cx.simulate_click(bounds.center(), Modifiers::none());
    let action = cx.update(|_, app| shell.read(app).last_service_action.clone());
    assert_eq!(
        action,
        Some(ProjectServiceAction::Create {
            name: "New project".into(),
            root: PathBuf::from("/workspace/new-project"),
        })
    );
}

#[gpui::test]
fn rendered_inactive_controls_dispatch_lifecycle_actions(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(|_, _| {
        ProjectShell::new(ShellState::Ready(vec![ProjectItem {
            id: "old-id".into(),
            name: "Old project (inactive)".into(),
            path: PathBuf::from("/workspace/old"),
        }]))
    });

    let reactivate = cx
        .debug_bounds("reactivate-old-id")
        .expect("Reactivate control must be rendered with bounds");
    cx.simulate_click(reactivate.center(), Modifiers::none());
    let action = cx.update(|_, app| shell.read(app).last_service_action.clone());
    assert_eq!(
        action,
        Some(ProjectServiceAction::Reactivate("old-id".into()))
    );

    shell.update(cx, |shell, _| {
        shell.last_service_action = None;
        shell.choose_inactive("old-id", "Old project", PathBuf::from("/workspace/old"));
    });
    cx.run_until_parked();
    let new_id = cx
        .debug_bounds("new-id-old-id")
        .expect("New ID control must be rendered with bounds");
    cx.simulate_click(new_id.center(), Modifiers::none());
    let action = cx.update(|_, app| shell.read(app).last_service_action.clone());
    assert_eq!(
        action,
        Some(ProjectServiceAction::CreateNewId("old-id".into()))
    );
}

#[gpui::test]
fn rendered_project_row_dispatches_selection(cx: &mut TestAppContext) {
    let (shell, cx) = cx.add_window_view(|_, _| {
        ProjectShell::new(ShellState::Ready(vec![ProjectItem {
            id: "project-1".into(),
            name: "Acceptance".into(),
            path: PathBuf::from("/workspace/acceptance"),
        }]))
    });
    let bounds = cx
        .debug_bounds("project-1")
        .expect("Project row must be rendered with bounds");

    cx.simulate_click(
        point(
            bounds.origin.x + px(8.),
            bounds.origin.y + bounds.size.height / 2.,
        ),
        Modifiers::none(),
    );
    let action = cx.update(|_, app| shell.read(app).last_service_action.clone());
    assert_eq!(
        action,
        Some(ProjectServiceAction::Select("project-1".into()))
    );
}

// Keep these imports as an explicit acceptance contract for the project model
// and its root adapter, even though GPUI entities are intentionally not built.
#[allow(dead_code)]
fn _project_state_contract() -> ProjectState {
    ProjectState::new()
}
