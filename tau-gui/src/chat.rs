//! Pure, typed chat model used by the GPUI view and scripted fixtures.
use std::collections::HashSet;
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
        session_id: String,
        turn_id: String,
        path: String,
        patch: String,
        approved: bool,
    },
    Permission {
        session_id: String,
        turn_id: String,
        request_id: String,
        tool: String,
        description: String,
    },
    Question {
        session_id: String,
        turn_id: String,
        question_id: String,
        question: String,
        answer: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatState {
    pub cards: Vec<Card>,
    pub session_id: Option<String>,
    pub usage: Option<u64>,
    pub status: ChatStatus,
    pub active_assistant: Option<usize>,
    /// Last accepted durable event, used to make reconnect/replay idempotent.
    pub last_sequence: u64,
    active_turn_id: Option<String>,
    seen_event_ids: HashSet<String>,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            cards: Vec::new(),
            session_id: None,
            usage: None,
            status: ChatStatus::Ready,
            active_assistant: None,
            last_sequence: 0,
            active_turn_id: None,
            seen_event_ids: HashSet::new(),
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
                if self
                    .session_id
                    .as_deref()
                    .is_some_and(|id| id != delta.session_id)
                {
                    return false;
                }
                if self.session_id.is_none() {
                    self.session_id = Some(delta.session_id.clone());
                }
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
                    if approved {
                        *value = true;
                    } else {
                        // Rejection is a terminal review decision. Remove the
                        // pending card so a rejected mutation cannot be
                        // accidentally submitted a second time.
                        self.cards.remove(index);
                    }
                    true
                }
                _ => false,
            },
            ChatAction::Permission(index, choice) => match self.cards.get(index) {
                Some(Card::Permission { .. }) => {
                    if matches!(
                        choice,
                        PermissionChoice::AllowOnce
                            | PermissionChoice::AllowAlways
                            | PermissionChoice::Reject
                    ) {
                        self.cards.remove(index);
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
            ChatAction::Submit(_) => false,
        }
    }

    fn apply_session_event(&mut self, event: SequencedEvent) -> bool {
        if self
            .session_id
            .as_deref()
            .is_some_and(|id| id != event.session_id)
        {
            return false;
        }
        if event.sequence <= self.last_sequence
            || !self.seen_event_ids.insert(event.event_id.clone())
        {
            return false;
        }
        let session_id = event.session_id.clone();
        let turn_id = event_turn_id(&event.event).to_owned();
        if let Some(active) = &self.active_turn_id {
            if active != &turn_id {
                return false;
            }
        }
        self.session_id = Some(session_id.clone());
        self.last_sequence = event.sequence;
        self.active_turn_id = Some(turn_id.clone());
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
                self.cards.push(Card::Tool {
                    name: "tool".into(),
                    input: String::new(),
                    output: output.inline,
                    expanded: false,
                });
                true
            }
            TurnEvent::PermissionRequested {
                request_id,
                tool,
                description,
                ..
            } => {
                self.cards.push(Card::Permission {
                    session_id,
                    turn_id: turn_id.clone(),
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
                    session_id,
                    turn_id: turn_id.clone(),
                    question_id,
                    question,
                    answer: None,
                });
                true
            }
            TurnEvent::DiffRequested { path, diff, .. } => {
                self.cards.push(Card::Diff {
                    session_id,
                    turn_id,
                    path,
                    patch: diff,
                    approved: false,
                });
                true
            }
            TurnEvent::TurnCompleted { .. } => {
                self.active_assistant = None;
                self.active_turn_id = None;
                self.status = ChatStatus::Ready;
                true
            }
            TurnEvent::TurnCancelled { .. } => {
                self.active_assistant = None;
                self.active_turn_id = None;
                self.status = ChatStatus::Ready;
                true
            }
            TurnEvent::TurnFailed { message, .. } => {
                self.active_assistant = None;
                self.active_turn_id = None;
                self.status = ChatStatus::Failed(message);
                true
            }
            TurnEvent::TurnStarted { .. } | TurnEvent::ArtifactCreated { .. } => false,
        }
    }

    fn finish(&mut self, result: CompletionStreamResult) -> bool {
        if self
            .session_id
            .as_deref()
            .is_some_and(|id| id != result.session_id)
        {
            return false;
        }
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

fn event_turn_id(event: &TurnEvent) -> &str {
    match event {
        TurnEvent::TurnStarted { turn_id }
        | TurnEvent::TextDelta { turn_id, .. }
        | TurnEvent::ToolOutput { turn_id, .. }
        | TurnEvent::PermissionRequested { turn_id, .. }
        | TurnEvent::QuestionAsked { turn_id, .. }
        | TurnEvent::DiffRequested { turn_id, .. }
        | TurnEvent::ArtifactCreated { turn_id, .. }
        | TurnEvent::TurnCompleted { turn_id, .. }
        | TurnEvent::TurnCancelled { turn_id }
        | TurnEvent::TurnFailed { turn_id, .. } => turn_id,
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
            session_id: "session".into(),
            turn_id: "turn".into(),
            request_id: "r1".into(),
            tool: "write".into(),
            description: "modify file".into(),
        });
        assert!(state.reduce(ChatAction::Permission(1, PermissionChoice::AllowOnce)));
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
        let mut sequence = 0;
        let mut event = |event| {
            sequence += 1;
            SequencedEvent {
                event_id: format!("event-{sequence}"),
                session_id: "session".into(),
                sequence,
                occurred_at: 0,
                request_id: None,
                event,
            }
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
            session_id: "session".into(),
            turn_id: "turn".into(),
            path: "file.rs".into(),
            patch: "@@".into(),
            approved: false,
        });
        assert!(state.reduce(ChatAction::ApproveDiff(2, false)));
        assert!(
            !state
                .cards
                .iter()
                .any(|card| matches!(card, Card::Diff { .. }))
        );
    }

    #[test]
    fn typed_response_actions_update_each_pending_card() {
        let mut state = ChatState {
            cards: vec![
                Card::Permission {
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    request_id: "permission-1".into(),
                    tool: "write".into(),
                    description: "edit file".into(),
                },
                Card::Question {
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    question_id: "question-1".into(),
                    question: "Continue?".into(),
                    answer: None,
                },
                Card::Diff {
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    path: "src/lib.rs".into(),
                    patch: "@@ -1 +1 @@".into(),
                    approved: false,
                },
            ],
            ..ChatState::default()
        };
        assert!(state.reduce(ChatAction::Permission(0, PermissionChoice::AllowAlways)));
        assert!(state.reduce(ChatAction::AnswerQuestion(0, "yes".into())));
        assert!(state.reduce(ChatAction::ApproveDiff(1, true)));
        assert!(matches!(state.cards[0], Card::Question { answer: Some(ref a), .. } if a == "yes"));
        assert!(matches!(state.cards[1], Card::Diff { approved: true, .. }));
    }

    #[test]
    fn typed_event_actions_are_reachable_without_view_specific_reducers() {
        let mut state = ChatState::default();
        let mut sequence = 0;
        let mut event = |event| {
            sequence += 1;
            SequencedEvent {
                event_id: format!("event-{sequence}"),
                session_id: "session".into(),
                sequence,
                occurred_at: 0,
                request_id: None,
                event,
            }
        };
        assert!(
            state.reduce(ChatAction::SessionEvent(event(TurnEvent::DiffRequested {
                turn_id: "turn".into(),
                request_id: "diff-1".into(),
                path: "README.md".into(),
                diff: "+ docs".into(),
            })))
        );
        assert!(matches!(&state.cards[0], Card::Diff { path, .. } if path == "README.md"));
    }

    #[test]
    fn stale_duplicate_and_wrong_session_events_are_ignored() {
        let mut state = ChatState::default();
        let make = |session: &str, sequence: u64, event_id: &str| SequencedEvent {
            event_id: event_id.into(),
            session_id: session.into(),
            sequence,
            occurred_at: 0,
            request_id: None,
            event: TurnEvent::TextDelta {
                turn_id: "t".into(),
                text: "ok".into(),
            },
        };
        assert!(state.reduce(ChatAction::SessionEvent(make("s", 1, "x"))));
        assert!(!state.reduce(ChatAction::SessionEvent(make("other", 2, "other"))));
        assert!(!state.reduce(ChatAction::SessionEvent(make("s", 1, "x"))));
        assert!(!state.reduce(ChatAction::SessionEvent(make("s", 2, "y"))));
        assert_eq!(state.last_sequence, 1);
    }
}
