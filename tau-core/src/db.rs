//! SQLite persistence layer: sessions, messages, usage, and Q&A records.
//!
//! Uses `rusqlite` (bundled SQLite) behind a single `Arc<Mutex<Connection>>`.
//! Schema is managed by `rusqlite_migration`. In the async daemon all calls go
//! through `spawn_blocking`; the `Db` itself is `Send + Sync`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── path helpers ──────────────────────────────────────────────────────────

/// Default database path: `~/.local/share/tau/tau.db` (XDG data dir).
/// Creates the parent directory if missing.
pub fn default_db_path() -> Result<PathBuf> {
    let base = dirs::data_dir().context("could not determine data directory")?;
    let dir = base.join("tau");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir.join("tau.db"))
}

// ── timestamp helper ──────────────────────────────────────────────────────

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── domain types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub seq: i64,
    pub created_at: i64,
    pub blocks: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "block_type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        call_id: String,
        name: String,
        args_json: String,
    },
    ToolResult {
        call_id: String,
        result_json: String,
        is_error: bool,
    },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub id: i64,
    pub session_id: String,
    pub message_id: Option<i64>,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaRecord {
    pub id: String,
    pub session_id: String,
    pub question: String,
    pub answer: String,
    pub created_at: i64,
}

// ── migrations ────────────────────────────────────────────────────────────

const MIGRATION_V1: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT PRIMARY KEY,
    cwd        TEXT NOT NULL,
    title      TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role       TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    seq        INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, seq)
);

CREATE TABLE IF NOT EXISTS content_blocks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id      INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL,
    block_type      TEXT NOT NULL CHECK (block_type IN ('text', 'tool_use', 'tool_result')),
    text            TEXT,
    tool_call_id    TEXT,
    tool_name       TEXT,
    tool_args_json  TEXT,
    tool_result_json TEXT,
    tool_is_error   INTEGER,
    UNIQUE(message_id, seq)
);

CREATE TABLE IF NOT EXISTS usage (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL REFERENCES sessions(id),
    message_id    INTEGER REFERENCES messages(id),
    model         TEXT NOT NULL,
    input_tokens  INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cached_tokens INTEGER,
    created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS qa_records (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    question   TEXT NOT NULL,
    answer     TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, seq);
CREATE INDEX IF NOT EXISTS idx_content_blocks_message ON content_blocks(message_id, seq);
CREATE INDEX IF NOT EXISTS idx_usage_session ON usage(session_id);
CREATE INDEX IF NOT EXISTS idx_qa_records_session ON qa_records(session_id);
";

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(MIGRATION_V1)])
}

// ── Db ────────────────────────────────────────────────────────────────────

/// Wrapper around a single SQLite connection (behind `Arc<Mutex>`).
/// Cloning is cheap — it clones the `Arc`, not the connection.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Open or create the database at `path`, then run migrations.
    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening database at {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    /// Open an in-memory database and run migrations. For tests.
    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory().context("opening in-memory database")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        migrations()
            .to_latest(&mut conn)
            .context("running database migrations")?;
        Ok(())
    }

    // ── sessions ──────────────────────────────────────────────────────

    pub fn create_session(&self, cwd: &str) -> Result<Session> {
        let now = now_ms();
        let session = Session {
            id: Uuid::new_v4().to_string(),
            cwd: cwd.to_string(),
            title: None,
            created_at: now,
            updated_at: now,
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (id, cwd, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                &session.id,
                &session.cwd,
                &session.title,
                session.created_at,
                session.updated_at,
            ],
        )?;
        Ok(session)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, cwd, title, created_at, updated_at FROM sessions ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Session {
                id: row.get(0)?,
                cwd: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        let mut sessions = Vec::new();
        for s in rows {
            sessions.push(s?);
        }
        Ok(sessions)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, cwd, title, created_at, updated_at FROM sessions WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], |row| {
            Ok(Session {
                id: row.get(0)?,
                cwd: row.get(1)?,
                title: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    // ── messages ──────────────────────────────────────────────────────

    pub fn append_message(
        &self,
        session_id: &str,
        role: &str,
        blocks: Vec<ContentBlock>,
    ) -> Result<Message> {
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .context("computing next message seq")?;
        tx.execute(
            "INSERT INTO messages (session_id, role, seq, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, role, seq, now],
        )?;
        let message_id = tx.last_insert_rowid();
        for (i, block) in blocks.iter().enumerate() {
            let block_seq = i as i64;
            insert_content_block(&tx, message_id, block_seq, block)?;
        }
        tx.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, session_id],
        )?;
        tx.commit()?;
        Ok(Message {
            id: message_id,
            session_id: session_id.to_string(),
            role: role.to_string(),
            seq,
            created_at: now,
            blocks,
        })
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, seq, created_at FROM messages WHERE session_id = ?1 ORDER BY seq",
        )?;
        let message_rows = stmt.query_map([session_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        let mut messages = Vec::new();
        for mr in message_rows {
            let (id, sid, role, seq, created_at) = mr?;
            let blocks = load_content_blocks(&conn, id)?;
            messages.push(Message {
                id,
                session_id: sid,
                role,
                seq,
                created_at,
                blocks,
            });
        }
        Ok(messages)
    }

    // ── usage ─────────────────────────────────────────────────────────

    pub fn record_usage(
        &self,
        session_id: &str,
        message_id: Option<i64>,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cached_tokens: Option<i64>,
    ) -> Result<Usage> {
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO usage (session_id, message_id, model, input_tokens, output_tokens, cached_tokens, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                session_id,
                message_id,
                model,
                input_tokens,
                output_tokens,
                cached_tokens,
                now,
            ],
        )?;
        Ok(Usage {
            id: conn.last_insert_rowid(),
            session_id: session_id.to_string(),
            message_id,
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cached_tokens,
            created_at: now,
        })
    }

    // ── Q&A ───────────────────────────────────────────────────────────

    pub fn record_qa(&self, session_id: &str, question: &str, answer: &str) -> Result<QaRecord> {
        let now = now_ms();
        let id = Uuid::new_v4().to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO qa_records (id, session_id, question, answer, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![&id, session_id, question, answer, now],
        )?;
        Ok(QaRecord {
            id,
            session_id: session_id.to_string(),
            question: question.to_string(),
            answer: answer.to_string(),
            created_at: now,
        })
    }

    pub fn get_qa_records(&self, session_id: &str) -> Result<Vec<QaRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, question, answer, created_at FROM qa_records WHERE session_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok(QaRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                question: row.get(2)?,
                answer: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut records = Vec::new();
        for r in rows {
            records.push(r?);
        }
        Ok(records)
    }
}

// ── content block helpers ─────────────────────────────────────────────────

fn insert_content_block(
    tx: &rusqlite::Transaction,
    message_id: i64,
    seq: i64,
    block: &ContentBlock,
) -> Result<()> {
    match block {
        ContentBlock::Text { text } => {
            tx.execute(
                "INSERT INTO content_blocks (message_id, seq, block_type, text) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![message_id, seq, "text", text],
            )?;
        }
        ContentBlock::ToolUse {
            call_id,
            name,
            args_json,
        } => {
            tx.execute(
                "INSERT INTO content_blocks (message_id, seq, block_type, tool_call_id, tool_name, tool_args_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![message_id, seq, "tool_use", call_id, name, args_json],
            )?;
        }
        ContentBlock::ToolResult {
            call_id,
            result_json,
            is_error,
        } => {
            tx.execute(
                "INSERT INTO content_blocks (message_id, seq, block_type, tool_call_id, tool_result_json, tool_is_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    message_id,
                    seq,
                    "tool_result",
                    call_id,
                    result_json,
                    *is_error as i64,
                ],
            )?;
        }
    }
    Ok(())
}

fn load_content_blocks(conn: &Connection, message_id: i64) -> Result<Vec<ContentBlock>> {
    let mut stmt = conn.prepare(
        "SELECT block_type, text, tool_call_id, tool_name, tool_args_json, tool_result_json, tool_is_error
         FROM content_blocks WHERE message_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map([message_id], |row| {
        let block_type: String = row.get(0)?;
        match block_type.as_str() {
            "text" => Ok(ContentBlock::Text { text: row.get(1)? }),
            "tool_use" => Ok(ContentBlock::ToolUse {
                call_id: row.get(2)?,
                name: row.get(3)?,
                args_json: row.get(4)?,
            }),
            "tool_result" => {
                let is_error_val: Option<i64> = row.get(6)?;
                Ok(ContentBlock::ToolResult {
                    call_id: row.get(2)?,
                    result_json: row.get(5)?,
                    is_error: is_error_val.unwrap_or(0) != 0,
                })
            }
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("unknown block_type: {other}").into(),
            )),
        }
    })?;
    let mut blocks = Vec::new();
    for b in rows {
        blocks.push(b?);
    }
    Ok(blocks)
}

// ── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        Db::open_in_memory().expect("in-memory db")
    }

    #[test]
    fn migration_idempotent() {
        let db = test_db();
        // Running migrations again should be a no-op.
        db.run_migrations().expect("re-migration is idempotent");
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
}
