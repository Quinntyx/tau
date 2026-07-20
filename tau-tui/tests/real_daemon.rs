use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tau_proto::prelude::{
    Capability, IdempotencyKey, ProjectCreateParams, ProjectIdParams, ProjectListParams,
    ProjectNewIdParams, ProtocolNegotiateParams, ProtocolVersion, RequestAction, TurnStartParams,
};
use tokio::net::UnixListener;

#[tokio::test]
async fn real_daemon_broadcasts_stream_and_replays_after_control_reply() -> Result<()> {
    let socket = std::env::temp_dir().join(format!(
        "tau-tui-real-daemon-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let listener = UnixListener::bind(&socket)?;
    let app = tau_server::router(tau_server::AppState::default());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });

    let client = tau_client::Client::connect(&socket).await?;
    let observer = tau_client::Client::connect(&socket).await?;
    for connection in [&client, &observer] {
        connection
            .negotiate_checked(ProtocolNegotiateParams {
                version: ProtocolVersion { major: 1, minor: 0 },
                capabilities: vec![
                    Capability::TurnStreaming,
                    Capability::TurnCancellation,
                    Capability::EventReplay,
                ],
            })
            .await?;
    }
    let mut events = observer.events();
    let project = client
        .project_create(tau_proto::prelude::ProjectCreateParams {
            name: "real-tui".into(),
            root: std::env::current_dir()?.to_string_lossy().into_owned(),
        })
        .await?
        .project;
    let params = TurnStartParams {
        project_id: project.id,
        model: "mock/model".into(),
        prompt: "stream while input remains live".into(),
        session_id: None,
        cwd: Some(std::env::current_dir()?.to_string_lossy().into_owned()),
        idempotency_key: IdempotencyKey::new("real-tui-test-turn"),
        agent: Some("default".into()),
        task_tier: Some(1),
        autonomous: Some(false),
        action: Some(RequestAction::Submit),
    };
    let mut turn = client.turn_start(params).await?;
    let observed = tokio::time::timeout(Duration::from_secs(2), events.next())
        .await?
        .context("observer did not receive broadcast")??;
    assert_eq!(observed.sequence, 1);
    let (session_id, turn_id) = match observed.event {
        tau_proto::prelude::TurnEvent::TurnStarted { turn_id } => (observed.session_id, turn_id),
        _ => anyhow::bail!("unexpected first event"),
    };

    // A control-plane response is independently routable while the turn
    // stream is still open; it must not wait for the stream consumer.
    let response = client
        .turn_response(tau_proto::prelude::TurnResponseParams {
            session_id: session_id.clone(),
            turn_id,
            idempotency_key: IdempotencyKey::new("real-tui-test-response"),
            response: tau_proto::prelude::ClientResponse::Question {
                question_id: "test-question".into(),
                answer: tau_proto::prelude::QuestionAnswer("yes".into()),
            },
        })
        .await?;
    assert!(response.accepted);
    let mut completed = false;
    while let Some(item) = tokio::time::timeout(Duration::from_secs(2), turn.next()).await? {
        if matches!(item?, tau_client::TurnStreamEvent::Complete(_)) {
            completed = true;
            break;
        }
    }
    assert!(completed);

    let replay = client
        .turn_replay(tau_proto::prelude::TurnReplayParams {
            session_id,
            after_sequence: 0,
            limit: Some(16),
        })
        .await?;
    assert!(!replay.events.is_empty());

    server.abort();
    let _ = tokio::fs::remove_file(socket).await;
    Ok(())
}

#[tokio::test]
async fn real_daemon_typed_project_and_session_lifecycle() -> Result<()> {
    let socket = std::env::temp_dir().join(format!(
        "tau-tui-registry-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let listener = UnixListener::bind(&socket)?;
    let app = tau_server::router(tau_server::AppState::default());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    let client = tau_client::Client::connect(&socket).await?;

    let empty = client.project_list(ProjectListParams::default()).await?;
    assert!(empty.projects.is_empty());
    let project = client
        .project_create(ProjectCreateParams {
            name: "scripted project".into(),
            root: std::env::temp_dir().to_string_lossy().into_owned(),
        })
        .await?
        .project;
    assert!(project.active);
    assert_eq!(
        client
            .project_list(ProjectListParams::default())
            .await?
            .projects
            .len(),
        1
    );

    let selected = tau_tui::AppState::with_ids("not-yet-created", project.id.clone());
    assert_eq!(selected.project_id.as_deref(), Some(project.id.as_str()));
    assert_eq!(selected.composer.project_id(), project.id);

    let session = client
        .session_create(tau_client::CreateSession {
            project_id: tau_client::ProjectId::new(project.id.clone()),
            cwd: project.root.clone(),
        })
        .await?;
    assert_eq!(session.project_id.as_str(), project.id);
    assert_eq!(
        client
            .session_list(tau_client::ProjectId::new(project.id.clone()))
            .await?
            .len(),
        1
    );
    let reference = tau_client::SessionRef {
        project_id: tau_client::ProjectId::new(project.id.clone()),
        session_id: session.session_id.clone(),
    };
    client.session_archive(reference.clone()).await?;
    assert!(
        client
            .session_list(tau_client::ProjectId::new(project.id.clone()))
            .await?
            .is_empty()
    );
    let restored = client.session_restore(reference).await?;
    assert_eq!(restored.session_id, session.session_id);

    let inactive = client
        .project_unregister(ProjectIdParams {
            project_id: project.id.clone(),
        })
        .await?
        .project;
    assert!(!inactive.active);
    let reactivated = client
        .project_reactivate(ProjectIdParams {
            project_id: project.id.clone(),
        })
        .await?
        .project;
    assert!(reactivated.active);
    let inactive_again = client
        .project_unregister(ProjectIdParams {
            project_id: project.id.clone(),
        })
        .await?
        .project;
    assert!(!inactive_again.active);
    let new_id = client
        .project_new_id(ProjectNewIdParams {
            project_id: project.id.clone(),
        })
        .await?
        .project_id;
    assert_ne!(new_id, project.id);
    assert!(
        client
            .project_reactivate(ProjectIdParams {
                project_id: project.id.clone()
            })
            .await
            .is_err()
    );

    assert!(
        client
            .project_reactivate(ProjectIdParams {
                project_id: "missing".into()
            })
            .await
            .is_err()
    );
    assert!(
        client
            .session_restore(tau_client::SessionRef {
                project_id: tau_client::ProjectId::new(project.id),
                session_id: tau_client::SessionId::new("missing"),
            })
            .await
            .is_err()
    );
    server.abort();
    let _ = tokio::fs::remove_file(socket).await;
    Ok(())
}
