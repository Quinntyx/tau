//! Pure, typed chat model used by the GPUI view and scripted fixtures.
use std::collections::BTreeMap;
use tau_client::CompletionEvent;
use tau_proto::prelude::{CompletionStreamResult, SequencedEvent, TurnEvent};

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
    Plan {
        request_id: String,
        revision: u64,
        accepted: Option<bool>,
    },
    Airtight {
        request_id: String,
        step: usize,
        granted: Option<bool>,
    },
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

/// Lifecycle of a policy request.  The card remains visible while a decision
/// is in flight; the daemon is the authority for transitioning to Ack/Error.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatState {
    pub cards: Vec<Card>,
    pub session_id: Option<String>,
    pub usage: Option<u64>,
    pub status: ChatStatus,
    pub active_assistant: Option<usize>,
    /// Correlation id -> lifecycle, retained independently of rendering so a
    /// late daemon acknowledgement cannot accidentally remove local state.
    pub policy_states: BTreeMap<String, PolicyState>,
    pub diff_decisions: BTreeMap<String, Vec<DiffDecision>>,
    /// Raw tool payload/metadata for inspect and copy affordances.
    pub raw_tool_metadata: BTreeMap<usize, String>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            cards: Vec::new(),
            session_id: None,
            usage: None,
            status: ChatStatus::Ready,
            active_assistant: None,
            policy_states: BTreeMap::new(),
            diff_decisions: BTreeMap::new(),
            raw_tool_metadata: BTreeMap::new(),
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
    /// Optimistically record a decision without deleting its card.
    PolicyDecision {
        request_id: String,
        choice: String,
    },
    PolicyAck {
        request_id: String,
    },
    PolicyError {
        request_id: String,
        message: String,
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
    RetryPolicy {
        request_id: String,
    },
}

impl ChatState {
    pub fn reduce(&mut self, action: ChatAction) -> bool {
        match action {
            ChatAction::SessionEvent(event) => self.apply_session_event(event),
            ChatAction::Submit(text)
                if !text.trim().is_empty() && self.active_assistant.is_none() =>
            {
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
                true
            }
            ChatAction::Completion(CompletionEvent::Delta(delta)) => {
                let Some(index) = self.active_assistant else {
                    return false;
                };
                if delta.text.starts_with("\n[tool ") {
                    self.cards[index] = Card::Tool {
                        name: delta.text.trim().to_owned(),
                        input: String::new(),
                        output: String::new(),
                        expanded: false,
                    };
                    self.cards.push(Card::Message {
                        role: Role::Assistant,
                        text: String::new(),
                    });
                    self.active_assistant = Some(self.cards.len() - 1);
                } else if let Card::Message { text, .. } = &mut self.cards[index] {
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
                self.status = ChatStatus::Failed(error);
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
                    approved: value, ..
                }) => {
                    // Keep the card until daemon acknowledgement; this also
                    // makes rejection feedback renderable by the view.
                    *value = approved;
                    true
                }
                _ => false,
            },
            ChatAction::Permission(index, choice) => match self.cards.get(index) {
                Some(Card::Permission { request_id, .. }) => {
                    if matches!(
                        choice,
                        PermissionChoice::AllowOnce
                            | PermissionChoice::AllowAlways
                            | PermissionChoice::Reject
                    ) {
                        self.policy_states
                            .insert(request_id.clone(), PolicyState::Pending);
                    }
                    true
                }
                _ => false,
            },
            ChatAction::AnswerQuestion(index, answer) => match self.cards.get_mut(index) {
                Some(Card::Question { answer: value, .. }) => {
                    *value = Some(answer);
                    true
                }
                _ => false,
            },
            ChatAction::PolicyDecision { request_id, .. } => {
                self.policy_states.insert(request_id, PolicyState::Pending);
                true
            }
            ChatAction::PolicyAck { request_id } => {
                self.policy_states
                    .insert(request_id.clone(), PolicyState::Ack);
                self.cards.retain(|card| match card {
                    Card::Permission { request_id: id, .. }
                    | Card::Diff { request_id: id, .. }
                    | Card::Plan { request_id: id, .. }
                    | Card::Airtight { request_id: id, .. } => id != &request_id,
                    Card::Question { question_id, .. } => question_id != &request_id,
                    _ => true,
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
        match event.event {
            TurnEvent::TextDelta { text, .. } => {
                let Some(index) = self.active_assistant else {
                    return false;
                };
                if let Card::Message { text: output, .. } = &mut self.cards[index] {
                    output.push_str(&text);
                    return true;
                }
                false
            }
            TurnEvent::ToolOutput { output, .. } => {
                let raw = output.inline.clone();
                self.cards.push(Card::Tool {
                    name: "tool".into(),
                    input: String::new(),
                    output: output.inline,
                    expanded: false,
                });
                self.raw_tool_metadata.insert(self.cards.len() - 1, raw);
                true
            }
            TurnEvent::PermissionRequested {
                request_id,
                tool,
                description,
                ..
            } => {
                self.policy_states
                    .insert(request_id.clone(), PolicyState::Pending);
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
                self.policy_states
                    .insert(question_id.clone(), PolicyState::Pending);
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
                self.policy_states
                    .insert(request_id.clone(), PolicyState::Pending);
                self.cards.push(Card::Diff {
                    request_id: request_id.clone(),
                    path,
                    patch: diff,
                    approved: false,
                });
                true
            }
            TurnEvent::TurnCompleted { .. } => {
                self.active_assistant = None;
                self.status = ChatStatus::Ready;
                true
            }
            TurnEvent::TurnCancelled { .. } => {
                self.active_assistant = None;
                self.status = ChatStatus::Ready;
                true
            }
            TurnEvent::TurnFailed { message, .. } => {
                self.active_assistant = None;
                self.status = ChatStatus::Failed(message);
                true
            }
            TurnEvent::TurnStarted { .. } | TurnEvent::ArtifactCreated { .. } => false,
        }
    }

    fn finish(&mut self, result: CompletionStreamResult) -> bool {
        let Some(index) = self.active_assistant.take() else {
            return false;
        };
        if let Card::Message { text, .. } = &mut self.cards[index] {
            *text = result.text;
        }
        self.session_id = Some(result.session_id);
        self.usage = Some(result.usage.total_tokens);
        self.status = ChatStatus::Ready;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_proto::prelude::{CompletionDelta, UsageSummary};
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
    }

    #[test]
    fn typed_prompt_events_keep_correlation_ids_and_diff_rejection_is_retained() {
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
            request_id: "d1".into(),
            path: "file.rs".into(),
            patch: "@@".into(),
            approved: false,
        });
        assert!(state.reduce(ChatAction::ApproveDiff(2, false)));
        assert!(state.cards.iter().any(|card| matches!(
            card,
            Card::Diff {
                approved: false,
                ..
            }
        )));
    }

    #[test]
    fn policy_decisions_stay_pending_until_ack_and_rejection_is_visible() {
        let mut state = ChatState::default();
        state.cards.push(Card::Permission {
            request_id: "p1".into(),
            tool: "shell".into(),
            description: "run".into(),
        });
        assert!(state.reduce(ChatAction::Permission(0, PermissionChoice::Reject)));
        assert!(matches!(state.policy_states["p1"], PolicyState::Pending));
        assert!(matches!(state.cards[0], Card::Permission { .. }));
        state.reduce(ChatAction::PolicyError {
            request_id: "p1".into(),
            message: "denied".into(),
        });
        assert!(matches!(state.policy_states["p1"], PolicyState::Error(ref e) if e == "denied"));
        state.reduce(ChatAction::RetryPolicy {
            request_id: "p1".into(),
        });
        assert!(matches!(state.policy_states["p1"], PolicyState::Pending));
        state.reduce(ChatAction::PolicyAck {
            request_id: "p1".into(),
        });
        assert!(matches!(state.policy_states["p1"], PolicyState::Ack));
    }

    #[test]
    fn diff_tracks_hunks_and_whole_file_without_losing_local_decisions() {
        let mut state = ChatState::default();
        state.reduce(ChatAction::DiffHunk {
            request_id: "d1".into(),
            hunk: 2,
            approved: false,
        });
        state.reduce(ChatAction::DiffWholeFile {
            request_id: "d1".into(),
            approved: true,
        });
        assert_eq!(
            state.diff_decisions["d1"],
            vec![
                DiffDecision::Hunk {
                    hunk: 2,
                    approved: false
                },
                DiffDecision::WholeFile { approved: true },
            ]
        );
        assert!(matches!(state.policy_states["d1"], PolicyState::Pending));
    }
}
