//! Typed interactive policy protocol.
use serde::{Deserialize, Serialize};

pub const METHOD_PERMISSION_REQUEST: &str = "policy.permission.request";
pub const METHOD_PERMISSION_REPLY: &str = "policy.permission.reply";
pub const METHOD_QUESTION_REQUEST: &str = "policy.question.request";
pub const METHOD_QUESTION_REPLY: &str = "policy.question.reply";
pub const METHOD_DIFF_REQUEST: &str = "policy.diff.request";
pub const METHOD_DIFF_DECISION: &str = "policy.diff.decision";
pub const METHOD_PLAN_REQUEST: &str = "policy.plan.request";
pub const METHOD_PLAN_REPLY: &str = "policy.plan.reply";
pub const METHOD_AUTONOMY_SET: &str = "policy.autonomy.set";
pub const METHOD_STEERING_ACTION: &str = "policy.steering.action";
pub const METHOD_PROMPT_TAKEOVER: &str = "policy.prompt.takeover";
pub const METHOD_AIRTIGHT_REQUEST: &str = "policy.airtight.request";
pub const METHOD_AIRTIGHT_REPLY: &str = "policy.airtight.reply";

/// Stable identifier for a policy interaction.  It is deliberately kept as a
/// string on the wire so older clients can continue to replay requests.
pub type PolicyRequestId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError(pub &'static str);

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ValidationError {}

fn required(value: &str, name: &'static str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        Err(ValidationError(name))
    } else {
        Ok(())
    }
}

pub trait Validate {
    fn validate(&self) -> Result<(), ValidationError>;
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTakeover {
    pub request_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirtightPromptRequest {
    pub request_id: String,
    pub session_id: String,
    pub plan_id: String,
    pub revision: u64,
    pub step: usize,
    pub initiating_client_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirtightPromptReply {
    pub request_id: String,
    pub idempotency_key: String,
    pub granted: bool,
    pub actor: PromptActor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionScope {
    Once,
    Session,
    Global,
    AlwaysAllow,
    AlwaysReject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionChoice {
    Allow,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptActor {
    Human,
    SteeringAgent,
    SuperAgent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub initiating_client_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionReply {
    pub request_id: String,
    pub idempotency_key: String,
    pub choice: PermissionChoice,
    pub scope: PermissionScope,
    pub actor: PromptActor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub request_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    pub initiating_client_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionReply {
    pub request_id: String,
    pub idempotency_key: String,
    pub answer: String,
    pub actor: PromptActor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffRequest {
    pub request_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub files: Vec<DiffFile>,
    pub initiating_client_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffFile {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
    #[serde(default)]
    pub binary: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub id: String,
    pub patch: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffDecision {
    pub request_id: String,
    pub idempotency_key: String,
    pub decisions: Vec<HunkDecision>,
    pub actor: PromptActor,
}

/// Typed acknowledgement for a diff prompt.  `DiffDecision` remains the
/// request-shaped decision type for wire compatibility with existing clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffReply {
    pub request_id: String,
    pub idempotency_key: String,
    pub accepted: bool,
    pub decisions: Vec<HunkDecision>,
    pub actor: PromptActor,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkDecision {
    pub file: String,
    pub hunk: Option<String>,
    pub accept: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRequest {
    pub request_id: String,
    pub session_id: String,
    pub revision: u64,
    pub action: PlanAction,
    pub actor: PromptActor,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanAction {
    Read,
    Revise {
        payload: String,
    },
    SetCurrent {
        step: usize,
    },
    MarkItem {
        step: usize,
        item: usize,
        checked: bool,
    },
    RequestAirtight {
        step: usize,
    },
    GrantAirtight {
        step: usize,
    },
    RevokeAirtight {
        step: usize,
    },
    Render,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanReply {
    pub request_id: String,
    pub idempotency_key: String,
    pub accepted: bool,
    pub revision: u64,
    pub actor: PromptActor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyMode {
    Manual,
    Normal,
    Super,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomySet {
    pub session_id: String,
    pub idempotency_key: String,
    pub mode: AutonomyMode,
    pub risk_accepted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteeringAction {
    pub session_id: String,
    pub idempotency_key: String,
    pub run_id: String,
    pub action: SteeringActionKind,
    pub actor: PromptActor,
}

impl Validate for PromptTakeover {
    fn validate(&self) -> Result<(), ValidationError> {
        required(&self.request_id, "request_id")?;
        required(&self.idempotency_key, "idempotency_key")
    }
}

macro_rules! validate_reply {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl Validate for $ty {
                fn validate(&self) -> Result<(), ValidationError> {
                    required(&self.request_id, "request_id")?;
                    required(&self.idempotency_key, "idempotency_key")
                }
            }
        )+
    };
}
validate_reply!(
    PermissionReply,
    QuestionReply,
    DiffDecision,
    DiffReply,
    PlanReply,
    AirtightPromptReply
);

impl Validate for PermissionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        required(&self.request_id, "request_id")?;
        required(&self.session_id, "session_id")?;
        required(&self.turn_id, "turn_id")?;
        required(&self.tool, "tool")?;
        required(&self.initiating_client_id, "initiating_client_id")
    }
}

impl Validate for QuestionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        required(&self.request_id, "request_id")?;
        required(&self.session_id, "session_id")?;
        required(&self.turn_id, "turn_id")?;
        required(&self.question, "question")?;
        required(&self.initiating_client_id, "initiating_client_id")
    }
}

impl Validate for AirtightPromptRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        required(&self.request_id, "request_id")?;
        required(&self.session_id, "session_id")?;
        required(&self.plan_id, "plan_id")?;
        required(&self.initiating_client_id, "initiating_client_id")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SteeringActionKind {
    Answer {
        request_id: String,
        answer: String,
    },
    GrantAirtight {
        plan_id: String,
        revision: u64,
        step: usize,
    },
    CompleteGoal {
        report: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_replies_require_request_and_idempotency_keys() {
        let reply = QuestionReply {
            request_id: "request".into(),
            idempotency_key: "reply".into(),
            answer: "yes".into(),
            actor: PromptActor::Human,
        };
        assert!(reply.validate().is_ok());
        assert!(
            QuestionReply {
                request_id: " ".into(),
                ..reply
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn airtight_reply_round_trips_with_actor() {
        let reply = AirtightPromptReply {
            request_id: "request".into(),
            idempotency_key: "reply".into(),
            granted: true,
            actor: PromptActor::SteeringAgent,
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(
            serde_json::from_str::<AirtightPromptReply>(&json)
                .unwrap()
                .granted
        );
    }
}
