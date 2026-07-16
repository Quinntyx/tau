//! tau — wire protocol.
//!
//! Pure serde types describing the tau JSON-RPC 2.0 protocol carried over the
//! daemon's WebSocket. No network code, no tokio, no agent logic. Shared by
//! `tau-server` and `tau-client` (and any future client emitter) so both sides
//! of the socket speak one typed contract. A client consumer only ever pulls in
//! `tau-client` + this crate — never `tau-core` or `tau-server`.
//!
//! Frame convention: text WebSocket frames carry JSON-RPC 2.0; binary frames
//! are reserved for multiplexed opaque channels (see [`binary`]).

use serde::{Deserialize, Serialize};

/// JSON-RPC version marker. Always serializes to the string `"2.0"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JsonRpc {
    #[default]
    #[serde(rename = "2.0")]
    V2,
}

/// A request/response id. JSON-RPC ids are numbers or strings here (null is not
/// used by tau).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Num(i64),
    Str(String),
}

impl Id {
    pub fn num(n: i64) -> Self {
        Id::Num(n)
    }
    pub fn string(s: impl Into<String>) -> Self {
        Id::Str(s.into())
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request<P = serde_json::Value> {
    pub jsonrpc: JsonRpc,
    pub id: Id,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<P>,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response<R = serde_json::Value> {
    pub jsonrpc: JsonRpc,
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<R>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// A JSON-RPC 2.0 notification (no id; server→client streaming events).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification<P = serde_json::Value> {
    pub jsonrpc: JsonRpc,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<P>,
}

impl<P> Request<P> {
    pub fn new(id: Id, method: impl Into<String>, params: Option<P>) -> Self {
        Self {
            jsonrpc: JsonRpc::default(),
            id,
            method: method.into(),
            params,
        }
    }
}

impl<R> Response<R> {
    pub fn err(id: Id, code: i32, message: impl Into<String>) -> Self {
        Response {
            jsonrpc: JsonRpc::default(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

impl<R: Serialize> Response<R> {
    pub fn ok(id: Id, result: R) -> Self {
        Response {
            jsonrpc: JsonRpc::default(),
            id,
            result: Some(result),
            error: None,
        }
    }
}

// ---- standard JSON-RPC 2.0 error codes ----
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

// ---- M0 methods ----
pub const METHOD_PING: &str = "ping";
pub const METHOD_HEALTH: &str = "health";

/// Result of the `health` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResult {
    pub version: String,
    pub uptime_ms: u64,
    pub pid: u32,
}

/// Reserved binary-frame channel convention.
///
/// Text WebSocket frames always carry JSON-RPC 2.0. Binary frames carry a
/// one-byte channel id followed by that channel's opaque payload:
/// `[u8 channel][payload bytes]`. No channel is implemented in M0; the daemon
/// logs and ignores binary frames. Channels land alongside the tools that need
/// them (e.g. PTY for the `bash` tool).
pub mod binary {
    /// Pseudo-terminal byte stream (for the `bash` tool).
    pub const CH_PTY: u8 = 0x01;
    /// File / attachment blob transfer.
    pub const CH_ATTACHMENT: u8 = 0x02;
}
