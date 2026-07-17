//! tau — async client library.
//!
//! Connects to the tau daemon over a Unix socket and speaks JSON-RPC 2.0 over
//! WebSocket. Consumed by the GUI, the TUI, and any long-lived client (e.g. a
//! Discord bridge). Only depends on `tau-proto` — never on `tau-core` or
//! `tau-server` — keeping the server/client boundary clean.

use std::{
    path::Path,
    pin::Pin,
    task::{Context as TaskContext, Poll},
};

use anyhow::{Context, Result};
use futures_util::{SinkExt, Stream, StreamExt};
use serde::Serialize;
use tau_proto::prelude::*;
use tokio::net::UnixStream;
use tokio_tungstenite::WebSocketStream;

/// A connected tau daemon client.
pub struct Client {
    ws: WebSocketStream<UnixStream>,
    next_id: i64,
}

/// Events emitted while a completion request is running.
#[derive(Debug, Clone)]
pub enum CompletionEvent {
    /// A text or usage notification from the daemon.
    Delta(CompletionDelta),
    /// The final JSON-RPC response.
    Complete(CompletionStreamResult),
}

/// In-flight completion stream borrowing its client's WebSocket.
pub struct CompletionStream<'a> {
    ws: &'a mut WebSocketStream<UnixStream>,
    request_id: Id,
    done: bool,
}

impl Client {
    /// Connect to the daemon at the given Unix socket path.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Client> {
        let stream = UnixStream::connect(path.as_ref())
            .await
            .with_context(|| format!("connecting to {}", path.as_ref().display()))?;
        let (ws, _resp) = tokio_tungstenite::client_async("ws://localhost/", stream)
            .await
            .context("websocket handshake")?;
        Ok(Client { ws, next_id: 1 })
    }

    fn next_id(&mut self) -> Id {
        let id = Id::num(self.next_id);
        self.next_id += 1;
        id
    }

    /// `ping` → `"pong"`.
    pub async fn ping(&mut self) -> Result<String> {
        let v = self.call0(METHOD_PING).await?;
        serde_json::from_value(v).context("decoding ping result")
    }

    /// `health` → server status.
    pub async fn health(&mut self) -> Result<HealthResult> {
        let v = self.call0(METHOD_HEALTH).await?;
        serde_json::from_value(v).context("decoding health result")
    }

    /// Call a method with no params.
    pub async fn call0(&mut self, method: &str) -> Result<serde_json::Value> {
        self.call::<serde_json::Value>(method, None).await
    }

    /// Call a method with typed params, returning the raw result value.
    pub async fn call<P: Serialize>(
        &mut self,
        method: &str,
        params: Option<P>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id();
        let params_val = match params.as_ref() {
            Some(p) => Some(serde_json::to_value(p)?),
            None => None,
        };
        let req = Request {
            jsonrpc: JsonRpc::default(),
            id: id.clone(),
            method: method.to_string(),
            params: params_val,
        };
        let text = serde_json::to_string(&req)?;
        self.ws
            .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .await
            .context("sending request")?;

        loop {
            let msg = self.ws.next().await.context("connection closed")??;
            match msg {
                tokio_tungstenite::tungstenite::Message::Text(t) => {
                    let value: serde_json::Value =
                        serde_json::from_str(t.as_str()).context("decoding message")?;
                    if value.get("method").is_some() {
                        continue;
                    }
                    let resp: Response =
                        serde_json::from_value(value).context("decoding response")?;
                    if resp.id == id {
                        if let Some(e) = resp.error {
                            anyhow::bail!("rpc error {}: {}", e.code, e.message);
                        }
                        return resp
                            .result
                            .ok_or_else(|| anyhow::anyhow!("response had no result"));
                    }
                    // id mismatch (e.g. a notification) — keep reading
                }
                tokio_tungstenite::tungstenite::Message::Binary(_) => { /* reserved */ }
                _ => {}
            }
        }
    }

    /// Start a streaming completion request.
    pub async fn completion_stream(
        &mut self,
        params: CompletionStreamParams,
    ) -> Result<CompletionStream<'_>> {
        let id = self.next_id();
        let req = Request::new(id.clone(), METHOD_COMPLETION_STREAM, Some(params));
        let text = serde_json::to_string(&req)?;
        self.ws
            .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .await
            .context("sending completion request")?;
        Ok(CompletionStream {
            ws: &mut self.ws,
            request_id: id,
            done: false,
        })
    }

    /// Server→client notification stream.
    ///
    /// Completion notifications are consumed by [`Client::completion_stream`].
    /// This general event surface remains reserved for notifications that are
    /// not tied to a request, such as tool events or permission requests.
    pub fn events(&mut self) -> impl futures_util::Stream<Item = Result<Notification>> + '_ {
        futures_util::stream::pending()
    }
}

impl Stream for CompletionStream<'_> {
    type Item = Result<CompletionEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }

        loop {
            let message = match Pin::new(&mut *self.ws).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    self.done = true;
                    return Poll::Ready(Some(Err(anyhow::anyhow!("connection closed"))));
                }
                Poll::Ready(Some(Ok(message))) => message,
                Poll::Ready(Some(Err(error))) => {
                    self.done = true;
                    return Poll::Ready(Some(Err(anyhow::anyhow!(error))));
                }
            };

            let tokio_tungstenite::tungstenite::Message::Text(text) = message else {
                continue;
            };
            let value: serde_json::Value = match serde_json::from_str(text.as_str()) {
                Ok(value) => value,
                Err(error) => {
                    self.done = true;
                    return Poll::Ready(Some(Err(error.into())));
                }
            };

            if value.get("method").is_some()
                && value.get("method").and_then(serde_json::Value::as_str)
                    != Some(METHOD_COMPLETION_DELTA)
            {
                continue;
            }
            if value.get("method").and_then(serde_json::Value::as_str)
                == Some(METHOD_COMPLETION_DELTA)
            {
                let notification: Notification<serde_json::Value> =
                    match serde_json::from_value(value) {
                        Ok(notification) => notification,
                        Err(error) => {
                            self.done = true;
                            return Poll::Ready(Some(Err(error.into())));
                        }
                    };
                if let Some(params) = notification.params {
                    let delta = match serde_json::from_value(params) {
                        Ok(delta) => delta,
                        Err(error) => {
                            self.done = true;
                            return Poll::Ready(Some(Err(error.into())));
                        }
                    };
                    return Poll::Ready(Some(Ok(CompletionEvent::Delta(delta))));
                }
                continue;
            }

            let response: Response<serde_json::Value> = match serde_json::from_value(value) {
                Ok(response) => response,
                Err(error) => {
                    self.done = true;
                    return Poll::Ready(Some(Err(error.into())));
                }
            };
            if response.id != self.request_id {
                continue;
            }
            self.done = true;
            if let Some(error) = response.error {
                return Poll::Ready(Some(Err(anyhow::anyhow!(
                    "rpc error {}: {}",
                    error.code,
                    error.message
                ))));
            }
            let Some(result) = response.result else {
                return Poll::Ready(Some(Err(anyhow::anyhow!("response had no result"))));
            };
            let result = match serde_json::from_value(result) {
                Ok(result) => result,
                Err(error) => return Poll::Ready(Some(Err(error.into()))),
            };
            return Poll::Ready(Some(Ok(CompletionEvent::Complete(result))));
        }
    }
}
