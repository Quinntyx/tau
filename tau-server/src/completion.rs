//! `completion.stream` WebSocket request handling.

use anyhow::{Result, bail};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures::StreamExt;
use rig_core::OneOrMany;
use rig_core::completion::{CompletionRequest, Message as RigMessage, Usage};
use serde::Serialize;
use serde_json::Value;
use tau_core::credentials::CredentialStore;
use tau_core::db::{ContentBlock, Message as DbMessage, Session};
use tau_core::provider::{Provider, TauDelta};
use tau_proto::prelude::*;

use crate::AppState;

pub(crate) async fn handle(
    socket: &mut WebSocket,
    state: &AppState,
    id: Id,
    raw_params: Option<Value>,
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

    let session = match resolve_session(state, &params).and_then(|session| {
        state
            .db()
            .get_messages(&session.id)
            .map(|messages| (session, messages))
    }) {
        Ok((session, messages)) => (session, messages),
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

    let request = CompletionRequest {
        model: None,
        preamble: None,
        chat_history: chat_history(&session.1, &params.prompt),
        documents: vec![],
        tools: vec![],
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        additional_params: None,
        output_schema: None,
    };
    if let Err(error) = state.db().append_message(
        &session.0.id,
        "user",
        vec![ContentBlock::text(&params.prompt)],
    ) {
        send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
        return;
    }

    let mut stream = match provider.stream(request).await {
        Ok(stream) => stream,
        Err(error) => {
            send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
            return;
        }
    };
    let mut text = String::new();
    let mut usage = Usage::default();
    while let Some(delta) = stream.next().await {
        match delta {
            Ok(TauDelta::Text(chunk)) => {
                text.push_str(&chunk);
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
            Ok(TauDelta::Usage(final_usage)) => {
                usage = final_usage;
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

    let assistant =
        match state
            .db()
            .append_message(&session.0.id, "assistant", vec![ContentBlock::text(&text)])
        {
            Ok(message) => message,
            Err(error) => {
                send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
                return;
            }
        };
    if let Err(error) = state.db().record_usage(
        &session.0.id,
        Some(assistant.id),
        &params.model,
        usage.input_tokens as i64,
        usage.output_tokens as i64,
        Some(usage.cached_input_tokens as i64),
    ) {
        send_error(socket, id, INTERNAL_ERROR, error.to_string()).await;
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

fn resolve_session(state: &AppState, params: &CompletionStreamParams) -> Result<Session> {
    if let Some(id) = params.session_id.as_deref() {
        return state
            .db()
            .get_session(id)?
            .ok_or_else(|| anyhow::anyhow!("session not found: {id}"));
    }
    let cwd = params
        .cwd
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("cwd is required when session_id is omitted"))?;
    state.db().create_session(cwd)
}

fn chat_history(messages: &[DbMessage], prompt: &str) -> OneOrMany<RigMessage> {
    let mut history = messages
        .iter()
        .filter_map(to_rig_message)
        .collect::<Vec<_>>();
    history.push(RigMessage::user(prompt));
    OneOrMany::many(history).expect("chat history always contains the prompt")
}

fn to_rig_message(message: &DbMessage) -> Option<RigMessage> {
    let text = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    if text.is_empty() {
        return None;
    }
    match message.role.as_str() {
        "system" => Some(RigMessage::system(text)),
        "user" => Some(RigMessage::user(text)),
        "assistant" => Some(RigMessage::assistant(text)),
        _ => None,
    }
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
