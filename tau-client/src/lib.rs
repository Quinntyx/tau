//! Async, multiplexed client for the tau JSON-RPC WebSocket protocol.
use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Context, Result};
use futures_util::{SinkExt, Stream, StreamExt};
use serde::Serialize;
use tau_proto::prelude::*;
use tokio::{
    net::UnixStream,
    sync::{Mutex, broadcast, mpsc},
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

#[derive(Debug)]
enum Incoming {
    Response(Response),
    Notification(Notification<serde_json::Value>),
}

struct Inner {
    sink: Mutex<futures_util::stream::SplitSink<WebSocketStream<UnixStream>, Message>>,
    pending: Mutex<HashMap<Id, mpsc::Sender<Incoming>>>,
    events: broadcast::Sender<SequencedEvent>,
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
    rx: mpsc::Receiver<Incoming>,
    inner: Arc<Inner>,
    id: Id,
    done: bool,
}
pub struct TurnStream {
    rx: mpsc::Receiver<Incoming>,
    inner: Arc<Inner>,
    id: Id,
    done: bool,
}
pub struct EventStream {
    rx: mpsc::Receiver<SequencedEvent>,
}

#[derive(Debug, Clone)]
pub enum TurnStreamEvent {
    Event(SequencedEvent),
    Complete(TurnStartResult),
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
        let (events, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            sink: Mutex::new(sink),
            pending: Mutex::new(HashMap::new()),
            events,
        });
        let routed = Arc::clone(&inner);
        tokio::spawn(async move {
            let error = loop {
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
                    let request_id = notification
                        .params
                        .as_ref()
                        .and_then(|p| p.get("request_id"))
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    if let Some(id) = request_id {
                        if let Some(tx) = routed.pending.lock().await.get(&id).cloned() {
                            let _ = tx.send(Incoming::Notification(notification)).await;
                        }
                    }
                } else if let Ok(response) = serde_json::from_value::<Response>(value) {
                    if let Some(tx) = routed.pending.lock().await.get(&response.id).cloned() {
                        let _ = tx.send(Incoming::Response(response)).await;
                    }
                }
            };
            let _ = error;
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
    ) -> Result<(Id, mpsc::Receiver<Incoming>)> {
        let id = self.next_id();
        let (tx, rx) = mpsc::channel(32);
        self.inner.pending.lock().await.insert(id.clone(), tx);
        let request = Request::new(id.clone(), method, params);
        let text = serde_json::to_string(&request)?;
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
            match rx.recv().await.context("connection closed")? {
                Incoming::Response(r) if r.id == id => break r,
                _ => {}
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

    pub async fn negotiate(
        &self,
        params: ProtocolNegotiateParams,
    ) -> Result<ProtocolNegotiateResult> {
        serde_json::from_value(self.call(METHOD_PROTOCOL_NEGOTIATE, Some(params)).await?)
            .context("decoding negotiation")
    }
    pub async fn turn_cancel(&self, params: TurnCancelParams) -> Result<TurnCancelResult> {
        serde_json::from_value(self.call(METHOD_TURN_CANCEL, Some(params)).await?)
            .context("decoding cancellation")
    }
    pub async fn turn_replay(&self, params: TurnReplayParams) -> Result<TurnReplayResult> {
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
        let (id, rx) = self.send(METHOD_TURN_START, Some(params)).await?;
        Ok(TurnStream {
            rx,
            inner: Arc::clone(&self.inner),
            id,
            done: false,
        })
    }
    pub fn events(&self) -> EventStream {
        let (tx, rx) = mpsc::channel(32);
        let mut source = self.inner.events.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = source.recv().await {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        EventStream { rx }
    }
}

impl Drop for CompletionStream {
    fn drop(&mut self) {
        self.inner
            .pending
            .try_lock()
            .ok()
            .map(|mut p| p.remove(&self.id));
    }
}
impl Drop for TurnStream {
    fn drop(&mut self) {
        self.inner
            .pending
            .try_lock()
            .ok()
            .map(|mut p| p.remove(&self.id));
    }
}

impl Stream for EventStream {
    type Item = Result<SequencedEvent>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.rx)
            .poll_recv(cx)
            .map(|r| r.map(Ok))
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
            std::task::Poll::Ready(Some(Incoming::Notification(n))) => std::task::Poll::Ready(
                n.params
                    .and_then(|p| serde_json::from_value(p).ok())
                    .map(|d| Ok(CompletionEvent::Delta(d))),
            ),
            std::task::Poll::Ready(Some(Incoming::Response(r))) => {
                self.done = true;
                std::task::Poll::Ready(Some(
                    r.result
                        .ok_or_else(|| anyhow::anyhow!("response had no result"))
                        .and_then(|v| {
                            serde_json::from_value(v)
                                .map(CompletionEvent::Complete)
                                .map_err(Into::into)
                        }),
                ))
            }
            std::task::Poll::Ready(None) => {
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
            std::task::Poll::Ready(Some(Incoming::Notification(n))) => std::task::Poll::Ready(
                n.params
                    .and_then(|p| serde_json::from_value(p).ok())
                    .map(|e| Ok(TurnStreamEvent::Event(e))),
            ),
            std::task::Poll::Ready(Some(Incoming::Response(r))) => {
                self.done = true;
                std::task::Poll::Ready(Some(
                    r.result
                        .ok_or_else(|| anyhow::anyhow!("response had no result"))
                        .and_then(|v| {
                            serde_json::from_value(v)
                                .map(TurnStreamEvent::Complete)
                                .map_err(Into::into)
                        }),
                ))
            }
            std::task::Poll::Ready(None) => {
                std::task::Poll::Ready(Some(Err(anyhow::anyhow!("connection closed"))))
            }
        }
    }
}
