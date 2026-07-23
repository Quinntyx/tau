use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tau_client::TurnStreamEvent;
use tau_proto::prelude::*;
use tokio::net::UnixListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gui_backend_follows_admission_through_terminal_daemon_events() -> Result<()> {
    unsafe {
        std::env::set_var(tau_core::provider::TEST_PROVIDER_ENABLE_ENV, "1");
    }
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace)?;
    let socket = temp.path().join("tau.sock");
    let listener = UnixListener::bind(&socket)?;
    let state = tau_server::AppState::new(
        Arc::new(tau_core::config::Config::default()),
        tau_core::db::Db::open(&temp.path().join("tau.db"))?,
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, tau_server::router(state).into_make_service()).await
    });

    let client = tau_client::Client::connect(&socket).await?;
    client
        .negotiate(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![],
        })
        .await?;
    let project = client
        .project_create(ProjectCreateParams {
            name: "delivery".into(),
            root: workspace.to_string_lossy().into_owned(),
        })
        .await?
        .project;

    let runtime = tokio::runtime::Handle::current();
    let backend = tau_gui::backend::Backend::from_parts(
        socket,
        runtime.clone(),
        None,
        project.root.clone(),
        "test/model".into(),
    );
    let mut stream = backend.turn_with_project("test:error-paths".into(), None, project.id.clone());
    let mut text = String::new();
    let mut terminal = false;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), stream.recv())
            .await?
            .context("GUI turn stream closed")?
            .map_err(anyhow::Error::msg)?;
        match event {
            TurnStreamEvent::Event(SequencedEvent {
                event: TurnEvent::TextDelta { text: delta, .. },
                ..
            }) => text.push_str(&delta),
            TurnStreamEvent::Event(SequencedEvent {
                event: TurnEvent::TurnCompleted { .. },
                ..
            }) => terminal = true,
            TurnStreamEvent::Complete(_) => break,
            _ => {}
        }
    }
    assert!(terminal, "GUI completion arrived before the terminal event");
    let verdict: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(verdict["status"], "passed");

    let mut failed = backend.turn_with_project_options(
        "show the failure".into(),
        None,
        None,
        Some("missing-provider/model".into()),
        Some(project.id),
    );
    let mut failure = None;
    let mut failed_terminal = false;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), failed.recv())
            .await?
            .context("failed GUI turn stream closed")?
            .map_err(anyhow::Error::msg)?;
        match event {
            TurnStreamEvent::Event(SequencedEvent {
                event: TurnEvent::TurnFailed { message, .. },
                ..
            }) => {
                failure = Some(message);
                failed_terminal = true;
            }
            TurnStreamEvent::Complete(_) => break,
            _ => {}
        }
    }
    assert!(
        failed_terminal,
        "provider failure was not delivered to the GUI"
    );
    assert!(
        failure
            .as_deref()
            .is_some_and(|message| message.contains("unknown provider"))
    );

    server.abort();
    let _ = server.await;
    unsafe {
        std::env::remove_var(tau_core::provider::TEST_PROVIDER_ENABLE_ENV);
    }
    Ok(())
}
