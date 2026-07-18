//! Functional acceptance coverage for the GUI's public backend seam.
//!
//! The GUI view is intentionally private, so these tests exercise the same
//! typed boundary used by `TauView`: `Backend` and `tau_client` protocol
//! values.  The scripted daemon below is deliberately typed; it documents the
//! state machine the GUI consumes without making assertions about wire JSON.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tau_client::TurnStreamEvent;
use tau_gui::backend::{Backend, DaemonAction};
use tau_gui::chat::{ChatAction, ChatState, PermissionChoice};
use tau_gui::picker::{fuzzy_models, next_agent};
use tau_proto::prelude::*;
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

#[derive(Clone, Copy)]
enum Script {
    Complete,
    Hold,
    Incompatible,
    Stale,
}

#[derive(Default, Clone)]
struct Requests {
    methods: Vec<String>,
    starts: Vec<TurnStartParams>,
    cancels: Vec<TurnCancelParams>,
    replays: Vec<TurnReplayParams>,
    responses: Vec<TurnResponseParams>,
}

struct ScriptedDaemon {
    socket: PathBuf,
    requests: Arc<Mutex<Requests>>,
    task: tokio::task::JoinHandle<()>,
}

impl ScriptedDaemon {
    fn start(runtime: &tokio::runtime::Runtime, script: Script) -> Self {
        let socket = PathBuf::from(format!(
            "/tmp/tau-gui-scripted-{}-{}.sock",
            std::process::id(),
            unique_id()
        ));
        let listener = {
            let _entered = runtime.enter();
            UnixListener::bind(&socket).expect("bind scripted daemon socket")
        };
        let requests = Arc::new(Mutex::new(Requests::default()));
        let state = Arc::clone(&requests);
        let task = runtime.spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let state = Arc::clone(&state);
                tokio::spawn(handle_connection(stream, script, state));
            }
        });
        Self {
            socket,
            requests,
            task,
        }
    }

    fn requests(&self) -> Requests {
        self.requests.lock().expect("script recorder").clone()
    }
}

impl Drop for ScriptedDaemon {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn unique_id() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    script: Script,
    requests: Arc<Mutex<Requests>>,
) {
    let Ok(mut socket) = accept_async(stream).await else {
        return;
    };
    while let Some(Ok(Message::Text(text))) = socket.next().await {
        let Ok(request) = serde_json::from_str::<Request>(text.as_ref()) else {
            continue;
        };
        if let Ok(mut recorded) = requests.lock() {
            recorded.methods.push(request.method.clone());
        }
        if matches!(script, Script::Stale) {
            continue;
        }
        let response = match request.method.as_str() {
            METHOD_PROTOCOL_NEGOTIATE => {
                if matches!(script, Script::Incompatible) {
                    Response::<serde_json::Value>::err(request.id, -32602, "unsupported protocol")
                } else {
                    ok_response(
                        request.id,
                        ProtocolNegotiateResult {
                            version: ProtocolVersion { major: 1, minor: 0 },
                            capabilities: vec![
                                Capability::TurnStreaming,
                                Capability::TurnCancellation,
                                Capability::EventReplay,
                                Capability::Idempotency,
                                Capability::ArtifactReferences,
                            ],
                        },
                    )
                }
            }
            METHOD_TURN_START => {
                let Some(params) = request
                    .params
                    .clone()
                    .and_then(|value| serde_json::from_value::<TurnStartParams>(value).ok())
                else {
                    return;
                };
                if let Ok(mut recorded) = requests.lock() {
                    recorded.starts.push(params);
                }
                let events = if matches!(script, Script::Hold) {
                    vec![event(
                        request.id.clone(),
                        1,
                        TurnEvent::TurnStarted {
                            turn_id: "turn-1".into(),
                        },
                    )]
                } else {
                    vec![
                        event(
                            request.id.clone(),
                            1,
                            TurnEvent::TurnStarted {
                                turn_id: "turn-1".into(),
                            },
                        ),
                        event(
                            request.id.clone(),
                            2,
                            TurnEvent::TextDelta {
                                turn_id: "turn-1".into(),
                                text: "hello".into(),
                            },
                        ),
                        event(
                            request.id.clone(),
                            3,
                            TurnEvent::PermissionRequested {
                                turn_id: "turn-1".into(),
                                request_id: "permission-1".into(),
                                tool: "shell".into(),
                                description: "run".into(),
                            },
                        ),
                        event(
                            request.id.clone(),
                            4,
                            TurnEvent::QuestionAsked {
                                turn_id: "turn-1".into(),
                                question_id: "question-1".into(),
                                question: "continue?".into(),
                            },
                        ),
                        event(
                            request.id.clone(),
                            5,
                            TurnEvent::DiffRequested {
                                turn_id: "turn-1".into(),
                                request_id: "diff-1".into(),
                                path: "src/lib.rs".into(),
                                diff: "@@".into(),
                            },
                        ),
                        event(
                            request.id.clone(),
                            6,
                            TurnEvent::TurnCompleted {
                                turn_id: "turn-1".into(),
                                message_id: Some(1),
                            },
                        ),
                    ]
                };
                for value in events {
                    let notification = Notification {
                        jsonrpc: JsonRpc::default(),
                        method: METHOD_TURN_EVENT.into(),
                        params: Some(value),
                    };
                    let Ok(text) = serde_json::to_string(&notification) else {
                        return;
                    };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                if matches!(script, Script::Hold) {
                    continue;
                }
                ok_response(
                    request.id,
                    TurnStartResult {
                        session_id: "session-1".into(),
                        turn_id: "turn-1".into(),
                    },
                )
            }
            METHOD_TURN_CANCEL => {
                let Some(params) = request
                    .params
                    .clone()
                    .and_then(|value| serde_json::from_value::<TurnCancelParams>(value).ok())
                else {
                    return;
                };
                if let Ok(mut recorded) = requests.lock() {
                    recorded.cancels.push(params.clone());
                }
                ok_response(
                    request.id,
                    TurnCancelResult {
                        session_id: params.session_id,
                        turn_id: params.turn_id,
                        cancelled: true,
                    },
                )
            }
            METHOD_TURN_REPLAY => {
                let Some(params) = request
                    .params
                    .clone()
                    .and_then(|value| serde_json::from_value::<TurnReplayParams>(value).ok())
                else {
                    return;
                };
                if let Ok(mut recorded) = requests.lock() {
                    recorded.replays.push(params.clone());
                }
                ok_response(
                    request.id,
                    TurnReplayResult {
                        events: vec![],
                        next_sequence: Some(params.after_sequence),
                        gap: false,
                    },
                )
            }
            METHOD_TURN_RESPONSE => {
                let Some(params) = request
                    .params
                    .clone()
                    .and_then(|value| serde_json::from_value::<TurnResponseParams>(value).ok())
                else {
                    return;
                };
                if let Ok(mut recorded) = requests.lock() {
                    recorded.responses.push(params.clone());
                }
                ok_response(
                    request.id,
                    TurnResponseResult {
                        session_id: params.session_id,
                        turn_id: params.turn_id,
                        accepted: true,
                    },
                )
            }
            _ => Response::<serde_json::Value>::err(request.id, -32601, "not scripted"),
        };
        let Ok(text) = serde_json::to_string(&response) else {
            return;
        };
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
}

fn event(request_id: Id, sequence: u64, value: TurnEvent) -> SequencedEvent {
    SequencedEvent {
        event_id: format!("event-{sequence}"),
        session_id: "session-1".into(),
        sequence,
        occurred_at: 0,
        request_id: Some(request_id),
        event: value,
    }
}

fn ok_response<T: serde::Serialize>(id: Id, result: T) -> Response<serde_json::Value> {
    Response::ok(
        id,
        serde_json::to_value(result).expect("script result serializes"),
    )
}

fn backend(socket: PathBuf) -> (Backend, tokio::runtime::Runtime) {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let backend = Backend::from_parts(
        socket,
        runtime.handle().clone(),
        None,
        "/tmp/gui-functional-acceptance".into(),
        "acceptance/model".into(),
    );
    (backend, runtime)
}

#[test]
fn backend_exposes_model_and_working_directory() {
    let (backend, _runtime) = backend(PathBuf::from("/tmp/tau-functional-no-daemon.sock"));
    assert_eq!(backend.model(), "acceptance/model");
    assert_eq!(backend.cwd(), "/tmp/gui-functional-acceptance");
    assert!(
        !backend.auto_started(),
        "a pre-existing daemon must not warn"
    );
}

#[test]
fn stale_or_missing_daemon_is_reported_through_typed_turn_stream() {
    let (backend, runtime) = backend(PathBuf::from(format!(
        "/tmp/tau-functional-stale-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    )));
    let mut stream = backend.turn_with_options(
        "negotiate stale daemon".into(),
        None,
        Some("Code".into()),
        Some("acceptance/model".into()),
    );
    let result = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), stream.recv())
            .await
            .expect("backend should answer promptly")
            .expect("backend should return an error item")
    });
    assert!(
        result.is_err(),
        "unreachable daemon must not fabricate events"
    );
}

#[test]
fn cancel_is_safe_before_turn_started() {
    let (backend, _runtime) = backend(PathBuf::from("/tmp/tau-functional-cancel.sock"));
    backend.cancel();
}

#[test]
fn scripted_daemon_covers_submit_stream_interaction_and_completion() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let daemon = ScriptedDaemon::start(&runtime, Script::Complete);
    let backend = Backend::from_parts(
        daemon.socket.clone(),
        runtime.handle().clone(),
        None,
        "/tmp/gui-functional-acceptance".into(),
        "acceptance/model".into(),
    );
    let mut stream = backend.turn_with_options(
        "hello".into(),
        None,
        Some("Code".into()),
        Some("acceptance/model".into()),
    );
    let events = runtime.block_on(async {
        let mut events = Vec::new();
        while let Some(event) = tokio::time::timeout(Duration::from_secs(2), stream.recv())
            .await
            .expect("scripted daemon response")
        {
            let done = matches!(event, Ok(TurnStreamEvent::Complete(_)) | Err(_));
            events.push(event);
            if done {
                break;
            }
        }
        events
    });
    assert!(events.iter().any(|value| matches!(
        value,
        Ok(TurnStreamEvent::Event(SequencedEvent {
            event: TurnEvent::TextDelta { text, .. },
            ..
        })) if text == "hello"
    )));
    assert!(events.iter().any(|value| matches!(
        value,
        Ok(TurnStreamEvent::Event(SequencedEvent {
            event: TurnEvent::TurnCompleted { .. },
            ..
        }))
    )));
    let recorded = daemon.requests();
    assert_eq!(recorded.starts[0].model, "acceptance/model");
    assert_eq!(recorded.starts[0].agent.as_deref(), Some("Code"));
    assert!(recorded.methods.contains(&METHOD_PROTOCOL_NEGOTIATE.into()));
    assert!(recorded.methods.contains(&METHOD_TURN_START.into()));
}

#[test]
fn scripted_daemon_replies_and_replay_use_typed_rpc_requests() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let daemon = ScriptedDaemon::start(&runtime, Script::Complete);
    let backend = Backend::from_parts(
        daemon.socket.clone(),
        runtime.handle().clone(),
        None,
        "/tmp/gui-functional-acceptance".into(),
        "acceptance/model".into(),
    );
    let mut stream = backend.turn("hello".into(), None);
    runtime.block_on(async {
        while let Some(item) = stream.recv().await {
            if matches!(item, Ok(TurnStreamEvent::Complete(_)) | Err(_)) {
                break;
            }
        }
    });
    for response in [
        backend.permission_reply(
            "session-1".into(),
            "turn-1".into(),
            "permission-1".into(),
            TurnPermissionChoice::AllowOnce,
        ),
        backend.question_reply("session-1".into(), "turn-1".into(), "yes".into()),
        backend.diff_reply(
            "session-1".into(),
            "turn-1".into(),
            0,
            TurnDiffDecision::Approve,
        ),
    ] {
        assert!(runtime.block_on(response).expect("reply result").is_ok());
    }
    assert!(
        runtime
            .block_on(backend.replay("session-1".into(), 5))
            .expect("replay result")
            .is_ok()
    );
    let recorded = daemon.requests();
    assert_eq!(recorded.responses.len(), 3);
    assert!(
        recorded
            .responses
            .iter()
            .any(|request| matches!(request.response, ClientResponse::Permission { .. }))
    );
    assert!(recorded.responses.iter().any(|request| matches!(
        request.response,
        ClientResponse::Question { ref answer } if answer == "yes"
    )));
    assert!(recorded.responses.iter().any(|request| matches!(
        request.response,
        ClientResponse::DiffHunk { approved: true, .. }
    )));
    assert_eq!(recorded.replays[0].after_sequence, 5);
}

#[test]
fn typed_view_actions_apply_prompt_and_diff_replies_to_streamed_cards() {
    let mut chat = ChatState::default();
    assert!(chat.reduce(ChatAction::Submit("hello".into())));
    for (sequence, value) in [
        (
            1,
            TurnEvent::TurnStarted {
                turn_id: "turn-1".into(),
            },
        ),
        (
            2,
            TurnEvent::PermissionRequested {
                turn_id: "turn-1".into(),
                request_id: "permission-1".into(),
                tool: "shell".into(),
                description: "run".into(),
            },
        ),
        (
            3,
            TurnEvent::QuestionAsked {
                turn_id: "turn-1".into(),
                question_id: "question-1".into(),
                question: "continue?".into(),
            },
        ),
        (
            4,
            TurnEvent::DiffRequested {
                turn_id: "turn-1".into(),
                request_id: "diff-1".into(),
                path: "src/lib.rs".into(),
                diff: "@@".into(),
            },
        ),
    ] {
        assert!(chat.reduce(ChatAction::SessionEvent(event(
            Id::num(99),
            sequence,
            value,
        ))));
    }
    assert!(chat.reduce(ChatAction::Permission(2, PermissionChoice::AllowOnce)));
    assert!(chat.reduce(ChatAction::AnswerQuestion(2, "yes".into())));
    assert!(chat.reduce(ChatAction::ApproveDiff(3, true)));
    assert!(chat.cards.iter().any(|card| matches!(
        card,
        tau_gui::chat::Card::Question { answer: Some(answer), .. } if answer == "yes"
    )));
    assert!(
        chat.cards
            .iter()
            .any(|card| matches!(card, tau_gui::chat::Card::Diff { approved: true, .. }))
    );
}

#[test]
fn model_and_agent_pickers_are_wired_to_typed_options() {
    let models = vec![
        tau_gui::picker::ModelOption {
            id: "gpt".into(),
            provider: "openai".into(),
            recent: false,
            favorite: false,
        },
        tau_gui::picker::ModelOption {
            id: "claude".into(),
            provider: "anthropic".into(),
            recent: false,
            favorite: false,
        },
    ];
    assert_eq!(fuzzy_models(&models, "cla"), vec![models[1].clone()]);
    let agents = vec![
        tau_gui::picker::AgentOption {
            name: "Plan".into(),
            in_tab_cycle: true,
        },
        tau_gui::picker::AgentOption {
            name: "Code".into(),
            in_tab_cycle: true,
        },
    ];
    assert_eq!(
        next_agent(&agents, Some("Plan"), false).as_deref(),
        Some("Code")
    );
    let request = tau_gui::view::TauView::turn_request_for(
        "hello",
        Some("session-1".into()),
        tau_gui::view::AgentMode::Code,
        Some("claude".into()),
    );
    assert_eq!(request.agent, "Code");
    assert_eq!(request.model.as_deref(), Some("claude"));
}

#[test]
fn scripted_daemon_cancel_is_a_real_typed_state_transition() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let daemon = ScriptedDaemon::start(&runtime, Script::Hold);
    let backend = Backend::from_parts(
        daemon.socket.clone(),
        runtime.handle().clone(),
        None,
        "/tmp/gui-functional-acceptance".into(),
        "acceptance/model".into(),
    );
    let mut stream = backend.turn("hold".into(), None);
    let started = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(2), stream.recv()).await })
        .expect("started timeout")
        .expect("started");
    assert!(matches!(
        started,
        Ok(TurnStreamEvent::Event(SequencedEvent {
            event: TurnEvent::TurnStarted { .. },
            ..
        }))
    ));
    backend.cancel();
    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
    assert_eq!(daemon.requests().cancels.len(), 1);
}

#[test]
fn scripted_daemon_negotiation_rejects_incompatible_and_stale_servers() {
    for script in [Script::Incompatible, Script::Stale] {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let daemon = ScriptedDaemon::start(&runtime, script);
        let backend = Backend::from_parts(
            daemon.socket.clone(),
            runtime.handle().clone(),
            None,
            "/tmp/gui-functional-acceptance".into(),
            "acceptance/model".into(),
        );
        let mut stream = backend.turn("negotiate".into(), None);
        let result = runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(2), stream.recv()).await });
        assert!(result.is_ok(), "stale/incompatible server must answer");
        assert!(result.expect("timeout").expect("stream item").is_err());
    }
}

#[test]
fn replay_reports_unreachable_daemon_without_panicking() {
    let socket = PathBuf::from(format!(
        "/tmp/tau-functional-replay-{}-{}.sock",
        std::process::id(),
        unique_id()
    ));
    let _ = std::fs::remove_file(&socket);
    let (backend, runtime) = backend(socket);
    let result = runtime.block_on(async { backend.replay("session".into(), 0).await });
    assert!(result.expect("replay task should answer").is_err());
}

#[test]
fn ownership_warning_actions_are_public_and_idempotent() {
    let (backend, _runtime) = backend(PathBuf::from("/tmp/tau-functional-owner.sock"));
    for action in [
        DaemonAction::Okay,
        DaemonAction::Quit,
        DaemonAction::Disown,
        // Preference-writing actions are covered by the production seam's
        // unit tests; acceptance tests must not alter the user's config.
    ] {
        backend.daemon_action(action);
    }
}
