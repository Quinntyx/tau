use crate::composer::Composer;
use crate::sessions::Navigator;
use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};
use tau_proto::prelude::SequencedEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Connection {
    #[default]
    Connected,
    Reconnecting,
    Disconnected,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionStage {
    #[default]
    Choose,
    AlwaysConfirm,
    Reject,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    AllowOnce,
    AllowAlways,
    AllowSession,
    DenyOnce,
    Reject,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Picker {
    #[default]
    None,
    Models,
    Agents,
    Commands,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolStatus {
    #[default]
    Running,
    Complete,
    Failed,
    Denied,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct Permission {
    pub tool: String,
    pub summary: String,
    pub choice: PermissionChoice,
    pub stage: PermissionStage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub prompt: String,
    pub answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffReply {
    pub accepted: Option<bool>,
}
#[derive(Debug, Clone)]
pub struct ToolCard {
    pub name: String,
    pub result: String,
    pub input: Value,
    pub status: ToolStatus,
    pub expanded: bool,
}
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub provider: String,
    pub favorite: bool,
}
#[derive(Debug, Clone)]
pub struct Hunk {
    pub header: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub lines: Vec<String>,
    pub accepted: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    /// Canonical composer contract; legacy fields below remain as a protocol
    /// projection for the existing transcript/reducer APIs.
    pub composer: Composer,
    /// Project root passed to the daemon-backed operations API.
    pub project_root: String,
    pub operations: crate::operations::OperationsState,
    pub operations_tab: OperationsTab,
    pub operations_focused: bool,
    pub operations_loading: bool,
    pub operations_error: Option<String>,
    pub operations_ack: Option<String>,
    pub connection: Connection,
    pub input: String,
    pub cursor: usize,
    pub selection: Option<usize>,
    pub clipboard: String,
    pub transcript: Vec<String>,
    /// Typed user entries used by the feed; `transcript` remains a legacy
    /// display projection for compatibility with the existing reducer tests.
    pub human_messages: Vec<String>,
    /// The display transcript is a projection; retain every typed event so
    /// replay and inspection never need to recover semantics from strings.
    pub raw_events: Vec<SequencedEvent>,
    pub assistant_index: Option<usize>,
    pub session_id: Option<String>,
    /// Explicitly selected active project.  There is intentionally no default
    /// or cwd-based fallback: turns must name the user's selection.
    pub project_id: Option<String>,
    pub turn_id: Option<String>,
    pub sequence: u64,
    pub model: String,
    pub agent: String,
    pub picker: Picker,
    pub picker_query: String,
    pub picker_index: usize,
    pub models: Vec<Model>,
    pub recent_models: Vec<String>,
    pub agents: Vec<String>,
    pub tools: Vec<ToolCard>,
    /// Correlates tool cards with protocol lifecycle events without changing
    /// the legacy card shape used by renderers.
    pub tool_call_ids: Vec<Option<String>>,
    pub permission: Option<Permission>,
    pub permission_request_id: Option<String>,
    pub question: Option<Question>,
    pub question_id: Option<String>,
    pub diff_reply: Option<DiffReply>,
    pub diff_request_id: Option<String>,
    pub diff_path: Option<String>,
    pub task_tier: u8,
    pub autonomous: bool,
    pub hunks: Vec<Hunk>,
    pub hunk_index: usize,
    pub undo: VecDeque<Vec<Option<bool>>>,
    pub redo: VecDeque<Vec<Option<bool>>>,
    pub cancelling: bool,
    pub replaying: bool,
    pub following: bool,
    pub expanded_feed: BTreeSet<u64>,
    pub server_index: usize,
    pub servers: Vec<String>,
    pub sessions: Navigator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperationsTab {
    #[default]
    Status,
    Git,
    Changes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferSnapshot {
    pub input: String,
    pub cursor: usize,
    pub selection: Option<usize>,
}

impl AppState {
    /// Apply the selected project's canonical root to daemon-backed operations.
    /// Switching projects (including repathing one) starts with a clean view.
    pub fn select_project(&mut self, id: impl Into<String>, root: impl Into<String>) {
        let id = id.into();
        self.project_id = Some(id.clone());
        self.project_root = root.into();
        self.operations
            .set_project_selection(id, self.project_root.clone());
        self.operations_loading = false;
        self.operations_error = None;
        self.operations_ack = None;
    }
    /// Build state with integration-owned identifiers; the composer keeps the
    /// values opaque while reducers pass the existing protocol fields through.
    pub fn with_ids(session_id: impl Into<String>, project_id: impl Into<String>) -> Self {
        let mut state = Self::default();
        let session_id = session_id.into();
        let project_id = project_id.into();
        state.session_id = Some(session_id.clone());
        state.project_id = Some(project_id.clone());
        state.composer = Composer::new(session_id, project_id);
        state.composer.set_model(state.model.clone());
        state.composer.set_agent(state.agent.clone());
        state
    }

    pub fn buffer_snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            input: self.input.clone(),
            cursor: self.cursor,
            selection: self.selection,
        }
    }

    pub fn restore_buffer(&mut self, snapshot: BufferSnapshot) {
        self.composer.set_text(snapshot.input);
        self.composer.set_selection(
            snapshot.selection.unwrap_or(snapshot.cursor),
            snapshot.cursor,
        );
        self.input = self.composer.text().to_owned();
        self.cursor = self.composer.selection().cursor;
        self.selection = (self.composer.selection().anchor != self.cursor)
            .then_some(self.composer.selection().anchor);
    }
}
impl Default for AppState {
    fn default() -> Self {
        let mut state = Self {
            composer: Composer::new("", ""),
            project_root: String::new(),
            operations: crate::operations::OperationsState::default(),
            operations_tab: OperationsTab::Status,
            operations_focused: false,
            operations_loading: false,
            operations_error: None,
            operations_ack: None,
            connection: Connection::Connected,
            input: String::new(),
            cursor: 0,
            selection: None,
            clipboard: String::new(),
            transcript: vec!["Welcome to tau. Select a model and type a prompt.".into()],
            human_messages: vec![],
            raw_events: vec![],
            assistant_index: None,
            session_id: None,
            project_id: None,
            turn_id: None,
            sequence: 0,
            model: "openai/gpt-4o".into(),
            agent: "default".into(),
            picker: Picker::None,
            picker_query: String::new(),
            picker_index: 0,
            models: vec![
                Model {
                    id: "openai/gpt-4o".into(),
                    provider: "OpenAI".into(),
                    favorite: true,
                },
                Model {
                    id: "anthropic/claude-sonnet".into(),
                    provider: "Anthropic".into(),
                    favorite: false,
                },
            ],
            recent_models: vec![],
            agents: vec!["default".into(), "explore".into(), "general".into()],
            tools: vec![],
            tool_call_ids: vec![],
            permission: None,
            permission_request_id: None,
            question: None,
            question_id: None,
            diff_reply: None,
            diff_request_id: None,
            diff_path: None,
            task_tier: 1,
            autonomous: false,
            hunks: vec![],
            hunk_index: 0,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            cancelling: false,
            replaying: false,
            following: true,
            expanded_feed: BTreeSet::new(),
            server_index: 0,
            servers: vec!["local".into()],
            sessions: Navigator::default(),
        };
        state.composer.set_model(state.model.clone());
        state.composer.set_agent(state.agent.clone());
        state
    }
}

#[cfg(test)]
mod project_tests {
    use super::*;

    #[test]
    fn project_selection_uses_canonical_root_and_clears_operations() {
        let mut state = AppState::default();
        state.operations.branch = "main".into();
        state.select_project("two", "/canonical/two");
        assert_eq!(state.project_id.as_deref(), Some("two"));
        assert_eq!(state.operations.project, "/canonical/two");
        assert!(state.operations.branch.is_empty());
    }
}
