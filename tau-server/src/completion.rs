//! The single canonical typed-turn executor.
//!
//! Transport code only admits work and sends the response.  Everything after
//! admission happens here, including context assembly, the Rig AgentRunner
//! loop, policy gates, persistence, compaction, and terminal events.

use anyhow::{Context, Result, bail};
use rig_core::OneOrMany;
use rig_core::completion::{CompletionRequest, Message as RigMessage};
use tau_core::agent::{AgentRunner, CancellationToken, LifecycleEvent, RunnerPolicy};
use tau_core::context::{ContextAssembler, ContextEpoch};
use tau_core::credentials::CredentialStore;
use tau_core::db::{ContentBlock, Message as DbMessage};
use tau_core::provider::Provider;
use tau_core::tools::ToolContext;
use tau_proto::prelude::*;

use crate::{AppState, runtime::CancellationHandle};

#[derive(Clone)]
pub(crate) struct TurnJob {
    pub params: TurnStartParams,
    pub session_id: String,
    pub turn_id: String,
    pub request_id: Id,
    pub cancellation: CancellationHandle,
}

pub(crate) async fn execute_turn(state: AppState, job: TurnJob) {
    let result = execute_inner(&state, &job).await;
    let terminal = match result {
        Ok(message_id) => TurnEvent::TurnCompleted {
            turn_id: job.turn_id.clone(),
            message_id,
        },
        Err(_error) if job.cancellation.is_cancelled() => TurnEvent::TurnCancelled {
            turn_id: job.turn_id.clone(),
        },
        Err(error) => TurnEvent::TurnFailed {
            turn_id: job.turn_id.clone(),
            message: error.to_string(),
        },
    };
    let _ = state
        .publish_event(&job.session_id, terminal, Some(&job.request_id))
        .await;
    state.runtime().cancellation.remove(&job.turn_id);
}

async fn execute_inner(state: &AppState, job: &TurnJob) -> Result<Option<i64>> {
    if job.cancellation.is_cancelled() {
        bail!("turn cancelled")
    }
    let session = db(state, {
        let id = job.session_id.clone();
        move |db| db.get_session(&id)
    })
    .await?
    .context("session disappeared")?;
    let messages = db(state, {
        let id = job.session_id.clone();
        move |db| db.get_messages(&id)
    })
    .await?;
    let (provider_id, model_id) = split_model(&job.params.model)?;
    let provider_config = state.config().providers.get(provider_id);
    let credentials = CredentialStore::new()?;
    let key = credentials.get(
        provider_id,
        provider_config.and_then(|c| c.api_key_env.as_deref()),
    );
    let provider = Provider::new(
        provider_id,
        model_id,
        key.as_deref(),
        provider_config.and_then(|c| c.api_base.as_deref()),
    )?;

    let mut context = ContextAssembler::new(context_limit());
    context.set_provider_metadata(provider_id, model_id);
    for message in &messages {
        if let Some((role, text)) = to_context_message(message) {
            context.push(role, text);
        }
    }
    context.push("user", job.params.prompt.clone());
    if context.should_compact() {
        let old = compact_context(&mut context);
        persist_epoch(state, &job.session_id, &old, "automatic").await?;
    }
    db(state, {
        let id = job.session_id.clone();
        let prompt = job.params.prompt.clone();
        move |db| db.append_message(&id, "user", vec![ContentBlock::text(prompt)])
    })
    .await?;

    let runner = AgentRunner::new(provider);
    let request = CompletionRequest {
        model: None,
        preamble: None,
        chat_history: context_history(&context),
        documents: vec![],
        tools: runner.tool_definitions(),
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        additional_params: None,
        output_schema: None,
    };
    let tool_context = ToolContext::new(&session.cwd)?;
    let policy = RunnerPolicy {
        permissions: std::sync::Arc::new(tokio::sync::Mutex::new(
            tau_core::permissions::PermissionEngine::default()
                .with_default(tau_core::permissions::Decision::Allow),
        )),
        autonomous: job.params.autonomous.unwrap_or(false),
        ..RunnerPolicy::default()
    };
    let token = CancellationToken::default();
    let token_for_wait = token.clone();
    let cancellation = job.cancellation.clone();
    let run = runner.run_loop(request, tool_context, policy, token, 16);
    tokio::pin!(run);
    let output = tokio::select! {
        result = &mut run => result.map_err(|e| anyhow::anyhow!(e.to_string()))?,
        _ = cancellation.cancelled() => {
            token_for_wait.cancel();
            // Do not wait for a provider stream after cancellation.  Provider
            // implementations are not required to observe tau's cancellation
            // token, and waiting here would make the protocol terminal event
            // depend on an unbounded network timeout.  The runner token is
            // still cancelled so an implementation which does observe it can
            // stop its work while this task reports the typed terminal state.
            bail!("turn cancelled")
        }
    };

    for event in &output.events {
        let rendered = match event {
            LifecycleEvent::ToolCallFinished { result, .. } => result.to_string(),
            LifecycleEvent::ToolCallFailed { error, .. } => error.clone(),
            _ => continue,
        };
        state
            .publish_event(
                &job.session_id,
                TurnEvent::ToolOutput {
                    turn_id: job.turn_id.clone(),
                    output: BoundedOutput {
                        inline: rendered,
                        truncated: false,
                        artifacts: vec![],
                    },
                },
                Some(&job.request_id),
            )
            .await?;
    }
    if !output.text.is_empty() {
        state
            .publish_event(
                &job.session_id,
                TurnEvent::TextDelta {
                    turn_id: job.turn_id.clone(),
                    text: output.text.clone(),
                },
                Some(&job.request_id),
            )
            .await?;
    }
    let mut blocks = vec![];
    if !output.text.is_empty() {
        blocks.push(ContentBlock::text(&output.text));
    }
    for event in &output.events {
        match event {
            LifecycleEvent::ToolCallStarted {
                identity,
                name,
                arguments,
            } => blocks.push(ContentBlock::ToolUse {
                call_id: identity.internal_call_id.clone().unwrap_or_default(),
                name: name.clone(),
                args_json: arguments.to_string(),
            }),
            LifecycleEvent::ToolCallFinished { identity, result } => {
                blocks.push(ContentBlock::ToolResult {
                    call_id: identity.internal_call_id.clone().unwrap_or_default(),
                    result_json: result.to_string(),
                    is_error: false,
                })
            }
            LifecycleEvent::ToolCallFailed { identity, error } => {
                blocks.push(ContentBlock::ToolResult {
                    call_id: identity.internal_call_id.clone().unwrap_or_default(),
                    result_json: error.clone(),
                    is_error: true,
                })
            }
            _ => {}
        }
    }
    if blocks.is_empty() {
        blocks.push(ContentBlock::text(""));
    }
    // Cancellation is also a mutation gate: never commit a completed
    // assistant turn after the client has cancelled it.
    if job.cancellation.is_cancelled() {
        bail!("turn cancelled")
    }
    let assistant = db(state, {
        let id = job.session_id.clone();
        move |db| db.append_message(&id, "assistant", blocks)
    })
    .await?;
    Ok(Some(assistant.id))
}

async fn db<T, F>(state: &AppState, operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&tau_core::db::Db) -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking({
        let db = state.db().clone();
        move || operation(&db)
    })
    .await?
}

fn split_model(model: &str) -> Result<(&str, &str)> {
    let (provider, model) = model
        .split_once('/')
        .context("model must use provider/model")?;
    if provider.is_empty() || model.is_empty() {
        bail!("model must use provider/model");
    }
    Ok((provider, model))
}

fn context_limit() -> usize {
    std::env::var("TAU_CONTEXT_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(128_000)
}

fn to_context_message(message: &DbMessage) -> Option<(String, String)> {
    let text = message
        .blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    (!text.is_empty() && matches!(message.role.as_str(), "system" | "user" | "assistant"))
        .then(|| (message.role.clone(), text))
}

fn context_history(context: &ContextAssembler) -> OneOrMany<RigMessage> {
    OneOrMany::many(
        context
            .messages()
            .iter()
            .map(|m| match m.role.as_str() {
                "system" => RigMessage::system(&m.content),
                "assistant" => RigMessage::assistant(&m.content),
                _ => RigMessage::user(&m.content),
            })
            .collect::<Vec<_>>(),
    )
    .expect("context has a user message")
}

fn compact_context(context: &mut ContextAssembler) -> ContextEpoch {
    let summary = context
        .messages()
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");
    context.compact_with_summary(summary)
}

async fn persist_epoch(
    state: &AppState,
    session_id: &str,
    epoch: &ContextEpoch,
    trigger: &str,
) -> Result<()> {
    let mut record = tau_core::db::ContextEpochRecord::new(
        session_id,
        epoch.number as i64,
        epoch
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        trigger,
    );
    record.provider = epoch.provider.clone();
    record.model = epoch.compaction_model.clone();
    record.retry_marker = epoch.retry_marker;
    db(state, move |db| db.append_context_epoch(&record))
        .await
        .map(|_| ())
}
