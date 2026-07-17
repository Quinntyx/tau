//! M6 model-turn orchestration boundary.
//!
//! The first runner delegates a turn to the normalized provider stream. Tool
//! execution and hook-driven multi-turn loops can be added behind this stable
//! boundary without changing clients.

use rig_core::completion::{CompletionError, CompletionRequest, ToolDefinition};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::provider::{Provider, TauStream};
use crate::tools::ToolRegistry;

/// Stable identities attached to every host lifecycle event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunIdentity {
    pub run_id: String,
    pub turn: u32,
    pub internal_call_id: Option<String>,
    pub provider_tool_id: Option<String>,
    pub provider_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LifecycleEvent {
    RunStarted {
        identity: RunIdentity,
    },
    TurnStarted {
        identity: RunIdentity,
    },
    ToolCallStarted {
        identity: RunIdentity,
        name: String,
        arguments: serde_json::Value,
    },
    ToolCallFinished {
        identity: RunIdentity,
        result: serde_json::Value,
    },
    ToolCallFailed {
        identity: RunIdentity,
        error: String,
    },
    TurnFinished {
        identity: RunIdentity,
    },
    RunFinished {
        identity: RunIdentity,
    },
    RunCancelled {
        identity: RunIdentity,
    },
}

/// Policy seams deliberately contain no policy: server/UI layers own decisions.
pub trait PermissionHook: Send + Sync {
    fn check(&self, identity: &RunIdentity, tool: &str, arguments: &serde_json::Value) -> bool;
}
pub trait PlanHook: Send + Sync {
    fn turn_started(&self, identity: &RunIdentity);
}
pub trait SnapshotHook: Send + Sync {
    fn snapshot(&self, identity: &RunIdentity, tool: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TaskTier {
    Unlimited,
    Max,
    XHigh,
    High,
    #[default]
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskLimits {
    pub max_depth: Option<usize>,
    pub max_tasks: Option<usize>,
}

impl TaskTier {
    pub const fn limits(self) -> TaskLimits {
        match self {
            Self::Unlimited => TaskLimits {
                max_depth: None,
                max_tasks: None,
            },
            Self::Max => TaskLimits {
                max_depth: Some(3),
                max_tasks: Some(8),
            },
            Self::XHigh => TaskLimits {
                max_depth: Some(2),
                max_tasks: Some(8),
            },
            Self::High => TaskLimits {
                max_depth: Some(2),
                max_tasks: Some(4),
            },
            Self::Medium => TaskLimits {
                max_depth: Some(1),
                max_tasks: Some(4),
            },
            Self::Low => TaskLimits {
                max_depth: Some(1),
                max_tasks: Some(1),
            },
        }
    }
}
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionItem {
    pub id: String,
    pub question: String,
    pub options: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub id: String,
    pub answer: String,
}

#[derive(Clone)]
pub struct QuestionTool {
    pending: Arc<std::sync::Mutex<Vec<QuestionItem>>>,
}
impl Default for QuestionTool {
    fn default() -> Self {
        Self {
            pending: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}
impl QuestionTool {
    pub fn ask(&self, item: QuestionItem) {
        self.pending.lock().unwrap().push(item);
    }
    pub fn pending(&self) -> Vec<QuestionItem> {
        self.pending.lock().unwrap().clone()
    }
    pub fn answer(&self, answer: QuestionAnswer) -> bool {
        let mut pending = self.pending.lock().unwrap();
        if let Some(index) = pending.iter().position(|item| item.id == answer.id) {
            pending.remove(index);
            true
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub struct AgentRunner {
    provider: Provider,
    tools: ToolRegistry,
}

impl AgentRunner {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            tools: ToolRegistry::with_builtins().unwrap_or_default(),
        }
    }

    pub fn with_tools(mut self, tools: ToolRegistry) -> Self {
        self.tools = tools;
        self
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .descriptors()
            .into_iter()
            .map(|descriptor| {
                let parameters = crate::tools::schema_for(&descriptor.name);
                ToolDefinition {
                    name: descriptor.name,
                    description: descriptor.description,
                    parameters,
                }
            })
            .collect()
    }

    pub async fn stream(&self, request: CompletionRequest) -> Result<TauStream, CompletionError> {
        self.provider.stream(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::schema_for;

    #[test]
    fn task_tiers_match_contract() {
        assert_eq!(
            TaskTier::Unlimited.limits(),
            TaskLimits {
                max_depth: None,
                max_tasks: None
            }
        );
        assert_eq!(
            TaskTier::Max.limits(),
            TaskLimits {
                max_depth: Some(3),
                max_tasks: Some(8)
            }
        );
        assert_eq!(
            TaskTier::XHigh.limits(),
            TaskLimits {
                max_depth: Some(2),
                max_tasks: Some(8)
            }
        );
        assert_eq!(
            TaskTier::High.limits(),
            TaskLimits {
                max_depth: Some(2),
                max_tasks: Some(4)
            }
        );
        assert_eq!(
            TaskTier::Medium.limits(),
            TaskLimits {
                max_depth: Some(1),
                max_tasks: Some(4)
            }
        );
        assert_eq!(
            TaskTier::Low.limits(),
            TaskLimits {
                max_depth: Some(1),
                max_tasks: Some(1)
            }
        );
    }

    #[test]
    fn cancellation_is_shared_and_question_answers_remove_one_item() {
        let token = CancellationToken::default();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
        let questions = QuestionTool::default();
        questions.ask(QuestionItem {
            id: "q1".into(),
            question: "continue?".into(),
            options: vec![],
        });
        assert!(questions.answer(QuestionAnswer {
            id: "q1".into(),
            answer: "yes".into()
        }));
        assert!(questions.pending().is_empty());
    }

    #[test]
    fn schemas_are_strict_and_require_primary_arguments() {
        let schema = schema_for("read");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "file_path");
        assert_eq!(schema["additionalProperties"], false);
    }
}
