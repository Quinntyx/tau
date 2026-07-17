//! `completion.stream` method — streaming LLM completion over JSON-RPC.
//!
//! Request: [`CompletionStreamParams`]. The server streams
//! [`CompletionDelta`] notifications (method `completion.delta`) as text
//! arrives, then resolves the original request with a final
//! [`CompletionStreamResult`] carrying the full text and token usage.

use serde::{Deserialize, Serialize};

use crate::envelope::Id;

/// Compile-time migration aliases; the wire methods are session.turn.*.
pub const METHOD_COMPLETION_STREAM: &str = crate::turn::METHOD_TURN_START;
pub const METHOD_COMPLETION_DELTA: &str = crate::turn::METHOD_TURN_EVENT;

/// Params for `completion.stream`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionStreamParams {
    /// Model id with provider prefix, e.g. `"openai/gpt-4o"`.
    pub model: String,
    /// User prompt text.
    pub prompt: String,
    /// Existing session id. When omitted the daemon creates a new session
    /// keyed by `cwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Working directory. Required when creating a new session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_tier: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomous: Option<bool>,
}

/// Final result of `completion.stream`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionStreamResult {
    pub session_id: String,
    pub message_id: i64,
    pub text: String,
    pub usage: UsageSummary,
}

/// One streaming delta notification (`completion.delta`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionDelta {
    pub request_id: Id,
    pub session_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageSummary>,
}

/// Token usage summary reported by the provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}
