//! Typed session-turn protocol and durable event shapes.
use serde::{Deserialize, Serialize};

use crate::envelope::Id;

pub const METHOD_PROTOCOL_NEGOTIATE: &str = "protocol.negotiate";
pub const METHOD_TURN_START: &str = "session.turn.start";
pub const METHOD_TURN_CANCEL: &str = "session.turn.cancel";
pub const METHOD_TURN_REPLAY: &str = "session.turn.replay";
pub const METHOD_TURN_EVENT: &str = "session.turn.event";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    TurnStreaming,
    TurnCancellation,
    EventReplay,
    Idempotency,
    ArtifactReferences,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolNegotiateParams {
    pub version: ProtocolVersion,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolNegotiateResult {
    pub version: ProtocolVersion,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(pub String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartParams {
    pub model: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub idempotency_key: IdempotencyKey,
    /// The selected OpenCode-style workflow context. These fields are kept on
    /// the request (rather than only in the view) so reconnect/replay clients
    /// can reproduce the exact turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_tier: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomous: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<RequestAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestAction {
    Submit,
    Command { command: String },
    Permission { choice: String },
    Cancel,
    Replay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartResult {
    pub session_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCancelParams {
    pub session_id: String,
    pub turn_id: String,
    pub idempotency_key: IdempotencyKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCancelResult {
    pub session_id: String,
    pub turn_id: String,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnReplayParams {
    pub session_id: String,
    pub after_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnReplayResult {
    pub events: Vec<SequencedEvent>,
    pub next_sequence: Option<u64>,
    /// True when the requested cursor predates retained history.
    #[serde(default)]
    pub gap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactReference {
    pub artifact_id: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub content_hash: String,
    pub storage_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedOutput {
    pub inline: String,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEvent {
    TurnStarted {
        turn_id: String,
    },
    TextDelta {
        turn_id: String,
        text: String,
    },
    ToolOutput {
        turn_id: String,
        output: BoundedOutput,
    },
    ArtifactCreated {
        turn_id: String,
        artifact: ArtifactReference,
    },
    TurnCompleted {
        turn_id: String,
        message_id: Option<i64>,
    },
    TurnCancelled {
        turn_id: String,
    },
    TurnFailed {
        turn_id: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencedEvent {
    pub event_id: String,
    pub session_id: String,
    pub sequence: u64,
    pub occurred_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Id>,
    pub event: TurnEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_public_shape_round_trips() {
        let values = [
            serde_json::to_value(ProtocolNegotiateParams {
                version: ProtocolVersion { major: 1, minor: 0 },
                capabilities: vec![Capability::EventReplay],
            })
            .unwrap(),
            serde_json::to_value(TurnStartParams {
                model: "m".into(),
                prompt: "p".into(),
                session_id: None,
                cwd: None,
                idempotency_key: IdempotencyKey::new("k"),
                agent: Some("default".into()),
                task_tier: Some(1),
                autonomous: Some(false),
                action: Some(RequestAction::Submit),
            })
            .unwrap(),
            serde_json::to_value(TurnCancelParams {
                session_id: "s".into(),
                turn_id: "t".into(),
                idempotency_key: IdempotencyKey::new("k"),
            })
            .unwrap(),
            serde_json::to_value(TurnReplayParams {
                session_id: "s".into(),
                after_sequence: 2,
                limit: Some(3),
            })
            .unwrap(),
            serde_json::to_value(TurnEvent::ToolOutput {
                turn_id: "t".into(),
                output: BoundedOutput {
                    inline: "x".into(),
                    truncated: true,
                    artifacts: vec![],
                },
            })
            .unwrap(),
        ];
        for value in values {
            assert_eq!(
                value,
                serde_json::from_value::<serde_json::Value>(value.clone()).unwrap()
            );
        }
    }
}
