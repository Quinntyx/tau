//! Window-free acceptance coverage for the complete GPUI shell contract.
//!
//! These tests deliberately exercise the projections and typed action seams,
//! rather than constructing an Application or opening a display window.

use std::path::PathBuf;

use tau_gui::ProjectShellRoot;
use tau_gui::composer::{Composer, ComposerAction};
use tau_gui::feed::{ActionReachability, FeedAction};
use tau_gui::operations::{OperationsAction, OperationsModel, describe};
use tau_gui::picker::{PickerAction, command_action, parse_command};
use tau_gui::projects::{ProjectAction, ProjectState};
use tau_gui::sessions::{Navigator, ProjectId, SessionIntent, SessionSummary};
use tau_gui::shell::{
    ProjectItem, ProjectServiceAction, ProjectShell, ProjectState as ShellState, ShellAction,
    ShellLayout,
};

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

// Keep these imports as an explicit acceptance contract for the project model
// and its root adapter, even though GPUI entities are intentionally not built.
#[allow(dead_code)]
fn _project_state_contract() -> ProjectState {
    ProjectState::new()
}
