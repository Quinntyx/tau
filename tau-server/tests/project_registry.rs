use anyhow::Result;
use futures::StreamExt;
use tau_client::{CreateSession, ProjectId as ClientProjectId};
use tau_proto::prelude::*;
use tokio::net::UnixListener;

#[tokio::test]
async fn project_rpc_lifecycle_and_turn_validation_are_daemon_backed() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("tau.sock");
    let listener = UnixListener::bind(&socket)?;
    let state = tau_server::AppState::default();
    let app = tau_server::router(state.clone());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });

    let client = tau_client::Client::connect(&socket).await?;
    let root = dir.path().join("project");
    let created = client
        .project_create(ProjectCreateParams {
            name: "Project".into(),
            root: root.to_string_lossy().into_owned(),
        })
        .await?
        .project;
    assert!(created.active && root.is_dir());
    assert_eq!(
        client
            .project_list(ProjectListParams::default())
            .await?
            .projects
            .len(),
        1
    );

    let renamed = client
        .project_rename(ProjectRenameParams {
            project_id: created.id.clone(),
            name: "Renamed".into(),
        })
        .await?
        .project;
    assert_eq!(renamed.id, created.id);
    let inactive = client
        .project_unregister(ProjectIdParams {
            project_id: created.id.clone(),
        })
        .await?
        .project;
    assert!(!inactive.active);
    let new_id = client
        .project_new_id(ProjectNewIdParams {
            project_id: created.id.clone(),
        })
        .await?
        .project_id;
    assert_ne!(new_id, created.id);
    assert!(
        client
            .project_reactivate(ProjectIdParams {
                project_id: created.id.clone()
            })
            .await
            .is_err()
    );
    // An inactive registry entry is not a session attachment target.
    assert!(
        client
            .session_create(CreateSession {
                project_id: ClientProjectId::new(created.id.clone()),
                cwd: root.to_string_lossy().into_owned(),
            })
            .await
            .is_err()
    );

    client
        .negotiate(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![Capability::TurnStreaming],
        })
        .await?;
    for project_id in ["missing", created.id.as_str()] {
        let mut stream = client
            .turn_start(TurnStartParams {
                project_id: project_id.into(),
                model: "unused/model".into(),
                prompt: "rejected".into(),
                session_id: None,
                cwd: None,
                idempotency_key: IdempotencyKey::new(format!("reject-{project_id}")),
                agent: None,
                task_tier: None,
                autonomous: None,
                action: Some(RequestAction::Submit),
            })
            .await?;
        assert!(stream.next().await.expect("validation response").is_err());
    }
    task.abort();
    Ok(())
}
