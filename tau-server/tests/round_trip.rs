//! In-process wire round-trip: bind the server on a temp socket, connect a
//! `tau-client`, and assert `ping` / `health` work. No subprocess spawn.

use std::path::PathBuf;

use anyhow::Result;
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
