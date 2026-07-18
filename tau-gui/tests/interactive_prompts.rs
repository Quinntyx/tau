//! Wire-level coverage for the GUI's typed interactive prompt client.
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tau_client::{Client, PolicyEvent};
use tau_gui::chat::{Card, ChatAction, ChatState, ChatStatus};
use tau_proto::prelude::*;
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

fn socket_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "tau-gui-prompts-{}-{}.sock",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ))
}

async fn next_request(
    server: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Value {
    loop {
        if let Some(Ok(Message::Text(text))) = server.next().await {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value.get("id").is_some() {
                return value;
            }
        }
    }
}

async fn acknowledge(
    server: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    request: &Value,
) {
    server
        .send(Message::Text(
            json!({"jsonrpc":"2.0", "id":request["id"], "result":{"ack":true}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn typed_prompt_replies_emit_stable_keys_and_receive_ack() {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(socket).await.unwrap();
        let expected = [
            METHOD_PERMISSION_REPLY,
            METHOD_QUESTION_REPLY,
            METHOD_DIFF_DECISION,
            METHOD_PLAN_REPLY,
            METHOD_AIRTIGHT_REPLY,
        ];
        for (index, method) in expected.into_iter().enumerate() {
            let request = next_request(&mut ws).await;
            assert_eq!(request["method"], method);
            let expected_request_id = ["p", "q", "d", "n", "a"][index];
            assert_eq!(request["params"]["request_id"], expected_request_id);
            assert_eq!(
                request["params"]["idempotency_key"],
                format!(
                    "gui-policy-{}-{}",
                    ["permission", "question", "diff", "plan", "airtight"][index],
                    expected_request_id
                )
            );
            if index == 0 {
                ws.send(Message::Text(
                    json!({
                        "jsonrpc":"2.0",
                        "method":METHOD_PERMISSION_REQUEST,
                        "params":{
                            "request_id":"p",
                            "session_id":"s",
                            "turn_id":"t",
                            "tool":"shell",
                            "arguments":{},
                            "initiating_client_id":"gui"
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            }
            acknowledge(&mut ws, &request).await;
        }
    });

    let client = Client::connect(&path).await.unwrap();
    let mut events = client.policy_events();
    client
        .permission_reply(PermissionReply {
            request_id: "p".into(),
            idempotency_key: "gui-policy-permission-p".into(),
            choice: PermissionChoice::Reject,
            scope: PermissionScope::Once,
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    assert!(matches!(
        events.next().await,
        Some(PolicyEvent::Permission(_))
    ));
    client
        .question_reply(QuestionReply {
            request_id: "q".into(),
            idempotency_key: "gui-policy-question-q".into(),
            answer: "yes".into(),
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    client
        .diff_reply(DiffReply {
            request_id: "d".into(),
            idempotency_key: "gui-policy-diff-d".into(),
            accepted: false,
            decisions: vec![],
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    client
        .plan_reply(PlanReply {
            request_id: "n".into(),
            idempotency_key: "gui-policy-plan-n".into(),
            accepted: true,
            revision: 2,
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    client
        .airtight_reply(AirtightPromptReply {
            request_id: "a".into(),
            idempotency_key: "gui-policy-airtight-a".into(),
            granted: false,
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    server.await.unwrap();
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn typed_client_surfaces_daemon_errors_for_retry() {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(socket).await.unwrap();
        let request = next_request(&mut ws).await;
        ws.send(Message::Text(
            json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-32001,"message":"not the initiating client"}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    });
    let client = Client::connect(&path).await.unwrap();
    let error = client
        .question_reply(QuestionReply {
            request_id: "q".into(),
            idempotency_key: "retry-q".into(),
            answer: "yes".into(),
            actor: PromptActor::Human,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not the initiating client"));
    server.await.unwrap();
    let _ = std::fs::remove_file(path);
}

#[test]
fn gui_state_waits_for_ack_and_keeps_errors_retryable() {
    let mut state = ChatState::default();
    state.cards.push(Card::Permission {
        request_id: "p".into(),
        tool: "shell".into(),
        description: "run".into(),
    });
    assert!(state.reduce(ChatAction::Permission(
        0,
        tau_gui::chat::PermissionChoice::Reject,
    )));
    assert!(matches!(state.cards[0], Card::Permission { .. }));
    assert!(state.reduce(ChatAction::PolicyError {
        request_id: "p".into(),
        message: "retryable daemon error".into(),
    }));
    assert!(matches!(
        state.policy_states["p"],
        tau_gui::chat::PolicyState::Error(_)
    ));
    assert!(state.reduce(ChatAction::RetryPolicy {
        request_id: "p".into()
    }));
    assert!(matches!(
        state.policy_states["p"],
        tau_gui::chat::PolicyState::Pending
    ));
    assert!(state.reduce(ChatAction::PolicyAck {
        request_id: "p".into()
    }));
    assert!(state.cards.is_empty());
    assert_eq!(state.status, ChatStatus::Ready);
}

#[test]
fn permission_decision_model_contains_all_five_choices() {
    use tau_gui::chat::PermissionChoice;

    let choices = [
        PermissionChoice::AllowOnce,
        PermissionChoice::AllowAlways,
        PermissionChoice::Reject,
        PermissionChoice::Inspect,
        PermissionChoice::Cancel,
    ];
    assert_eq!(choices.len(), 5);
    assert!(choices.contains(&PermissionChoice::AllowOnce));
    assert!(choices.contains(&PermissionChoice::AllowAlways));
    assert!(choices.contains(&PermissionChoice::Reject));
    assert!(choices.contains(&PermissionChoice::Inspect));
    assert!(choices.contains(&PermissionChoice::Cancel));
}

#[test]
fn gui_state_tracks_diff_decisions_without_mutating_rendered_card() {
    let mut state = ChatState::default();
    state.cards.push(Card::Diff {
        request_id: "d".into(),
        path: "src/lib.rs".into(),
        patch: "+new".into(),
        approved: false,
    });

    assert!(state.reduce(ChatAction::DiffHunk {
        request_id: "d".into(),
        hunk: 2,
        approved: true,
    }));
    assert_eq!(
        state.diff_decisions["d"],
        vec![tau_gui::chat::DiffDecision::Hunk {
            hunk: 2,
            approved: true,
        }]
    );
    assert!(matches!(
        state.cards[0],
        Card::Diff {
            approved: false,
            ..
        }
    ));

    assert!(state.reduce(ChatAction::DiffWholeFile {
        request_id: "d".into(),
        approved: false,
    }));
    assert_eq!(state.diff_decisions["d"].len(), 2);
}
