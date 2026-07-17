//! tau — long-running daemon server.
//!
//! Serves the tau JSON-RPC 2.0 protocol over a WebSocket on a loopback Unix
//! socket. One connection per client; text frames carry JSON-RPC, binary frames
//! are reserved (logged + ignored in M0). systemd-friendly: clean SIGTERM/SIGINT
//! shutdown, single-instance via an advisory lockfile.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use futures::{SinkExt, StreamExt};
use tau_proto::prelude::*;
use tokio::{
    net::UnixListener,
    sync::{Mutex, mpsc},
};

pub mod runtime;

/// Per-process server state shared with every connection.
#[derive(Clone)]
pub struct AppState {
    started: Instant,
    config: Arc<tau_core::config::Config>,
    db: Arc<tau_core::db::Db>,
    runtime: runtime::Runtime,
    #[allow(dead_code)]
    sessions: Arc<Mutex<HashMap<String, Arc<runtime::SessionTurnQueue<()>>>>>,
}

impl AppState {
    pub fn new(config: Arc<tau_core::config::Config>, db: tau_core::db::Db) -> Self {
        Self {
            started: Instant::now(),
            config,
            db: Arc::new(db),
            runtime: runtime::Runtime::new(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
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

    pub fn runtime(&self) -> &runtime::Runtime {
        &self.runtime
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

async fn handle_socket(socket: WebSocket, state: AppState) {
    let client_id = state.runtime.connections.attach();
    let (mut sink, mut input) = futures::StreamExt::split(socket);
    let (out, mut outgoing) = mpsc::channel::<Message>(128);
    let writer = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });
    while let Some(msg) = input.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let req: Request = match serde_json::from_str(&text) {
                    Ok(req) => req,
                    Err(error) => {
                        let response = serde_json::to_string(&Response::<serde_json::Value>::err(
                            Id::num(0),
                            PARSE_ERROR,
                            error.to_string(),
                        ))
                        .unwrap_or_default();
                        if out.send(Message::Text(response.into())).await.is_err() {
                            break;
                        }
                        continue;
                    }
                };
                let state = state.clone();
                let out = out.clone();
                tokio::spawn(async move {
                    let response = handle_request(req, &state, &out).await;
                    let _ = out.send(Message::Text(response.into())).await;
                });
            }
            Ok(Message::Binary(_)) => {
                tracing::debug!("ignoring reserved binary frame");
            }
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    drop(out);
    let _ = writer.await;
    state.runtime.connections.detach(client_id);
}

/// Dispatch one non-streaming JSON-RPC request into a response.
async fn handle_request(req: Request, state: &AppState, out: &mpsc::Sender<Message>) -> String {
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
        METHOD_PROTOCOL_NEGOTIATE => match req
            .params
            .and_then(|p| serde_json::from_value::<ProtocolNegotiateParams>(p).ok())
        {
            Some(params) if params.version.major == 1 => serde_json::to_string(&Response::ok(
                id,
                ProtocolNegotiateResult {
                    version: ProtocolVersion { major: 1, minor: 0 },
                    capabilities: params.capabilities,
                },
            ))
            .unwrap_or_default(),
            _ => serde_json::to_string(&Response::<serde_json::Value>::err(
                id,
                INVALID_PARAMS,
                "unsupported protocol version",
            ))
            .unwrap_or_default(),
        },
        METHOD_TURN_START => start_turn(id, req.params, state, out).await,
        METHOD_TURN_CANCEL => cancel_turn(id, req.params, state).await,
        METHOD_TURN_REPLAY => replay_turn(id, req.params, state).await,
        other => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        ))
        .unwrap_or_default(),
    }
}

async fn start_turn(
    id: Id,
    value: Option<serde_json::Value>,
    state: &AppState,
    out: &mpsc::Sender<Message>,
) -> String {
    let Some(params) = value.and_then(|v| serde_json::from_value::<TurnStartParams>(v).ok()) else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid turn.start params",
        ))
        .unwrap_or_default();
    };
    let db = Arc::clone(&state.db);
    let request_id = id.clone();
    let session = params.session_id.clone();
    let cwd = params.cwd.clone().unwrap_or_else(|| ".".into());
    let result = tokio::task::spawn_blocking(
        move || -> anyhow::Result<(String, String, SequencedEvent)> {
            let session_id = match session {
                Some(id) if db.get_session(&id)?.is_some() => id,
                Some(id) => id,
                None => db.create_session(&cwd)?.id,
            };
            let turn_id = uuid::Uuid::new_v4().to_string();
            let event = db.append_event(
                &session_id,
                &TurnEvent::TurnStarted {
                    turn_id: turn_id.clone(),
                },
                Some(&serde_json::to_string(&request_id)?),
            )?;
            Ok((session_id, turn_id, event))
        },
    )
    .await;
    match result {
        Ok(Ok((session_id, turn_id, event))) => {
            let mut params = serde_json::to_value(&event).unwrap_or_default();
            params["request_id"] = serde_json::to_value(&id).unwrap_or_default();
            let notification = Notification {
                jsonrpc: JsonRpc::default(),
                method: METHOD_TURN_EVENT.to_string(),
                params: Some(params),
            };
            if let Ok(text) = serde_json::to_string(&notification) {
                let _ = out.send(Message::Text(text.into())).await;
            }
            serde_json::to_string(&Response::ok(
                id,
                TurnStartResult {
                    session_id,
                    turn_id,
                },
            ))
            .unwrap_or_default()
        }
        Ok(Err(e)) => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INTERNAL_ERROR,
            e.to_string(),
        ))
        .unwrap_or_default(),
        Err(e) => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INTERNAL_ERROR,
            e.to_string(),
        ))
        .unwrap_or_default(),
    }
}

async fn cancel_turn(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(params) = value.and_then(|v| serde_json::from_value::<TurnCancelParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid turn.cancel params",
        ))
        .unwrap_or_default();
    };
    let cancelled = state
        .runtime
        .cancellation
        .cancel(params.turn_id.parse().unwrap_or_default());
    serde_json::to_string(&Response::ok(
        id,
        TurnCancelResult {
            session_id: params.session_id,
            turn_id: params.turn_id,
            cancelled,
        },
    ))
    .unwrap_or_default()
}

async fn replay_turn(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(params) = value.and_then(|v| serde_json::from_value::<TurnReplayParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid replay params",
        ))
        .unwrap_or_default();
    };
    let db = Arc::clone(&state.db);
    let result = tokio::task::spawn_blocking(move || {
        db.replay_events(&params.session_id, params.after_sequence, params.limit)
    })
    .await;
    match result {
        Ok(Ok(events)) => {
            let next_sequence = events.last().map(|e| e.sequence);
            serde_json::to_string(&Response::ok(
                id,
                TurnReplayResult {
                    events,
                    next_sequence,
                },
            ))
            .unwrap_or_default()
        }
        Ok(Err(e)) => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INTERNAL_ERROR,
            e.to_string(),
        ))
        .unwrap_or_default(),
        Err(e) => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INTERNAL_ERROR,
            e.to_string(),
        ))
        .unwrap_or_default(),
    }
}

/// Run the daemon on the given Unix socket path, blocking until shutdown.
pub async fn run(socket_path: PathBuf) -> Result<()> {
    let _lock = acquire_lock()?;

    let config = match tokio::task::spawn_blocking(tau_core::config::Config::load).await {
        Ok(Ok(c)) => {
            match tau_core::config::config_path() {
                Ok(p) if p.exists() => tracing::info!(path = %p.display(), "loaded config"),
                _ => tracing::info!("no config file found (using defaults)"),
            }
            c
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "config load failed; using defaults");
            tau_core::config::Config::default()
        }
        Err(e) => return Err(anyhow::anyhow!("config worker failed: {e}")),
    };
    let state = {
        let path = tokio::task::spawn_blocking(tau_core::db::default_db_path).await??;
        let db = match tokio::task::spawn_blocking(move || tau_core::db::Db::open(&path)).await? {
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
