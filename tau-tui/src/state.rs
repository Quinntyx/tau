use serde_json::Value;
use std::collections::VecDeque;

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
    pub connection: Connection,
    pub input: String,
    pub cursor: usize,
    pub selection: Option<usize>,
    pub clipboard: String,
    pub transcript: Vec<String>,
    pub session_id: Option<String>,
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
    pub permission: Option<Permission>,
    pub question: Option<Question>,
    pub diff_reply: Option<DiffReply>,
    pub task_tier: u8,
    pub autonomous: bool,
    pub hunks: Vec<Hunk>,
    pub hunk_index: usize,
    pub undo: VecDeque<Vec<Option<bool>>>,
    pub redo: VecDeque<Vec<Option<bool>>>,
    pub cancelling: bool,
    pub replaying: bool,
    pub server_index: usize,
    pub servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferSnapshot {
    pub input: String,
    pub cursor: usize,
    pub selection: Option<usize>,
}

impl AppState {
    pub fn buffer_snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            input: self.input.clone(),
            cursor: self.cursor,
            selection: self.selection,
        }
    }

    pub fn restore_buffer(&mut self, snapshot: BufferSnapshot) {
        self.input = snapshot.input;
        self.cursor = snapshot.cursor.min(self.input.len());
        self.selection = snapshot
            .selection
            .map(|cursor| cursor.min(self.input.len()));
    }
}
impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: Connection::Connected,
            input: String::new(),
            cursor: 0,
            selection: None,
            clipboard: String::new(),
            transcript: vec!["Welcome to tau. Select a model and type a prompt.".into()],
            session_id: None,
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
            permission: None,
            question: None,
            diff_reply: None,
            task_tier: 1,
            autonomous: false,
            hunks: vec![],
            hunk_index: 0,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            cancelling: false,
            replaying: false,
            server_index: 0,
            servers: vec!["local".into()],
        }
    }
}
