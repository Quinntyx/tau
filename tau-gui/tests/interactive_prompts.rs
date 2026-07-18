//! Wire-level coverage for the public tau-client interactive-policy API.
//!
//! The small websocket peer below deliberately speaks only the subset needed by
//! this test.  This keeps the test independent of a running tau daemon while
//! still checking the actual JSON-RPC envelopes emitted by Client.

use futures_util::StreamExt;
use tau_client::{Client, PolicyEvent};
use tau_proto::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

async fn frame(stream: &mut tokio::net::UnixStream) -> String {
    let mut h = [0; 2];
    stream.read_exact(&mut h).await.unwrap();
    assert_eq!(h[0] & 0x0f, 1);
    let masked = h[1] & 0x80 != 0;
    let mut len = (h[1] & 0x7f) as usize;
    if len == 126 {
        let mut b = [0; 2];
        stream.read_exact(&mut b).await.unwrap();
        len = u16::from_be_bytes(b) as usize;
    }
    let mut mask = [0; 4];
    if masked {
        stream.read_exact(&mut mask).await.unwrap();
    }
    let mut body = vec![0; len];
    stream.read_exact(&mut body).await.unwrap();
    if masked {
        for (i, b) in body.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    String::from_utf8(body).unwrap()
}

async fn text(stream: &mut tokio::net::UnixStream, value: &str) {
    let bytes = value.as_bytes();
    assert!(bytes.len() < 126);
    stream.write_all(&[0x81, bytes.len() as u8]).await.unwrap();
    stream.write_all(bytes).await.unwrap();
}

#[tokio::test]
async fn policy_requests_notifications_replies_and_ids_are_wire_visible() {
    let path = std::env::temp_dir().join(format!("tau-policy-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut b = [0; 1];
            socket.read_exact(&mut b).await.unwrap();
            request.push(b[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        socket.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: x\r\n\r\n").await.unwrap();
        let notifications = [
            (
                METHOD_PERMISSION_REQUEST,
                r#"{"request_id":"p1","session_id":"s","turn_id":"t","tool":"shell","arguments":{},"initiating_client_id":"c"}"#,
            ),
            (
                METHOD_QUESTION_REQUEST,
                r#"{"request_id":"q1","session_id":"s","turn_id":"t","question":"why","options":[],"initiating_client_id":"c"}"#,
            ),
            (
                METHOD_DIFF_REQUEST,
                r#"{"request_id":"d1","session_id":"s","turn_id":"t","files":[],"initiating_client_id":"c"}"#,
            ),
            (
                METHOD_PLAN_REQUEST,
                r#"{"request_id":"n1","session_id":"s","revision":1,"action":{"type":"read"},"actor":"human"}"#,
            ),
            (
                METHOD_AIRTIGHT_REQUEST,
                r#"{"request_id":"a1","session_id":"s","plan_id":"p","revision":1,"step":0,"initiating_client_id":"c"}"#,
            ),
        ];
        for (method, params) in notifications {
            text(
                &mut socket,
                &format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params}}}"#),
            )
            .await;
            let wire = frame(&mut socket).await;
            assert!(wire.contains("idempotency_key"));
            assert!(wire.contains("request_id"));
            let id = wire
                .split("\"id\":")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap();
            text(
                &mut socket,
                &format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"ack":true}}}}"#),
            )
            .await;
        }
    });

    let client = Client::connect(&path).await.unwrap();
    let mut events = client.policy_events();
    let p = events.next().await.unwrap();
    assert!(matches!(p, PolicyEvent::Permission(_)));
    client
        .permission_reply(PermissionReply {
            request_id: "p1".into(),
            idempotency_key: "k1".into(),
            choice: PermissionChoice::Reject,
            scope: PermissionScope::Once,
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    assert!(matches!(
        events.next().await.unwrap(),
        PolicyEvent::Question(_)
    ));
    client
        .question_reply(QuestionReply {
            request_id: "q1".into(),
            idempotency_key: "k2".into(),
            answer: "retry".into(),
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    assert!(matches!(events.next().await.unwrap(), PolicyEvent::Diff(_)));
    client
        .diff_reply(DiffReply {
            request_id: "d1".into(),
            idempotency_key: "k3".into(),
            accepted: false,
            decisions: vec![],
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    assert!(matches!(events.next().await.unwrap(), PolicyEvent::Plan(_)));
    client
        .plan_reply(PlanReply {
            request_id: "n1".into(),
            idempotency_key: "k4".into(),
            accepted: true,
            revision: 1,
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    assert!(matches!(
        events.next().await.unwrap(),
        PolicyEvent::Airtight(_)
    ));
    client
        .airtight_reply(AirtightPromptReply {
            request_id: "a1".into(),
            idempotency_key: "k5".into(),
            granted: false,
            actor: PromptActor::Human,
        })
        .await
        .unwrap();
    server.await.unwrap();
    let _ = std::fs::remove_file(path);
}
