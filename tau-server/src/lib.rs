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
}

struct PolicyBroker {
    events: broadcast::Sender<Notification<serde_json::Value>>,
    waiters: Mutex<HashMap<String, oneshot::Sender<Result<serde_json::Value, String>>>>,
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
                tokio::spawn(async move {
                    let response = handle_request(req, &state, &out, &client_id).await;
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
            serde_json::to_string(&Response::ok(
                id,
                ProtocolNegotiateResult {
                    version: ProtocolVersion { major: 1, minor: 0 },
                    capabilities: vec![
                        Capability::TurnStreaming,
                        Capability::TurnCancellation,
                        Capability::EventReplay,
                        Capability::Idempotency,
                        Capability::ArtifactReferences,
                    ],
                },
            ))
            .unwrap_or_default()
        }
        METHOD_TURN_START => start_turn(id, req.params, state, out).await,
        METHOD_TURN_CANCEL => cancel_turn(id, req.params, state).await,
        METHOD_TURN_RESPONSE => respond_turn(id, req.params, state).await,
        METHOD_TURN_REPLAY => replay_turn(id, req.params, state).await,
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
        other => serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        ))
        .unwrap_or_default(),
    }
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
            let session_id = match session {
                Some(id) if db.get_session(&id)?.is_some() => id,
                Some(id) => id,
                None => db.create_session(&cwd)?.id,
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

async fn respond_turn(id: Id, value: Option<serde_json::Value>, _state: &AppState) -> String {
    let Some(params) = value.and_then(|v| serde_json::from_value::<TurnResponseParams>(v).ok())
    else {
        return serde_json::to_string(&Response::<serde_json::Value>::err(
            id,
            INVALID_PARAMS,
            "invalid turn.response params",
        ))
        .unwrap_or_default();
    };

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
