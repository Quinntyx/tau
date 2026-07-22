use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::{Value, json};
use tau_client::{CreateSession, ProjectId};
use tau_proto::prelude::*;
use tokio::net::UnixListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn environment_gated_test_provider_stresses_every_tool_through_the_daemon() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace)?;
    std::fs::write(workspace.join("fixture.rs"), "fn fixture() {}\n")?;

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test_provider.py");
    unsafe {
        std::env::remove_var(tau_core::provider::TEST_PROVIDER_ENABLE_ENV);
        std::env::remove_var(tau_core::provider::TEST_PROVIDER_FIXTURE_ENV);
    }
    assert!(
        tau_core::provider::Provider::new("test", "model", None, None).is_err(),
        "the deterministic provider must require explicit opt-in"
    );
    unsafe {
        std::env::set_var(tau_core::provider::TEST_PROVIDER_ENABLE_ENV, "1");
        std::env::set_var(
            tau_core::provider::TEST_PROVIDER_FIXTURE_ENV,
            fixture.as_os_str(),
        );
    }

    let state = tau_server::AppState::new(
        Arc::new(tau_core::config::Config::default()),
        tau_core::db::Db::open(&temp.path().join("tau.db"))?,
    );
    let socket = temp.path().join("tau.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(async move {
        axum::serve(listener, tau_server::router(state).into_make_service()).await
    });
    let client = tau_client::Client::connect(&socket).await?;
    client
        .negotiate(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![Capability::TurnStreaming, Capability::EventReplay],
        })
        .await?;
    let project = client
        .project_create(ProjectCreateParams {
            name: "test-provider".into(),
            root: workspace.to_string_lossy().into_owned(),
        })
        .await?
        .project;

    for (scenario, expected_calls) in [
        ("test:all-tools", 13),
        ("test:filesystem", 8),
        ("test:integrations", 2),
        ("test:orchestration", 3),
        ("test:error-paths", 3),
    ] {
        let _ = std::fs::remove_file(workspace.join("generated.txt"));
        let actual = run_scenario(&client, &project, scenario, expected_calls).await?;
        assert_eq!(
            actual,
            json!({
                "scenario": scenario,
                "status": "passed",
                "tool_calls": expected_calls,
                "differences": [],
            }),
            "test provider reported a contract difference for {scenario}"
        );
    }

    server.abort();
    let _ = server.await;
    unsafe {
        std::env::remove_var(tau_core::provider::TEST_PROVIDER_ENABLE_ENV);
        std::env::remove_var(tau_core::provider::TEST_PROVIDER_FIXTURE_ENV);
    }
    Ok(())
}

async fn run_scenario(
    client: &tau_client::Client,
    project: &tau_proto::projects::Project,
    scenario: &str,
    expected_calls: usize,
) -> Result<Value> {
    let project_id = ProjectId::new(project.id.clone());
    let session = client
        .session_create(CreateSession {
            project_id,
            cwd: project.root.clone(),
        })
        .await?;
    let mut feed = client.events();
    let mut admission = client
        .turn_start(TurnStartParams {
            project_id: project.id.clone(),
            model: "test/model".into(),
            prompt: scenario.into(),
            session_id: Some(session.session_id.as_str().to_owned()),
            cwd: Some(project.root.clone()),
            idempotency_key: IdempotencyKey::new(format!("{scenario}-request")),
            agent: Some("code".into()),
            task_tier: Some(1),
            autonomous: Some(false),
            action: Some(RequestAction::Submit),
        })
        .await?;
    let started = loop {
        match admission
            .next()
            .await
            .context("admission stream closed")??
        {
            tau_client::TurnStreamEvent::Complete(started) => break started,
            tau_client::TurnStreamEvent::Event(_) => {}
        }
    };
    let mut text = String::new();
    let mut tool_outputs = 0;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), feed.next())
            .await?
            .context("event stream closed")??;
        match event.event {
            TurnEvent::ToolOutput { turn_id, .. } if turn_id == started.turn_id => {
                tool_outputs += 1;
            }
            TurnEvent::TextDelta {
                turn_id,
                text: delta,
            } if turn_id == started.turn_id => {
                text.push_str(&delta);
            }
            TurnEvent::TurnCompleted { turn_id, .. } if turn_id == started.turn_id => break,
            TurnEvent::TurnFailed { turn_id, message } if turn_id == started.turn_id => {
                anyhow::bail!("{scenario} failed: {message}")
            }
            _ => {}
        }
    }
    assert_eq!(tool_outputs, expected_calls, "{scenario} tool output count");
    serde_json::from_str(&text).context("test provider response was not valid JSON")
}
