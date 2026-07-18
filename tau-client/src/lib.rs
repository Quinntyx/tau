//! Async, multiplexed client for the tau JSON-RPC WebSocket protocol.
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result};
use futures_util::{SinkExt, Stream, StreamExt};
use serde::Serialize;
use tau_proto::prelude::*;
use tokio::{
    net::UnixStream,
    sync::{Mutex, broadcast, mpsc},
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

// Diff replies use the protocol's historical `decision` method on the wire.
// Keep that detail here rather than making callers of the typed API know about
// the compatibility name.
const METHOD_DIFF_REPLY: &str = METHOD_DIFF_DECISION;

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn diff_reply_uses_protocol_decision_method() {
        assert_eq!(METHOD_DIFF_REPLY, METHOD_DIFF_DECISION);
    }
}

#[derive(Debug)]
enum Incoming {
    Response(Response),
    Notification(Notification<serde_json::Value>),
    Closed,
}

/// Server-originated interactive policy requests.  These are delivered on the
/// client event stream so a broker can answer them without sharing the wire
/// envelope or guessing at JSON fields.
#[derive(Debug, Clone)]
pub enum PolicyEvent {
    Permission(PermissionRequest),
    Question(QuestionRequest),
    Diff(DiffRequest),
    Plan(PlanRequest),
    Airtight(AirtightPromptRequest),
}

struct Inner {
    sink: Mutex<futures_util::stream::SplitSink<WebSocketStream<UnixStream>, Message>>,
    pending: Mutex<HashMap<Id, mpsc::UnboundedSender<Incoming>>>,
    negotiation: Mutex<Option<NegotiationReport>>,
    events: broadcast::Sender<SequencedEvent>,
    policy_events: broadcast::Sender<PolicyEvent>,
    disconnected: broadcast::Sender<()>,
    is_disconnected: AtomicBool,
}

/// A connected tau daemon client. All reads are performed by one background
/// task, so calls, streams, and event subscriptions can coexist safely.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
    next_id: Arc<std::sync::atomic::AtomicI64>,
}

#[derive(Debug, Clone)]
pub enum CompletionEvent {
    Delta(CompletionDelta),
    Complete(CompletionStreamResult),
}

pub struct CompletionStream {
    rx: mpsc::UnboundedReceiver<Incoming>,
    inner: Arc<Inner>,
    id: Id,
    done: bool,
}
pub struct TurnStream {
    rx: mpsc::UnboundedReceiver<Incoming>,
    inner: Arc<Inner>,
    id: Id,
    done: bool,
}
pub struct EventStream {
    rx: mpsc::UnboundedReceiver<Result<SequencedEvent>>,
}

pub struct PolicyEventStream {
    rx: mpsc::UnboundedReceiver<PolicyEvent>,
}

impl PolicyEventStream {
    /// Construct a policy stream backed by an application-owned receiver.
    pub fn from_receiver(rx: mpsc::UnboundedReceiver<PolicyEvent>) -> Self {
        Self { rx }
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum TurnStreamEvent {
    Event(SequencedEvent),
    Complete(TurnStartResult),
}

/// The result of comparing a negotiated protocol with the features a client
/// needs.  This is deliberately separate from the wire result: a daemon can
/// successfully negotiate while still lacking optional GUI features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolCompatibility {
    Compatible,
    Degraded,
    Incompatible,
}

#[derive(Debug, Clone)]
pub struct NegotiationReport {
    pub requested: ProtocolNegotiateParams,
    pub server: ProtocolNegotiateResult,
    pub compatibility: ProtocolCompatibility,
    pub missing_capabilities: Vec<Capability>,
}

impl NegotiationReport {
    pub fn is_compatible(&self) -> bool {
        self.compatibility == ProtocolCompatibility::Compatible
    }

    pub fn is_usable(&self) -> bool {
        self.compatibility != ProtocolCompatibility::Incompatible
    }
}

fn classify_negotiation(
    requested: ProtocolNegotiateParams,
    server: ProtocolNegotiateResult,
) -> NegotiationReport {
    let missing_capabilities: Vec<Capability> = requested
        .capabilities
        .iter()
        .filter(|capability| !server.capabilities.contains(capability))
        .cloned()
        .collect();
    let version_compatible = server.version.major == requested.version.major
        && server.version.minor >= requested.version.minor;
    let core_available = !missing_capabilities.contains(&Capability::TurnStreaming);
    let compatibility = if !version_compatible || !core_available {
        ProtocolCompatibility::Incompatible
    } else if missing_capabilities.is_empty() {
        ProtocolCompatibility::Compatible
    } else {
        ProtocolCompatibility::Degraded
    };
    NegotiationReport {
        requested,
        server,
        compatibility,
        missing_capabilities,
    }
}

impl Client {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self> {
        let stream = UnixStream::connect(path.as_ref())
            .await
            .with_context(|| format!("connecting to {}", path.as_ref().display()))?;
        let (ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
            .await
            .context("websocket handshake")?;
        let (sink, mut reader) = ws.split();
        let (events, _) = broadcast::channel(4096);
        let (policy_events, _) = broadcast::channel(4096);
        let (disconnected, _) = broadcast::channel(1);
        let inner = Arc::new(Inner {
            sink: Mutex::new(sink),
            pending: Mutex::new(HashMap::new()),
            negotiation: Mutex::new(None),
            events,
            policy_events,
            disconnected,
            is_disconnected: AtomicBool::new(false),
        });
        let routed = Arc::clone(&inner);
        tokio::spawn(async move {
            let _error = loop {
                let message = match reader.next().await {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => break anyhow::anyhow!(error),
                    None => break anyhow::anyhow!("connection closed"),
                };
                let value: serde_json::Value = match serde_json::from_str(&message) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if value.get("method").is_some() {
                    let notification: Notification<serde_json::Value> =
                        match serde_json::from_value(value) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                    if notification.method == METHOD_TURN_EVENT {
                        if let Some(params) = notification.params.clone() {
                            if let Ok(event) = serde_json::from_value::<SequencedEvent>(params) {
                                let _ = routed.events.send(event);
                            }
                        }
                    }
                    let policy = notification
                        .params
                        .clone()
                        .and_then(|params| match notification.method.as_str() {
                            METHOD_PERMISSION_REQUEST => serde_json::from_value(params)
                                .ok()
                                .map(PolicyEvent::Permission),
                            METHOD_QUESTION_REQUEST => serde_json::from_value(params)
                                .ok()
                                .map(PolicyEvent::Question),
                            METHOD_DIFF_REQUEST => {
                                serde_json::from_value(params).ok().map(PolicyEvent::Diff)
                            }
                            METHOD_PLAN_REQUEST => {
                                serde_json::from_value(params).ok().map(PolicyEvent::Plan)
                            }
                            METHOD_AIRTIGHT_REQUEST => serde_json::from_value(params)
                                .ok()
                                .map(PolicyEvent::Airtight),
                            _ => None,
                        });
                    if let Some(event) = policy {
                        let _ = routed.policy_events.send(event);
                    }
                    let request_id = notification
                        .params
                        .as_ref()
                        .and_then(|p| p.get("request_id"))
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    if let Some(id) = request_id {
                        if let Some(tx) = routed.pending.lock().await.get(&id).cloned() {
                            let _ = tx.send(Incoming::Notification(notification));
                        }
                    }
                } else if let Ok(response) = serde_json::from_value::<Response>(value) {
                    if let Some(tx) = routed.pending.lock().await.get(&response.id).cloned() {
                        let _ = tx.send(Incoming::Response(response));
                    }
                }
            };
            // Wake both ordinary and policy subscribers. In particular, do
            // not leave a policy subscription waiting on a broadcast sender
            // which remains alive in `Inner` after the socket has gone away.
            routed
                .is_disconnected
                .store(true, std::sync::atomic::Ordering::Release);
            let _ = routed.disconnected.send(());
            let mut pending = routed.pending.lock().await;
            for tx in pending.values() {
                let _ = tx.send(Incoming::Closed);
            }
            // Requests cannot receive a response after the transport closes.
            // Drop their senders so the request table is clean for any owner
            // that establishes a replacement connection.
            pending.clear();
            routed.negotiation.lock().await.take();
        });
        Ok(Self {
            inner,
            next_id: Arc::new(std::sync::atomic::AtomicI64::new(1)),
        })
    }

    fn next_id(&self) -> Id {
        Id::num(
            self.next_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
    }

    async fn send<P: Serialize>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<(Id, mpsc::UnboundedReceiver<Incoming>)> {
        let id = self.next_id();
        let (tx, rx) = mpsc::unbounded_channel();
        let request = Request::new(id.clone(), method, params);
        let text = serde_json::to_string(&request)?;
        self.inner.pending.lock().await.insert(id.clone(), tx);
        if let Err(e) = self
            .inner
            .sink
            .lock()
            .await
            .send(Message::Text(text.into()))
            .await
        {
            self.inner.pending.lock().await.remove(&id);
            return Err(e.into());
        }
        Ok((id, rx))
    }

    async fn response<P: Serialize>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<serde_json::Value> {
        let (id, mut rx) = self.send(method, params).await?;
        let response = loop {
            match rx.recv().await {
                Some(Incoming::Response(r)) if r.id == id => break r,
                Some(Incoming::Closed) => {
                    self.inner.pending.lock().await.remove(&id);
                    anyhow::bail!("connection closed (request interrupted)");
                }
                Some(_) => {}
                None => {
                    self.inner.pending.lock().await.remove(&id);
                    anyhow::bail!("connection closed");
                }
            }
        };
        self.inner.pending.lock().await.remove(&id);
        if let Some(e) = response.error {
            anyhow::bail!("rpc error {}: {}", e.code, e.message);
        }
        response.result.context("response had no result")
    }

    pub async fn call0(&self, method: &str) -> Result<serde_json::Value> {
        self.response(method, None::<serde_json::Value>).await
    }
    pub async fn call<P: Serialize>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<serde_json::Value> {
        self.response(method, params).await
    }
    pub async fn ping(&self) -> Result<String> {
        serde_json::from_value(self.call0(METHOD_PING).await?).context("decoding ping result")
    }
    pub async fn health(&self) -> Result<HealthResult> {
        serde_json::from_value(self.call0(METHOD_HEALTH).await?).context("decoding health result")
    }
    pub async fn permission_reply(&self, params: PermissionReply) -> Result<serde_json::Value> {
        self.call(METHOD_PERMISSION_REPLY, Some(params)).await
    }
    pub async fn question_reply(&self, params: QuestionReply) -> Result<serde_json::Value> {
        self.call(METHOD_QUESTION_REPLY, Some(params)).await
    }
    pub async fn diff_decision(&self, params: DiffDecision) -> Result<serde_json::Value> {
        self.call(METHOD_DIFF_DECISION, Some(params)).await
    }
    pub async fn diff_reply(&self, params: DiffReply) -> Result<serde_json::Value> {
        self.call(METHOD_DIFF_REPLY, Some(params)).await
    }
    pub async fn plan_reply(&self, params: PlanReply) -> Result<serde_json::Value> {
        self.call(METHOD_PLAN_REPLY, Some(params)).await
    }
    pub async fn airtight_reply(&self, params: AirtightPromptReply) -> Result<serde_json::Value> {
        self.call(METHOD_AIRTIGHT_REPLY, Some(params)).await
    }
    pub async fn set_autonomy(&self, params: AutonomySet) -> Result<serde_json::Value> {
        self.call(METHOD_AUTONOMY_SET, Some(params)).await
    }
    pub async fn steering_action(&self, params: SteeringAction) -> Result<serde_json::Value> {
        self.call(METHOD_STEERING_ACTION, Some(params)).await
    }
    pub async fn prompt_takeover(&self, params: PromptTakeover) -> Result<serde_json::Value> {
        self.call(METHOD_PROMPT_TAKEOVER, Some(params)).await
    }

    pub async fn negotiate(
        &self,
        params: ProtocolNegotiateParams,
    ) -> Result<ProtocolNegotiateResult> {
        let requested = params.clone();
        self.inner.negotiation.lock().await.take();
        let server: ProtocolNegotiateResult =
            serde_json::from_value(self.call(METHOD_PROTOCOL_NEGOTIATE, Some(params)).await?)
                .context("decoding negotiation")?;
        let report = classify_negotiation(requested, server.clone());
        *self.inner.negotiation.lock().await = Some(report);
        Ok(server)
    }

    /// Negotiate and classify the result against the caller's requirements.
    /// The server is authoritative for the selected version and capabilities;
    /// missing capabilities are reported rather than silently ignored.
    pub async fn negotiate_checked(
        &self,
        params: ProtocolNegotiateParams,
    ) -> Result<NegotiationReport> {
        let requested = params.clone();
        let server = self.negotiate(params).await?;
        Ok(classify_negotiation(requested, server))
    }
    pub async fn turn_cancel(&self, params: TurnCancelParams) -> Result<TurnCancelResult> {
        self.ensure_capability(Capability::TurnCancellation).await?;
        serde_json::from_value(self.call(METHOD_TURN_CANCEL, Some(params)).await?)
            .context("decoding cancellation")
    }
    pub async fn turn_response(&self, params: TurnResponseParams) -> Result<TurnResponseResult> {
        serde_json::from_value(self.call(METHOD_TURN_RESPONSE, Some(params)).await?)
            .context("decoding turn response")
    }
    pub async fn turn_replay(&self, params: TurnReplayParams) -> Result<TurnReplayResult> {
        self.ensure_capability(Capability::EventReplay).await?;
        serde_json::from_value(self.call(METHOD_TURN_REPLAY, Some(params)).await?)
            .context("decoding replay")
    }

    pub async fn completion_stream(
        &self,
        params: CompletionStreamParams,
    ) -> Result<CompletionStream> {
        let (id, rx) = self.send(METHOD_COMPLETION_STREAM, Some(params)).await?;
        Ok(CompletionStream {
            rx,
            inner: Arc::clone(&self.inner),
            id,
            done: false,
        })
    }
    pub async fn turn_start(&self, params: TurnStartParams) -> Result<TurnStream> {
        self.ensure_capability(Capability::TurnStreaming).await?;
        let (id, rx) = self.send(METHOD_TURN_START, Some(params)).await?;
        Ok(TurnStream {
            rx,
            inner: Arc::clone(&self.inner),
            id,
            done: false,
        })
    }

    async fn ensure_capability(&self, capability: Capability) -> Result<()> {
        let negotiation = self.inner.negotiation.lock().await;
        let Some(report) = negotiation.as_ref() else {
            anyhow::bail!("protocol negotiation required before session.turn.*")
        };
        if !report.is_usable() || !report.server.capabilities.contains(&capability) {
            anyhow::bail!("daemon protocol incompatible; capability unavailable: {capability:?}")
        }
        Ok(())
    }
    pub fn events(&self) -> EventStream {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut source = self.inner.events.subscribe();
        let mut disconnected = self.inner.disconnected.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    event = source.recv() => match event {
                        Ok(event) => if tx.send(Ok(event)).is_err() { break },
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    },
                    closed = disconnected.recv() => {
                        if closed.is_ok() { let _ = tx.send(Err(anyhow::anyhow!("connection closed"))); }
                        break;
                    }
                }
            }
        });
        EventStream { rx }
    }

    pub fn policy_events(&self) -> PolicyEventStream {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut source = self.inner.policy_events.subscribe();
        let mut disconnected = self.inner.disconnected.subscribe();
        let already_disconnected = self
            .inner
            .is_disconnected
            .load(std::sync::atomic::Ordering::Acquire);
        tokio::spawn(async move {
            if already_disconnected {
                return;
            }
            loop {
                tokio::select! {
                    event = source.recv() => match event {
                        Ok(event) => if tx.send(event).is_err() { break },
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    disconnected = disconnected.recv() => {
                        let _ = disconnected;
                        break;
                    }
                }
            }
        });
        PolicyEventStream::from_receiver(rx)
    }
}

impl Drop for CompletionStream {
    fn drop(&mut self) {
        let pending = Arc::clone(&self.inner);
        let id = self.id.clone();
        tokio::spawn(async move {
            pending.pending.lock().await.remove(&id);
        });
    }
}
impl Drop for TurnStream {
    fn drop(&mut self) {
        let pending = Arc::clone(&self.inner);
        let id = self.id.clone();
        tokio::spawn(async move {
            pending.pending.lock().await.remove(&id);
        });
    }
}

impl Stream for EventStream {
    type Item = Result<SequencedEvent>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.rx).poll_recv(cx).map(|r| r)
    }
}

impl Stream for PolicyEventStream {
    type Item = PolicyEvent;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.rx).poll_recv(cx)
    }
}

impl Stream for CompletionStream {
    type Item = Result<CompletionEvent>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.done {
            return std::task::Poll::Ready(None);
        }
        match std::pin::Pin::new(&mut self.rx).poll_recv(cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Some(Incoming::Notification(n))) => {
                let result = n
                    .params
                    .ok_or_else(|| anyhow::anyhow!("completion notification had no params"))
                    .and_then(|p| serde_json::from_value(p).map_err(Into::into))
                    .map(CompletionEvent::Delta);
                std::task::Poll::Ready(Some(result))
            }
            std::task::Poll::Ready(Some(Incoming::Response(r))) => {
                self.done = true;
                std::task::Poll::Ready(Some(
                    if let Some(error) = r.error {
                        Err(anyhow::anyhow!(
                            "rpc error {}: {}",
                            error.code,
                            error.message
                        ))
                    } else {
                        r.result
                            .ok_or_else(|| anyhow::anyhow!("response had no result"))
                    }
                    .and_then(|v| {
                        serde_json::from_value(v)
                            .map(CompletionEvent::Complete)
                            .map_err(Into::into)
                    }),
                ))
            }
            std::task::Poll::Ready(Some(Incoming::Closed)) => {
                self.done = true;
                std::task::Poll::Ready(Some(Err(anyhow::anyhow!("connection closed"))))
            }
            std::task::Poll::Ready(None) => {
                self.done = true;
                std::task::Poll::Ready(Some(Err(anyhow::anyhow!("connection closed"))))
            }
        }
    }
}
impl Stream for TurnStream {
    type Item = Result<TurnStreamEvent>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.done {
            return std::task::Poll::Ready(None);
        }
        match std::pin::Pin::new(&mut self.rx).poll_recv(cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Some(Incoming::Notification(n))) => {
                let result = n
                    .params
                    .ok_or_else(|| anyhow::anyhow!("turn event notification had no params"))
                    .and_then(|p| serde_json::from_value(p).map_err(Into::into))
                    .map(TurnStreamEvent::Event);
                std::task::Poll::Ready(Some(result))
            }
            std::task::Poll::Ready(Some(Incoming::Response(r))) => {
                self.done = true;
                std::task::Poll::Ready(Some(
                    if let Some(error) = r.error {
                        Err(anyhow::anyhow!(
                            "rpc error {}: {}",
                            error.code,
                            error.message
                        ))
                    } else {
                        r.result
                            .ok_or_else(|| anyhow::anyhow!("response had no result"))
                    }
                    .and_then(|v| {
                        serde_json::from_value(v)
                            .map(TurnStreamEvent::Complete)
                            .map_err(Into::into)
                    }),
                ))
            }
            std::task::Poll::Ready(Some(Incoming::Closed)) => {
                self.done = true;
                std::task::Poll::Ready(Some(Err(anyhow::anyhow!("connection closed"))))
            }
            std::task::Poll::Ready(None) => {
                self.done = true;
                std::task::Poll::Ready(Some(Err(anyhow::anyhow!("connection closed"))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(required: Vec<Capability>, offered: Vec<Capability>) -> NegotiationReport {
        let requested = ProtocolNegotiateParams {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: required,
        };
        let server = ProtocolNegotiateResult {
            version: ProtocolVersion { major: 1, minor: 0 },
            capabilities: offered,
        };
        classify_negotiation(requested, server)
    }

    #[test]
    fn missing_capabilities_are_structured() {
        let result = report(vec![Capability::TurnStreaming], vec![]);
        assert_eq!(result.missing_capabilities, vec![Capability::TurnStreaming]);
        assert!(!result.is_compatible());
        assert!(!result.is_usable());
    }

    #[test]
    fn incompatible_versions_are_not_usable() {
        let requested = ProtocolVersion { major: 2, minor: 0 };
        let server = ProtocolVersion { major: 1, minor: 0 };
        let version_compatible = server.major == requested.major && server.minor >= requested.minor;
        let result = NegotiationReport {
            requested: ProtocolNegotiateParams {
                version: requested,
                capabilities: vec![],
            },
            server: ProtocolNegotiateResult {
                version: server,
                capabilities: vec![],
            },
            compatibility: if version_compatible {
                ProtocolCompatibility::Compatible
            } else {
                ProtocolCompatibility::Incompatible
            },
            missing_capabilities: vec![],
        };
        assert_eq!(result.compatibility, ProtocolCompatibility::Incompatible);
        assert!(!result.is_usable());
    }

    #[test]
    fn optional_capability_loss_is_degraded_but_turn_usable() {
        let result = report(
            vec![Capability::TurnStreaming, Capability::EventReplay],
            vec![Capability::TurnStreaming],
        );
        assert_eq!(result.compatibility, ProtocolCompatibility::Degraded);
        assert!(result.is_usable());
    }
}
