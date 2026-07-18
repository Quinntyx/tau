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
    sync::{Mutex, broadcast, mpsc, oneshot},
};

mod completion;
pub mod runtime;

/// Per-process server state shared with every connection.
#[derive(Clone)]
pub struct AppState {
    started: Instant,
    config: Arc<tau_core::config::Config>,
    db: Arc<tau_core::db::Db>,
    runtime: runtime::Runtime,
    sessions: Arc<Mutex<HashMap<String, Arc<runtime::SessionTurnQueue<String>>>>>,
    events: tokio::sync::broadcast::Sender<SequencedEvent>,
    policy: Arc<PolicyBroker>,
    provider_override: Option<tau_core::provider::Provider>,
    git_acks: Arc<Mutex<HashMap<String, bool>>>,
}

struct PolicyBroker {
    events: broadcast::Sender<Notification<serde_json::Value>>,
    waiters: Mutex<HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>,
}

#[derive(Clone, Debug)]
struct NegotiatedProtocol {
    capabilities: Vec<Capability>,
}

impl PolicyBroker {
    fn new() -> Self {
        let (events, _) = broadcast::channel(4096);
        Self {
            events,
            waiters: Mutex::new(HashMap::new()),
        }
    }

    async fn resolve(&self, request_id: &str, value: Result<serde_json::Value, String>) {
        if let Some(waiter) = self.waiters.lock().await.remove(request_id) {
            let _ = waiter.send(value);
        }
    }

    async fn interrupt_all(&self) {
        let mut waiters = self.waiters.lock().await;
        for (_, waiter) in waiters.drain() {
            let _ = waiter.send(Err("prompt interrupted by daemon restart".to_owned()));
        }
    }
}

impl AppState {
    pub fn new(config: Arc<tau_core::config::Config>, db: tau_core::db::Db) -> Self {
        Self {
            started: Instant::now(),
            config,
            db: Arc::new(db),
            runtime: runtime::Runtime::new(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            events: tokio::sync::broadcast::channel(4096).0,
            policy: Arc::new(PolicyBroker::new()),
            provider_override: None,
            git_acks: Arc::new(Mutex::new(HashMap::new())),
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

    pub(crate) fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<SequencedEvent> {
        self.events.subscribe()
    }

    pub(crate) async fn publish_event(
        &self,
        session_id: &str,
        event: TurnEvent,
        request_id: Option<&Id>,
    ) -> anyhow::Result<SequencedEvent> {
        let db = Arc::clone(&self.db);
        let session_id = session_id.to_owned();
        let request = request_id.map(serde_json::to_string).transpose()?;
        let persisted = tokio::task::spawn_blocking(move || {
            db.append_event(&session_id, &event, request.as_deref())
        })
        .await??;
        let _ = self.events.send(persisted.clone());
        Ok(persisted)
    }

    /// Publish a durable typed policy request and wait indefinitely for its
    /// terminal reply. The caller is the turn runner: resolving the prompt
    /// wakes this future, while the database remains the replay authority.
    pub async fn request_policy_prompt(
        &self,
        method: &str,
        session_id: &str,
        turn_id: &str,
        kind: &str,
        mut payload: serde_json::Value,
        client_id: &str,
    ) -> Result<serde_json::Value> {
        let _requested_id = payload
            .get("request_id")
            .and_then(serde_json::Value::as_str)
            .context("policy request missing request_id")?
            .to_owned();
        let db = Arc::clone(&self.db);
        let session_id_owned = session_id.to_owned();
        let turn_id_owned = turn_id.to_owned();
        let kind_owned = kind.to_owned();
        let payload_json = serde_json::to_string(&payload)?;
        let client_id_owned = client_id.to_owned();
        let idempotency_owned = payload
            .get("idempotency_key")
            .and_then(serde_json::Value::as_str)
            .filter(|key| !key.is_empty())
            .map(str::to_owned);
        let prompt = tokio::task::spawn_blocking(move || {
            db.create_interactive_prompt(
                &session_id_owned,
                &turn_id_owned,
                &kind_owned,
                &payload_json,
                &client_id_owned,
                idempotency_owned.as_deref(),
            )
        })
        .await??;
        if prompt.status != "pending" {
            return prompt
                .reply_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?
                .context("terminal policy prompt has no reply");
        }
        let request_id = prompt.id.clone();
        payload["request_id"] = serde_json::Value::String(request_id.clone());
        let (tx, rx) = oneshot::channel();
        self.policy
            .waiters
            .lock()
            .await
            .insert(request_id.clone(), tx);
        let _ = self.policy.events.send(Notification {
            jsonrpc: JsonRpc::default(),
            method: method.to_owned(),
            params: Some(payload),
        });
        match rx.await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(error)) => anyhow::bail!(error),
            Err(_) => anyhow::bail!("policy prompt broker closed"),
        }
    }

    /// Test-only-in-practice provider seam: the transport and completion
    /// implementation remain production code while the external model is
    /// scripted at the provider boundary.
    pub fn with_provider(mut self, provider: tau_core::provider::Provider) -> Self {
        self.provider_override = Some(provider);
        self
    }

    pub(crate) fn provider_override(&self) -> Option<&tau_core::provider::Provider> {
        self.provider_override.as_ref()
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
    let connection_id = state.runtime.connections.attach();
    let client_id = connection_id.to_string();
    // Negotiation is connection-scoped: a successful handshake on one socket
    // must never authorize requests arriving on another socket.
    let negotiated = Arc::new(Mutex::new(None::<NegotiatedProtocol>));
    let (mut sink, mut input) = futures::StreamExt::split(socket);
    let (out, mut outgoing) = mpsc::channel::<Message>(128);
    let mut events = state.subscribe_events();
    let event_out = out.clone();
    let event_forwarder = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            let notification = Notification {
                jsonrpc: JsonRpc::default(),
                method: METHOD_TURN_EVENT.to_string(),
                params: Some(event),
            };
            let Ok(text) = serde_json::to_string(&notification) else {
                continue;
            };
            if event_out.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let mut policy_events = state.policy.events.subscribe();
    let policy_out = out.clone();
    let policy_forwarder = tokio::spawn(async move {
        while let Ok(notification) = policy_events.recv().await {
            let Ok(text) = serde_json::to_string(&notification) else {
                continue;
            };
            if policy_out.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
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
                let client_id = client_id.clone();
                let negotiated = Arc::clone(&negotiated);
                tokio::spawn(async move {
                    let response = handle_request(req, &state, &out, &client_id, &negotiated).await;
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
    policy_forwarder.abort();
    drop(out);
    event_forwarder.abort();
    let _ = writer.await;
    state.runtime.connections.detach(connection_id);
}

/// Dispatch one non-streaming JSON-RPC request into a response.
async fn handle_request(
    req: Request,
    state: &AppState,
    out: &mpsc::Sender<Message>,
    client_id: &str,
    negotiated: &Arc<Mutex<Option<NegotiatedProtocol>>>,
) -> String {
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
        METHOD_PROTOCOL_NEGOTIATE => {
            let Some(params) = req
                .params
                .and_then(|value| serde_json::from_value::<ProtocolNegotiateParams>(value).ok())
            else {
                return serde_json::to_string(&Response::<serde_json::Value>::err(
                    id,
                    INVALID_PARAMS,
                    "invalid protocol negotiation parameters",
                ))
                .unwrap_or_default();
            };
            if params.version.major != 1 || params.version.minor > 0 {
                return serde_json::to_string(&Response::<serde_json::Value>::err(
                    id,
                    INVALID_PARAMS,
                    "unsupported protocol version",
                ))
                .unwrap_or_default();
            }
            let server_capabilities = [
                Capability::TurnStreaming,
                Capability::TurnCancellation,
                Capability::EventReplay,
                Capability::Idempotency,
                Capability::ArtifactReferences,
            ];
            let capabilities = params
                .capabilities
                .into_iter()
                .filter(|capability| server_capabilities.contains(capability))
                .collect::<Vec<_>>();
            *negotiated.lock().await = Some(NegotiatedProtocol {
                capabilities: capabilities.clone(),
            });
            serde_json::to_string(&Response::ok(
                id,
                ProtocolNegotiateResult {
                    version: ProtocolVersion { major: 1, minor: 0 },
                    capabilities,
                },
            ))
            .unwrap_or_default()
        }
        METHOD_PROJECT_LIST => project_list(id, req.params, state).await,
        METHOD_PROJECT_CREATE => project_create(id, req.params, state).await,
        METHOD_PROJECT_RENAME => project_rename(id, req.params, state).await,
        METHOD_PROJECT_REPATH => project_repath(id, req.params, state).await,
        METHOD_PROJECT_UNREGISTER => project_unregister(id, req.params, state).await,
        METHOD_PROJECT_REACTIVATE => project_reactivate(id, req.params, state).await,
        METHOD_PROJECT_NEW_ID => project_new_id(id, req.params, state).await,
        METHOD_TURN_START => {
            if let Some(error) =
                require_capability(id.clone(), negotiated, Capability::TurnStreaming).await
            {
                error
            } else {
                start_turn(id, req.params, state, out).await
            }
        }
        METHOD_TURN_CANCEL => {
            if let Some(error) =
                require_capability(id.clone(), negotiated, Capability::TurnCancellation).await
            {
                error
            } else {
                cancel_turn(id, req.params, state).await
            }
        }
        METHOD_TURN_RESPONSE => respond_turn(id, req.params, state).await,
        METHOD_SESSION_CREATE => session_create(id, req.params, state).await,
        METHOD_SESSION_LIST => session_list(id, req.params, state).await,
        METHOD_SESSION_GET_HISTORY => session_history(id, req.params, state).await,
        METHOD_SESSION_RENAME => session_rename(id, req.params, state).await,
        METHOD_SESSION_ARCHIVE => session_archive(id, req.params, state).await,
        METHOD_SESSION_RESTORE => session_restore(id, req.params, state).await,
        METHOD_TURN_REPLAY => {
            if let Some(error) =
                require_capability(id.clone(), negotiated, Capability::EventReplay).await
            {
                error
            } else {
                replay_turn(id, req.params, state).await
            }
        }
        METHOD_PERMISSION_REPLY => {
            resolve_prompt(id, req.params, state, client_id, "permission", true).await
        }
        METHOD_QUESTION_REPLY => {
            resolve_prompt(id, req.params, state, client_id, "question", false).await
        }
        METHOD_DIFF_DECISION => {
            resolve_prompt(id, req.params, state, client_id, "diff", true).await
        }
        METHOD_PLAN_REPLY => resolve_prompt(id, req.params, state, client_id, "plan", true).await,
        METHOD_AIRTIGHT_REPLY => {
            resolve_prompt(id, req.params, state, client_id, "airtight", true).await
        }
        METHOD_PROMPT_TAKEOVER => takeover_prompt(id, req.params, state, client_id).await,
        METHOD_GIT_STATUS => git_status(id, req.params, state).await,
        METHOD_GIT_FILE => git_file(id, req.params, state).await,
        METHOD_GIT_STAGE => git_path_op(id, req.params, state, true).await,
        METHOD_GIT_UNSTAGE => git_path_op(id, req.params, state, false).await,
        METHOD_GIT_REVERT => git_revert(id, req.params, state).await,
        METHOD_GIT_ACK => git_ack(id, req.params, state).await,
        METHOD_GIT_BRANCHES => git_branches(id, req.params, state).await,
        METHOD_GIT_BRANCH_CREATE => git_branch_create(id, req.params, state).await,
        METHOD_GIT_BRANCH_SWITCH => git_branch_switch(id, req.params, state).await,
        other => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        ))
        .unwrap_or_default(),
    }
}

fn project_wire(project: tau_core::db::Project) -> tau_proto::projects::Project {
    tau_proto::projects::Project {
        id: project.id,
        name: project.name,
        root: project.root,
        active: project.active,
        created_at: project.created_at,
        updated_at: project.updated_at,
    }
}

fn rpc_error(id: Id, code: i32, message: impl Into<String>) -> String {
    serde_json::to_string(&Response::<serde_json::Value>::err(
        id,
        code,
        message.into(),
    ))
    .unwrap_or_default()
}

fn git_error(id: Id, error: impl std::fmt::Display) -> String {
    serde_json::to_string(&Response::<serde_json::Value>::err(
        id,
        INVALID_PARAMS,
        error.to_string(),
    ))
    .unwrap_or_default()
}

async fn project_list(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let params = match value {
        None => ProjectListParams::default(),
        Some(value) => match serde_json::from_value(value) {
            Ok(params) => params,
            Err(error) => return rpc_error(id, INVALID_PARAMS, error.to_string()),
        },
    };
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || {
        db.list_projects(params.include_inactive.unwrap_or(false))
    })
    .await
    {
        Ok(Ok(projects)) => serde_json::to_string(&Response::ok(
            id,
            ProjectListResult {
                projects: projects.into_iter().map(project_wire).collect(),
            },
        ))
        .unwrap_or_default(),
        Ok(Err(error)) => rpc_error(id, INTERNAL_ERROR, error.to_string()),
        Err(error) => rpc_error(id, INTERNAL_ERROR, error.to_string()),
    }
}

async fn project_create(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(value) = value else {
        return rpc_error(id, INVALID_PARAMS, "project parameters required");
    };
    let params: ProjectCreateParams = match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => return rpc_error(id, INVALID_PARAMS, error.to_string()),
    };
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || db.create_project(&params.name, params.root)).await {
        Ok(Ok(project)) => serde_json::to_string(&Response::ok(
            id,
            ProjectCreateResult {
                project: project_wire(project),
            },
        ))
        .unwrap_or_default(),
        Ok(Err(error)) => rpc_error(id, INVALID_PARAMS, error.to_string()),
        Err(error) => rpc_error(id, INTERNAL_ERROR, error.to_string()),
    }
}

async fn project_rename(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(value) = value else {
        return rpc_error(id, INVALID_PARAMS, "project parameters required");
    };
    let params: ProjectRenameParams = match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => return rpc_error(id, INVALID_PARAMS, error.to_string()),
    };
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || db.rename_project(&params.project_id, &params.name))
        .await
    {
        Ok(Ok(project)) => serde_json::to_string(&Response::ok(
            id,
            ProjectMutationResult {
                project: project_wire(project),
            },
        ))
        .unwrap_or_default(),
        Ok(Err(error)) => rpc_error(id, INVALID_PARAMS, error.to_string()),
        Err(error) => rpc_error(id, INTERNAL_ERROR, error.to_string()),
    }
}

async fn project_repath(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(value) = value else {
        return rpc_error(id, INVALID_PARAMS, "project parameters required");
    };
    let params: ProjectRepathParams = match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => return rpc_error(id, INVALID_PARAMS, error.to_string()),
    };
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || db.repath_project(&params.project_id, params.root))
        .await
    {
        Ok(Ok(project)) => serde_json::to_string(&Response::ok(
            id,
            ProjectMutationResult {
                project: project_wire(project),
            },
        ))
        .unwrap_or_default(),
        Ok(Err(error)) => rpc_error(id, INVALID_PARAMS, error.to_string()),
        Err(error) => rpc_error(id, INTERNAL_ERROR, error.to_string()),
    }
}

async fn project_unregister(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    project_lifecycle(id, value, state, false).await
}

async fn project_reactivate(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    project_lifecycle(id, value, state, true).await
}

async fn project_lifecycle(
    id: Id,
    value: Option<serde_json::Value>,
    state: &AppState,
    active: bool,
) -> String {
    let Some(value) = value else {
        return rpc_error(id, INVALID_PARAMS, "project parameters required");
    };
    let params: ProjectIdParams = match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => return rpc_error(id, INVALID_PARAMS, error.to_string()),
    };
    let db = Arc::clone(&state.db);
    let result = tokio::task::spawn_blocking(move || {
        if active {
            db.reactivate_project(&params.project_id)
        } else {
            db.unregister_project(&params.project_id)
        }
    })
    .await;
    match result {
        Ok(Ok(project)) => serde_json::to_string(&Response::ok(
            id,
            ProjectActionResult {
                project: project_wire(project),
            },
        ))
        .unwrap_or_default(),
        Ok(Err(error)) => rpc_error(id, INVALID_PARAMS, error.to_string()),
        Err(error) => rpc_error(id, INTERNAL_ERROR, error.to_string()),
    }
}

async fn project_new_id(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(value) = value else {
        return rpc_error(id, INVALID_PARAMS, "project_id is required");
    };
    let params: ProjectNewIdParams = match serde_json::from_value(value) {
        Ok(params) => params,
        Err(error) => return rpc_error(id, INVALID_PARAMS, error.to_string()),
    };
    let Some(source_id) = params.project_id else {
        return rpc_error(id, INVALID_PARAMS, "project_id is required");
    };
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || {
        let project = db.new_project_id(&source_id)?;
        Ok::<_, anyhow::Error>(ProjectNewIdResult {
            project_id: project.id,
        })
    })
    .await
    {
        Ok(Ok(result)) => serde_json::to_string(&Response::ok(id, result)).unwrap_or_default(),
        Ok(Err(error)) => rpc_error(id, INVALID_PARAMS, error.to_string()),
        Err(error) => rpc_error(id, INTERNAL_ERROR, error.to_string()),
    }
}

fn session_record(r: tau_core::db::SessionRecord) -> tau_proto::session::SessionRecord {
    tau_proto::session::SessionRecord {
        id: tau_proto::session::SessionId::new(r.id),
        project_id: tau_proto::session::ProjectId::new(r.project_id.as_str()),
        cwd: r.cwd,
        title: r.title,
        created_at: r.created_at,
        updated_at: r.updated_at,
        archived_at: r.archived_at,
    }
}

async fn session_create(id: Id, raw: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(p) =
        raw.and_then(|v| serde_json::from_value::<tau_proto::session::CreateParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid session.create parameters",
        ))
        .unwrap_or_default();
    };
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || {
        db.create_session_for_project(&tau_core::db::ProjectId::new(p.project_id.as_str()), &p.cwd)
    })
    .await
    {
        Ok(Ok(r)) => {
            serde_json::to_string(&Response::ok(id, session_record(r))).unwrap_or_default()
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

async fn session_list(id: Id, raw: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(p) =
        raw.and_then(|v| serde_json::from_value::<tau_proto::session::ListParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid session.list parameters",
        ))
        .unwrap_or_default();
    };
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || {
        db.list_sessions_for_project(
            &tau_core::db::ProjectId::new(p.project_id.as_str()),
            p.include_archived,
        )
    })
    .await
    {
        Ok(Ok(rs)) => serde_json::to_string(&Response::ok(
            id,
            rs.into_iter().map(session_record).collect::<Vec<_>>(),
        ))
        .unwrap_or_default(),
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

async fn session_history(id: Id, raw: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(p) =
        raw.and_then(|v| serde_json::from_value::<tau_proto::session::HistoryParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid session.get_history parameters",
        ))
        .unwrap_or_default();
    };
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || {
        let project_id = tau_core::db::ProjectId::new(p.project_id.as_str());
        if db
            .get_session_record_for_project(&project_id, p.session_id.as_str())?
            .is_none()
        {
            anyhow::bail!("session is not in the requested project");
        }
        db.replay_events(p.session_id.as_str(), p.after_sequence, p.limit)
    })
    .await
    {
        Ok(Ok(r)) => serde_json::to_string(&Response::ok(id, r)).unwrap_or_default(),
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

async fn session_rename(id: Id, raw: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(p) =
        raw.and_then(|v| serde_json::from_value::<tau_proto::session::RenameParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid session.rename parameters",
        ))
        .unwrap_or_default();
    };
    let project_id = p.project_id.clone();
    session_mutate(id, state, project_id, move |db| {
        db.rename_session_for_project(
            &tau_core::db::ProjectId::new(p.project_id.as_str()),
            p.session_id.as_str(),
            &p.title,
        )
    })
    .await
}
async fn session_archive(id: Id, raw: Option<serde_json::Value>, state: &AppState) -> String {
    session_toggle(id, raw, state, true).await
}
async fn session_restore(id: Id, raw: Option<serde_json::Value>, state: &AppState) -> String {
    session_toggle(id, raw, state, false).await
}
async fn session_toggle(
    id: Id,
    raw: Option<serde_json::Value>,
    state: &AppState,
    archive: bool,
) -> String {
    let Some(p) =
        raw.and_then(|v| serde_json::from_value::<tau_proto::session::SessionIdParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid session lifecycle parameters",
        ))
        .unwrap_or_default();
    };
    let project_id = p.project_id.clone();
    session_mutate(id, state, project_id, move |db| {
        if archive {
            db.archive_session_for_project(
                &tau_core::db::ProjectId::new(p.project_id.as_str()),
                p.session_id.as_str(),
            )
        } else {
            db.restore_session_for_project(
                &tau_core::db::ProjectId::new(p.project_id.as_str()),
                p.session_id.as_str(),
            )
        }
    })
    .await
}
async fn session_mutate<F>(
    id: Id,
    state: &AppState,
    project_id: tau_proto::session::ProjectId,
    f: F,
) -> String
where
    F: FnOnce(&tau_core::db::Db) -> anyhow::Result<Option<tau_core::db::SessionRecord>>
        + Send
        + 'static,
{
    let db = Arc::clone(&state.db);
    match tokio::task::spawn_blocking(move || f(&db)).await {
        Ok(Ok(Some(record))) => {
            serde_json::to_string(&Response::ok(id, session_record(record))).unwrap_or_default()
        }
        Ok(Ok(None)) => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            METHOD_NOT_FOUND,
            format!("session not found in project {}", project_id.as_str()),
        ))
        .unwrap_or_default(),
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

fn parse_git<T: serde::de::DeserializeOwned>(
    value: Option<serde_json::Value>,
    message: &str,
) -> anyhow::Result<T> {
    value
        .ok_or_else(|| anyhow::anyhow!(message.to_owned()))
        .and_then(|value| serde_json::from_value(value).map_err(Into::into))
}

fn ack_response(id: Id) -> String {
    serde_json::to_string(&Response::ok(
        id,
        tau_proto::git::GitAckResult { acknowledged: true },
    ))
    .unwrap_or_default()
}

async fn git_status(id: Id, value: Option<serde_json::Value>, _state: &AppState) -> String {
    let Ok(params) =
        parse_git::<tau_proto::git::GitStatusParams>(value, "invalid git status parameters")
    else {
        return git_error(id, "invalid git status parameters");
    };
    let result = tokio::task::spawn_blocking(move || {
        let workspace = tau_core::git::GitWorkspace::open(params.project)?;
        let status = workspace.status(std::path::Path::new("."))?;
        Ok::<_, anyhow::Error>(tau_proto::git::GitStatusResult {
            branch: status.branch,
            files: status
                .files
                .into_iter()
                .map(|file| tau_proto::git::GitFileStatus {
                    path: file.path,
                    staged: file.staged,
                    modified: file.modified,
                    untracked: file.untracked,
                })
                .collect(),
        })
    })
    .await;
    match result {
        Ok(Ok(result)) => serde_json::to_string(&Response::ok(id, result)).unwrap_or_default(),
        Ok(Err(error)) => git_error(id, error),
        Err(error) => git_error(id, error),
    }
}

async fn git_file(id: Id, value: Option<serde_json::Value>, _state: &AppState) -> String {
    let Ok(params) =
        parse_git::<tau_proto::git::GitFileParams>(value, "invalid git file parameters")
    else {
        return git_error(id, "invalid git file parameters");
    };
    let result = tokio::task::spawn_blocking(move || {
        let workspace = tau_core::git::GitWorkspace::open(&params.project)?;
        let file = workspace.file(
            std::path::Path::new("."),
            std::path::Path::new(&params.path),
        )?;
        Ok::<_, anyhow::Error>(tau_proto::git::GitFileResult {
            path: file.path,
            content: file.content,
            diff: file.diff,
        })
    })
    .await;
    match result {
        Ok(Ok(result)) => serde_json::to_string(&Response::ok(id, result)).unwrap_or_default(),
        Ok(Err(error)) => git_error(id, error),
        Err(error) => git_error(id, error),
    }
}

async fn git_path_op(
    id: Id,
    value: Option<serde_json::Value>,
    _state: &AppState,
    stage: bool,
) -> String {
    let Ok(params) =
        parse_git::<tau_proto::git::GitPathParams>(value, "invalid git path parameters")
    else {
        return git_error(id, "invalid git path parameters");
    };
    let result = tokio::task::spawn_blocking(move || {
        let workspace = tau_core::git::GitWorkspace::open(&params.project)?;
        let project = std::path::Path::new(".");
        let path = std::path::Path::new(&params.path);
        if stage {
            workspace.stage(project, path)?;
        } else {
            workspace.unstage(project, path)?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    match result {
        Ok(Ok(())) => ack_response(id),
        Ok(Err(error)) => git_error(id, error),
        Err(error) => git_error(id, error),
    }
}

async fn git_revert(id: Id, value: Option<serde_json::Value>, _state: &AppState) -> String {
    let Ok(params) =
        parse_git::<tau_proto::git::GitRevertParams>(value, "invalid revert parameters")
    else {
        return git_error(id, "invalid revert parameters");
    };
    if !params.validate() {
        return git_error(id, "confirmation and safe path required");
    }
    let result = tokio::task::spawn_blocking(move || {
        let workspace = tau_core::git::GitWorkspace::open(&params.project)?;
        workspace.revert(
            std::path::Path::new("."),
            std::path::Path::new(&params.path),
            params.confirmed,
        )
    })
    .await;
    match result {
        Ok(Ok(())) => ack_response(id),
        Ok(Err(error)) => git_error(id, error),
        Err(error) => git_error(id, error),
    }
}

async fn git_ack(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let Ok(params) = parse_git::<tau_proto::git::GitAckParams>(value, "invalid ack parameters")
    else {
        return git_error(id, "invalid ack parameters");
    };
    let project = params.project.clone();
    let valid =
        tokio::task::spawn_blocking(move || tau_core::git::GitWorkspace::open(project).map(|_| ()))
            .await;
    match valid {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return git_error(id, error),
        Err(error) => return git_error(id, error),
    }
    let key = format!("{}\0{}", params.project, params.operation);
    let mut acknowledgements = state.git_acks.lock().await;
    let acknowledged = *acknowledgements.entry(key).or_insert(params.acknowledged);
    serde_json::to_string(&Response::ok(
        id,
        tau_proto::git::GitAckResult { acknowledged },
    ))
    .unwrap_or_default()
}

async fn git_branches(id: Id, value: Option<serde_json::Value>, _state: &AppState) -> String {
    let Ok(params) =
        parse_git::<tau_proto::git::GitBranchesParams>(value, "invalid branches parameters")
    else {
        return git_error(id, "invalid branches parameters");
    };
    let result = tokio::task::spawn_blocking(move || {
        let workspace = tau_core::git::GitWorkspace::open(params.project)?;
        let current = workspace.status(std::path::Path::new("."))?.branch;
        let branches = workspace
            .branches(std::path::Path::new("."))?
            .into_iter()
            .map(|branch| tau_proto::git::GitBranch {
                name: branch.name,
                current: branch.current,
            })
            .collect();
        Ok::<_, anyhow::Error>(tau_proto::git::GitBranchesResult { current, branches })
    })
    .await;
    match result {
        Ok(Ok(result)) => serde_json::to_string(&Response::ok(id, result)).unwrap_or_default(),
        Ok(Err(error)) => git_error(id, error),
        Err(error) => git_error(id, error),
    }
}

async fn git_branch_create(id: Id, value: Option<serde_json::Value>, _state: &AppState) -> String {
    let Ok(params) =
        parse_git::<tau_proto::git::GitBranchCreateParams>(value, "invalid branch parameters")
    else {
        return git_error(id, "invalid branch parameters");
    };
    let result = tokio::task::spawn_blocking(move || {
        let workspace = tau_core::git::GitWorkspace::open(&params.project)?;
        workspace.create_branch(std::path::Path::new("."), &params.name)
    })
    .await;
    match result {
        Ok(Ok(())) => ack_response(id),
        Ok(Err(error)) => git_error(id, error),
        Err(error) => git_error(id, error),
    }
}

async fn git_branch_switch(id: Id, value: Option<serde_json::Value>, _state: &AppState) -> String {
    let Ok(params) =
        parse_git::<tau_proto::git::GitBranchSwitchParams>(value, "invalid branch parameters")
    else {
        return git_error(id, "invalid branch parameters");
    };
    let result = tokio::task::spawn_blocking(move || {
        let workspace = tau_core::git::GitWorkspace::open(&params.project)?;
        workspace.switch_branch(std::path::Path::new("."), &params.name)
    })
    .await;
    match result {
        Ok(Ok(())) => ack_response(id),
        Ok(Err(error)) => git_error(id, error),
        Err(error) => git_error(id, error),
    }
}

async fn require_capability(
    id: Id,
    negotiated: &Arc<Mutex<Option<NegotiatedProtocol>>>,
    required: Capability,
) -> Option<String> {
    let state = negotiated.lock().await;
    let Some(protocol) = state.as_ref() else {
        return Some(
            serde_json::to_string(&Response::<serde_json::Value>::err(
                id,
                METHOD_NOT_FOUND,
                "protocol negotiation required before session.turn.*; reconnect and call protocol.negotiate (the daemon may be stale)",
            ))
            .unwrap_or_default(),
        );
    };
    if !protocol.capabilities.contains(&required) {
        return Some(
            serde_json::to_string(&Response::<serde_json::Value>::err(
                id,
                METHOD_NOT_FOUND,
                format!(
                    "requested session capability is unavailable after negotiation: {required:?}; reconnect to a compatible daemon or negotiate that capability"
                ),
            ))
            .unwrap_or_default(),
        );
    }
    None
}

async fn resolve_prompt(
    id: Id,
    value: Option<serde_json::Value>,
    state: &AppState,
    client_id: &str,
    kind: &str,
    owner_required: bool,
) -> String {
    let Some(value) = value else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "missing prompt reply",
        ))
        .unwrap_or_default();
    };
    let request_id = value
        .get("request_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let key = value
        .get("idempotency_key")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if request_id.is_empty() || key.is_empty() {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "request_id and idempotency_key are required",
        ))
        .unwrap_or_default();
    }
    let validation = match kind {
        "permission" => serde_json::from_value::<PermissionReply>(value.clone())
            .map_err(|error| error.to_string())
            .and_then(|reply| reply.validate().map_err(|error| error.to_string())),
        "question" => serde_json::from_value::<QuestionReply>(value.clone())
            .map_err(|error| error.to_string())
            .and_then(|reply| reply.validate().map_err(|error| error.to_string())),
        "diff" => serde_json::from_value::<DiffDecision>(value.clone())
            .map_err(|error| error.to_string())
            .and_then(|reply| reply.validate().map_err(|error| error.to_string())),
        "plan" => serde_json::from_value::<PlanReply>(value.clone())
            .map_err(|error| error.to_string())
            .and_then(|reply| reply.validate().map_err(|error| error.to_string())),
        "airtight" => serde_json::from_value::<AirtightPromptReply>(value.clone())
            .map_err(|error| error.to_string())
            .and_then(|reply| reply.validate().map_err(|error| error.to_string())),
        _ => Err("unknown prompt kind".to_owned()),
    };
    if let Err(error) = validation {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            format!("invalid {kind} reply: {error}"),
        ))
        .unwrap_or_default();
    }
    // Keep the wire boundary typed.  The database deliberately stores the
    // reply as JSON, but accepting an arbitrary object here would let a
    // client smuggle a reply for a different prompt kind (or actor policy).
    let actor = match kind {
        "permission" => {
            serde_json::from_value::<PermissionReply>(value.clone()).map(|reply| reply.actor)
        }
        "question" => {
            serde_json::from_value::<QuestionReply>(value.clone()).map(|reply| reply.actor)
        }
        "diff" => serde_json::from_value::<DiffDecision>(value.clone()).map(|reply| reply.actor),
        "plan" => serde_json::from_value::<PlanReply>(value.clone()).map(|reply| reply.actor),
        "airtight" => {
            serde_json::from_value::<AirtightPromptReply>(value.clone()).map(|reply| reply.actor)
        }
        _ => {
            return serde_json::to_string(&Response::<serde_json::Value>::err(
                id,
                INVALID_PARAMS,
                "unknown prompt kind",
            ))
            .unwrap_or_default();
        }
    };
    let actor = match actor {
        Ok(actor) => actor,
        Err(error) => {
            return serde_json::to_string(&Response::<serde_json::Value>::err(
                id,
                INVALID_PARAMS,
                format!("invalid {kind} reply: {error}"),
            ))
            .unwrap_or_default();
        }
    };
    // Airtight/policy-affecting decisions cannot be delegated to the super
    // agent.  Super may answer only the soft question prompt; the canonical
    // policy engine remains the authority for permissions.
    if matches!(actor, PromptActor::SuperAgent) && kind != "question" {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            -32001,
            "actor is not authorized for this prompt",
        ))
        .unwrap_or_default();
    }
    let db = Arc::clone(&state.db);
    let reply = value.to_string();
    let client = client_id.to_string();
    let kind = kind.to_string();
    let broker_request_id = request_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let prompt = db.get_interactive_prompt(&request_id)?;
        if prompt.kind != kind {
            anyhow::bail!("prompt kind mismatch")
        };
        let resolved = if kind == "permission" {
            let permission: PermissionReply = serde_json::from_str(&reply)?;
            let resolved = match permission.choice {
                PermissionChoice::Reject => db.reject_interactive_prompt(
                    &request_id,
                    &client,
                    &reply,
                    &key,
                    owner_required,
                )?,
                PermissionChoice::Allow => db.resolve_interactive_prompt(
                    &request_id,
                    &client,
                    &reply,
                    &key,
                    owner_required,
                )?,
            };
            if matches!(
                permission.scope,
                PermissionScope::Session | PermissionScope::Global
            ) {
                let payload: PermissionRequest = serde_json::from_str(&prompt.payload_json)?;
                let scope = match permission.scope {
                    PermissionScope::Session => "session",
                    PermissionScope::Global => "global",
                    _ => unreachable!(),
                };
                let pattern = format!("{}:{}", payload.tool, payload.arguments);
                db.save_policy_decision(
                    (scope == "session").then_some(prompt.session_id.as_str()),
                    scope,
                    &serde_json::to_string(&permission.actor)?,
                    &pattern,
                    &serde_json::to_string(&permission.choice)?,
                )?;
            }
            resolved
        } else {
            db.resolve_interactive_prompt(&request_id, &client, &reply, &key, owner_required)?
        };
        Ok::<_, anyhow::Error>(resolved)
    })
    .await;
    match result {
        Ok(Ok(prompt)) => {
            if let Some(reply_json) = prompt.reply_json.as_deref() {
                match serde_json::from_str(reply_json) {
                    Ok(reply) => state.policy.resolve(&broker_request_id, Ok(reply)).await,
                    Err(error) => {
                        state
                            .policy
                            .resolve(&broker_request_id, Err(error.to_string()))
                            .await;
                    }
                }
            }
            serde_json::to_string(&Response::ok(id, prompt)).unwrap_or_default()
        }
        Ok(Err(e)) => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
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

async fn takeover_prompt(
    id: Id,
    value: Option<serde_json::Value>,
    state: &AppState,
    client_id: &str,
) -> String {
    let Some(value) = value else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "missing takeover",
        ))
        .unwrap_or_default();
    };
    let takeover = match serde_json::from_value::<PromptTakeover>(value) {
        Ok(value) if !value.request_id.is_empty() && !value.idempotency_key.is_empty() => value,
        _ => {
            return serde_json::to_string(&Response::<serde_json::Value>::err(
                id,
                INVALID_PARAMS,
                "request_id and idempotency_key are required",
            ))
            .unwrap_or_default();
        }
    };
    let request_id = takeover.request_id;
    let idempotency_key = takeover.idempotency_key;
    let db = Arc::clone(&state.db);
    let client = client_id.to_string();
    let result = tokio::task::spawn_blocking(move || {
        db.take_over_interactive_prompt(&request_id, &client, &idempotency_key)
    })
    .await;
    match result {
        Ok(Ok(prompt)) => serde_json::to_string(&Response::ok(id, prompt)).unwrap_or_default(),
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

async fn start_turn(
    id: Id,
    value: Option<serde_json::Value>,
    state: &AppState,
    _out: &mpsc::Sender<Message>,
) -> String {
    let Some(value) = value else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid turn.start params",
        ))
        .unwrap_or_default();
    };
    // `project_id` is carried by the current typed protocol revision.  Keep
    // this extraction separate from TurnStartParams so daemons built while a
    // protocol slice is being integrated can still reject invalid project
    // references rather than silently creating an unanchored session.
    let project_id = value
        .get("project_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let Some(params) = serde_json::from_value::<TurnStartParams>(value).ok() else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid turn.start params",
        ))
        .unwrap_or_default();
    };
    let Some(project_id) = project_id else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "project_id is required",
        ))
        .unwrap_or_default();
    };
    let db = Arc::clone(&state.db);
    let request_id = id.clone();
    let session = params.session_id.clone();
    let request_hash = {
        let bytes = serde_json::to_vec(&params).unwrap_or_default();
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    };
    let params_for_db = params.clone();
    let result = tokio::task::spawn_blocking(
        move || -> anyhow::Result<(String, String, Option<SequencedEvent>, Option<TurnStartResult>)> {
            let project = db
                .get_project(&project_id)?
                .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
            if !project.active {
                anyhow::bail!("project is inactive: {project_id}");
            }
            let session_id = match session {
                Some(id) => {
                    let existing = db
                        .get_session(&id)?
                        .ok_or_else(|| anyhow::anyhow!("unknown session: {id}"))?;
                    if existing.project_id != project_id {
                        anyhow::bail!("session does not belong to project: {project_id}");
                    }
                    id
                }
                None => db.create_session(&project_id)?.id,
            };
            let turn_id = uuid::Uuid::new_v4().to_string();
            let result = TurnStartResult { session_id: session_id.clone(), turn_id: turn_id.clone() };
            // Reserve the idempotency key before journaling.  This is the
            // admission gate: concurrent retries must observe the winner and
            // cannot append a second TurnStarted event or enqueue execution.
            if !db.remember_idempotency(&session_id, &params_for_db.idempotency_key, &request_hash, &result)? {
                let existing = db
                    .idempotent_result::<TurnStartResult>(&session_id, &params_for_db.idempotency_key)?
                    .context("idempotency record missing result")?;
                return Ok((
                    existing.session_id.clone(),
                    existing.turn_id.clone(),
                    None,
                    Some(existing),
                ));
            }
            let event = db.append_event(
                &session_id,
                &TurnEvent::TurnStarted {
                    turn_id: turn_id.clone(),
                },
                Some(&serde_json::to_string(&request_id)?),
            )?;
            Ok((session_id, turn_id, Some(event), Some(result)))
        },
    )
    .await;
    match result {
        Ok(Ok((session_id, turn_id, event, Some(result)))) => {
            if event.is_none() {
                return serde_json::to_string(&Response::ok(id, result)).unwrap_or_default();
            }
            // Keep the wire turn identifier as the cancellation key.  Runtime
            // queue identifiers are admission counters and must never leak
            // into the typed protocol.
            let cancel = state.runtime.cancellation.register(turn_id.clone());
            let queue = {
                let mut sessions = state.sessions.lock().await;
                sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| Arc::new(runtime::SessionTurnQueue::new()))
                    .clone()
            };
            let (_admission_id, _completion) = queue.submit(turn_id.clone());
            let _ = state
                .events
                .send(event.expect("new turn has a start event"));
            let job = completion::TurnJob {
                params,
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                request_id: id.clone(),
                cancellation: cancel,
            };
            let worker_queue = queue.clone();
            let worker_state = state.clone();
            tokio::spawn(async move {
                worker_queue
                    .run_next(|_| async move {
                        completion::execute_turn(worker_state, job).await;
                    })
                    .await;
            });
            serde_json::to_string(&Response::ok(id, result)).unwrap_or_default()
        }
        Ok(Ok((_session_id, _turn_id, _event, None))) => {
            serde_json::to_string(&Response::<serde_json::Value>::err(
                id,
                INTERNAL_ERROR,
                "missing turn result",
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
    let cancelled = state.runtime.cancellation.cancel(&params.turn_id);
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

async fn respond_turn(id: Id, value: Option<serde_json::Value>, state: &AppState) -> String {
    let Some(params) = value.and_then(|v| serde_json::from_value::<TurnResponseParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid turn.response params",
        ))
        .unwrap_or_default();
    };

    let db = Arc::clone(&state.db);
    let session_id = params.session_id.clone();
    let session_exists = tokio::task::spawn_blocking(move || {
        db.get_session(&session_id).map(|session| session.is_some())
    })
    .await;
    match session_exists {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            return rpc_error(id, INVALID_PARAMS, "unknown session");
        }
        Ok(Err(error)) => return rpc_error(id, INTERNAL_ERROR, error.to_string()),
        Err(error) => return rpc_error(id, INTERNAL_ERROR, error.to_string()),
    }

    // The runtime's interactive brokers own the eventual answer.  This
    // transport acknowledgement is intentionally immediate: the TUI must not
    // wait for the model turn (or a SQLite write) before returning to input.
    // Until a broker is attached, accepting a well-formed response is the
    // compatible no-op used by the M12 transport boundary.
    serde_json::to_string(&Response::ok(
        id,
        TurnResponseResult {
            session_id: params.session_id,
            turn_id: params.turn_id,
            accepted: true,
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
            let next_sequence = events.last().map(|e| e.sequence + 1);
            serde_json::to_string(&Response::ok(
                id,
                TurnReplayResult {
                    events,
                    next_sequence,
                    gap: false,
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
        // A daemon restart is an explicit interruption boundary. Pending prompts
        // remain replayable, but can never silently resume an old live turn.
        let _ = db.interrupt_interactive_prompts();
        let state = AppState::new(Arc::new(config), db);
        // The in-memory side of the boundary matters as well: queued turns
        // must not survive a daemon lifecycle even when their durable prompt
        // rows have already been marked interrupted.
        state.runtime.interrupt_for_restart();
        state.policy.interrupt_all().await;
        state
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

#[cfg(test)]
mod session_rpc_tests {
    use super::*;

    #[tokio::test]
    async fn session_rpc_lifecycle_is_project_scoped_and_typed() {
        let state = AppState::new(
            Arc::new(tau_core::config::Config::default()),
            tau_core::db::Db::open_in_memory().unwrap(),
        );
        let (out, _received) = mpsc::channel(1);
        let negotiated = Arc::new(Mutex::new(None));
        let request =
            |id, method, params| Request::new(Id::num(id), method, Some(serde_json::json!(params)));
        let response = handle_request(
            request(
                1,
                METHOD_SESSION_CREATE,
                serde_json::json!({
                    "project_id": "project-a", "cwd": "/tmp/a"
                }),
            ),
            &state,
            &out,
            "test-client",
            &negotiated,
        )
        .await;
        let created: Response<serde_json::Value> = serde_json::from_str(&response).unwrap();
        let session: tau_proto::session::SessionRecord =
            serde_json::from_value(created.result.unwrap()).unwrap();
        assert_eq!(session.project_id.as_str(), "project-a");
        assert!(session.title.is_none());

        let response = handle_request(
            request(
                2,
                METHOD_SESSION_LIST,
                serde_json::json!({
                    "project_id": "project-a"
                }),
            ),
            &state,
            &out,
            "test-client",
            &negotiated,
        )
        .await;
        let listed: Response<serde_json::Value> = serde_json::from_str(&response).unwrap();
        let listed: Vec<tau_proto::session::SessionRecord> =
            serde_json::from_value(listed.result.unwrap()).unwrap();
        assert_eq!(listed.len(), 1);

        let response = handle_request(
            request(
                3,
                METHOD_SESSION_RENAME,
                serde_json::json!({
                    "project_id": "project-a", "session_id": session.id, "title": "Work"
                }),
            ),
            &state,
            &out,
            "test-client",
            &negotiated,
        )
        .await;
        let renamed: Response<serde_json::Value> = serde_json::from_str(&response).unwrap();
        let renamed: tau_proto::session::SessionRecord =
            serde_json::from_value(renamed.result.unwrap()).unwrap();
        assert_eq!(renamed.title.as_deref(), Some("Work"));

        let response = handle_request(
            request(
                4,
                METHOD_SESSION_ARCHIVE,
                serde_json::json!({
                    "project_id": "project-a", "session_id": session.id
                }),
            ),
            &state,
            &out,
            "test-client",
            &negotiated,
        )
        .await;
        let archived: Response<serde_json::Value> = serde_json::from_str(&response).unwrap();
        let archived: tau_proto::session::SessionRecord =
            serde_json::from_value(archived.result.unwrap()).unwrap();
        assert!(archived.archived_at.is_some());
    }
}
