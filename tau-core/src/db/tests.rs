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
fn policy_decisions_round_trip_by_scope_and_replace_idempotently() {
    let db = test_db();
    let project = db.create_project("policy", "/tmp/policy").unwrap();
    let session = db.create_session(&project.id).unwrap();
    let first = db
        .save_policy_decision(Some(&session.id), "session", "human", "read:*", "allow")
        .unwrap();
    let second = db
        .save_policy_decision(Some(&session.id), "session", "human", "read:*", "reject")
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.decision_json, "reject");
    assert_eq!(
        db.list_policy_decisions(Some(&session.id), "session")
            .unwrap()
            .len(),
        1
    );

    db.save_policy_decision(None, "global", "human", "network:*", "allow")
        .unwrap();
    assert_eq!(db.list_policy_decisions(None, "global").unwrap().len(), 1);
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
    let project = db.create_project("upgrade", "/tmp/upgrade").unwrap();
    let session = db.create_session(&project.id).unwrap();
    assert_eq!(db.replay_events(&session.id, 0, None).unwrap().len(), 0);
}

#[test]
fn context_epochs_are_append_only_and_reloaded() {
    let db = test_db();
    let project = db.create_project("project", "/tmp/project").unwrap();
    let session = db.create_session(&project.id).unwrap();
    let first = db
        .append_context_epoch(&ContextEpochRecord::new(&session.id, 0, "first", "manual"))
        .unwrap();
    let second = db
        .append_context_epoch(&ContextEpochRecord::new(
            &session.id,
            1,
            "second",
            "automatic",
        ))
        .unwrap();
    assert!(first.id < second.id);
    assert_eq!(db.context_epochs(&session.id).unwrap().len(), 2);
    assert_eq!(
        db.latest_context_epoch(&session.id)
            .unwrap()
            .unwrap()
            .summary,
        "second"
    );
}

#[test]
fn epoch_schema_has_restart_metadata_and_duplicate_append_is_atomic() {
    let db = test_db();
    let project = db.create_project("project", "/tmp/project").unwrap();
    let session = db.create_session(&project.id).unwrap();
    let columns: Vec<String> = {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(context_epochs)").unwrap();
        stmt.query_map([], |row| row.get(1))
            .unwrap()
            .collect::<std::result::Result<Vec<String>, _>>()
            .unwrap()
    };
    for name in [
        "parent_epoch",
        "estimated_tokens",
        "terminal_status",
        "is_baseline",
        "system_message",
    ] {
        assert!(
            columns.iter().any(|column| column == name),
            "missing {name}"
        );
    }
    let record = ContextEpochRecord::new(&session.id, 0, "baseline", "manual");
    db.append_context_epoch(&record).unwrap();
    assert!(db.append_context_epoch(&record).is_err());
    assert_eq!(db.context_epochs(&session.id).unwrap().len(), 1);
}

#[test]
fn session_create_and_get() {
    let db = test_db();
    let project = db.create_project("project", "/tmp/project").unwrap();
    let s = db.create_session(&project.id).unwrap();
    assert_eq!(s.cwd, project.root);
    assert_eq!(s.project_id, project.id);
    assert!(s.title.is_none());
    assert!(!s.id.is_empty());

    let fetched = db.get_session(&s.id).unwrap().expect("session exists");
    assert_eq!(fetched.cwd, "/tmp/project");
    assert_eq!(fetched.created_at, s.created_at);
}

#[test]
fn session_list() {
    let db = test_db();
    let parent = tempfile::tempdir().unwrap();
    let a = db.create_project("a", parent.path().join("a")).unwrap();
    let b = db.create_project("b", parent.path().join("b")).unwrap();
    db.create_session(&a.id).unwrap();
    db.create_session(&b.id).unwrap();
    let sessions = db.list_sessions().unwrap();
    assert_eq!(sessions.len(), 2);
}

#[test]
fn message_append_and_get() {
    let db = test_db();
    let project = db.create_project("proj", "/tmp/proj").unwrap();
    let s = db.create_session(&project.id).unwrap();
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
    let project = db.create_project("proj", "/tmp/proj").unwrap();
    let s = db.create_session(&project.id).unwrap();
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
    let project = db.create_project("proj", "/tmp/proj").unwrap();
    let s = db.create_session(&project.id).unwrap();
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
    let project = db.create_project("proj", "/tmp/proj").unwrap();
    let s = db.create_session(&project.id).unwrap();
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
    let project = db.create_project("proj", "/tmp/proj").unwrap();
    let s = db.create_session(&project.id).unwrap();
    let original_updated = s.updated_at;
    std::thread::sleep(std::time::Duration::from_millis(10));
    db.append_message(&s.id, "user", vec![ContentBlock::text("hi")])
        .unwrap();
    let fetched = db.get_session(&s.id).unwrap().unwrap();
    assert!(fetched.updated_at > original_updated);
}

#[test]
fn project_sessions_are_filtered_flat_newest_first_and_recoverable() {
    let db = test_db();
    let roots = tempfile::tempdir().unwrap();
    let alpha = ProjectId::new(
        db.create_project("alpha", roots.path().join("alpha"))
            .unwrap()
            .id,
    );
    let beta = ProjectId::new(
        db.create_project("beta", roots.path().join("beta"))
            .unwrap()
            .id,
    );
    let first = db.create_session_for_project(&alpha, "/alpha/one").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = db.create_session_for_project(&alpha, "/alpha/two").unwrap();
    let foreign = db.create_session_for_project(&beta, "/beta").unwrap();

    let visible = db.list_sessions_for_project(&alpha, false).unwrap();
    assert_eq!(
        visible.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec![second.id.as_str(), first.id.as_str()]
    );
    assert!(!visible.iter().any(|s| s.id == foreign.id));

    let archived = db
        .archive_session_for_project(&alpha, &first.id)
        .unwrap()
        .unwrap();
    assert!(archived.archived_at.is_some());
    assert_eq!(
        db.list_sessions_for_project(&alpha, false).unwrap().len(),
        1
    );
    assert_eq!(db.list_sessions_for_project(&alpha, true).unwrap().len(), 2);
    let restored = db
        .restore_session_for_project(&alpha, &first.id)
        .unwrap()
        .unwrap();
    assert!(restored.archived_at.is_none());
    assert!(
        db.get_session_record_for_project(&alpha, &first.id)
            .unwrap()
            .is_some()
    );
    assert!(
        db.get_session_record_for_project(&beta, &first.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn managed_session_creation_requires_a_project_id() {
    let db = test_db();
    assert!(
        db.create_session_for_project(&ProjectId::new(""), "/tmp/no-project")
            .is_err()
    );
}

#[test]
fn event_journal_sequences_replay_and_idempotency() {
    let db = test_db();
    let project = db.create_project("proj", "/tmp/proj").unwrap();
    let session = db.create_session(&project.id).unwrap();
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
    let project = db.create_project("proj", "/tmp/proj").unwrap();
    let session = db.create_session(&project.id).unwrap();
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

#[test]
fn project_registry_enforces_canonical_active_roots_and_lifecycle() {
    let db = test_db();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("created-once");
    let first = db.create_project("First", &root).unwrap();
    assert!(root.is_dir());
    assert_eq!(
        first.root,
        std::fs::canonicalize(&root).unwrap().to_string_lossy()
    );
    assert!(db.create_project("duplicate", &root).is_err());

    let session = db.create_session(&first.id).unwrap();
    db.unregister_project(&first.id).unwrap();
    assert!(db.create_session(&first.id).is_err());
    let second = db.new_project_id(&first.id).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(first.root, second.root);
    assert!(db.reactivate_project(&first.id).is_err());
    assert_eq!(
        db.get_session(&session.id).unwrap().unwrap().project_id,
        first.id
    );
    assert_eq!(db.list_projects(true).unwrap().len(), 2);
}

#[test]
fn project_paths_only_create_one_missing_final_directory() {
    let db = test_db();
    let parent = tempfile::tempdir().unwrap();
    let too_deep = parent.path().join("missing").join("child");
    assert!(db.create_project("deep", too_deep).is_err());

    let existing_file = parent.path().join("file");
    std::fs::write(&existing_file, "not a directory").unwrap();
    assert!(db.create_project("file", existing_file).is_err());
}

#[test]
fn v6_session_data_is_rebuilt_with_mandatory_active_project_fk() {
    let file = tempfile::NamedTempFile::new().unwrap();
    {
        let conn = rusqlite::Connection::open(file.path()).unwrap();
        for migration in [
            include_str!("sql/v1.sql"),
            include_str!("sql/v2.sql"),
            include_str!("sql/v3.sql"),
            include_str!("sql/v4.sql"),
            include_str!("sql/v5.sql"),
            include_str!("sql/v6.sql"),
        ] {
            conn.execute_batch(migration).unwrap();
        }
        conn.execute(
            "INSERT INTO projects (id,name,root,active,created_at,updated_at) VALUES ('p','p','/tmp',1,1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id,project_id,cwd,created_at,updated_at) VALUES ('valid','p','/tmp',1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id,project_id,cwd,created_at,updated_at) VALUES ('orphan',NULL,'/tmp',1,1)",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 6).unwrap();
    }

    let db = Db::open(file.path()).unwrap();
    let sessions = db.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "valid");
    assert_eq!(sessions[0].project_id, "p");
    assert!(db.create_session("missing").is_err());
}
