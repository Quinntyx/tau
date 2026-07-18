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
                ClientEvent::Tool { name, output } => {
                    s.tools.push(crate::state::ToolCard {
                        name: name.clone(),
                        result: output.inline.clone(),
                        input: serde_json::Value::Null,
                        status: crate::state::ToolStatus::Complete,
                        expanded: false,
                    });
                    s.tool_call_ids.push(None);
                }
                ClientEvent::Permission { tool, summary } => {
                    s.permission_request_id = None;
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
    s.assistant_index = Some(s.transcript.len() - 1);
    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    let projects = client
        .project_list(tau_proto::prelude::ProjectListParams::default())
        .await;
    if let Ok(projects) = projects {
        if let Some(project) = projects.projects.into_iter().find(|project| {
            project.active
                && std::fs::canonicalize(&project.root).ok() == std::fs::canonicalize(&cwd).ok()
        }) {
            s.project_id = project.id;
        } else {
            let name = std::path::Path::new(&cwd)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("project")
                .to_owned();
            s.project_id = client
                .project_create(tau_proto::prelude::ProjectCreateParams {
                    name,
                    root: cwd.clone(),
                })
                .await?
                .project
                .id;
        }
    } else {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        cwd.hash(&mut hasher);
        s.project_id = format!("legacy-{hash:016x}", hash = hasher.finish());
    }
    let params = reducer::params(s, prompt, Some(cwd));
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
    s.raw_events.push(event.clone());
    match event.event {
        TurnEvent::TurnStarted { turn_id } => {
            s.turn_id = Some(turn_id);
            if s.assistant_index.is_none() {
                s.transcript.push(String::new());
                s.assistant_index = Some(s.transcript.len() - 1);
            }
        }
        TurnEvent::TextDelta { text, .. } => {
            let index = ensure_assistant(s);
            s.transcript[index].push_str(&text);
        }
        TurnEvent::ReasoningDelta { text, .. } => {
            s.transcript.push(format!("Reasoning: {text}"));
        }
        TurnEvent::ToolStarted {
            tool_call_id,
            tool,
            input,
            ..
        } => {
            s.tools.push(crate::state::ToolCard {
                name: tool,
                result: String::new(),
                input: input.unwrap_or(serde_json::Value::Null),
                status: crate::state::ToolStatus::Running,
                expanded: false,
            });
            s.tool_call_ids.push(Some(tool_call_id));
        }
        TurnEvent::ToolStatus {
            tool_call_id,
            status,
            metadata,
            ..
        } => {
            if let Some(index) = find_tool(s, &tool_call_id) {
                s.tools[index].status = match status {
                    tau_proto::prelude::ToolStatusValue::Pending
                    | tau_proto::prelude::ToolStatusValue::Running => {
                        crate::state::ToolStatus::Running
                    }
                    tau_proto::prelude::ToolStatusValue::Completed => {
                        crate::state::ToolStatus::Complete
                    }
                    tau_proto::prelude::ToolStatusValue::Failed => crate::state::ToolStatus::Failed,
                    tau_proto::prelude::ToolStatusValue::Cancelled => {
                        crate::state::ToolStatus::Cancelled
                    }
                };
                if let Some(metadata) = metadata {
                    s.tools[index].input = metadata;
                }
            }
        }
        TurnEvent::ToolCompleted {
            tool_call_id,
            output,
            ..
        } => {
            if let Some(index) = find_tool(s, &tool_call_id) {
                s.tools[index].status = crate::state::ToolStatus::Complete;
                if let Some(output) = output {
                    s.tools[index].result = output.inline;
                }
            } else {
                push_tool(s, tool_call_id, "tool".into(), output);
            }
        }
        TurnEvent::ToolError {
            tool_call_id,
            error,
            ..
        } => {
            if let Some(index) = find_tool(s, &tool_call_id) {
                s.tools[index].status = crate::state::ToolStatus::Failed;
                s.tools[index].result = error;
            } else {
                push_tool(s, tool_call_id, "tool".into(), None);
                if let Some(index) = s.tools.len().checked_sub(1) {
                    s.tools[index].status = crate::state::ToolStatus::Failed;
                    s.tools[index].result = error;
                }
            }
        }
        TurnEvent::ToolOutput { output, .. } => {
            push_tool(s, String::new(), "tool".into(), Some(output));
        }
        TurnEvent::PermissionRequested {
            request_id,
            tool,
            description,
            ..
        } => {
            s.permission_request_id = Some(request_id);
            s.permission = Some(crate::state::Permission {
                tool,
                summary: description,
                choice: crate::state::PermissionChoice::AllowOnce,
                stage: crate::state::PermissionStage::Choose,
            });
        }
        TurnEvent::QuestionAsked {
            question_id,
            question,
            ..
        } => {
            s.question_id = Some(question_id);
            s.question = Some(crate::state::Question {
                prompt: question,
                answer: None,
            });
        }
        TurnEvent::DiffRequested {
            request_id,
            path,
            diff,
            ..
        } => {
            s.diff_request_id = Some(request_id);
            s.diff_path = Some(path.clone());
            s.diff_reply = Some(crate::state::DiffReply { accepted: None });
            s.transcript
                .push(format!("tau diff review: {path}\n{diff}"));
        }
        TurnEvent::ArtifactCreated { artifact, .. } => s.transcript.push(format!(
            "artifact {} ({}, {} bytes)",
            artifact.artifact_id, artifact.media_type, artifact.size_bytes
        )),
        TurnEvent::CompactionStarted { .. } => s.transcript.push("Compaction started".into()),
        TurnEvent::CompactionCompleted { summary, .. } => s
            .transcript
            .push(summary.unwrap_or_else(|| "Compaction completed".into())),
        TurnEvent::SystemMessage { message, .. } => s.transcript.push(message),
        TurnEvent::IntegrationEvent {
            integration,
            event,
            data,
            ..
        } => s.transcript.push(format!(
            "{integration}: {event}{}",
            data.map(|value| format!("\n{value}")).unwrap_or_default()
        )),
        TurnEvent::PlanUpdated { plan, .. } => s.transcript.push(format!("plan: {plan}")),
        TurnEvent::StatusChanged {
            status, message, ..
        } => {
            if matches!(status.as_str(), "failed" | "error") {
                s.transcript
                    .push(format!("tau error: {}", message.unwrap_or(status)));
            }
        }
        TurnEvent::Telemetry { .. } => {}
        TurnEvent::TurnCompleted { .. } => {
            s.cancelling = false;
            s.assistant_index = None;
        }
        TurnEvent::TurnCancelled { .. } => {
            s.cancelling = false;
            s.assistant_index = None;
        }
        TurnEvent::TurnFailed { message, .. } => {
            s.transcript.push(format!("tau error: {message}"));
            s.assistant_index = None;
        }
    }
}

fn ensure_assistant(s: &mut AppState) -> usize {
    if let Some(index) = s.assistant_index {
        return index;
    }
    s.transcript.push(String::new());
    let index = s.transcript.len() - 1;
    s.assistant_index = Some(index);
    index
}

fn find_tool(s: &AppState, call_id: &str) -> Option<usize> {
    s.tool_call_ids
        .iter()
        .position(|value| value.as_deref() == Some(call_id))
}

fn push_tool(s: &mut AppState, call_id: String, name: String, output: Option<BoundedOutput>) {
    s.tools.push(crate::state::ToolCard {
        name,
        result: output.map(|value| value.inline).unwrap_or_default(),
        input: serde_json::Value::Null,
        status: crate::state::ToolStatus::Complete,
        expanded: false,
    });
    s.tool_call_ids
        .push((!call_id.is_empty()).then_some(call_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sequence: u64, event: TurnEvent) -> SequencedEvent {
        SequencedEvent {
            event_id: format!("e{sequence}"),
            session_id: "s".into(),
            sequence,
            occurred_at: 0,
            request_id: None,
            event,
        }
    }

    #[test]
    fn typed_reducer_preserves_events_and_correlates_tool_lifecycle() {
        let mut state = AppState::default();
        reduce_event(
            &mut state,
            event(
                1,
                TurnEvent::TurnStarted {
                    turn_id: "t".into(),
                },
            ),
        );
        reduce_event(
            &mut state,
            event(
                2,
                TurnEvent::TextDelta {
                    turn_id: "t".into(),
                    text: "answer".into(),
                },
            ),
        );
        reduce_event(
            &mut state,
            event(
                3,
                TurnEvent::ToolStarted {
                    turn_id: "t".into(),
                    tool_call_id: "call".into(),
                    tool: "shell".into(),
                    input: Some(serde_json::json!({"command": "pwd"})),
                },
            ),
        );
        reduce_event(
            &mut state,
            event(
                4,
                TurnEvent::ToolStatus {
                    turn_id: "t".into(),
                    tool_call_id: "call".into(),
                    status: tau_proto::prelude::ToolStatusValue::Running,
                    metadata: Some(serde_json::json!({"pid": 42})),
                },
            ),
        );
        reduce_event(
            &mut state,
            event(
                5,
                TurnEvent::ToolCompleted {
                    turn_id: "t".into(),
                    tool_call_id: "call".into(),
                    output: Some(BoundedOutput {
                        inline: "ok".into(),
                        truncated: false,
                        artifacts: vec![],
                    }),
                },
            ),
        );

        assert_eq!(state.raw_events.len(), 5);
        assert_eq!(state.transcript[state.assistant_index.unwrap()], "answer");
        assert_eq!(state.tools.len(), 1);
        assert_eq!(state.tools[0].status, crate::state::ToolStatus::Complete);
        assert_eq!(state.tools[0].result, "ok");
        assert_eq!(state.tool_call_ids, vec![Some("call".into())]);
    }
}
