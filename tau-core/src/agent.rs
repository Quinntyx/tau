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

use crate::provider::{Provider, TauDelta, TauStream};
use crate::tools::ToolRegistry;
use crate::{
    permissions::{PermissionBroker, PermissionEngine, PermissionReply, authorize},
    plan::{Plan, allows_tool},
};
use futures::StreamExt;
use rig_core::{
    OneOrMany,
    completion::{AssistantContent, Message},
};
use uuid::Uuid;

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

type PendingQuestion = (QuestionItem, Option<String>);

#[derive(Clone)]
pub struct QuestionTool {
    pending: Arc<std::sync::Mutex<Vec<QuestionItem>>>,
}

/// Async question coordination used by the question tool and clients. Answers
/// are correlated by id and remain pending for the lifetime of the process.
#[derive(Clone, Default)]
pub struct QuestionBroker {
    pending: Arc<tokio::sync::Mutex<std::collections::BTreeMap<String, PendingQuestion>>>,
    changed: Arc<tokio::sync::Notify>,
}
impl QuestionBroker {
    pub async fn ask(&self, question: impl Into<String>, options: Vec<String>) -> QuestionItem {
        let item = QuestionItem {
            id: Uuid::new_v4().to_string(),
            question: question.into(),
            options,
        };
        self.pending
            .lock()
            .await
            .insert(item.id.clone(), (item.clone(), None));
        item
    }
    pub async fn answer(&self, answer: QuestionAnswer) -> bool {
        let mut pending = self.pending.lock().await;
        let Some((_, value)) = pending.get_mut(&answer.id) else {
            return false;
        };
        *value = Some(answer.answer);
        self.changed.notify_one();
        true
    }
    pub async fn wait(&self, id: &str) -> Option<String> {
        loop {
            let mut pending = self.pending.lock().await;
            if let Some((_, Some(value))) = pending.get(id) {
                let value = value.clone();
                pending.remove(id);
                return Some(value);
            }
            if !pending.contains_key(id) {
                return None;
            }
            drop(pending);
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }
    pub async fn pending(&self) -> Vec<QuestionItem> {
        self.pending
            .lock()
            .await
            .values()
            .map(|(q, _)| q.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    DepthExceeded,
    TaskLimitExceeded,
}

/// Per-run recursive task budget. A child gets its own depth but shares the
/// task counter, preventing fan-out from bypassing the selected tier.
#[derive(Clone)]
pub struct TaskBudget {
    limits: TaskLimits,
    depth: usize,
    tasks: Arc<std::sync::atomic::AtomicUsize>,
}
impl TaskBudget {
    pub fn new(tier: TaskTier) -> Self {
        Self {
            limits: tier.limits(),
            depth: 0,
            tasks: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }
    pub fn child(&self) -> Result<Self, TaskError> {
        if self.limits.max_depth.is_some_and(|max| self.depth >= max) {
            return Err(TaskError::DepthExceeded);
        }
        if let Some(max) = self.limits.max_tasks {
            if self.tasks.fetch_add(1, Ordering::AcqRel) >= max {
                self.tasks.fetch_sub(1, Ordering::AcqRel);
                return Err(TaskError::TaskLimitExceeded);
            }
        }
        Ok(Self {
            limits: self.limits,
            depth: self.depth + 1,
            tasks: self.tasks.clone(),
        })
    }
    pub fn depth(&self) -> usize {
        self.depth
    }
}

#[derive(Clone)]
pub struct RunnerPolicy {
    pub permissions: Arc<tokio::sync::Mutex<PermissionEngine>>,
    pub approvals: PermissionBroker,
    pub plan: Option<Arc<tokio::sync::Mutex<Plan>>>,
    pub autonomous: bool,
}
impl Default for RunnerPolicy {
    fn default() -> Self {
        Self {
            permissions: Arc::new(tokio::sync::Mutex::new(PermissionEngine::default())),
            approvals: PermissionBroker::default(),
            plan: None,
            autonomous: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunOutput {
    pub text: String,
    pub events: Vec<LifecycleEvent>,
    pub turns: usize,
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
            .schemas()
            .into_iter()
            .map(|(descriptor, parameters)| ToolDefinition {
                name: descriptor.name,
                description: descriptor.description,
                parameters,
            })
            .collect()
    }

    pub async fn stream(&self, request: CompletionRequest) -> Result<TauStream, CompletionError> {
        self.provider.stream(request).await
    }

    /// The production Rig loop: each request is made through Rig's concrete
    /// `CompletionModel::stream` implementation, tool calls are policy checked
    /// immediately before execution, and tool results are fed into a second
    /// model request. The server may render the same lifecycle events to any
    /// transport without owning orchestration.
    pub async fn run_loop(
        &self,
        mut request: CompletionRequest,
        context: crate::tools::ToolContext,
        policy: RunnerPolicy,
        cancellation: CancellationToken,
        max_turns: usize,
    ) -> Result<RunOutput, CompletionError> {
        let run_id = Uuid::new_v4().to_string();
        let mut events = vec![];
        let mut text = String::new();
        let mut turns = 0;
        events.push(LifecycleEvent::RunStarted {
            identity: RunIdentity {
                run_id: run_id.clone(),
                turn: 0,
                internal_call_id: None,
                provider_tool_id: None,
                provider_call_id: None,
            },
        });
        loop {
            if cancellation.is_cancelled() {
                events.push(LifecycleEvent::RunCancelled {
                    identity: RunIdentity {
                        run_id: run_id.clone(),
                        turn: turns as u32,
                        internal_call_id: None,
                        provider_tool_id: None,
                        provider_call_id: None,
                    },
                });
                break;
            }
            if turns >= max_turns {
                break;
            }
            turns += 1;
            let base = RunIdentity {
                run_id: run_id.clone(),
                turn: turns as u32,
                internal_call_id: None,
                provider_tool_id: None,
                provider_call_id: None,
            };
            events.push(LifecycleEvent::TurnStarted {
                identity: base.clone(),
            });
            let mut stream = self.stream(request.clone()).await?;
            let mut calls = Vec::new();
            let mut turn_text = String::new();
            while let Some(item) = stream.next().await {
                match item? {
                    TauDelta::Text(value) => {
                        text.push_str(&value);
                        turn_text.push_str(&value);
                    }
                    TauDelta::ToolCall(call) => calls.push(call),
                    TauDelta::Usage(_) => {}
                }
            }
            if calls.is_empty() {
                events.push(LifecycleEvent::TurnFinished { identity: base });
                break;
            }
            let mut history: Vec<Message> = request.chat_history.clone().into_iter().collect();
            if !turn_text.is_empty() {
                history.push(Message::assistant(turn_text));
            }
            for call in calls {
                let call_id = Uuid::new_v4().to_string();
                let identity = RunIdentity {
                    internal_call_id: Some(call_id),
                    provider_tool_id: Some(call.id.clone()),
                    provider_call_id: call.call_id.clone(),
                    ..base.clone()
                };
                let args = call.function.arguments.clone();
                events.push(LifecycleEvent::ToolCallStarted {
                    identity: identity.clone(),
                    name: call.function.name.clone(),
                    arguments: args.clone(),
                });
                let decision = {
                    let mut permissions = policy.permissions.lock().await;
                    authorize(&mut permissions, &call.function.name, &args)
                };
                let allowed = match decision {
                    Ok(()) => true,
                    Err(crate::permissions::PermissionError::Rejected { .. }) => false,
                    Err(crate::permissions::PermissionError::Ask { .. }) => {
                        let approval = policy
                            .approvals
                            .request(
                                identity.run_id.clone(),
                                call.function.name.clone(),
                                args.clone(),
                            )
                            .await;
                        matches!(
                            policy.approvals.wait(&approval.id).await,
                            Some(PermissionReply::Allow)
                        )
                    }
                };
                let gate = if let Some(plan) = &policy.plan {
                    let plan = plan.lock().await;
                    allows_tool(&plan, policy.autonomous, &call.function.name).is_ok()
                } else {
                    true
                };
                let result = if allowed && gate {
                    match self.tools.execute(&call.function.name, args, &context) {
                        Ok(result) => {
                            let output = result.output;
                            events.push(LifecycleEvent::ToolCallFinished {
                                identity: identity.clone(),
                                result: output.clone(),
                            });
                            output
                        }
                        Err(error) => {
                            let message = error.to_string();
                            events.push(LifecycleEvent::ToolCallFailed {
                                identity: identity.clone(),
                                error: message.clone(),
                            });
                            serde_json::json!({"error": message})
                        }
                    }
                } else {
                    let message = if !allowed {
                        "permission denied"
                    } else {
                        "mutation requires an airtight plan step"
                    };
                    events.push(LifecycleEvent::ToolCallFailed {
                        identity: identity.clone(),
                        error: message.into(),
                    });
                    serde_json::json!({"error": message})
                };
                history.push(Message::Assistant {
                    id: None,
                    content: OneOrMany::one(AssistantContent::ToolCall(call.clone())),
                });
                history.push(Message::tool_result(call.id, result.to_string()));
            }
            request.chat_history = OneOrMany::many(history)
                .map_err(|_| CompletionError::RequestError("empty history".into()))?;
        }
        events.push(LifecycleEvent::RunFinished {
            identity: RunIdentity {
                run_id,
                turn: turns as u32,
                internal_call_id: None,
                provider_tool_id: None,
                provider_call_id: None,
            },
        });
        Ok(RunOutput {
            text,
            events,
            turns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::completion_request;
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

    #[tokio::test]
    async fn rig_tool_call_policy_execution_and_second_turn_are_real() {
        use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
        let model = MockCompletionModel::from_stream_turns([
            [
                MockStreamEvent::tool_call(
                    "c1",
                    "read",
                    serde_json::json!({"file_path":"README.md"}),
                ),
                MockStreamEvent::final_response_with_total_tokens(1),
            ],
            [
                MockStreamEvent::text("done"),
                MockStreamEvent::final_response_with_total_tokens(2),
            ],
        ]);
        let runner = AgentRunner::new(Provider::Mock(model))
            .with_tools(ToolRegistry::with_builtins().unwrap());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "hello").unwrap();
        let mut request = completion_request("inspect");
        request.tools = runner.tool_definitions();
        let output = runner
            .run_loop(
                request,
                crate::tools::ToolContext::new(dir.path()).unwrap(),
                RunnerPolicy {
                    permissions: Arc::new(tokio::sync::Mutex::new(
                        PermissionEngine::default()
                            .with_default(crate::permissions::Decision::Allow),
                    )),
                    ..RunnerPolicy::default()
                },
                CancellationToken::default(),
                3,
            )
            .await
            .unwrap();
        assert_eq!(output.text, "done");
        assert!(
            output
                .events
                .iter()
                .any(|event| matches!(event, LifecycleEvent::ToolCallFinished { .. }))
        );
        assert_eq!(output.turns, 2);
    }

    #[tokio::test]
    async fn denied_mutation_never_reaches_tool_body() {
        use rig_core::test_utils::{MockCompletionModel, MockStreamEvent};
        let model = MockCompletionModel::from_stream_turns([
            [
                MockStreamEvent::tool_call(
                    "c1",
                    "write",
                    serde_json::json!({"path":"blocked.txt","content":"no"}),
                ),
                MockStreamEvent::final_response_with_total_tokens(1),
            ],
            [
                MockStreamEvent::text("blocked"),
                MockStreamEvent::final_response_with_total_tokens(2),
            ],
        ]);
        let runner = AgentRunner::new(Provider::Mock(model));
        let dir = tempfile::tempdir().unwrap();
        let mut permissions =
            PermissionEngine::default().with_default(crate::permissions::Decision::Allow);
        permissions.add_rule(
            crate::permissions::Rule::new("write:*", crate::permissions::Decision::Reject).unwrap(),
        );
        let policy = RunnerPolicy {
            permissions: Arc::new(tokio::sync::Mutex::new(permissions)),
            ..RunnerPolicy::default()
        };
        let mut request = completion_request("write");
        request.tools = runner.tool_definitions();
        runner
            .run_loop(
                request,
                crate::tools::ToolContext::new(dir.path()).unwrap(),
                policy,
                CancellationToken::default(),
                2,
            )
            .await
            .unwrap();
        assert!(!dir.path().join("blocked.txt").exists());
    }

    #[tokio::test]
    async fn question_and_recursive_task_tiers_wait_and_enforce_limits() {
        let questions = QuestionBroker::default();
        let item = questions.ask("which file?", vec!["a".into()]).await;
        let waiter = {
            let questions = questions.clone();
            let id = item.id.clone();
            tokio::spawn(async move { questions.wait(&id).await })
        };
        assert!(
            questions
                .answer(QuestionAnswer {
                    id: item.id,
                    answer: "a".into()
                })
                .await
        );
        assert_eq!(waiter.await.unwrap().as_deref(), Some("a"));
        let budget = TaskBudget::new(TaskTier::Low);
        assert!(budget.child().is_ok());
        assert!(matches!(budget.child(), Err(TaskError::TaskLimitExceeded)));
        let depth_budget = TaskBudget::new(TaskTier::Medium);
        assert!(matches!(
            depth_budget.child().unwrap().child(),
            Err(TaskError::DepthExceeded)
        ));
    }
}
