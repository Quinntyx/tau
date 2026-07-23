use std::sync::Arc;

use anyhow::Result;
use tau_proto::prelude::*;
use tokio::net::UnixListener;

#[tokio::test]
async fn daemon_exposes_typed_codex_account_status_and_rejects_unknown_providers() -> Result<()> {
    let temp = tempfile::tempdir()?;
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
        .negotiate_checked(ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: vec![Capability::TurnStreaming],
        })
        .await?;

    let status = client
        .auth_status(AuthProviderParams {
            provider: OPENAI_CODEX_PROVIDER.into(),
        })
        .await?;
    assert_eq!(status.provider, OPENAI_CODEX_PROVIDER);
    assert!(!matches!(status.status, AuthState::Pending { .. }));

    let error = client
        .auth_status(AuthProviderParams {
            provider: "unknown".into(),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unsupported account provider"));

    server.abort();
    let _ = server.await;
    Ok(())
}
