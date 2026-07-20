use anyhow::Result;
use tau_client::{CreateSession, ProjectId, SessionId, SessionRef};
use tau_proto::prelude::*;
use tokio::net::UnixListener;

fn code(error: anyhow::Error, expected: i32) {
    let text = error.to_string();
    assert!(text.contains(&format!("rpc error {expected}")), "{text}");
}

#[tokio::test]
async fn registry_and_session_ownership_audit() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("tau.sock");
    let listener = UnixListener::bind(&socket)?;
    let app = tau_server::router(tau_server::AppState::default());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    let client = tau_client::Client::connect(&socket).await?;

    let root_a = dir.path().join("a");
    let root_b = dir.path().join("b");
    let a = client
        .project_create(ProjectCreateParams {
            name: "A".into(),
            root: root_a.to_string_lossy().into_owned(),
        })
        .await?
        .project;
    let b = client
        .project_create(ProjectCreateParams {
            name: "B".into(),
            root: root_b.to_string_lossy().into_owned(),
        })
        .await?
        .project;
    assert_eq!(
        client
            .project_list(ProjectListParams::default())
            .await?
            .projects
            .len(),
        2
    );

    let session = client
        .session_create(CreateSession {
            project_id: ProjectId::new(a.id.clone()),
            cwd: root_a.join("subdir").to_string_lossy().into_owned(),
        })
        .await?;
    assert_eq!(session.project_id.as_str(), a.id);
    assert!(session.cwd.ends_with("subdir"));

    let foreign = SessionRef {
        project_id: ProjectId::new(b.id.clone()),
        session_id: SessionId::new(session.session_id.as_str()),
    };
    code(
        client.session_history(foreign.clone()).await.unwrap_err(),
        -32602,
    );
    code(client.session_archive(foreign).await.unwrap_err(), -32602);
    assert_eq!(
        client
            .session_list(ProjectId::new(a.id.clone()))
            .await?
            .len(),
        1
    );

    let inactive = client
        .project_unregister(ProjectIdParams {
            project_id: a.id.clone(),
        })
        .await?
        .project;
    assert!(!inactive.active);
    assert!(
        client
            .project_list(ProjectListParams::default())
            .await?
            .projects
            .iter()
            .all(|p| p.id != a.id)
    );
    assert_eq!(
        client
            .project_list(ProjectListParams {
                include_inactive: Some(true)
            })
            .await?
            .projects
            .len(),
        2
    );
    code(
        client
            .session_create(CreateSession {
                project_id: ProjectId::new(a.id.clone()),
                cwd: root_a.to_string_lossy().into_owned(),
            })
            .await
            .unwrap_err(),
        -32602,
    );

    let replacement = client
        .project_new_id(ProjectNewIdParams {
            project_id: a.id.clone(),
        })
        .await?
        .project_id;
    assert_ne!(replacement, a.id);
    code(
        client
            .project_reactivate(ProjectIdParams {
                project_id: a.id.clone(),
            })
            .await
            .unwrap_err(),
        -32602,
    );
    let replacement_record = client
        .project_list(ProjectListParams {
            include_inactive: Some(true),
        })
        .await?
        .projects
        .into_iter()
        .find(|p| p.id == replacement)
        .expect("replacement project");
    assert!(replacement_record.active);
    let fresh = client
        .session_create(CreateSession {
            project_id: ProjectId::new(replacement),
            cwd: root_a.to_string_lossy().into_owned(),
        })
        .await?;
    assert_ne!(fresh.session_id, session.session_id);

    let empty_id_error = client
        .project_rename(ProjectRenameParams {
            project_id: "".into(),
            name: "x".into(),
        })
        .await
        .unwrap_err();
    assert!(
        empty_id_error
            .to_string()
            .contains("project_id must not be empty")
    );
    task.abort();
    Ok(())
}
