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
    pub lines: Vec<String>,
    pub accepted: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub connection: Connection,
    pub input: String,
    pub cursor: usize,
    pub transcript: Vec<String>,
    pub session_id: Option<String>,
    pub model: String,
    pub agent: String,
    pub picker: Picker,
    pub picker_query: String,
    pub picker_index: usize,
    pub models: Vec<Model>,
    pub agents: Vec<String>,
    pub tools: Vec<ToolCard>,
    pub permission: Option<Permission>,
    pub task_tier: u8,
    pub autonomous: bool,
    pub hunks: Vec<Hunk>,
    pub hunk_index: usize,
    pub undo: VecDeque<Vec<Option<bool>>>,
    pub redo: VecDeque<Vec<Option<bool>>>,
    pub cancelling: bool,
}
impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: Connection::Connected,
            input: String::new(),
            cursor: 0,
            transcript: vec!["Welcome to tau. Select a model and type a prompt.".into()],
            session_id: None,
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
            agents: vec!["default".into(), "explore".into(), "general".into()],
            tools: vec![],
            permission: None,
            task_tier: 1,
            autonomous: false,
            hunks: vec![],
            hunk_index: 0,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            cancelling: false,
        }
    }
}
