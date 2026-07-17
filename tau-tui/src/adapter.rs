use crate::{
    reducer,
    state::{AppState, Connection},
};
use anyhow::Result;
use futures_util::StreamExt;
use tau_client::{Client, TurnStreamEvent};
use tau_proto::prelude::{
    BoundedOutput, ClientResponse, IdempotencyKey, SequencedEvent, TurnEvent, TurnResponseParams,
};
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Turn(SequencedEvent),
    Complete { session_id: String, turn_id: String },
    Tool { name: String, output: BoundedOutput },
    Permission { tool: String, summary: String },
    Disconnected,
    Reconnected,
    Error(String),
}
#[derive(Debug, Default)]
pub struct ScriptedClient {
    pub events: Vec<ClientEvent>,
}
impl ScriptedClient {
    pub fn drive(&self, s: &mut AppState) {
        for e in &self.events {
            match e {
                ClientEvent::Turn(event) => reduce_event(s, event.clone()),
                ClientEvent::Complete {
                    session_id,
                    turn_id,
                } => {
                    s.session_id = Some(session_id.clone());
                    s.turn_id = Some(turn_id.clone());
                }
                ClientEvent::Tool { name, output } => s.tools.push(crate::state::ToolCard {
                    name: name.clone(),
                    result: output.inline.clone(),
                    input: serde_json::Value::Null,
                    status: crate::state::ToolStatus::Complete,
                    expanded: false,
                }),
                ClientEvent::Permission { tool, summary } => {
                    s.permission = Some(crate::state::Permission {
                        tool: tool.clone(),
                        summary: summary.clone(),
                        choice: crate::state::PermissionChoice::AllowOnce,
                        stage: crate::state::PermissionStage::Choose,
                    })
                }
                ClientEvent::Disconnected => s.connection = Connection::Disconnected,
                ClientEvent::Reconnected => s.connection = Connection::Connected,
                ClientEvent::Error(message) => s.transcript.push(format!("tau error: {message}")),
            }
        }
    }
}
/// Run a turn away from the terminal task.  The sender is deliberately
/// unbounded: rendering must never apply backpressure to cancellation or
/// terminal input while a daemon is producing output.
pub async fn turn_task(
    client: Client,
    params: tau_proto::prelude::TurnStartParams,
    tx: tokio::sync::mpsc::UnboundedSender<ClientEvent>,
) {
    let result = async {
        let mut stream = client.turn_start(params).await?;
        while let Some(e) = stream.next().await {
            match e? {
                TurnStreamEvent::Event(event) => {
                    let _ = tx.send(ClientEvent::Turn(event));
                }
                TurnStreamEvent::Complete(r) => {
                    let _ = tx.send(ClientEvent::Complete {
                        session_id: r.session_id,
                        turn_id: r.turn_id,
                    });
                    break;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        let _ = tx.send(ClientEvent::Error(error.to_string()));
    }
}

pub async fn complete(
    s: &mut AppState,
    client: &Client,
    prompt: String,
    tx: tokio::sync::mpsc::UnboundedSender<ClientEvent>,
) -> Result<()> {
    s.transcript.push("tau: ".into());
    let params = reducer::params(
        s,
        prompt,
        Some(std::env::current_dir()?.to_string_lossy().into_owned()),
    );
    tokio::spawn(turn_task(client.clone(), params, tx));
    Ok(())
}

pub async fn persistent_events(
    client: Client,
    tx: tokio::sync::mpsc::UnboundedSender<ClientEvent>,
) {
    let mut stream = client.events();
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => {
                if tx.send(ClientEvent::Turn(event)).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = tx.send(ClientEvent::Error(error.to_string()));
                break;
            }
        }
    }
    let _ = tx.send(ClientEvent::Disconnected);
}

pub async fn cancel(s: &mut AppState, client: &Client) -> Result<()> {
    if let (Some(session_id), Some(turn_id)) = (s.session_id.clone(), s.turn_id.clone()) {
        let result = client
            .turn_cancel(tau_proto::prelude::TurnCancelParams {
                session_id,
                turn_id,
                idempotency_key: tau_proto::prelude::IdempotencyKey::new("tau-tui-cancel"),
            })
            .await?;
        s.cancelling = !result.cancelled;
    }
    Ok(())
}

/// Send an interactive answer on the control plane.  This is intentionally a
/// separate request from the active turn stream so a permission/question/diff
/// decision is never queued behind generation.
pub async fn respond(s: &AppState, client: &Client, response: ClientResponse) -> Result<()> {
    let (Some(session_id), Some(turn_id)) = (s.session_id.clone(), s.turn_id.clone()) else {
        anyhow::bail!("cannot answer a turn before the daemon provides its session and turn ids");
    };
    client
        .turn_response(TurnResponseParams {
            session_id,
            turn_id,
            idempotency_key: IdempotencyKey::new(format!("tau-tui-response-{}", s.sequence)),
            response,
        })
        .await?;
    Ok(())
}

pub async fn replay_task(
    client: Client,
    session_id: String,
    after_sequence: u64,
    tx: tokio::sync::mpsc::UnboundedSender<ClientEvent>,
) {
    match client
        .turn_replay(tau_proto::prelude::TurnReplayParams {
            session_id,
            after_sequence,
            limit: Some(256),
        })
        .await
    {
        Ok(result) => {
            for event in result.events {
                if tx.send(ClientEvent::Turn(event)).is_err() {
                    return;
                }
            }
            let _ = tx.send(ClientEvent::Reconnected);
        }
        Err(error) => {
            let _ = tx.send(ClientEvent::Error(error.to_string()));
        }
    }
}

pub fn reduce_event(s: &mut AppState, event: SequencedEvent) {
    if event.sequence <= s.sequence {
        return;
    }
    s.sequence = s.sequence.max(event.sequence);
    match event.event {
        TurnEvent::TurnStarted { turn_id } => s.turn_id = Some(turn_id),
        TurnEvent::TextDelta { text, .. } => {
            if let Some(last) = s.transcript.last_mut() {
                last.push_str(&text);
            } else {
                s.transcript.push(text);
            }
        }
        TurnEvent::ToolOutput { output, .. } => s.tools.push(crate::state::ToolCard {
            name: "tool".into(),
            result: output.inline,
            input: serde_json::Value::Null,
            status: crate::state::ToolStatus::Complete,
            expanded: false,
        }),
        TurnEvent::PermissionRequested {
            tool, description, ..
        } => {
            s.permission = Some(crate::state::Permission {
                tool,
                summary: description,
                choice: crate::state::PermissionChoice::AllowOnce,
                stage: crate::state::PermissionStage::Choose,
            });
        }
        TurnEvent::QuestionAsked { question, .. } => {
            s.question = Some(crate::state::Question {
                prompt: question,
                answer: None,
            });
        }
        TurnEvent::ArtifactCreated { artifact, .. } => s.transcript.push(format!(
            "artifact {} ({})",
            artifact.artifact_id, artifact.media_type
        )),
        TurnEvent::TurnCompleted { .. } => s.cancelling = false,
        TurnEvent::TurnCancelled { .. } => s.cancelling = false,
        TurnEvent::TurnFailed { message, .. } => s.transcript.push(format!("tau error: {message}")),
    }
}
