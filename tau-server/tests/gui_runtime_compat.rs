//! GUI-facing compatibility contracts: old daemons must fail usefully, while
//! the daemon socket remains alive and the normal negotiate-before-turn flow
//! is exercised through real Unix/WebSocket connections.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use tau_proto::prelude::*;
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

async fn stale_daemon() -> Result<(
    TempDir,
    PathBuf,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("stale.sock");
    let listener = UnixListener::bind(&path)?;
    let turn_submissions = Arc::new(AtomicUsize::new(0));
    let observed_submissions = Arc::clone(&turn_submissions);
    let task = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            if let Ok(mut ws) = accept_async(stream).await {
                while let Some(Ok(Message::Text(text))) = ws.next().await {
                    let request: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    let id = request
                        .get("id")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    if request.get("method").and_then(|v| v.as_str()) == Some(METHOD_TURN_START) {
                        observed_submissions.fetch_add(1, Ordering::Relaxed);
                    }
                    let response = if request.get("method").and_then(|v| v.as_str())
                        == Some(METHOD_PROTOCOL_NEGOTIATE)
                    {
                        serde_json::json!({
                            "jsonrpc":"2.0", "id":id,
                            "error":{"code":-32601,"message":"Method not found"}
                        })
                    } else if request.get("method").and_then(|v| v.as_str()) == Some("ping") {
                        serde_json::json!({"jsonrpc":"2.0", "id":id, "result":"pong"})
                    } else {
                        continue;
                    };
                    if ws
                        .send(Message::Text(response.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    Ok((dir, path, turn_submissions, task))
}

#[tokio::test]
async fn stale_daemon_reports_actionable_incompatibility_without_killing_socket() -> Result<()> {
    let (_dir, socket, turn_submissions, task) = stale_daemon().await?;
    let client = tau_client::Client::connect(&socket).await?;
    let error = client
        .negotiate(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![Capability::TurnStreaming],
        })
        .await
        .expect_err("old daemon must reject the new negotiation method");
    let message = format!("{error:#}");
    assert!(message.contains("-32601") || message.contains("Method not found"));
    let turn = client
        .turn_start(TurnStartParams {
            model: "stale-model".into(),
            prompt: "must not submit".into(),
            session_id: None,
            cwd: None,
            idempotency_key: IdempotencyKey::new("stale-no-submit"),
            agent: None,
            task_tier: None,
            autonomous: None,
            action: Some(RequestAction::Submit),
        })
        .await;
    assert!(
        turn.is_err(),
        "incompatible daemon must reject local turn start"
    );
    assert_eq!(turn_submissions.load(Ordering::Relaxed), 0);
    assert_eq!(
        client.ping().await?,
        "pong",
        "failed negotiation must not close daemon"
    );
    task.abort();
    Ok(())
}

#[tokio::test]
async fn gui_negotiates_before_turn_and_replays_after_reconnect() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("tau.sock");
    let listener = UnixListener::bind(&socket)?;
    let state = tau_server::AppState::default();
    let app = tau_server::router(state.clone());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    let client = tau_client::Client::connect(&socket).await?;
    let request = ProtocolNegotiateParams {
        version: ProtocolVersion { major: 1, minor: 0 },
        capabilities: vec![Capability::EventReplay, Capability::TurnStreaming],
    };
    let report = client.negotiate_checked(request).await?;
    assert!(
        report.is_usable(),
        "GUI should get a classified compatibility report"
    );
    let session = state.db().create_session("/tmp/gui-compat")?;
    let _turn = client
        .turn_start(TurnStartParams {
            model: "missing-model".into(),
            prompt: "probe".into(),
            session_id: Some(session.id.clone()),
            cwd: None,
            idempotency_key: IdempotencyKey::new("gui-compat"),
            agent: None,
            task_tier: None,
            autonomous: None,
            action: Some(RequestAction::Submit),
        })
        .await
        .context("turn must be available after negotiation")?;
    drop(client);
    let replay_client = tau_client::Client::connect(&socket).await?;
    replay_client
        .negotiate_checked(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![Capability::EventReplay],
        })
        .await?;
    let replay = tokio::time::timeout(
        Duration::from_secs(2),
        replay_client.turn_replay(TurnReplayParams {
            session_id: session.id,
            after_sequence: 0,
            limit: None,
        }),
    )
    .await??;
    assert!(
        !replay.events.is_empty(),
        "reconnect must permit event replay"
    );
    task.abort();
    Ok(())
}
