use crate::{
    reducer,
    state::{AppState, Connection},
};
use anyhow::Result;
use futures_util::StreamExt;
use tau_client::{Client, TurnStreamEvent};
use tau_proto::prelude::{BoundedOutput, SequencedEvent, TurnEvent};
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Turn(SequencedEvent),
    Complete { session_id: String, turn_id: String },
    Tool { name: String, output: BoundedOutput },
    Permission { tool: String, summary: String },
    Disconnected,
    Reconnected,
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
            }
        }
    }
}
pub async fn complete(s: &mut AppState, client: &mut Client, prompt: String) -> Result<()> {
    s.transcript.push("tau: ".into());
    let mut stream = client
        .turn_start(reducer::params(
            s,
            prompt,
            Some(std::env::current_dir()?.to_string_lossy().into_owned()),
        ))
        .await?;
    while let Some(e) = stream.next().await {
        match e? {
            TurnStreamEvent::Event(event) => reduce_event(s, event),
            TurnStreamEvent::Complete(r) => {
                s.session_id = Some(r.session_id);
                s.turn_id = Some(r.turn_id);
            }
        }
    }
    Ok(())
}

pub async fn cancel(s: &mut AppState, client: &mut Client) -> Result<()> {
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

pub async fn replay(s: &mut AppState, client: &mut Client) -> Result<()> {
    if let Some(session_id) = s.session_id.clone() {
        let result = client
            .turn_replay(tau_proto::prelude::TurnReplayParams {
                session_id,
                after_sequence: s.sequence,
                limit: Some(256),
            })
            .await?;
        for event in result.events {
            reduce_event(s, event);
        }
        s.replaying = false;
    }
    Ok(())
}

pub fn reduce_event(s: &mut AppState, event: SequencedEvent) {
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
            s.transcript.push(format!("tau question: {question}"));
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
