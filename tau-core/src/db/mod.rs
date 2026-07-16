//! `Db` type, connection management, and schema migration.

pub mod domain;
mod messages;
mod qa_records;
mod sessions;
mod usage;

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

pub use domain::{ContentBlock, Message, QaRecord, Session, Usage, default_db_path};

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
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(include_str!("sql/v1.sql"))])
}

#[cfg(test)]
mod tests;
