//! Pure, typed chat model used by the GPUI view and scripted fixtures.
use std::collections::{BTreeMap, HashMap};

use tau_client::CompletionEvent;
use tau_proto::prelude::{CompletionStreamResult, SequencedEvent, ToolStatusValue, TurnEvent};

/// Correlation and lifecycle information retained alongside the presentation cards.
/// This deliberately is not rendered as text: consumers can reconstruct an exact
/// transcript (including events which have no visual card) from this log.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedEvent {
    pub event_id: String,
    pub session_id: String,
    pub sequence: u64,
    pub occurred_at: i64,
    pub request_id: Option<String>,
    pub turn_id: String,
    pub kind: EventKind,
    /// Lossless protocol payload; projections never parse display strings.
    pub raw: TurnEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Started,
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolOutput {
        inline: String,
        truncated: bool,
        artifacts: Vec<String>,
    },
    ToolStarted {
        call_id: String,
        tool: String,
        input: Option<String>,
    },
    ToolStatus {
        call_id: String,
        status: ToolStatusValue,
        metadata: Option<String>,
    },
    ToolCompleted {
        call_id: String,
        output: Option<String>,
    },
    ToolError {
        call_id: String,
        error: String,
    },
    PermissionRequested {
        permission_id: String,
        tool: String,
        description: String,
    },
    QuestionAsked {
        question_id: String,
        question: String,
    },
    DiffRequested {
        request_id: String,
        path: String,
        diff: String,
    },
    ArtifactCreated {
        artifact_id: String,
        media_type: String,
        size_bytes: u64,
        content_hash: String,
        storage_ref: String,
    },
    Completed {
        message_id: Option<i64>,
    },
    Cancelled,
    Failed {
        message: String,
    },
    CompactionStarted,
    CompactionCompleted {
        summary: Option<String>,
    },
    SystemMessage {
        message: String,
    },
    IntegrationEvent {
        integration: String,
        event: String,
        data: Option<String>,
    },
    PlanUpdated {
        plan: String,
    },
    StatusChanged {
        status: String,
        message: Option<String>,
    },
    Telemetry {
        usage: Option<String>,
        context: Option<String>,
    },
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SidebarState {
    pub plan: Option<String>,
    pub current_step: Option<String>,
    pub airtight: Option<bool>,
    pub lsp: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub context_tokens: Option<u64>,
    pub context_percent: Option<u8>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub roots: Vec<String>,
    pub mcp: Option<String>,
    pub task_tier: Option<u8>,
    pub autonomous: Option<bool>,
    pub agent: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    Message {
        role: Role,
        text: String,
    },
    Tool {
        name: String,
        input: String,
        output: String,
        expanded: bool,
    },
    Diff {
        request_id: String,
        path: String,
        patch: String,
        approved: bool,
    },
    Permission {
        request_id: String,
        tool: String,
        description: String,
    },
    Question {
        question_id: String,
        question: String,
        answer: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Message,
    Tool,
    Diff,
    Permission,
    Question,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardMetadata {
    pub kind: CardKind,
    pub interactive: bool,
    pub expanded: bool,
}

impl Card {
    pub fn metadata(&self) -> CardMetadata {
        match self {
            Self::Message { .. } => CardMetadata {
                kind: CardKind::Message,
                interactive: false,
                expanded: false,
            },
            Self::Tool { expanded, .. } => CardMetadata {
                kind: CardKind::Tool,
                interactive: true,
                expanded: *expanded,
            },
            Self::Diff { approved, .. } => CardMetadata {
                kind: CardKind::Diff,
                interactive: !*approved,
                expanded: true,
            },
            Self::Permission { .. } => CardMetadata {
                kind: CardKind::Permission,
                interactive: true,
                expanded: true,
            },
            Self::Question { answer, .. } => CardMetadata {
                kind: CardKind::Question,
                interactive: answer.is_none(),
                expanded: true,
            },
        }
    }

    pub fn preview(&self, max_chars: usize) -> String {
        let text = match self {
            Self::Message { text, .. } => text,
            Self::Tool { name, output, .. } => {
                if output.is_empty() {
                    name
                } else {
                    output
                }
            }
            Self::Diff { path, patch, .. } => {
                if patch.is_empty() {
                    path
                } else {
                    patch
                }
            }
            Self::Permission { description, .. } => description,
            Self::Question { question, .. } => question,
        };
        text.chars().take(max_chars).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatStatus {
    Ready,
    Streaming,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    AllowOnce,
    AllowAlways,
    Reject,
    Inspect,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyState {
    Pending,
    Ack,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffDecision {
    Hunk { hunk: usize, approved: bool },
    WholeFile { approved: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatState {
    pub cards: Vec<Card>,
    pub session_id: Option<String>,
    pub usage: Option<u64>,
    pub status: ChatStatus,
    pub active_assistant: Option<usize>,
    /// All protocol events, including lifecycle and artifact events without cards.
    pub events: Vec<TypedEvent>,
    /// Most recent raw user input and assistant output, retained independently of
    /// the legacy display cards.
    pub raw_input: Option<String>,
    pub raw_output: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub artifacts: Vec<(String, String, u64, String, String)>,
    pub sidebar: SidebarState,
    /// Correlates protocol tool lifecycle events to one rendered card.
    pub tool_indices: HashMap<String, usize>,
    pub tool_statuses: HashMap<String, ToolStatusValue>,
    pub policy_states: BTreeMap<String, PolicyState>,
    pub diff_decisions: BTreeMap<String, Vec<DiffDecision>>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            cards: Vec::new(),
            session_id: None,
            usage: None,
            status: ChatStatus::Ready,
            active_assistant: None,
            events: Vec::new(),
            raw_input: None,
            raw_output: None,
            loading: false,
            error: None,
            artifacts: Vec::new(),
            sidebar: SidebarState::default(),
            tool_indices: HashMap::new(),
            tool_statuses: HashMap::new(),
            policy_states: BTreeMap::new(),
            diff_decisions: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ChatAction {
    Submit(String),
    Completion(CompletionEvent),
    /// Durable event from the typed session backend.
    SessionEvent(SequencedEvent),
    Error(String),
    ToggleTool(usize),
    ApproveDiff(usize, bool),
    Permission(usize, PermissionChoice),
    AnswerQuestion(usize, String),
    PolicyAck {
        request_id: String,
    },
    PolicyError {
        request_id: String,
        message: String,
    },
    RetryPolicy {
        request_id: String,
    },
    DiffHunk {
        request_id: String,
        hunk: usize,
        approved: bool,
    },
    DiffWholeFile {
        request_id: String,
        approved: bool,
    },
}

impl ChatState {
    pub fn is_streaming(&self) -> bool {
        matches!(self.status, ChatStatus::Streaming)
    }

    pub fn can_submit(&self) -> bool {
        !self.is_streaming()
    }

    pub fn history(&self) -> impl Iterator<Item = &str> {
        self.cards.iter().filter_map(|card| match card {
            Card::Message {
                role: Role::User,
                text,
            } => Some(text.as_str()),
            _ => None,
        })
    }

    pub fn reduce(&mut self, action: ChatAction) -> bool {
        match action {
            ChatAction::SessionEvent(event) => self.apply_session_event(event),
            ChatAction::Submit(text)
                if !text.trim().is_empty() && self.active_assistant.is_none() =>
            {
                self.raw_input = Some(text.clone());
                self.cards.push(Card::Message {
                    role: Role::User,
                    text,
                });
                let index = self.cards.len();
                self.cards.push(Card::Message {
                    role: Role::Assistant,
                    text: String::new(),
                });
                self.active_assistant = Some(index);
                self.status = ChatStatus::Streaming;
                self.loading = true;
                self.error = None;
                true
            }
            ChatAction::Completion(CompletionEvent::Delta(delta)) => {
                let Some(index) = self.active_assistant else {
                    return false;
                };
                if let Card::Message { text, .. } = &mut self.cards[index] {
                    text.push_str(&delta.text);
                }
                true
            }
            ChatAction::Completion(CompletionEvent::Complete(result)) => self.finish(result),
            ChatAction::Error(error) => {
                let Some(index) = self.active_assistant.take() else {
                    return false;
                };
                if let Card::Message { text, .. } = &mut self.cards[index] {
                    *text = format!("Error: {error}");
                }
                self.status = ChatStatus::Failed(error.clone());
                self.loading = false;
                self.error = Some(error);
                true
            }
            ChatAction::ToggleTool(index) => match self.cards.get_mut(index) {
                Some(Card::Tool { expanded, .. }) => {
                    *expanded = !*expanded;
                    true
                }
                _ => false,
            },
            ChatAction::ApproveDiff(index, approved) => match self.cards.get_mut(index) {
                Some(Card::Diff {
                    request_id,
                    approved: value,
                    ..
                }) => {
                    let _ = (value, approved);
                    self.policy_states
                        .insert(request_id.clone(), PolicyState::Pending);
                    true
                }
                _ => false,
            },
            ChatAction::Permission(index, choice) => match self.cards.get(index) {
                Some(Card::Permission { request_id, .. }) => {
                    if !matches!(choice, PermissionChoice::Inspect) {
                        self.policy_states
                            .insert(request_id.clone(), PolicyState::Pending);
                    }
                    true
                }
                _ => false,
            },
            ChatAction::AnswerQuestion(index, answer) => match self.cards.get_mut(index) {
                Some(Card::Question { question_id, .. }) => {
                    let _ = answer;
                    self.policy_states
                        .insert(question_id.clone(), PolicyState::Pending);
                    true
                }
                _ => false,
            },
            ChatAction::PolicyAck { request_id } => {
                self.policy_states
                    .insert(request_id.clone(), PolicyState::Ack);
                self.cards.retain(|card| {
                    !matches!(card,
                        Card::Permission { request_id: id, .. }
                        | Card::Diff { request_id: id, .. }
                        | Card::Question { question_id: id, .. }
                        if id == &request_id)
                });
                true
            }
            ChatAction::PolicyError {
                request_id,
                message,
            } => {
                self.policy_states
                    .insert(request_id, PolicyState::Error(message));
                true
            }
            ChatAction::RetryPolicy { request_id } => {
                self.policy_states.insert(request_id, PolicyState::Pending);
                true
            }
            ChatAction::DiffHunk {
                request_id,
                hunk,
                approved,
            } => {
                self.policy_states
                    .insert(request_id.clone(), PolicyState::Pending);
                self.diff_decisions
                    .entry(request_id)
                    .or_default()
                    .push(DiffDecision::Hunk { hunk, approved });
                true
            }
            ChatAction::DiffWholeFile {
                request_id,
                approved,
            } => {
                self.policy_states
                    .insert(request_id.clone(), PolicyState::Pending);
                self.diff_decisions
                    .entry(request_id)
                    .or_default()
                    .push(DiffDecision::WholeFile { approved });
                true
            }
            ChatAction::Submit(_) => false,
        }
    }

    fn apply_session_event(&mut self, event: SequencedEvent) -> bool {
        let typed = typed_event(&event);
        self.raw_output = match &typed.kind {
            EventKind::TextDelta { text } => Some(format!(
                "{}{}",
                self.raw_output.as_deref().unwrap_or(""),
                text
            )),
            _ => self.raw_output.clone(),
        };
        self.session_id = Some(event.session_id.clone());
        self.sidebar.session_id = Some(event.session_id.clone());
        self.events.push(typed);
        match event.event {
            TurnEvent::TurnStarted { .. } => {
                self.status = ChatStatus::Streaming;
                self.loading = true;
                self.error = None;
                if self.active_assistant.is_none() {
                    self.cards.push(Card::Message {
                        role: Role::Assistant,
                        text: String::new(),
                    });
                    self.active_assistant = Some(self.cards.len() - 1);
                }
                true
            }
            TurnEvent::TextDelta { text, .. } => {
                let index = if let Some(index) = self.active_assistant {
                    index
                } else {
                    self.cards.push(Card::Message {
                        role: Role::Assistant,
                        text: String::new(),
                    });
                    self.active_assistant = Some(self.cards.len() - 1);
                    self.cards.len() - 1
                };
                self.status = ChatStatus::Streaming;
                self.loading = true;
                if let Card::Message { text: output, .. } = &mut self.cards[index] {
                    output.push_str(&text);
                    return true;
                }
                false
            }
            TurnEvent::ReasoningDelta { text, .. } => {
                self.cards.push(Card::Message {
                    role: Role::Assistant,
                    text: format!("Reasoning: {text}"),
                });
                true
            }
            TurnEvent::ToolOutput { output, .. } => {
                self.cards.push(Card::Tool {
                    name: "tool".into(),
                    input: String::new(),
                    output: output.inline,
                    expanded: false,
                });
                true
            }
            TurnEvent::ToolStarted {
                tool_call_id,
                tool,
                input,
                ..
            } => {
                self.cards.push(Card::Tool {
                    name: tool,
                    input: input.map(|value| value.to_string()).unwrap_or_default(),
                    output: String::new(),
                    expanded: false,
                });
                self.tool_indices
                    .insert(tool_call_id.clone(), self.cards.len() - 1);
                self.tool_statuses
                    .insert(tool_call_id, ToolStatusValue::Running);
                true
            }
            TurnEvent::ToolStatus {
                tool_call_id,
                status,
                ..
            } => {
                self.tool_statuses.insert(tool_call_id, status);
                true
            }
            TurnEvent::ToolCompleted {
                tool_call_id,
                output,
                ..
            } => {
                self.tool_statuses
                    .insert(tool_call_id.clone(), ToolStatusValue::Completed);
                if let Some(index) = self.tool_indices.get(&tool_call_id).copied() {
                    if let Some(Card::Tool { output: value, .. }) = self.cards.get_mut(index) {
                        if let Some(output) = output {
                            *value = output.inline;
                        }
                    }
                } else if let Some(output) = output {
                    self.cards.push(Card::Tool {
                        name: "tool".into(),
                        input: String::new(),
                        output: output.inline,
                        expanded: false,
                    });
                    self.tool_indices.insert(tool_call_id, self.cards.len() - 1);
                }
                true
            }
            TurnEvent::ToolError {
                tool_call_id,
                error,
                ..
            } => {
                self.tool_statuses
                    .insert(tool_call_id.clone(), ToolStatusValue::Failed);
                if let Some(index) = self.tool_indices.get(&tool_call_id).copied() {
                    if let Some(Card::Tool { output, .. }) = self.cards.get_mut(index) {
                        *output = error;
                    }
                } else {
                    self.cards.push(Card::Message {
                        role: Role::Tool,
                        text: format!("Tool error: {error}"),
                    });
                }
                true
            }
            TurnEvent::PermissionRequested {
                request_id,
                tool,
                description,
                ..
            } => {
                self.cards.push(Card::Permission {
                    request_id,
                    tool,
                    description,
                });
                true
            }
            TurnEvent::QuestionAsked {
                question_id,
                question,
                ..
            } => {
                self.cards.push(Card::Question {
                    question_id,
                    question,
                    answer: None,
                });
                true
            }
            TurnEvent::DiffRequested {
                request_id,
                path,
                diff,
                ..
            } => {
                self.cards.push(Card::Diff {
                    request_id,
                    path,
                    patch: diff,
                    approved: false,
                });
                true
            }
            TurnEvent::TurnCompleted { .. } => {
                self.active_assistant = None;
                self.status = ChatStatus::Ready;
                self.loading = false;
                true
            }
            TurnEvent::TurnCancelled { .. } => {
                self.active_assistant = None;
                self.status = ChatStatus::Ready;
                self.loading = false;
                true
            }
            TurnEvent::TurnFailed { message, .. } => {
                self.active_assistant = None;
                self.error = Some(message.clone());
                self.status = ChatStatus::Failed(message);
                self.loading = false;
                true
            }
            TurnEvent::ArtifactCreated { artifact, .. } => {
                self.artifacts.push((
                    artifact.artifact_id,
                    artifact.media_type,
                    artifact.size_bytes,
                    artifact.content_hash,
                    artifact.storage_ref,
                ));
                true
            }
            TurnEvent::CompactionStarted { .. } => {
                self.cards.push(Card::Message {
                    role: Role::System,
                    text: "Compaction started".into(),
                });
                true
            }
            TurnEvent::CompactionCompleted { summary, .. } => {
                self.cards.push(Card::Message {
                    role: Role::System,
                    text: summary.unwrap_or_else(|| "Compaction completed".into()),
                });
                true
            }
            TurnEvent::SystemMessage { message, .. } => {
                self.cards.push(Card::Message {
                    role: Role::System,
                    text: message,
                });
                true
            }
            TurnEvent::IntegrationEvent {
                integration,
                event,
                data,
                ..
            } => {
                if integration.eq_ignore_ascii_case("lsp") {
                    self.sidebar.lsp = Some(event.clone());
                } else if integration.eq_ignore_ascii_case("mcp") {
                    self.sidebar.mcp = Some(event.clone());
                }
                self.cards.push(Card::Message {
                    role: Role::System,
                    text: format!(
                        "{integration}: {event}{}",
                        data.map(|value| format!("\n{value}")).unwrap_or_default()
                    ),
                });
                true
            }
            TurnEvent::PlanUpdated { plan, .. } => {
                self.sidebar.plan = plan
                    .get("title")
                    .or_else(|| plan.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                self.sidebar.current_step = plan
                    .get("current_step")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| {
                        plan.get("steps")
                            .and_then(serde_json::Value::as_array)
                            .and_then(|steps| {
                                steps.iter().find_map(|step| {
                                    (step.get("status").and_then(serde_json::Value::as_str)
                                        == Some("running"))
                                    .then(|| {
                                        step.get("name")
                                            .or_else(|| step.get("title"))
                                            .and_then(serde_json::Value::as_str)
                                    })
                                    .flatten()
                                })
                            })
                    })
                    .map(str::to_owned);
                self.sidebar.airtight = plan.get("airtight").and_then(serde_json::Value::as_bool);
                true
            }
            TurnEvent::StatusChanged {
                status, message, ..
            } => {
                if matches!(status.as_str(), "failed" | "error") {
                    let error = message.unwrap_or_else(|| status.clone());
                    self.error = Some(error.clone());
                    self.status = ChatStatus::Failed(error);
                }
                true
            }
            TurnEvent::Telemetry { usage, context, .. } => {
                update_usage(&mut self.sidebar, usage.as_ref());
                update_context(&mut self.sidebar, context.as_ref());
                true
            }
        }
    }

    fn finish(&mut self, result: CompletionStreamResult) -> bool {
        let Some(index) = self.active_assistant.take() else {
            return false;
        };
        if let Card::Message { text, .. } = &mut self.cards[index] {
            self.raw_output = Some(result.text.clone());
            *text = result.text;
        }
        self.session_id = Some(result.session_id);
        self.usage = Some(result.usage.total_tokens);
        self.status = ChatStatus::Ready;
        self.loading = false;
        self.error = None;
        true
    }
}

fn typed_event(event: &SequencedEvent) -> TypedEvent {
    let (turn_id, kind) = match &event.event {
        TurnEvent::TurnStarted { turn_id } => (turn_id.clone(), EventKind::Started),
        TurnEvent::TextDelta { turn_id, text } => {
            (turn_id.clone(), EventKind::TextDelta { text: text.clone() })
        }
        TurnEvent::ReasoningDelta { turn_id, text } => (
            turn_id.clone(),
            EventKind::ReasoningDelta { text: text.clone() },
        ),
        TurnEvent::ToolOutput { turn_id, output } => (
            turn_id.clone(),
            EventKind::ToolOutput {
                inline: output.inline.clone(),
                truncated: output.truncated,
                artifacts: output
                    .artifacts
                    .iter()
                    .map(|a| a.artifact_id.clone())
                    .collect(),
            },
        ),
        TurnEvent::ToolStarted {
            turn_id,
            tool_call_id,
            tool,
            input,
        } => (
            turn_id.clone(),
            EventKind::ToolStarted {
                call_id: tool_call_id.clone(),
                tool: tool.clone(),
                input: input.as_ref().map(ToString::to_string),
            },
        ),
        TurnEvent::ToolStatus {
            turn_id,
            tool_call_id,
            status,
            metadata,
        } => (
            turn_id.clone(),
            EventKind::ToolStatus {
                call_id: tool_call_id.clone(),
                status: status.clone(),
                metadata: metadata.as_ref().map(ToString::to_string),
            },
        ),
        TurnEvent::ToolCompleted {
            turn_id,
            tool_call_id,
            output,
        } => (
            turn_id.clone(),
            EventKind::ToolCompleted {
                call_id: tool_call_id.clone(),
                output: output.as_ref().map(|value| value.inline.clone()),
            },
        ),
        TurnEvent::ToolError {
            turn_id,
            tool_call_id,
            error,
        } => (
            turn_id.clone(),
            EventKind::ToolError {
                call_id: tool_call_id.clone(),
                error: error.clone(),
            },
        ),
        TurnEvent::PermissionRequested {
            turn_id,
            request_id,
            tool,
            description,
        } => (
            turn_id.clone(),
            EventKind::PermissionRequested {
                permission_id: request_id.clone(),
                tool: tool.clone(),
                description: description.clone(),
            },
        ),
        TurnEvent::QuestionAsked {
            turn_id,
            question_id,
            question,
        } => (
            turn_id.clone(),
            EventKind::QuestionAsked {
                question_id: question_id.clone(),
                question: question.clone(),
            },
        ),
        TurnEvent::DiffRequested {
            turn_id,
            request_id,
            path,
            diff,
        } => (
            turn_id.clone(),
            EventKind::DiffRequested {
                request_id: request_id.clone(),
                path: path.clone(),
                diff: diff.clone(),
            },
        ),
        TurnEvent::ArtifactCreated { turn_id, artifact } => (
            turn_id.clone(),
            EventKind::ArtifactCreated {
                artifact_id: artifact.artifact_id.clone(),
                media_type: artifact.media_type.clone(),
                size_bytes: artifact.size_bytes,
                content_hash: artifact.content_hash.clone(),
                storage_ref: artifact.storage_ref.clone(),
            },
        ),
        TurnEvent::TurnCompleted {
            turn_id,
            message_id,
        } => (
            turn_id.clone(),
            EventKind::Completed {
                message_id: *message_id,
            },
        ),
        TurnEvent::TurnCancelled { turn_id } => (turn_id.clone(), EventKind::Cancelled),
        TurnEvent::TurnFailed { turn_id, message } => (
            turn_id.clone(),
            EventKind::Failed {
                message: message.clone(),
            },
        ),
        TurnEvent::CompactionStarted { turn_id } => (turn_id.clone(), EventKind::CompactionStarted),
        TurnEvent::CompactionCompleted { turn_id, summary } => (
            turn_id.clone(),
            EventKind::CompactionCompleted {
                summary: summary.clone(),
            },
        ),
        TurnEvent::SystemMessage { turn_id, message } => (
            turn_id.clone(),
            EventKind::SystemMessage {
                message: message.clone(),
            },
        ),
        TurnEvent::IntegrationEvent {
            turn_id,
            integration,
            event,
            data,
        } => (
            turn_id.clone(),
            EventKind::IntegrationEvent {
                integration: integration.clone(),
                event: event.clone(),
                data: data.as_ref().map(ToString::to_string),
            },
        ),
        TurnEvent::PlanUpdated { turn_id, plan } => (
            turn_id.clone(),
            EventKind::PlanUpdated {
                plan: plan.to_string(),
            },
        ),
        TurnEvent::StatusChanged {
            turn_id,
            status,
            message,
        } => (
            turn_id.clone(),
            EventKind::StatusChanged {
                status: status.clone(),
                message: message.clone(),
            },
        ),
        TurnEvent::Telemetry {
            turn_id,
            usage,
            context,
        } => (
            turn_id.clone(),
            EventKind::Telemetry {
                usage: usage.as_ref().map(ToString::to_string),
                context: context.as_ref().map(ToString::to_string),
            },
        ),
    };
    let request_id = event.request_id.as_ref().map(|id| match id {
        tau_proto::envelope::Id::Num(value) => value.to_string(),
        tau_proto::envelope::Id::Str(value) => value.clone(),
    });
    TypedEvent {
        event_id: event.event_id.clone(),
        session_id: event.session_id.clone(),
        sequence: event.sequence,
        occurred_at: event.occurred_at,
        request_id,
        turn_id,
        kind,
        raw: event.event.clone(),
    }
}

fn update_usage(sidebar: &mut SidebarState, usage: Option<&serde_json::Value>) {
    let Some(usage) = usage.and_then(serde_json::Value::as_object) else {
        return;
    };
    sidebar.input_tokens = usage
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64);
    sidebar.output_tokens = usage
        .get("output_tokens")
        .and_then(serde_json::Value::as_u64);
    sidebar.cached_tokens = usage
        .get("cached_tokens")
        .and_then(serde_json::Value::as_u64);
    sidebar.total_tokens = usage
        .get("total_tokens")
        .and_then(serde_json::Value::as_u64)
        .or(sidebar.total_tokens);
}

fn update_context(sidebar: &mut SidebarState, context: Option<&serde_json::Value>) {
    let Some(context) = context.and_then(serde_json::Value::as_object) else {
        return;
    };
    sidebar.context_tokens = context
        .get("tokens")
        .and_then(serde_json::Value::as_u64)
        .or(sidebar.context_tokens);
    sidebar.context_percent = context
        .get("percent")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u8::try_from(value).ok());
    sidebar.cwd = context
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| sidebar.cwd.clone());
    if let Some(roots) = context.get("roots").and_then(serde_json::Value::as_array) {
        sidebar.roots = roots
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_proto::prelude::{CompletionDelta, UsageSummary};

    #[test]
    fn card_metadata_and_preview_are_stable_for_long_cards() {
        let card = Card::Tool {
            name: "shell".into(),
            input: "x".into(),
            output: "abcdef".into(),
            expanded: false,
        };
        assert_eq!(
            card.metadata(),
            CardMetadata {
                kind: CardKind::Tool,
                interactive: true,
                expanded: false,
            }
        );
        assert_eq!(card.preview(3), "abc");
    }

    #[test]
    fn submit_records_history_and_streaming_disables_submit() {
        let mut state = ChatState::default();
        assert!(state.can_submit());
        assert!(state.reduce(ChatAction::Submit("hello".into())));
        assert_eq!(state.history().collect::<Vec<_>>(), vec!["hello"]);
        assert!(!state.can_submit());
        assert!(!state.reduce(ChatAction::Submit("second".into())));
    }

    #[test]
    fn reducer_streams_and_finishes() {
        let mut state = ChatState::default();
        assert!(state.reduce(ChatAction::Submit("hello".into())));
        let delta = CompletionDelta {
            request_id: tau_proto::envelope::Id::num(1),
            session_id: "s".into(),
            text: "hi".into(),
            usage: None,
        };
        assert!(state.reduce(ChatAction::Completion(CompletionEvent::Delta(delta))));
        let result = CompletionStreamResult {
            session_id: "s".into(),
            message_id: 1,
            text: "hi!".into(),
            usage: UsageSummary {
                input_tokens: 1,
                output_tokens: 2,
                total_tokens: 3,
            },
        };
        state.reduce(ChatAction::Completion(CompletionEvent::Complete(result)));
        assert_eq!(state.status, ChatStatus::Ready);
        assert_eq!(state.usage, Some(3));
    }

    #[test]
    fn safety_cards_are_actionable_and_raw_tool_details_survive() {
        let mut state = ChatState::default();
        state.cards.push(Card::Tool {
            name: "shell".into(),
            input: "{\"cmd\":\"pwd\"}".into(),
            output: "ok".into(),
            expanded: false,
        });
        assert!(state.reduce(ChatAction::ToggleTool(0)));
        assert!(
            matches!(&state.cards[0], Card::Tool { expanded: true, input, .. } if input.contains("pwd"))
        );
        state.cards.push(Card::Permission {
            request_id: "r1".into(),
            tool: "write".into(),
            description: "modify file".into(),
        });
        assert!(state.reduce(ChatAction::Permission(1, PermissionChoice::AllowOnce)));
        assert!(
            state
                .cards
                .iter()
                .any(|card| matches!(card, Card::Permission { .. }))
        );
        assert!(matches!(state.policy_states["r1"], PolicyState::Pending));
        assert!(state.reduce(ChatAction::PolicyAck {
            request_id: "r1".into(),
        }));
        assert!(
            !state
                .cards
                .iter()
                .any(|card| matches!(card, Card::Permission { .. }))
        );
    }

    #[test]
    fn typed_prompt_events_keep_correlation_ids_and_diff_rejection_is_terminal() {
        let mut state = ChatState::default();
        let event = |event| SequencedEvent {
            event_id: "event".into(),
            session_id: "session".into(),
            sequence: 1,
            occurred_at: 0,
            request_id: None,
            event,
        };

        assert!(state.reduce(ChatAction::SessionEvent(event(
            TurnEvent::PermissionRequested {
                turn_id: "turn".into(),
                request_id: "permission".into(),
                tool: "write".into(),
                description: "change file".into(),
            },
        ))));
        assert!(matches!(
            &state.cards[0],
            Card::Permission { request_id, .. } if request_id == "permission"
        ));

        assert!(
            state.reduce(ChatAction::SessionEvent(event(TurnEvent::QuestionAsked {
                turn_id: "turn".into(),
                question_id: "question".into(),
                question: "Continue?".into(),
            },)))
        );
        assert!(matches!(
            &state.cards[1],
            Card::Question { question_id, .. } if question_id == "question"
        ));

        state.cards.push(Card::Diff {
            request_id: "diff".into(),
            path: "file.rs".into(),
            patch: "@@".into(),
            approved: false,
        });
        assert!(state.reduce(ChatAction::ApproveDiff(2, false)));
        assert!(
            state
                .cards
                .iter()
                .any(|card| matches!(card, Card::Diff { .. }))
        );
        assert!(matches!(state.policy_states["diff"], PolicyState::Pending));
    }

    #[test]
    fn typed_lifecycle_and_operational_events_project_to_render_state() {
        let mut state = ChatState::default();
        let mut sequence = 1;
        let mut apply = |event| {
            let accepted = state.reduce(ChatAction::SessionEvent(SequencedEvent {
                event_id: format!("event-{sequence}"),
                session_id: "session".into(),
                sequence,
                occurred_at: 0,
                request_id: None,
                event,
            }));
            sequence += 1;
            assert!(accepted);
        };
        apply(TurnEvent::TurnStarted {
            turn_id: "turn".into(),
        });
        apply(TurnEvent::ReasoningDelta {
            turn_id: "turn".into(),
            text: "inspect".into(),
        });
        apply(TurnEvent::ToolStarted {
            turn_id: "turn".into(),
            tool_call_id: "call".into(),
            tool: "read".into(),
            input: Some(serde_json::json!({"path":"README"})),
        });
        apply(TurnEvent::ToolStatus {
            turn_id: "turn".into(),
            tool_call_id: "call".into(),
            status: ToolStatusValue::Running,
            metadata: None,
        });
        apply(TurnEvent::IntegrationEvent {
            turn_id: "turn".into(),
            integration: "lsp".into(),
            event: "ready".into(),
            data: None,
        });
        apply(TurnEvent::PlanUpdated {
            turn_id: "turn".into(),
            plan: serde_json::json!({"title":"M13", "current_step":"render", "airtight":true}),
        });
        apply(TurnEvent::Telemetry {
            turn_id: "turn".into(),
            usage: Some(serde_json::json!({"input_tokens": 3, "output_tokens": 2})),
            context: Some(serde_json::json!({"tokens": 5, "percent": 10})),
        });
        apply(TurnEvent::CompactionCompleted {
            turn_id: "turn".into(),
            summary: Some("kept decisions".into()),
        });
        assert_eq!(state.events.len(), 8);
        assert_eq!(state.sidebar.lsp.as_deref(), Some("ready"));
        assert_eq!(state.sidebar.plan.as_deref(), Some("M13"));
        assert_eq!(state.sidebar.current_step.as_deref(), Some("render"));
        assert_eq!(state.sidebar.context_percent, Some(10));
        assert!(state.cards.iter().any(|card| matches!(
            card,
            Card::Message {
                role: Role::System,
                ..
            }
        )));
    }
}
