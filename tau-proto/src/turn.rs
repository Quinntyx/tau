//! Typed session-turn protocol and durable event shapes.
use serde::{Deserialize, Serialize};

use crate::envelope::Id;

pub const METHOD_PROTOCOL_NEGOTIATE: &str = "protocol.negotiate";
pub const METHOD_TURN_START: &str = "session.turn.start";
pub const METHOD_TURN_CANCEL: &str = "session.turn.cancel";
pub const METHOD_TURN_REPLAY: &str = "session.turn.replay";
pub const METHOD_TURN_EVENT: &str = "session.turn.event";
/// Submit an answer to an interactive turn event.
pub const METHOD_TURN_RESPONSE: &str = "session.turn.response";

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
    Command {
        command: String,
    },
    PermissionReply {
        request_id: String,
        choice: TurnPermissionChoice,
    },
    QuestionReply {
        question_id: String,
        answer: QuestionAnswer,
    },
    DiffReply {
        path: String,
        decision: TurnDiffDecision,
    },
    Cancel,
    Replay,
}

/// A typed answer to an interactive turn event.
///
/// This is deliberately separate from [`RequestAction`]: actions describe how
/// a turn is started, while these values are replies to a permission prompt,
/// question, or diff review request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientResponse {
    Permission {
        request_id: String,
        choice: TurnPermissionChoice,
    },
    Question {
        question_id: String,
        answer: QuestionAnswer,
    },
    DiffHunk {
        request_id: String,
        path: String,
        index: u32,
        approved: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnResponseParams {
    pub session_id: String,
    pub turn_id: String,
    pub idempotency_key: IdempotencyKey,
    pub response: ClientResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnResponseResult {
    pub session_id: String,
    pub turn_id: String,
    pub accepted: bool,
}

/// Explicit replies emitted by interactive clients.  Keeping these typed on
/// the wire prevents a GUI from accidentally treating a question or diff as
/// a permission response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPermissionChoice {
    AllowOnce,
    AllowAlways,
    AllowSession,
    DenyOnce,
    Reject,
    Inspect,
    Cancel,
}

/// A free-form answer to a rendered question control.  The newtype keeps the
/// reply distinct from unrelated strings while retaining the wire shape used
/// by older clients (`"answer": "..."`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuestionAnswer(pub String);

impl From<String> for QuestionAnswer {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// The explicit decision made by a rendered diff control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnDiffDecision {
    Approve,
    Reject,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReference {
    pub artifact_id: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub content_hash: String,
    pub storage_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundedOutput {
    pub inline: String,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEvent {
    TurnStarted {
        turn_id: String,
    },
    TextDelta {
        turn_id: String,
        text: String,
    },
    ReasoningDelta {
        turn_id: String,
        text: String,
    },
    ToolOutput {
        turn_id: String,
        output: BoundedOutput,
    },
    /// A tool invocation has been observed, before its output is available.
    ToolStarted {
        turn_id: String,
        tool_call_id: String,
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    /// Incremental tool metadata/status (for example, a subprocess pid).
    ToolStatus {
        turn_id: String,
        tool_call_id: String,
        status: ToolStatusValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
    ToolCompleted {
        turn_id: String,
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<BoundedOutput>,
    },
    ToolError {
        turn_id: String,
        tool_call_id: String,
        error: String,
    },
    PermissionRequested {
        turn_id: String,
        #[serde(default)]
        request_id: String,
        tool: String,
        description: String,
    },
    QuestionAsked {
        turn_id: String,
        #[serde(default)]
        question_id: String,
        question: String,
    },
    DiffRequested {
        turn_id: String,
        /// Correlates the review with the originating turn request. Older
        /// daemons did not emit this field, so clients default it to empty.
        #[serde(default)]
        request_id: String,
        path: String,
        diff: String,
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
    CompactionStarted {
        turn_id: String,
    },
    CompactionCompleted {
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    SystemMessage {
        turn_id: String,
        message: String,
    },
    IntegrationEvent {
        turn_id: String,
        integration: String,
        event: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    PlanUpdated {
        turn_id: String,
        plan: serde_json::Value,
    },
    StatusChanged {
        turn_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Telemetry {
        turn_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatusValue {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
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
    fn permission_reply_is_typed_on_the_wire() {
        let action = RequestAction::PermissionReply {
            request_id: "r1".into(),
            choice: TurnPermissionChoice::AllowOnce,
        };
        let json = serde_json::to_value(&action).unwrap();
        assert_eq!(json["type"], "permission_reply");
        assert_eq!(json["choice"], "allow_once");
        assert_eq!(
            serde_json::from_value::<RequestAction>(json).unwrap(),
            action
        );
        assert!(
            serde_json::from_str::<RequestAction>(
                r#"{"type":"permission_reply","request_id":"r1","choice":"approve"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn question_reply_round_trips_without_becoming_a_generic_action() {
        let action = RequestAction::QuestionReply {
            question_id: "q1".into(),
            answer: QuestionAnswer("yes".into()),
        };
        let json = serde_json::to_value(&action).unwrap();
        assert_eq!(json["type"], "question_reply");
        assert_eq!(
            serde_json::from_value::<RequestAction>(json).unwrap(),
            action
        );
    }

    #[test]
    fn diff_reply_round_trips_with_an_explicit_decision() {
        let action = RequestAction::DiffReply {
            path: "src/lib.rs".into(),
            decision: TurnDiffDecision::Reject,
        };
        let json = serde_json::to_value(&action).unwrap();
        assert_eq!(json["decision"], "reject");
        assert_eq!(
            serde_json::from_value::<RequestAction>(json).unwrap(),
            action
        );
    }

    #[test]
    fn rendered_interaction_events_deserialize_with_control_ids() {
        let permission = r#"{"type":"permission_requested","turn_id":"t1","request_id":"r1","tool":"shell","description":"run"}"#;
        assert!(matches!(
            serde_json::from_str::<TurnEvent>(permission).unwrap(),
            TurnEvent::PermissionRequested { .. }
        ));
        let question =
            r#"{"type":"question_asked","turn_id":"t1","question_id":"q1","question":"Continue?"}"#;
        assert!(matches!(
            serde_json::from_str::<TurnEvent>(question).unwrap(),
            TurnEvent::QuestionAsked { question_id, .. } if question_id == "q1"
        ));
        let diff = r#"{"type":"diff_requested","turn_id":"t1","request_id":"d1","path":"a.txt","diff":"@@"}"#;
        assert!(matches!(
            serde_json::from_str::<TurnEvent>(diff).unwrap(),
            TurnEvent::DiffRequested { request_id, path, .. }
                if request_id == "d1" && path == "a.txt"
        ));
    }

    #[test]
    fn extended_events_round_trip_without_parsing_payload_strings() {
        let events = [
            TurnEvent::ReasoningDelta {
                turn_id: "t".into(),
                text: "raw prefix: value".into(),
            },
            TurnEvent::ToolStarted {
                turn_id: "t".into(),
                tool_call_id: "c".into(),
                tool: "shell".into(),
                input: Some(serde_json::json!({"command": "printf x", "nested": [1, true]})),
            },
            TurnEvent::ToolStatus {
                turn_id: "t".into(),
                tool_call_id: "c".into(),
                status: ToolStatusValue::Running,
                metadata: Some(serde_json::json!({"pid": 42})),
            },
            TurnEvent::ToolCompleted {
                turn_id: "t".into(),
                tool_call_id: "c".into(),
                output: Some(BoundedOutput {
                    inline: "output".into(),
                    truncated: false,
                    artifacts: vec![],
                }),
            },
            TurnEvent::ToolError {
                turn_id: "t".into(),
                tool_call_id: "c".into(),
                error: "failed".into(),
            },
            TurnEvent::CompactionCompleted {
                turn_id: "t".into(),
                summary: Some("summary".into()),
            },
            TurnEvent::SystemMessage {
                turn_id: "t".into(),
                message: "system".into(),
            },
            TurnEvent::IntegrationEvent {
                turn_id: "t".into(),
                integration: "slack".into(),
                event: "message".into(),
                data: Some(serde_json::json!({"raw": true})),
            },
            TurnEvent::PlanUpdated {
                turn_id: "t".into(),
                plan: serde_json::json!({"steps": [{"status": "custom"}]}),
            },
            TurnEvent::StatusChanged {
                turn_id: "t".into(),
                status: "thinking".into(),
                message: None,
            },
            TurnEvent::Telemetry {
                turn_id: "t".into(),
                usage: Some(serde_json::json!({"input": 3})),
                context: None,
            },
            TurnEvent::TurnCancelled {
                turn_id: "t".into(),
            },
            TurnEvent::TurnFailed {
                turn_id: "t".into(),
                message: "error".into(),
            },
        ];
        for event in events {
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(serde_json::from_value::<TurnEvent>(value).unwrap(), event);
        }
    }

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

    #[test]
    fn client_responses_round_trip_with_stable_wire_tags() {
        let responses = [
            ClientResponse::Permission {
                request_id: "r1".into(),
                choice: TurnPermissionChoice::AllowOnce,
            },
            ClientResponse::Question {
                question_id: "q1".into(),
                answer: QuestionAnswer("yes".into()),
            },
            ClientResponse::DiffHunk {
                request_id: "d1".into(),
                path: "src/lib.rs".into(),
                index: 2,
                approved: false,
            },
        ];
        for response in responses {
            let value = serde_json::to_value(&response).unwrap();
            let decoded: ClientResponse = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(decoded, response);
            assert!(value.get("type").is_some());
        }

        let params = TurnResponseParams {
            session_id: "s".into(),
            turn_id: "t".into(),
            idempotency_key: IdempotencyKey::new("k"),
            response: ClientResponse::DiffHunk {
                request_id: "d1".into(),
                path: "src/lib.rs".into(),
                index: 0,
                approved: true,
            },
        };
        let decoded: TurnResponseParams =
            serde_json::from_value(serde_json::to_value(&params).unwrap()).unwrap();
        assert_eq!(decoded, params);
    }
}
