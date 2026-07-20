//! Lossless, typed projection of the turn journal for terminal presentation.

use crate::state::Connection;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tau_proto::prelude::{SequencedEvent, TurnEvent};

pub const DETAIL_LIMIT: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedAction {
    ToggleDetails,
    Permission,
    Question,
    Diff,
}

#[derive(Debug, Clone)]
pub struct FeedItem {
    /// The complete wire event, retained for replay, inspection, and typed
    /// response correlation.  Rendering only reads presentation fields below.
    pub event: SequencedEvent,
    pub mark: &'static str,
    pub title: String,
    pub body: String,
    pub preview: String,
    pub metadata: String,
    pub collapsed: bool,
    pub actions: Vec<FeedAction>,
}

impl FeedItem {
    pub fn visible_body(&self) -> &str {
        if self.collapsed {
            &self.preview
        } else {
            &self.body
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeedProjection {
    pub items: Vec<FeedItem>,
    pub human_messages: Vec<String>,
    pub streaming: bool,
    pub following: bool,
}

pub fn project(
    events: &[SequencedEvent],
    connection: Connection,
    following: bool,
) -> FeedProjection {
    project_with_humans(events, &[], connection, following)
}

pub fn project_with_humans(
    events: &[SequencedEvent],
    human_messages: &[String],
    connection: Connection,
    following: bool,
) -> FeedProjection {
    // Sort for replay/reconnect without deduplicating: callers may inspect
    // every supplied payload, including duplicate sequence values from a
    // reconnect race.  The reducer owns sequence de-duplication semantics.
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|event| event.sequence);
    let mut streaming = false;
    let items = sorted
        .into_iter()
        .map(|event| {
            update_streaming(&mut streaming, &event.event);
            item(event)
        })
        .collect();
    FeedProjection {
        items,
        human_messages: human_messages.to_vec(),
        streaming,
        following: following && connection == Connection::Connected,
    }
}

fn update_streaming(streaming: &mut bool, event: &TurnEvent) {
    match event {
        TurnEvent::TurnStarted { .. }
        | TurnEvent::TextDelta { .. }
        | TurnEvent::ReasoningDelta { .. }
        | TurnEvent::ToolStarted { .. }
        | TurnEvent::ToolStatus { .. }
        | TurnEvent::ToolOutput { .. } => *streaming = true,
        TurnEvent::TurnCompleted { .. }
        | TurnEvent::TurnCancelled { .. }
        | TurnEvent::TurnFailed { .. } => *streaming = false,
        _ => {}
    }
}

fn json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "<invalid json>".into())
}

fn item(event: SequencedEvent) -> FeedItem {
    let (mark, title, body, actions) = describe(&event.event);
    let preview = body.chars().take(DETAIL_LIMIT).collect::<String>();
    let collapsed = body.chars().count() > DETAIL_LIMIT;
    FeedItem {
        metadata: format!(
            "#{} · {} · {}",
            event.sequence, event.occurred_at, event.event_id
        ),
        event,
        mark,
        title,
        body,
        preview,
        collapsed,
        actions,
    }
}

fn describe(event: &TurnEvent) -> (&'static str, String, String, Vec<FeedAction>) {
    use TurnEvent::*;
    match event {
        TurnStarted { .. } => ("◇", "turn started".into(), String::new(), vec![]),
        TextDelta { text, .. } => ("A", "assistant".into(), text.clone(), vec![]),
        ReasoningDelta { text, .. } => (
            "∴",
            "reasoning".into(),
            text.clone(),
            vec![FeedAction::ToggleDetails],
        ),
        ToolOutput { output, .. } => (
            "⚙",
            "tool output".into(),
            output.inline.clone(),
            vec![FeedAction::ToggleDetails],
        ),
        ToolStarted {
            tool,
            tool_call_id,
            input,
            ..
        } => (
            "⚙",
            format!("tool {tool} · {tool_call_id}"),
            input.as_ref().map(json).unwrap_or_default(),
            vec![FeedAction::ToggleDetails],
        ),
        ToolStatus {
            tool_call_id,
            status,
            metadata,
            ..
        } => (
            "⚙",
            format!("tool {tool_call_id}: {status:?}"),
            metadata.as_ref().map(json).unwrap_or_default(),
            vec![FeedAction::ToggleDetails],
        ),
        ToolCompleted {
            tool_call_id,
            output,
            ..
        } => (
            "✓",
            format!("tool {tool_call_id} complete"),
            output
                .as_ref()
                .map(|value| value.inline.clone())
                .unwrap_or_default(),
            vec![FeedAction::ToggleDetails],
        ),
        ToolError {
            tool_call_id,
            error,
            ..
        } => (
            "!",
            format!("tool {tool_call_id} failed"),
            error.clone(),
            vec![FeedAction::ToggleDetails],
        ),
        PermissionRequested {
            tool, description, ..
        } => (
            "?",
            format!("permission: {tool}"),
            description.clone(),
            vec![FeedAction::Permission],
        ),
        QuestionAsked { question, .. } => (
            "?",
            "question".into(),
            question.clone(),
            vec![FeedAction::Question],
        ),
        DiffRequested { path, diff, .. } => (
            "±",
            format!("diff {path}"),
            diff.clone(),
            vec![FeedAction::Diff, FeedAction::ToggleDetails],
        ),
        ArtifactCreated { artifact, .. } => (
            "▣",
            format!("artifact {}", artifact.artifact_id),
            format!("{} · {} bytes", artifact.media_type, artifact.size_bytes),
            vec![],
        ),
        CompactionStarted { .. } => ("…", "compaction started".into(), String::new(), vec![]),
        CompactionCompleted { summary, .. } => (
            "…",
            "compaction complete".into(),
            summary.clone().unwrap_or_default(),
            vec![FeedAction::ToggleDetails],
        ),
        SystemMessage { message, .. } => ("·", "system".into(), message.clone(), vec![]),
        IntegrationEvent {
            integration,
            event,
            data,
            ..
        } => (
            "↔",
            format!("{integration}: {event}"),
            data.as_ref().map(json).unwrap_or_default(),
            vec![FeedAction::ToggleDetails],
        ),
        PlanUpdated { plan, .. } => (
            "☷",
            "plan updated".into(),
            json(plan),
            vec![FeedAction::ToggleDetails],
        ),
        StatusChanged {
            status, message, ..
        } => (
            "·",
            format!("status: {status}"),
            message.clone().unwrap_or_default(),
            vec![],
        ),
        Telemetry { usage, context, .. } => (
            "·",
            "telemetry".into(),
            format!(
                "usage={}\ncontext={}",
                usage.as_ref().map(json).unwrap_or_default(),
                context.as_ref().map(json).unwrap_or_default()
            ),
            vec![FeedAction::ToggleDetails],
        ),
        TurnCompleted { .. } => ("✓", "turn complete".into(), String::new(), vec![]),
        TurnCancelled { .. } => ("×", "turn cancelled".into(), String::new(), vec![]),
        TurnFailed { message, .. } => ("!", "turn failed".into(), message.clone(), vec![]),
    }
}

pub fn render(frame: &mut Frame, area: Rect, feed: &FeedProjection) {
    let mut lines = Vec::new();
    for (index, message) in feed.human_messages.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled("U", Style::default().fg(Color::Green)),
            Span::raw(format!(" human  prompt-{}", index + 1)),
        ]));
        lines.push(Line::from(format!("  {message}")));
    }
    for item in &feed.items {
        lines.push(Line::from(vec![
            Span::styled(item.mark, Style::default().fg(Color::Cyan)),
            Span::raw(format!(" {}  {}", item.title, item.metadata)),
        ]));
        let detail = if item.collapsed {
            format!("{}… [details]", item.visible_body())
        } else {
            item.visible_body().to_owned()
        };
        lines.push(Line::from(format!("  {detail}")));
    }
    let status = if feed.streaming {
        " · streaming"
    } else if feed.following {
        " · following"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Conversation{status} ")),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn event(sequence: u64, event: TurnEvent) -> SequencedEvent {
        SequencedEvent {
            event_id: format!("event-{sequence}"),
            session_id: "session".into(),
            sequence,
            occurred_at: 123,
            request_id: None,
            event,
        }
    }

    #[test]
    fn projection_preserves_payloads_and_interaction_actions() {
        let raw = TurnEvent::PermissionRequested {
            turn_id: "turn".into(),
            request_id: "permission".into(),
            tool: "shell".into(),
            description: "run command".into(),
        };
        let raw_copy = raw.clone();
        let feed = project(&[event(9, raw)], Connection::Connected, true);
        assert_eq!(feed.items[0].event.event_id, "event-9");
        assert_eq!(feed.items[0].event.event, raw_copy);
        assert_eq!(feed.items[0].actions, vec![FeedAction::Permission]);
    }

    #[test]
    fn long_content_collapses_without_losing_the_wire_event() {
        let raw = TurnEvent::TextDelta {
            turn_id: "turn".into(),
            text: "x".repeat(DETAIL_LIMIT + 1),
        };
        let feed = project(&[event(1, raw.clone())], Connection::Connected, true);
        assert!(feed.items[0].collapsed);
        assert_eq!(feed.items[0].body, "x".repeat(DETAIL_LIMIT + 1));
        assert_eq!(feed.items[0].event.event, raw);
    }

    #[test]
    fn reconnect_does_not_drop_duplicate_sequence_payloads() {
        let first = event(
            2,
            TurnEvent::SystemMessage {
                turn_id: "turn".into(),
                message: "first".into(),
            },
        );
        let second = event(
            2,
            TurnEvent::SystemMessage {
                turn_id: "turn".into(),
                message: "second".into(),
            },
        );
        let feed = project(&[second, first], Connection::Reconnecting, true);
        assert_eq!(feed.items.len(), 2);
        assert!(!feed.following);
    }

    #[test]
    fn snapshot_has_typography_and_action_metadata() {
        let events = [event(
            1,
            TurnEvent::QuestionAsked {
                turn_id: "turn".into(),
                question_id: "question".into(),
                question: "Continue?".into(),
            },
        )];
        let feed = project(&events, Connection::Connected, true);
        let mut terminal = Terminal::new(TestBackend::new(72, 8)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &feed))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("question"));
        assert!(text.contains("#1"));
    }
}
