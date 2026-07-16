//! tau — async client library.
//!
//! Connects to the tau daemon over a Unix socket and speaks JSON-RPC 2.0 over
//! WebSocket. Consumed by the GUI, the TUI, and any long-lived client (e.g. a
//! Discord bridge). Only depends on `tau-proto` — never on `tau-core` or
//! `tau-server` — keeping the server/client boundary clean.

use std::path::Path;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tau_proto::{
    HealthResult, Id, JsonRpc, METHOD_HEALTH, METHOD_PING, Notification, Request, Response,
};
use tokio::net::UnixStream;
use tokio_tungstenite::WebSocketStream;

/// A connected tau daemon client.
pub struct Client {
    ws: WebSocketStream<UnixStream>,
    next_id: i64,
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
                    let resp: Response =
                        serde_json::from_str(t.as_str()).context("decoding response")?;
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

    /// Server→client notification stream.
    ///
    /// NOT YET IMPLEMENTED in M0 — the server emits no notifications, so this
    /// stream never yields. It exists so the capability-group surface is in
    /// place for streaming events (token deltas, tool events, permission
    /// requests, plan updates, …) landing in later milestones.
    pub fn events(&mut self) -> impl futures_util::Stream<Item = Result<Notification>> + '_ {
        futures_util::stream::pending()
    }
}
