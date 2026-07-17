//! Domain types and path helpers for the persistence layer.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Default database path: `~/.local/share/tau/tau.db` (XDG data dir).
/// Creates the parent directory if missing.
pub fn default_db_path() -> Result<PathBuf> {
    let base = dirs::data_dir().context("could not determine data directory")?;
    let dir = base.join("tau");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join("tau.db"))
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub seq: i64,
    pub created_at: i64,
    pub blocks: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "block_type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        call_id: String,
        name: String,
        args_json: String,
    },
    ToolResult {
        call_id: String,
        result_json: String,
        is_error: bool,
    },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub id: i64,
    pub session_id: String,
    pub message_id: Option<i64>,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaRecord {
    pub id: String,
    pub session_id: String,
    pub question: String,
    pub answer: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanRevision {
    pub plan_id: String,
    pub revision: i64,
    pub parent_revision: Option<i64>,
    pub payload: String,
    pub airtight: bool,
    pub revoked: bool,
    pub actor: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SteeringMode {
    Normal,
    Super,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SteeringRun {
    pub id: String,
    pub session_id: String,
    pub mode: SteeringMode,
    pub model: String,
    pub vision: String,
    pub authority: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl SteeringRun {
    /// Normal runs steer planning only; super runs may answer every soft prompt.
    pub fn may_answer(&self, prompt: &str) -> bool {
        self.status == "running" && (self.mode == SteeringMode::Super || prompt == "plan")
    }
    pub fn is_indefinite(&self) -> bool {
        self.status == "running"
    }
}
