//! Focused, credential-free coverage for the public typed-turn/runtime seams.
//!
//! The model executor is intentionally not faked here: it is private and its
//! provider is selected from configuration/credentials.  These tests therefore
//! pin the deterministic admission, cancellation, and durable-event contracts
//! that surround it.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tau_proto::prelude::*;
use tau_server::runtime::{CancellationAuthority, EventLog, SessionTurnQueue, WaitError};

#[test]
fn typed_turn_start_is_distinct_from_legacy_completion_stream() {
    assert_eq!(METHOD_TURN_START, "session.turn.start");
    assert_ne!(METHOD_TURN_START, METHOD_COMPLETION_STREAM);

    let params = TurnStartParams {
        project_id: "project-test".into(),
        model: "mock/model".into(),
        prompt: "deterministic prompt".into(),
        session_id: None,
        cwd: Some("/tmp".into()),
        idempotency_key: IdempotencyKey::new("turn-1"),
        agent: None,
        task_tier: None,
        autonomous: Some(false),
        action: Some(RequestAction::Submit),
    };
    let encoded = serde_json::to_value(&params).unwrap();
    assert_eq!(encoded["idempotency_key"], "turn-1");
    assert!(encoded.get("completion_stream").is_none());
}

#[tokio::test]
async fn session_queue_is_fifo_and_never_runs_two_turns_at_once() {
    let queue = Arc::new(SessionTurnQueue::new());
    let (first_id, first_done) = queue.submit("first".to_owned());
    let (second_id, second_done) = queue.submit("second".to_owned());
    assert_eq!((first_id, second_id), (1, 2));

    let active = Arc::clone(&queue);
    let active_count = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let count = Arc::clone(&active_count);
    let max = Arc::clone(&peak);
    let values = Arc::clone(&seen);
    let worker = tokio::spawn(async move {
        active
            .run_next(|value| async move {
                values.lock().unwrap().push(value);
                let now = count.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                count.fetch_sub(1, Ordering::SeqCst);
            })
            .await;
    });
    worker.await.unwrap();
    queue
        .run_next(|value| async {
            seen.lock().unwrap().push(value);
        })
        .await;

    assert_eq!(*seen.lock().unwrap(), ["first", "second"]);
    assert_eq!(peak.load(Ordering::SeqCst), 1);
    assert_eq!(first_done.await.unwrap(), Ok(()));
    assert_eq!(second_done.await.unwrap(), Ok(()));
}

#[test]
fn durable_event_log_replays_in_sequence_and_retains_only_durable_window() {
    let log = EventLog::new(3);
    assert_eq!(log.append("turn.started"), 1);
    assert_eq!(log.append("tool.output"), 2);
    assert_eq!(log.append("turn.completed"), 3);
    assert_eq!(
        log.replay_since(1),
        vec![(2, "tool.output"), (3, "turn.completed")]
    );
    assert_eq!(log.replay_since(3), Vec::<(u64, &str)>::new());
}

#[tokio::test]
async fn cancellation_is_wakeable_and_terminally_observable() {
    let authority = CancellationAuthority::new();
    let handle = authority.register("turn-42");
    let waiter = handle.clone();
    let task = tokio::spawn(async move {
        waiter.cancelled().await;
    });
    assert!(authority.cancel("turn-42"));
    task.await.unwrap();
    assert!(handle.is_cancelled());
    assert!(!authority.cancel("unknown"));

    let queue = SessionTurnQueue::new();
    let (_, done) = queue.submit("cancelled");
    let active = queue.next().await;
    active.finish(Err(WaitError::Cancelled));
    assert_eq!(done.await.unwrap(), Err(WaitError::Cancelled));
}
