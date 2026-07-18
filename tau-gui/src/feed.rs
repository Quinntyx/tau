//! Lossless, typography-first projection of the typed conversation journal.
//!
//! A feed item is presentation metadata only.  The protocol event, ids, and
//! sequence remain attached to it so controls can dispatch typed responses
//! without recovering meaning from labels or rendered text.

use crate::chat::{EventKind, Role, TypedEvent};
use serde_json::Value;

pub const DEFAULT_COLLAPSE_AT: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedCategory {
    Human,
    Assistant,
    Reasoning,
    Tool,
    Permission,
    Question,
    Diff,
    Plan,
    System,
    Integration,
    Compaction,
    Error,
    Cancel,
    Lifecycle,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedAction {
    ToggleTool,
    RespondPermission,
    AnswerQuestion,
    ReviewDiff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActionReachability {
    pub tool_toggle: bool,
    pub permission: bool,
    pub question: bool,
    pub diff: bool,
}

impl ActionReachability {
    pub fn actions(self) -> impl Iterator<Item = FeedAction> {
        [
            (self.tool_toggle, FeedAction::ToggleTool),
            (self.permission, FeedAction::RespondPermission),
            (self.question, FeedAction::AnswerQuestion),
            (self.diff, FeedAction::ReviewDiff),
        ]
        .into_iter()
        .filter_map(|(enabled, action)| enabled.then_some(action))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedState {
    pub following: bool,
    pub streaming: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedItem {
    pub event_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub sequence: u64,
    pub timestamp: i64,
    pub request_id: Option<String>,
    pub role: Role,
    pub category: FeedCategory,
    pub avatar: char,
    pub role_mark: &'static str,
    pub readable_width: bool,
    pub compact_metadata: bool,
    pub detail: String,
    pub preview: String,
    pub collapsed: bool,
    pub raw: tau_proto::prelude::TurnEvent,
    pub raw_json: Value,
    pub actions: ActionReachability,
}

impl FeedItem {
    pub fn action_list(&self) -> impl Iterator<Item = FeedAction> {
        self.actions.actions()
    }

    pub fn visible_detail(&self) -> &str {
        if self.collapsed {
            &self.preview
        } else {
            &self.detail
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedProjection {
    pub items: Vec<FeedItem>,
    pub state: FeedState,
}

impl FeedProjection {
    pub fn from_events(events: &[TypedEvent]) -> Self {
        Self::with_collapse_at(events, DEFAULT_COLLAPSE_AT)
    }

    pub fn with_collapse_at(events: &[TypedEvent], limit: usize) -> Self {
        let mut streaming = false;
        let items = events
            .iter()
            .map(|event| {
                update_streaming(&mut streaming, &event.kind);
                project(event, limit)
            })
            .collect();
        Self {
            items,
            state: FeedState {
                following: true,
                streaming,
            },
        }
    }

    pub fn set_following(&mut self, following: bool) {
        self.state.following = following;
    }
}

pub fn project(event: &TypedEvent, collapse_at: usize) -> FeedItem {
    let (category, role, detail, actions) = classify(&event.kind);
    let preview = detail.chars().take(collapse_at).collect::<String>();
    let collapsed = detail.chars().count() > collapse_at;
    let compact_metadata = matches!(
        category,
        FeedCategory::Tool
            | FeedCategory::Lifecycle
            | FeedCategory::Compaction
            | FeedCategory::Integration
    );
    let readable_width = !compact_metadata && !matches!(category, FeedCategory::Other);
    FeedItem {
        event_id: event.event_id.clone(),
        session_id: event.session_id.clone(),
        turn_id: event.turn_id.clone(),
        sequence: event.sequence,
        timestamp: event.occurred_at,
        request_id: event.request_id.clone(),
        role,
        category,
        avatar: match role {
            Role::User => '●',
            Role::Assistant => '✦',
            Role::Tool => '⚙',
            Role::System => 'ⓘ',
        },
        role_mark: match role {
            Role::User => "human",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System => "system",
        },
        readable_width,
        compact_metadata,
        detail,
        preview,
        collapsed,
        raw: event.raw.clone(),
        raw_json: serde_json::to_value(&event.raw).unwrap_or(Value::Null),
        actions,
    }
}

fn update_streaming(streaming: &mut bool, kind: &EventKind) {
    match kind {
        EventKind::Started
        | EventKind::TextDelta { .. }
        | EventKind::ReasoningDelta { .. }
        | EventKind::ToolStarted { .. }
        | EventKind::ToolStatus { .. }
        | EventKind::ToolOutput { .. } => *streaming = true,
        EventKind::Completed { .. } | EventKind::Cancelled | EventKind::Failed { .. } => {
            *streaming = false
        }
        _ => {}
    }
}

fn classify(kind: &EventKind) -> (FeedCategory, Role, String, ActionReachability) {
    let mut actions = ActionReachability::default();
    let (category, role, detail) = match kind {
        EventKind::TextDelta { text } => (FeedCategory::Assistant, Role::Assistant, text.clone()),
        EventKind::ReasoningDelta { text } => {
            (FeedCategory::Reasoning, Role::Assistant, text.clone())
        }
        EventKind::ToolStarted { tool, input, .. } => {
            actions.tool_toggle = true;
            let detail = input
                .as_deref()
                .map(|value| format!("{tool}\n{value}"))
                .unwrap_or_else(|| tool.clone());
            (FeedCategory::Tool, Role::Tool, detail)
        }
        EventKind::ToolOutput { inline, .. } | EventKind::ToolError { error: inline, .. } => {
            (FeedCategory::Tool, Role::Tool, inline.clone())
        }
        EventKind::ToolStatus {
            status, metadata, ..
        } => (
            FeedCategory::Tool,
            Role::Tool,
            format!(
                "{status:?}{}",
                metadata
                    .as_deref()
                    .map(|value| format!("\n{value}"))
                    .unwrap_or_default()
            ),
        ),
        EventKind::ToolCompleted { output, .. } => (
            FeedCategory::Tool,
            Role::Tool,
            output.clone().unwrap_or_default(),
        ),
        EventKind::PermissionRequested { description, .. } => {
            actions.permission = true;
            (FeedCategory::Permission, Role::System, description.clone())
        }
        EventKind::QuestionAsked { question, .. } => {
            actions.question = true;
            (FeedCategory::Question, Role::System, question.clone())
        }
        EventKind::DiffRequested { path, diff, .. } => {
            actions.diff = true;
            (FeedCategory::Diff, Role::System, format!("{path}\n{diff}"))
        }
        EventKind::PlanUpdated { plan } => (FeedCategory::Plan, Role::System, plan.clone()),
        EventKind::SystemMessage { message } => {
            (FeedCategory::System, Role::System, message.clone())
        }
        EventKind::IntegrationEvent {
            integration,
            event,
            data,
            ..
        } => (
            FeedCategory::Integration,
            Role::System,
            format!(
                "{integration}: {event}{}",
                data.as_deref()
                    .map(|value| format!("\n{value}"))
                    .unwrap_or_default()
            ),
        ),
        EventKind::CompactionStarted => (
            FeedCategory::Compaction,
            Role::System,
            "Compaction started".into(),
        ),
        EventKind::CompactionCompleted { summary } => (
            FeedCategory::Compaction,
            Role::System,
            summary
                .clone()
                .unwrap_or_else(|| "Compaction completed".into()),
        ),
        EventKind::Failed { message } => (FeedCategory::Error, Role::System, message.clone()),
        EventKind::Cancelled => (FeedCategory::Cancel, Role::System, "Turn cancelled".into()),
        EventKind::Started => (FeedCategory::Lifecycle, Role::System, "Turn started".into()),
        EventKind::Completed { message_id } => (
            FeedCategory::Lifecycle,
            Role::System,
            format!(
                "Turn completed{}",
                message_id
                    .map(|id| format!(" (message {id})"))
                    .unwrap_or_default()
            ),
        ),
        EventKind::ArtifactCreated {
            artifact_id,
            media_type,
            size_bytes,
            storage_ref,
            ..
        } => (
            FeedCategory::Other,
            Role::System,
            format!("{artifact_id} · {media_type} · {size_bytes} bytes · {storage_ref}"),
        ),
        EventKind::StatusChanged { status, message } => (
            FeedCategory::System,
            Role::System,
            format!(
                "{status}{}",
                message
                    .as_deref()
                    .map(|value| format!(": {value}"))
                    .unwrap_or_default()
            ),
        ),
        EventKind::Telemetry { usage, context } => (
            FeedCategory::Other,
            Role::System,
            format!(
                "usage={} context={}",
                usage.as_deref().unwrap_or("unavailable"),
                context.as_deref().unwrap_or("unavailable")
            ),
        ),
        EventKind::Other => (
            FeedCategory::Other,
            Role::System,
            "Unknown typed event".into(),
        ),
    };
    (category, role, detail, actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_proto::prelude::{SequencedEvent, TurnEvent};

    fn event(sequence: u64, event: TurnEvent) -> TypedEvent {
        TypedEvent {
            event_id: format!("event-{sequence}"),
            session_id: "session".into(),
            sequence,
            occurred_at: 42,
            request_id: None,
            turn_id: "turn".into(),
            kind: match &event {
                TurnEvent::TurnStarted { .. } => EventKind::Started,
                TurnEvent::TextDelta { text, .. } => EventKind::TextDelta { text: text.clone() },
                TurnEvent::TurnFailed { message, .. } => EventKind::Failed {
                    message: message.clone(),
                },
                TurnEvent::TurnCancelled { .. } => EventKind::Cancelled,
                TurnEvent::PermissionRequested {
                    request_id,
                    tool,
                    description,
                    ..
                } => EventKind::PermissionRequested {
                    permission_id: request_id.clone(),
                    tool: tool.clone(),
                    description: description.clone(),
                },
                _ => EventKind::Other,
            },
            raw: event,
        }
    }

    #[test]
    fn projection_keeps_wire_identity_and_payload() {
        let raw = TurnEvent::PermissionRequested {
            turn_id: "turn".into(),
            request_id: "permission".into(),
            tool: "shell".into(),
            description: "run".into(),
        };
        let item = project(&event(7, raw.clone()), 32);
        assert_eq!(item.event_id, "event-7");
        assert_eq!(item.sequence, 7);
        assert_eq!(item.raw, raw);
        assert!(item.actions.permission);
        assert_eq!(item.raw_json["type"], "permission_requested");
    }

    #[test]
    fn long_details_have_a_lossless_collapsed_preview() {
        let text = "x".repeat(20);
        let item = project(
            &event(
                1,
                TurnEvent::TextDelta {
                    turn_id: "turn".into(),
                    text,
                },
            ),
            8,
        );
        assert!(item.collapsed);
        assert_eq!(item.preview, "xxxxxxxx");
        assert_eq!(item.detail.len(), 20);
        assert_eq!(item.visible_detail(), "xxxxxxxx");
    }

    #[test]
    fn stream_state_follows_terminal_lifecycle() {
        let events = vec![
            event(
                1,
                TurnEvent::TurnStarted {
                    turn_id: "turn".into(),
                },
            ),
            event(
                2,
                TurnEvent::TurnFailed {
                    turn_id: "turn".into(),
                    message: "nope".into(),
                },
            ),
        ];
        assert!(!FeedProjection::from_events(&events).state.streaming);
    }

    #[test]
    fn ids_are_not_reconstructed_from_display_text() {
        let event = event(
            4,
            TurnEvent::TurnStarted {
                turn_id: "turn".into(),
            },
        );
        let item = project(&event, DEFAULT_COLLAPSE_AT);
        assert_eq!(item.event_id, event.event_id);
        assert_eq!(item.turn_id, event.turn_id);
        let _ = SequencedEvent {
            event_id: item.event_id,
            session_id: item.session_id,
            sequence: item.sequence,
            occurred_at: item.timestamp,
            request_id: None,
            event: item.raw,
        };
    }
}
