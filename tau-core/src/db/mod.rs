//! `Db` type, connection management, and schema migration.

pub mod domain;
mod epochs;
mod journal;
mod messages;
mod policy;
mod prompts;
mod qa_records;
mod sessions;
mod usage;

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

pub use domain::{
    ContentBlock, ContextEpochRecord, InteractivePrompt, Message, PlanRevision,
    PolicyDecisionRecord, QaRecord, Session, SteeringMode, SteeringRun, Usage, default_db_path,
};
pub use journal::StoredArtifact;
pub use sessions::{ProjectId, SessionRecord};

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
        let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < 6 {
            migrations()
                .to_version(&mut conn, 6)
                .context("running database migrations")?;
        }
        ensure_session_columns(&mut conn)?;
        migrations()
            .to_latest(&mut conn)
            .context("running post-project session migrations")?;
        Ok(())
    }
}

/// S1 owns project v6, including the authoritative project foreign key.  S3
/// only ensures compatibility columns when running standalone or upgrading a
/// database created before project v6; it never alters an existing project_id
/// column, constraint, or nullability.
fn ensure_session_columns(conn: &mut Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    if !columns.iter().any(|column| column == "project_id") {
        conn.execute("ALTER TABLE sessions ADD COLUMN project_id TEXT", [])?;
    }
    if !columns.iter().any(|column| column == "archived_at") {
        conn.execute("ALTER TABLE sessions ADD COLUMN archived_at INTEGER", [])?;
    }
    Ok(())
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("sql/v1.sql")),
        M::up(include_str!("sql/v2.sql")),
        M::up(include_str!("sql/v3.sql")),
        M::up(include_str!("sql/v4.sql")),
        M::up(include_str!("sql/v5.sql")),
        M::up(include_str!("sql/v6.sql")),
        M::up(include_str!("sql/v7.sql")),
    ])
}

#[cfg(test)]
mod tests;
