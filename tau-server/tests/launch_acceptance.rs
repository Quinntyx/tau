//! End-to-end launch smoke test: a real websocket daemon and tau-client, with
//! only the model boundary scripted.

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use futures::StreamExt;
use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
use tau_client::{ActiveSessionStore, CreateSession, ProjectId, SessionId, SessionRef};
use tau_proto::prelude::*;
use tokio::net::UnixListener;

#[tokio::test]
async fn clean_home_launches_and_reopens_the_scripted_workflow() -> Result<()> {
    let home = tempfile::tempdir()?;
    let root = home.path().join("project");
    std::fs::create_dir(&root)?;
    std::process::Command::new("git")
        .args(["init", "--quiet", root.to_str().unwrap()])
        .status()?;
    let initial = std::process::Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "-c",
            "user.name=tau acceptance",
            "-c",
            "user.email=tau@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "initial",
        ])
        .status()?;
    anyhow::ensure!(
        initial.success(),
        "could not create the test repository commit"
    );

    let config_dir = home.path().join("config");
    std::fs::create_dir(&config_dir)?;
    let config_path = config_dir.join("config.kdl");
    std::fs::write(&config_path, "model \"scripted/model\"\n")?;
    let db_path = config_dir.join("tau.db");
    let config = tau_core::config::Config::load_from(&config_path)?;
    let scripted = MockCompletionModel::from_stream_turns([[
        MockStreamEvent::text("acceptance reply"),
        MockStreamEvent::final_response_with_total_tokens(2),
    ]]);
    let state = tau_server::AppState::new(Arc::new(config), tau_core::db::Db::open(&db_path)?)
        .with_provider(tau_core::provider::Provider::scripted(scripted));
    let socket: PathBuf = home.path().join("tau.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(async move {
        axum::serve(listener, tau_server::router(state).into_make_service()).await
    });
    let client = tau_client::Client::connect(&socket).await?;
    let negotiated = client
        .negotiate(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![Capability::TurnStreaming, Capability::EventReplay],
        })
        .await?;
    assert_eq!(negotiated.version.major, 1);

    let project = client
        .project_create(ProjectCreateParams {
            name: "acceptance".into(),
            root: root.to_string_lossy().into_owned(),
        })
        .await?
        .project;
    assert!(
        client
            .project_list(ProjectListParams {
                include_inactive: Some(false)
            })
            .await?
            .projects
            .iter()
            .any(|p| p.id == project.id)
    );
    let project_id = ProjectId::new(project.id.clone());
    let session = client
        .session_create(CreateSession {
            project_id: project_id.clone(),
            cwd: project.root.clone(),
        })
        .await?;
    assert!(
        client
            .session_list(project_id.clone())
            .await?
            .iter()
            .any(|s| s.session_id == session.session_id)
    );
    assert!(
        client
            .git_status(GitStatusParams {
                project: project.id.clone()
            })
            .await?
            .files
            .is_empty()
    );

    let mut feed = client.events();
    let mut turn = client
        .turn_start(TurnStartParams {
            project_id: project.id.clone(),
            model: "scripted/model".into(),
            prompt: "typed launch".into(),
            session_id: Some(session.session_id.as_str().to_owned()),
            cwd: Some(project.root.clone()),
            idempotency_key: IdempotencyKey::new("launch-acceptance"),
            agent: Some("code".into()),
            task_tier: Some(1),
            autonomous: Some(false),
            action: Some(RequestAction::Submit),
        })
        .await?;
    let started = loop {
        match turn.next().await.context("admission closed")?? {
            tau_client::TurnStreamEvent::Complete(v) => break v,
            tau_client::TurnStreamEvent::Event(_) => {}
        }
    };
    let mut text = String::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), feed.next())
            .await?
            .context("feed closed")??;
        match event.event {
            TurnEvent::TextDelta {
                turn_id,
                text: delta,
            } if turn_id == started.turn_id => text.push_str(&delta),
            TurnEvent::TurnCompleted { turn_id, .. } if turn_id == started.turn_id => break,
            TurnEvent::TurnFailed { message, .. } => anyhow::bail!(message),
            _ => {}
        }
    }
    assert_eq!(text, "acceptance reply");

    // The UI seams preserve both project/session selection and composer data.
    let mut tui = tau_tui::AppState::with_ids(session.session_id.as_str(), project.id.clone());
    tau_tui::reduce(&mut tui, tau_tui::Action::Paste("reopen me".into()));
    assert_eq!(
        tau_tui::reduce(&mut tui, tau_tui::Action::Submit).as_deref(),
        Some("reopen me")
    );
    assert_eq!(tui.composer.text(), "");
    let store = ActiveSessionStore::new(home.path().join("config").join("active-session.json"));
    store.save(&SessionRef {
        project_id: project_id.clone(),
        session_id: SessionId::new(session.session_id.as_str()),
    })?;
    server.abort();
    let _ = server.await;
    std::fs::remove_file(&socket)?;
    let reopened_state = tau_server::AppState::new(
        Arc::new(tau_core::config::Config::load_from(&config_path)?),
        tau_core::db::Db::open(&db_path)?,
    );
    let reopened_listener = UnixListener::bind(&socket)?;
    let reopened_server = tokio::spawn(async move {
        axum::serve(
            reopened_listener,
            tau_server::router(reopened_state).into_make_service(),
        )
        .await
    });
    let reopened = tau_client::Client::connect(&socket).await?;
    let reopened_negotiated = reopened
        .negotiate(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![Capability::TurnStreaming, Capability::EventReplay],
        })
        .await?;
    assert_eq!(reopened_negotiated.version.major, 1);
    assert!(
        reopened
            .session_list(project_id.clone())
            .await?
            .iter()
            .any(|s| s.session_id == session.session_id)
    );
    let restored = store
        .restore(&reopened)
        .await?
        .context("selection was not restored")?;
    assert_eq!(restored.session_id, session.session_id);
    assert!(
        !reopened
            .session_history(SessionRef {
                project_id: project_id.clone(),
                session_id: SessionId::new(session.session_id.as_str()),
            })
            .await?
            .entries
            .is_empty()
    );
    reopened_server.abort();
    Ok(())
}
