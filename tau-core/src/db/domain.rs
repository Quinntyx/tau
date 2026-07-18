//! Domain types and path helpers for the persistence layer.

use std::path::{Path, PathBuf};

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
    pub project_id: Option<String>,
    pub cwd: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

pub(crate) fn canonical_project_root(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    let canonical = if path.exists() {
        if !path.is_dir() {
            anyhow::bail!("project root is not a directory: {}", path.display());
        }
        std::fs::canonicalize(path)
    } else {
        let parent = path.parent().context("project root has no parent")?;
        if !parent.exists() || !parent.is_dir() || path.file_name().is_none() {
            anyhow::bail!("project root must exist or be one missing final directory");
        }
        // Deliberately use `create_dir`, not `create_dir_all`: registration may
        // create exactly one missing final directory and never manufacture a
        // missing ancestor by accident.
        std::fs::create_dir(path)
            .with_context(|| format!("creating project root {}", path.display()))?;
        std::fs::canonicalize(path)
    }?;
    Ok(canonical.to_string_lossy().into_owned())
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
pub struct InteractivePrompt {
    pub id: String,
    pub session_id: String,
    pub turn_id: String,
    pub kind: String,
    pub payload_json: String,
    pub initiating_client_id: String,
    pub owner_client_id: String,
    pub status: String,
    pub idempotency_key: Option<String>,
    pub reply_idempotency_key: Option<String>,
    pub takeover_idempotency_key: Option<String>,
    pub reply_json: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyDecisionRecord {
    pub id: String,
    pub session_id: Option<String>,
    pub scope: String,
    pub actor: String,
    pub pattern: String,
    pub decision_json: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextEpochRecord {
    pub id: i64,
    pub session_id: String,
    pub epoch: i64,
    pub summary: String,
    pub plan_context: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub trigger: String,
    pub retry_marker: bool,
    pub created_at: i64,
    pub parent_epoch: Option<i64>,
    pub estimated_tokens: Option<i64>,
    pub terminal_status: Option<String>,
    pub is_baseline: bool,
    pub system_message: Option<String>,
}

impl ContextEpochRecord {
    pub fn new(
        session_id: impl Into<String>,
        epoch: i64,
        summary: impl Into<String>,
        trigger: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            session_id: session_id.into(),
            epoch,
            summary: summary.into(),
            plan_context: None,
            provider: None,
            model: None,
            input_tokens: None,
            trigger: trigger.into(),
            retry_marker: false,
            created_at: now_ms(),
            parent_epoch: (epoch > 0).then_some(epoch - 1),
            estimated_tokens: None,
            terminal_status: None,
            is_baseline: epoch == 0,
            system_message: None,
        }
    }
}
