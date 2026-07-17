//! Integration tests for the persistence layer.

use super::*;

fn test_db() -> Db {
    Db::open_in_memory().expect("in-memory db")
}

#[test]
fn migration_idempotent() {
    let db = test_db();
    db.run_migrations().expect("re-migration is idempotent");
}

#[test]
fn v1_database_upgrades_forward_to_event_storage() {
    let file = tempfile::NamedTempFile::new().unwrap();
    {
        let conn = rusqlite::Connection::open(file.path()).unwrap();
        conn.execute_batch(include_str!("sql/v1.sql")).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let db = Db::open(file.path()).unwrap();
    let session = db.create_session("/tmp/upgrade").unwrap();
    assert_eq!(db.replay_events(&session.id, 0, None).unwrap().len(), 0);
}

#[test]
fn session_create_and_get() {
    let db = test_db();
    let s = db.create_session("/tmp/project").unwrap();
    assert_eq!(s.cwd, "/tmp/project");
    assert!(s.title.is_none());
    assert!(!s.id.is_empty());

    let fetched = db.get_session(&s.id).unwrap().expect("session exists");
    assert_eq!(fetched.cwd, "/tmp/project");
    assert_eq!(fetched.created_at, s.created_at);
}

#[test]
fn session_list() {
    let db = test_db();
    db.create_session("/a").unwrap();
    db.create_session("/b").unwrap();
    let sessions = db.list_sessions().unwrap();
    assert_eq!(sessions.len(), 2);
}

#[test]
fn message_append_and_get() {
    let db = test_db();
    let s = db.create_session("/tmp/proj").unwrap();
    let msg = db
        .append_message(&s.id, "user", vec![ContentBlock::text("hello world")])
        .unwrap();
    assert_eq!(msg.role, "user");
    assert_eq!(msg.seq, 1);
    assert_eq!(msg.blocks.len(), 1);

    let msg2 = db
        .append_message(&s.id, "assistant", vec![ContentBlock::text("hi there")])
        .unwrap();
    assert_eq!(msg2.seq, 2);

    let messages = db.get_messages(&s.id).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].blocks[0], ContentBlock::text("hello world"));
    assert_eq!(messages[1].blocks[0], ContentBlock::text("hi there"));
}

#[test]
fn tool_use_tool_result_round_trip() {
    let db = test_db();
    let s = db.create_session("/tmp/proj").unwrap();
    db.append_message(
        &s.id,
        "assistant",
        vec![ContentBlock::ToolUse {
            call_id: "call_1".into(),
            name: "read".into(),
            args_json: r#"{"path":"/tmp/f.txt"}"#.into(),
        }],
    )
    .unwrap();
    db.append_message(
        &s.id,
        "user",
        vec![ContentBlock::ToolResult {
            call_id: "call_1".into(),
            result_json: r#""file contents""#.into(),
            is_error: false,
        }],
    )
    .unwrap();
    let messages = db.get_messages(&s.id).unwrap();
    assert_eq!(messages.len(), 2);
    match &messages[0].blocks[0] {
        ContentBlock::ToolUse {
            name, args_json, ..
        } => {
            assert_eq!(name, "read");
            assert!(args_json.contains("f.txt"));
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
    match &messages[1].blocks[0] {
        ContentBlock::ToolResult {
            is_error,
            result_json,
            ..
        } => {
            assert!(!*is_error);
            assert!(result_json.contains("file contents"));
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn usage_round_trip() {
    let db = test_db();
    let s = db.create_session("/tmp/proj").unwrap();
    let u = db
        .record_usage(&s.id, None, "gpt-4o", 100, 50, Some(20))
        .unwrap();
    assert_eq!(u.model, "gpt-4o");
    assert_eq!(u.input_tokens, 100);
    assert_eq!(u.output_tokens, 50);
    assert_eq!(u.cached_tokens, Some(20));
}

#[test]
fn qa_record_round_trip() {
    let db = test_db();
    let s = db.create_session("/tmp/proj").unwrap();
    let qa = db.record_qa(&s.id, "which database?", "rusqlite").unwrap();
    assert_eq!(qa.question, "which database?");
    assert_eq!(qa.answer, "rusqlite");

    let records = db.get_qa_records(&s.id).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].answer, "rusqlite");
}

#[test]
fn session_updated_on_message_append() {
    let db = test_db();
    let s = db.create_session("/tmp/proj").unwrap();
    let original_updated = s.updated_at;
    std::thread::sleep(std::time::Duration::from_millis(10));
    db.append_message(&s.id, "user", vec![ContentBlock::text("hi")])
        .unwrap();
    let fetched = db.get_session(&s.id).unwrap().unwrap();
    assert!(fetched.updated_at > original_updated);
}

#[test]
fn event_journal_sequences_replay_and_idempotency() {
    let db = test_db();
    let session = db.create_session("/tmp/proj").unwrap();
    let event = tau_proto::turn::TurnEvent::TextDelta {
        turn_id: "t".into(),
        text: "hello".into(),
    };
    assert_eq!(
        db.append_event(&session.id, &event, None).unwrap().sequence,
        1
    );
    assert_eq!(
        db.append_event(&session.id, &event, None).unwrap().sequence,
        2
    );
    assert_eq!(db.replay_events(&session.id, 1, None).unwrap().len(), 1);
    let key = tau_proto::turn::IdempotencyKey::new("request-1");
    assert!(
        db.remember_idempotency(&session.id, &key, "hash", &"result")
            .unwrap()
    );
    assert!(
        !db.remember_idempotency(&session.id, &key, "hash", &"result")
            .unwrap()
    );
    assert_eq!(
        db.idempotent_result::<String>(&session.id, &key).unwrap(),
        Some("result".into())
    );
    assert!(
        db.remember_idempotency(&session.id, &key, "other", &"result")
            .is_err()
    );
}

#[test]
fn artifact_reference_round_trip() {
    let db = test_db();
    let session = db.create_session("/tmp/proj").unwrap();
    let reference = tau_proto::turn::ArtifactReference {
        artifact_id: "a".into(),
        media_type: "text/plain".into(),
        size_bytes: 4,
        content_hash: "sha".into(),
        storage_ref: "file:///a".into(),
    };
    db.create_artifact(&session.id, reference).unwrap();
    assert_eq!(
        db.list_artifacts(&session.id).unwrap()[0]
            .reference
            .artifact_id,
        "a"
    );
}
