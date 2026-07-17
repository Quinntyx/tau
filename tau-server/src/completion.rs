//! `completion.stream` WebSocket request handling.

use anyhow::{Result, bail};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures::StreamExt;
use rig_core::OneOrMany;
use rig_core::completion::{AssistantContent, CompletionRequest, Message as RigMessage, Usage};
use serde::Serialize;
use serde_json::Value;
use tau_core::agent::AgentRunner;
use tau_core::context::{ContextAssembler, ContextEpoch};
use tau_core::credentials::CredentialStore;
use tau_core::db::{ContentBlock, Message as DbMessage, Session};
use tau_core::permissions::{Decision, PermissionEngine, authorize};
use tau_core::provider::{Provider, TauDelta};
use tau_core::tools::ToolContext;
use tau_proto::prelude::*;

use crate::AppState;

async fn db_blocking<T, F>(db: tau_core::db::Db, operation: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&tau_core::db::Db) -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || operation(&db))
        .await
        .map_err(|error| anyhow::anyhow!("database worker failed: {error}"))?
}

pub(crate) async fn handle(
    socket: &mut WebSocket,
    state: &AppState,
    id: Id,
    raw_params: Option<Value>,
) {
    handle_inner(socket, state, id, raw_params, false).await;
}

pub(crate) async fn handle_turn(
    socket: &mut WebSocket,
    state: &AppState,
    id: Id,
    raw_params: Option<Value>,
) {
    let typed = match raw_params
        .as_ref()
        .and_then(|v| serde_json::from_value::<TurnStartParams>(v.clone()).ok())
    {
        Some(params) => params,
        None => {
            send_error(
                socket,
                id,
                INVALID_PARAMS,
                "session.turn.start requires typed params".into(),
            )
            .await;
            return;
        }
    };
    let params = CompletionStreamParams {
        model: typed.model,
        prompt: typed.prompt,
        session_id: typed.session_id,
        cwd: typed.cwd,
        agent: typed.agent,
        task_tier: typed.task_tier,
        autonomous: typed.autonomous,
    };
    handle_inner(socket, state, id, serde_json::to_value(params).ok(), true).await;
}

async fn handle_inner(
    socket: &mut WebSocket,
    state: &AppState,
    id: Id,
    raw_params: Option<Value>,
    typed: bool,
) {
    let params = match raw_params
        .ok_or_else(|| anyhow::anyhow!("completion.stream requires params"))
        .and_then(|value| {
            serde_json::from_value::<CompletionStreamParams>(value).map_err(Into::into)
        }) {
        Ok(params) => params,
        Err(error) => {
            send_error(socket, id, INVALID_PARAMS, error.to_string()).await;
            return;
        }
    };

    let (provider_id, model_id) = match split_model(&params.model) {
        Ok(parts) => parts,
        Err(error) => {
            send_error(socket, id, INVALID_PARAMS, error.to_string()).await;
            return;
        }
    };

    let session = match resolve_session(state, &params).await {
        Ok(session) => match db_blocking(state.db().clone(), {
            let session_id = session.id.clone();
            move |db| db.get_messages(&session_id)
        })
        .await
        {
            Ok(messages) => (session, messages),
            Err(error) => {
                send_error(socket, id, INVALID_PARAMS, error.to_string()).await;
                return;
            }
        },
        Err(error) => {
            send_error(socket, id, INVALID_PARAMS, error.to_string()).await;
            return;
        }
    };

    let provider_config = state.config().providers.get(provider_id);
    let custom_env = provider_config.and_then(|config| config.api_key_env.as_deref());
    let api_base = provider_config.and_then(|config| config.api_base.as_deref());
    let credentials = match CredentialStore::new() {
        Ok(store) => store,
        Err(error) => {
            send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
            return;
        }
    };
    let api_key = credentials.get(provider_id, custom_env);
    let provider = match Provider::new(provider_id, model_id, api_key.as_deref(), api_base) {
        Ok(provider) => provider,
        Err(error) => {
            send_error(socket, id, INVALID_PARAMS, error.to_string()).await;
            return;
        }
    };

    // Assemble every turn through the context epoch boundary.  This keeps the
    // 80% guard and the one-shot overflow retry in the production path rather
    // than leaving them as an unused library helper.
    let mut context = ContextAssembler::new(context_limit());
    context.set_provider_metadata(provider_id, model_id);
    for message in &session.1 {
        if let Some(message) = to_context_message(message) {
            context.push(message.0, message.1);
        }
    }
    context.push("user", params.prompt.clone());
    if context.should_compact_at(context_limit()) {
        let previous = compact_context(&mut context);
        persist_epoch(state, &session.0.id, &previous, "automatic").await;
    }
    let mut history = context_history(&context);
    let runner = AgentRunner::new(provider);
    let tool_definitions = runner.tool_definitions();
    let tool_context = match ToolContext::new(&session.0.cwd) {
        Ok(context) => context,
        Err(error) => {
            send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
            return;
        }
    };
    if let Err(error) = db_blocking(state.db().clone(), {
        let session_id = session.0.id.clone();
        let prompt = params.prompt.clone();
        move |db| db.append_message(&session_id, "user", vec![ContentBlock::text(&prompt)])
    })
    .await
    {
        send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
        return;
    }

    let mut text = String::new();
    let mut usage = Usage::default();
    // Keep authorization at the last possible boundary: no model call can
    // reach a tool body without passing the canonical engine (including hard
    // denials). Session-specific rules are loaded by higher-level turn APIs;
    // the completion compatibility path starts with the safe allow baseline.
    let mut permissions = PermissionEngine::default().with_default(Decision::Allow);
    let typed_turn_id = format!("turn-{id:?}");
    let mut sequence = 0u64;
    if typed {
        emit_turn(
            socket,
            state,
            &session.0.id,
            &typed_turn_id,
            &mut sequence,
            TurnEvent::TurnStarted {
                turn_id: typed_turn_id.clone(),
            },
        )
        .await;
    }
    loop {
        let request = CompletionRequest {
            model: None,
            preamble: None,
            chat_history: history.clone(),
            documents: vec![],
            tools: tool_definitions.clone(),
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            additional_params: None,
            output_schema: None,
        };
        let mut stream = match runner.stream(request).await {
            Ok(stream) => stream,
            Err(error) => {
                // Providers differ in their error types; all supported
                // providers expose overflow text in their display form.  A
                // single retry is deliberately consumed by the context epoch
                // so a permanently-too-large request cannot loop forever.
                if is_context_overflow(&error.to_string()) && context.mark_overflow_retry() {
                    let previous = compact_context(&mut context);
                    persist_epoch(state, &session.0.id, &previous, "overflow_retry").await;
                    // Compaction starts a new epoch; retain the fact that
                    // this request has already consumed its sole retry.
                    let _ = context.mark_overflow_retry();
                    history = context_history(&context);
                    continue;
                }
                send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
                return;
            }
        };
        let mut turn_text = String::new();
        let mut calls = Vec::new();
        while let Some(delta) = stream.next().await {
            match delta {
                Ok(TauDelta::Text(chunk)) => {
                    turn_text.push_str(&chunk);
                    if typed {
                        if !emit_turn(
                            socket,
                            state,
                            &session.0.id,
                            &typed_turn_id,
                            &mut sequence,
                            TurnEvent::TextDelta {
                                turn_id: typed_turn_id.clone(),
                                text: chunk,
                            },
                        )
                        .await
                        {
                            return;
                        }
                        continue;
                    }
                    let notification = Notification {
                        jsonrpc: JsonRpc::default(),
                        method: METHOD_COMPLETION_DELTA.to_string(),
                        params: Some(CompletionDelta {
                            request_id: id.clone(),
                            session_id: session.0.id.clone(),
                            text: chunk,
                            usage: None,
                        }),
                    };
                    if !send(socket, &notification).await {
                        return;
                    }
                }
                Ok(TauDelta::ToolCall(call)) => calls.push(call),
                Ok(TauDelta::Usage(final_usage)) => {
                    usage = final_usage;
                    if typed {
                        continue;
                    }
                    let notification = Notification {
                        jsonrpc: JsonRpc::default(),
                        method: METHOD_COMPLETION_DELTA.to_string(),
                        params: Some(CompletionDelta {
                            request_id: id.clone(),
                            session_id: session.0.id.clone(),
                            text: String::new(),
                            usage: Some(usage_summary(usage)),
                        }),
                    };
                    if !send(socket, &notification).await {
                        return;
                    }
                }
                Err(error) => {
                    send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
                    return;
                }
            }
        }
        text.push_str(&turn_text);
        if calls.is_empty() {
            break;
        }
        let mut next_history = history.into_iter().collect::<Vec<_>>();
        if !turn_text.is_empty() {
            next_history.push(RigMessage::assistant(turn_text));
        }
        for call in calls {
            next_history.push(RigMessage::Assistant {
                id: None,
                content: OneOrMany::one(AssistantContent::ToolCall(call.clone())),
            });
            let rendered = match authorize(
                &mut permissions,
                &call.function.name,
                &call.function.arguments,
            ) {
                Ok(()) => match runner.tools().execute(
                    &call.function.name,
                    call.function.arguments.clone(),
                    &tool_context,
                ) {
                    Ok(result) => result.rendered,
                    Err(error) => format!("tool error: {error}"),
                },
                Err(error) => format!("tool error: {error}"),
            };
            if typed
                && !emit_turn(
                    socket,
                    state,
                    &session.0.id,
                    &typed_turn_id,
                    &mut sequence,
                    TurnEvent::ToolOutput {
                        turn_id: typed_turn_id.clone(),
                        output: BoundedOutput {
                            inline: rendered.clone(),
                            truncated: false,
                            artifacts: vec![],
                        },
                    },
                )
                .await
            {
                return;
            }
            let card = format!("\n[tool {}]\n{}\n", call.function.name, rendered);
            if !typed
                && !send(
                    socket,
                    &Notification {
                        jsonrpc: JsonRpc::default(),
                        method: METHOD_COMPLETION_DELTA.to_string(),
                        params: Some(CompletionDelta {
                            request_id: id.clone(),
                            session_id: session.0.id.clone(),
                            text: card,
                            usage: None,
                        }),
                    },
                )
                .await
            {
                return;
            }
            next_history.push(RigMessage::tool_result(call.id, rendered));
        }
        history = OneOrMany::many(next_history).expect("history contains the prompt");
    }

    let assistant = match db_blocking(state.db().clone(), {
        let session_id = session.0.id.clone();
        let text = text.clone();
        move |db| db.append_message(&session_id, "assistant", vec![ContentBlock::text(&text)])
    })
    .await
    {
        Ok(message) => message,
        Err(error) => {
            send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
            return;
        }
    };
    if let Err(error) = db_blocking(state.db().clone(), {
        let session_id = session.0.id.clone();
        let model = params.model.clone();
        let message_id = assistant.id;
        move |db| {
            db.record_usage(
                &session_id,
                Some(message_id),
                &model,
                usage.input_tokens as i64,
                usage.output_tokens as i64,
                Some(usage.cached_input_tokens as i64),
            )
        }
    })
    .await
    {
        send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
        return;
    }

    if typed {
        if !emit_turn(
            socket,
            state,
            &session.0.id,
            &typed_turn_id,
            &mut sequence,
            TurnEvent::TurnCompleted {
                turn_id: typed_turn_id.clone(),
                message_id: Some(assistant.id),
            },
        )
        .await
        {
            return;
        }
        send(
            socket,
            &Response::ok(
                id,
                TurnStartResult {
                    session_id: session.0.id,
                    turn_id: typed_turn_id,
                },
            ),
        )
        .await;
        return;
    }
    let result = CompletionStreamResult {
        session_id: session.0.id,
        message_id: assistant.id,
        text,
        usage: usage_summary(usage),
    };
    send(socket, &Response::ok(id, result)).await;
}

fn split_model(model: &str) -> Result<(&str, &str)> {
    let (provider, model_id) = model
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("model must use the provider/model form"))?;
    if provider.is_empty() || model_id.is_empty() {
        bail!("model must use the provider/model form");
    }
    Ok((provider, model_id))
}

async fn resolve_session(state: &AppState, params: &CompletionStreamParams) -> Result<Session> {
    if let Some(id) = params.session_id.as_deref() {
        let id = id.to_string();
        let lookup_id = id.clone();
        return db_blocking(state.db().clone(), move |db| db.get_session(&lookup_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("session not found: {id}"));
    }
    let cwd = params
        .cwd
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("cwd is required when session_id is omitted"))?;
    let cwd = cwd.to_string();
    db_blocking(state.db().clone(), move |db| db.create_session(&cwd)).await
}

fn context_limit() -> usize {
    std::env::var("TAU_CONTEXT_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(128_000)
}

fn to_context_message(message: &DbMessage) -> Option<(String, String)> {
    let text = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    if text.is_empty() || !matches!(message.role.as_str(), "system" | "user" | "assistant") {
        return None;
    }
    Some((message.role.clone(), text))
}

fn context_history(context: &ContextAssembler) -> OneOrMany<RigMessage> {
    let messages = context
        .messages()
        .iter()
        .map(|message| match message.role.as_str() {
            "system" => RigMessage::system(message.content.clone()),
            "assistant" => RigMessage::assistant(message.content.clone()),
            _ => RigMessage::user(message.content.clone()),
        })
        .collect::<Vec<_>>();
    OneOrMany::many(messages).expect("context always contains a user turn")
}

fn compact_context(context: &mut ContextAssembler) -> ContextEpoch {
    let summary = context
        .messages()
        .iter()
        .map(|message| format!("{}: {}", message.role, message.content))
        .collect::<Vec<_>>()
        .join("\n");
    context.compact_with_summary(summary)
}

async fn persist_epoch(state: &AppState, session_id: &str, epoch: &ContextEpoch, trigger: &str) {
    let mut record = tau_core::db::ContextEpochRecord::new(
        session_id,
        epoch.number as i64,
        epoch
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        trigger,
    );
    record.plan_context = epoch.plan_context.clone();
    record.provider = epoch.provider.clone();
    record.model = epoch.compaction_model.clone();
    record.retry_marker = epoch.retry_marker;
    let _ = db_blocking(state.db().clone(), move |db| {
        db.append_context_epoch(&record)
    })
    .await;
}

fn is_context_overflow(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "context length",
        "context window",
        "too many tokens",
        "maximum context",
        "prompt is too long",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

fn usage_summary(usage: Usage) -> UsageSummary {
    UsageSummary {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
    }
}

async fn send_error(socket: &mut WebSocket, id: Id, code: i32, message: String) {
    send(socket, &Response::<Value>::err(id, code, message)).await;
}

async fn send<T: Serialize>(socket: &mut WebSocket, value: &T) -> bool {
    let Ok(text) = serde_json::to_string(value) else {
        return false;
    };
    socket.send(WsMessage::Text(text.into())).await.is_ok()
}

async fn emit_turn(
    socket: &mut WebSocket,
    state: &AppState,
    session_id: &str,
    _turn_id: &str,
    sequence: &mut u64,
    event: TurnEvent,
) -> bool {
    *sequence += 1;
    let Ok(persisted) = db_blocking(state.db().clone(), {
        let session_id = session_id.to_string();
        let event = event.clone();
        move |db| db.append_event(&session_id, &event, None)
    }).await else { return false; };
    send(
        socket,
        &Notification {
            jsonrpc: JsonRpc::default(),
            method: METHOD_TURN_EVENT.to_string(),
            params: Some(SequencedEvent {
                event_id: persisted.event_id,
                session_id: session_id.to_string(),
                sequence: persisted.sequence,
                occurred_at: persisted.occurred_at,
                request_id: persisted.request_id,
                event: persisted.event,
            }),
        },
    )
    .await
}
