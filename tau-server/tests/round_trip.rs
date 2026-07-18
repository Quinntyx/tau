//! In-process wire round-trip: bind the server on a temp socket, connect a
//! `tau-client`, and assert `ping` / `health` work. No subprocess spawn.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt;
use tokio::net::UnixListener;

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

    let client = tau_client::Client::connect(&socket).await?;

    assert_eq!(client.ping().await?, "pong");

    let h = client.health().await?;
    assert!(h.pid > 0);
    assert_eq!(h.version, env!("CARGO_PKG_VERSION"));

    server.abort();
    Ok(())
}

#[tokio::test]
async fn negotiation_rejects_unsupported_minor_and_clients_multiplex() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("tau.sock");
    let listener = UnixListener::bind(&socket)?;
    let app = tau_server::router(tau_server::AppState::default());
    let server = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(std::future::pending::<()>())
            .await
    });

    let first = tau_client::Client::connect(&socket).await?;
    let second = tau_client::Client::connect(&socket).await?;
    let unsupported = first
        .negotiate(tau_proto::prelude::ProtocolNegotiateParams {
            version: tau_proto::prelude::ProtocolVersion { major: 1, minor: 1 },
            capabilities: vec![],
        })
        .await;
    assert!(unsupported.is_err());
    let (left, right) = tokio::join!(first.ping(), second.health());
    assert_eq!(left?, "pong");
    assert!(right?.pid > 0);
    server.abort();
    Ok(())
}

#[tokio::test]
async fn durable_policy_prompt_broadcasts_to_multiple_clients_and_resumes_owner() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("tau.sock");
    let listener = UnixListener::bind(&socket)?;
    let state = tau_server::AppState::default();
    let project = state
        .db()
        .create_project("policy-test", "/tmp/policy-test")?;
    let session_id = state.db().create_session(&project.id)?.id;
    let app = tau_server::router(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(std::future::pending::<()>())
            .await
    });

    let owner = tau_client::Client::connect(&socket).await?;
    let other = tau_client::Client::connect(&socket).await?;
    let mut owner_events = owner.policy_events();
    let mut other_events = other.policy_events();
    let request_state = state.clone();
    let request_session_id = session_id.clone();
    let request = tokio::spawn(async move {
        request_state
            .request_policy_prompt(
                tau_proto::prelude::METHOD_PERMISSION_REQUEST,
                &request_session_id,
                "turn",
                "permission",
                serde_json::json!({
                    "request_id": "prompt-1",
                    "session_id": request_session_id,
                    "turn_id": "turn",
                    "tool": "write",
                    "arguments": {"path": "file"},
                    "initiating_client_id": "1",
                    "idempotency_key": "create-1"
                }),
                "1",
            )
            .await
    });

    let owner_prompt = tokio::time::timeout(Duration::from_secs(5), owner_events.next())
        .await?
        .expect("owner prompt");
    let other_prompt = tokio::time::timeout(Duration::from_secs(5), other_events.next())
        .await?
        .expect("other prompt");
    let request_id = match owner_prompt {
        tau_client::PolicyEvent::Permission(request) => request.request_id,
        _ => panic!("expected permission prompt"),
    };
    assert!(matches!(
        other_prompt,
        tau_client::PolicyEvent::Permission(_)
    ));

    let unauthorized = other
        .permission_reply(tau_proto::prelude::PermissionReply {
            request_id: request_id.clone(),
            idempotency_key: "reply-other".into(),
            choice: tau_proto::prelude::PermissionChoice::Allow,
            scope: tau_proto::prelude::PermissionScope::Once,
            actor: tau_proto::prelude::PromptActor::Human,
        })
        .await;
    assert!(unauthorized.is_err());
    owner
        .permission_reply(tau_proto::prelude::PermissionReply {
            request_id,
            idempotency_key: "reply-owner".into(),
            choice: tau_proto::prelude::PermissionChoice::Allow,
            scope: tau_proto::prelude::PermissionScope::Session,
            actor: tau_proto::prelude::PromptActor::Human,
        })
        .await?;
    let resolved = tokio::time::timeout(Duration::from_secs(5), request).await???;
    assert_eq!(resolved["choice"], "allow");

    server.abort();
    Ok(())
}
