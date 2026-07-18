use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tau_proto::prelude::{
    Capability, IdempotencyKey, ProtocolNegotiateParams, ProtocolVersion, RequestAction,
    TurnStartParams,
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
    let params = TurnStartParams {
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
                answer: "yes".into(),
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
