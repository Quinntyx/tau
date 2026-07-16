//! Core JSON-RPC 2.0 envelope types: `JsonRpc`, `Id`, `Request`, `Response`,
//! `Notification`, `RpcError`.

use serde::{Deserialize, Serialize};

/// JSON-RPC version marker. Always serializes to the string `"2.0"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JsonRpc {
    #[default]
    #[serde(rename = "2.0")]
    V2,
}

/// A request/response id. JSON-RPC ids are numbers or strings (null is not
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

/// A JSON-RPC 2.0 notification (no id; server→client streaming events).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification<P = serde_json::Value> {
    pub jsonrpc: JsonRpc,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<P>,
}
