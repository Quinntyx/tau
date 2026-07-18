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
    PolicyDecisionRecord, Project, QaRecord, Session, SteeringMode, SteeringRun, Usage,
    default_db_path,
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
        migrations()
            .to_latest(&mut conn)
            .context("running database migrations")?;
        Ok(())
    }
}

impl Db {
    pub fn create_project(&self, name: &str, root: impl AsRef<Path>) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("project name must not be empty");
        }
        let root = domain::canonical_project_root(root)?;
        let now = domain::now_ms();
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            root,
            active: true,
            created_at: now,
            updated_at: now,
        };
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO projects (id,name,root,active,created_at,updated_at) VALUES (?1,?2,?3,1,?4,?4)", rusqlite::params![project.id, project.name, project.root, now])?;
        Ok(project)
    }

    pub fn list_projects(&self, include_inactive: bool) -> Result<Vec<Project>> {
        let conn = self.conn.lock().unwrap();
        let sql = if include_inactive {
            "SELECT id,name,root,active,created_at,updated_at FROM projects ORDER BY active DESC, name, id"
        } else {
            "SELECT id,name,root,active,created_at,updated_at FROM projects WHERE active=1 ORDER BY name, id"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], project_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>> {
        let conn = self.conn.lock().unwrap();
        let mut s = conn.prepare(
            "SELECT id,name,root,active,created_at,updated_at FROM projects WHERE id=?1",
        )?;
        let mut rows = s.query_map([id], |r| {
            Ok(Project {
                id: r.get(0)?,
                name: r.get(1)?,
                root: r.get(2)?,
                active: r.get::<_, i64>(3)? != 0,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn rename_project(&self, id: &str, name: &str) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("project name must not be empty");
        }
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE projects SET name=?1,updated_at=?2 WHERE id=?3",
            rusqlite::params![name, domain::now_ms(), id],
        )?;
        if changed == 0 {
            anyhow::bail!("project not found");
        }
        drop(conn);
        self.get_project(id)?
            .context("project disappeared after rename")
    }

    pub fn repath_project(&self, id: &str, root: impl AsRef<Path>) -> Result<Project> {
        let root = domain::canonical_project_root(root)?;
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE projects SET root=?1,updated_at=?2 WHERE id=?3",
            rusqlite::params![root, domain::now_ms(), id],
        )?;
        if changed == 0 {
            anyhow::bail!("project not found");
        }
        drop(conn);
        self.get_project(id)?
            .context("project disappeared after repath")
    }

    /// Create a distinct active ID for an inactive project's canonical root.
    /// The old record and its sessions remain untouched.
    pub fn new_project_id(&self, id: &str) -> Result<Project> {
        let source = self.get_project(id)?.context("project not found")?;
        if source.active {
            anyhow::bail!("new project ID requires an inactive project");
        }
        self.create_project(&source.name, &source.root)
    }

    pub fn unregister_project(&self, id: &str) -> Result<Project> {
        self.set_project_active(id, false)
    }
    pub fn reactivate_project(&self, id: &str) -> Result<Project> {
        self.set_project_active(id, true)
    }
    fn set_project_active(&self, id: &str, active: bool) -> Result<Project> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE projects SET active=?1,updated_at=?2 WHERE id=?3",
            rusqlite::params![active as i64, domain::now_ms(), id],
        )?;
        if n == 0 {
            anyhow::bail!("project not found");
        }
        drop(conn);
        self.get_project(id)?
            .context("project disappeared after lifecycle update")
    }
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        root: row.get(2)?,
        active: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
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
