//! In-process wire round-trip: bind the server on a temp socket, connect a
//! `tau-client`, and assert `ping` / `health` work. No subprocess spawn.

use std::path::PathBuf;

use anyhow::Result;
use futures::{SinkExt, StreamExt};
use tau_client::CompletionEvent;
use tau_proto::prelude::*;
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

#[tokio::test]
async fn ping_and_health_round_trip() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket: PathBuf = dir.path().join("tau.sock");

    let listener = UnixListener::bind(&socket)?;
    let app = tau_server::router(tau_server::AppState::default());
    let server = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(std::future::pending::<()>())
            .await
    });

    let mut client = tau_client::Client::connect(&socket).await?;

    assert_eq!(client.ping().await?, "pong");

    let h = client.health().await?;
    assert!(h.pid > 0);
    assert_eq!(h.version, env!("CARGO_PKG_VERSION"));

    server.abort();
    Ok(())
}

#[tokio::test]
async fn completion_stream_round_trip() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("tau-completion.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut ws = accept_async(stream).await?;
        let request = ws
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("no request"))??;
        let Message::Text(request) = request else {
            anyhow::bail!("expected text request");
        };
        let _: Request = serde_json::from_str(request.as_str())?;

        let delta = Notification {
            jsonrpc: JsonRpc::default(),
            method: METHOD_COMPLETION_DELTA.to_string(),
            params: Some(CompletionDelta {
                session_id: "session-1".into(),
                text: "hello".into(),
                usage: None,
            }),
        };
        ws.send(Message::Text(serde_json::to_string(&delta)?.into()))
            .await?;

        let result = CompletionStreamResult {
            session_id: "session-1".into(),
            message_id: 7,
            text: "hello".into(),
            usage: UsageSummary {
                input_tokens: 2,
                output_tokens: 3,
                total_tokens: 5,
            },
        };
        let response = Response::ok(Id::num(1), result);
        ws.send(Message::Text(serde_json::to_string(&response)?.into()))
            .await?;
        Ok::<_, anyhow::Error>(())
    });

    let mut client = tau_client::Client::connect(&socket).await?;
    let params = CompletionStreamParams {
        model: "mock/model".into(),
        prompt: "hi".into(),
        session_id: None,
        cwd: Some("/tmp".into()),
    };
    let mut completion = client.completion_stream(params).await?;
    let mut text = String::new();
    let mut final_result = None;
    while let Some(event) = completion.next().await {
        match event? {
            CompletionEvent::Delta(delta) => text.push_str(&delta.text),
            CompletionEvent::Complete(result) => final_result = Some(result),
        }
    }
    drop(completion);

    let result = final_result.expect("completion should finish");
    assert_eq!(text, "hello");
    assert_eq!(result.message_id, 7);
    assert_eq!(result.usage.total_tokens, 5);
    server.await??;
    Ok(())
}
