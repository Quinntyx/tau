use tau_gui::operations::{OperationsAction, OperationsModel, describe};
use tau_proto::git::{GitFileResult, GitFileStatus};

#[test]
fn listener_projection_covers_root_status_file_diff_and_keep_revision() {
    let mut model = OperationsModel::new();
    model.apply_status(
        "/workspace/project".into(),
        vec![GitFileStatus {
            path: "src/lib.rs".into(),
            staged: false,
            modified: true,
            untracked: false,
        }],
    );
    model.select_file(GitFileResult {
        path: "src/lib.rs".into(),
        content: "new".into(),
        diff: "@@ -1 +1 @@\n-old\n+new".into(),
    });
    model.toggle_keep("src/lib.rs", "revision-1");
    assert!(model.is_kept("src/lib.rs", "revision-1"));
    model.invalidate_revision("src/lib.rs", "revision-2");
    assert!(!model.is_kept("src/lib.rs", "revision-1"));

    let projection = describe(&model);
    assert_eq!(projection.branch, "/workspace/project");
    assert_eq!(projection.selected_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(
        projection.changes,
        vec![("src/lib.rs".into(), false, true, false)]
    );
}

#[test]
fn action_surface_requires_explicit_revert_and_projects_ack() {
    assert!(matches!(
        OperationsAction::RevertConfirmed("tracked.txt".into()),
        OperationsAction::RevertConfirmed(_)
    ));
    let mut model = OperationsModel::new();
    model.acknowledge(true);
    model.acknowledge(true);
    assert_eq!(describe(&model).acknowledgement, Some(true));
}
