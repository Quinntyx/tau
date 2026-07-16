//! tau — long-running daemon server.
//!
//! Serves the tau JSON-RPC 2.0 protocol over a WebSocket on a loopback Unix
//! socket. One connection per client; text frames carry JSON-RPC, binary frames
//! are reserved (logged + ignored in M0). systemd-friendly: clean SIGTERM/SIGINT
//! shutdown, single-instance via an advisory lockfile.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use tau_proto::{
    HealthResult, Id, JsonRpc, METHOD_HEALTH, METHOD_NOT_FOUND, METHOD_PING, PARSE_ERROR, Request,
    Response,
};
use tokio::net::UnixListener;

/// Per-process server state shared with every connection.
#[derive(Clone)]
pub struct AppState {
    started: Instant,
    config: Arc<tau_core::config::Config>,
    db: Arc<tau_core::db::Db>,
}

impl AppState {
    pub fn new(config: Arc<tau_core::config::Config>, db: tau_core::db::Db) -> Self {
        Self {
            started: Instant::now(),
            config,
            db: Arc::new(db),
        }
    }

    pub fn config(&self) -> &tau_core::config::Config {
        &self.config
    }

    pub fn db(&self) -> &tau_core::db::Db {
        &self.db
    }

    pub fn started(&self) -> Instant {
        self.started
    }
}

impl Default for AppState {
    fn default() -> Self {
        let db = tau_core::db::Db::open_in_memory()
            .expect("AppState::default must open an in-memory database");
        Self::new(Arc::new(tau_core::config::Config::default()), db)
    }
}

/// The axum router for the daemon.
pub fn router(state: AppState) -> Router {
    Router::new().route("/", get(ws_handler)).with_state(state)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                let resp = handle_text(&text, &state);
                if socket.send(Message::Text(resp.into())).await.is_err() {
                    break;
                }
            }
            Ok(Message::Binary(_)) => {
                tracing::debug!("ignoring reserved binary frame");
            }
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
}

/// Dispatch one JSON-RPC text request into a JSON-RPC text response.
fn handle_text(text: &str, state: &AppState) -> String {
    let req: Request = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            return serde_json::to_string(&Response::<serde_json::Value>::err(
                Id::num(0),
                PARSE_ERROR,
                e.to_string(),
            ))
            .unwrap_or_default();
        }
    };
    let id = req.id;
    match req.method.as_str() {
        METHOD_PING => serde_json::to_string(&Response::ok(id, "pong")).unwrap_or_default(),
        METHOD_HEALTH => {
            let result = HealthResult {
                version: env!("CARGO_PKG_VERSION").to_string(),
                uptime_ms: state.started.elapsed().as_millis() as u64,
                pid: std::process::id(),
            };
            serde_json::to_string(&Response::ok(id, result)).unwrap_or_default()
        }
        other => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        ))
        .unwrap_or_default(),
    }
}

/// Run the daemon on the given Unix socket path, blocking until shutdown.
pub async fn run(socket_path: PathBuf) -> Result<()> {
    let _lock = acquire_lock()?;

    let config = match tau_core::config::Config::load() {
        Ok(c) => {
            match tau_core::config::config_path() {
                Ok(p) if p.exists() => tracing::info!(path = %p.display(), "loaded config"),
                _ => tracing::info!("no config file found (using defaults)"),
            }
            c
        }
        Err(e) => {
            tracing::warn!(error = %e, "config load failed; using defaults");
            tau_core::config::Config::default()
        }
    };
    let state = {
        let db = match tau_core::db::Db::open(&tau_core::db::default_db_path()?) {
            Ok(d) => {
                tracing::info!("database opened");
                d
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to open database");
                return Err(e);
            }
        };
        AppState::new(Arc::new(config), db)
    };

    // Stale-socket cleanup is safe: we hold the single-instance lock, so any
    // pre-existing socket file cannot belong to another live daemon.
    if socket_path.exists() {
        let _ = tokio::fs::remove_file(&socket_path).await;
    }
    if let Some(parent) = socket_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("binding {}", socket_path.display()))?;
    tracing::info!(path = %socket_path.display(), "tau daemon listening");

    let app = router(state);
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    let _ = tokio::fs::remove_file(&socket_path).await;
    tracing::info!("tau daemon stopped");
    Ok(())
}

/// Advisory single-instance lock held for the lifetime of the daemon process.
pub struct InstanceLock {
    #[allow(dead_code)]
    _guard: fd_lock::RwLockWriteGuard<'static, std::fs::File>,
}

fn acquire_lock() -> Result<InstanceLock> {
    let lock_path = tau_core::home_tau()?.join("tau.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    // Leak the lock container so the write guard can be `'static` and stored in
    // `InstanceLock`; a singleton daemon holds it for the whole process.
    let lock: &'static mut fd_lock::RwLock<std::fs::File> =
        Box::leak(Box::new(fd_lock::RwLock::new(file)));
    let guard = lock.try_write().map_err(|_| {
        anyhow::anyhow!(
            "another tau daemon is already running (lock held at {})",
            lock_path.display()
        )
    })?;
    Ok(InstanceLock { _guard: guard })
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT (ctrl-c), shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

// keep JsonRpc referenced so future wire additions land cleanly
const _: JsonRpc = JsonRpc::V2;
