//! `sessions` table operations.

use anyhow::Result;
use uuid::Uuid;

use super::Db;
use super::domain::*;

impl Db {
    pub fn create_session(&self, project_id: &str) -> Result<Session> {
        let now = now_ms();
        let session = Session {
            id: Uuid::new_v4().to_string(),
            project_id: Some(project_id.to_string()),
            cwd: String::new(),
            title: None,
            created_at: now,
            updated_at: now,
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, cwd, title, created_at, updated_at) \
             SELECT ?1, id, root, ?2, ?3, ?4 FROM projects \
             WHERE id = ?5 AND active = 1",
            rusqlite::params![
                &session.id,
                &session.title,
                session.created_at,
                session.updated_at,
                project_id,
            ],
        )?;
        if conn.changes() != 1 {
            anyhow::bail!("project does not exist or is inactive");
        }
        let cwd: String = conn.query_row(
            "SELECT cwd FROM sessions WHERE id=?1",
            [&session.id],
            |row| row.get(0),
        )?;
        Ok(Session { cwd, ..session })
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, cwd, title, created_at, updated_at \
             FROM sessions ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Session {
                id: row.get(0)?,
                project_id: row.get(1)?,
                cwd: row.get(2)?,
                title: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        let mut sessions = Vec::new();
        for r in rows {
            sessions.push(r?);
        }
        Ok(sessions)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, cwd, title, created_at, updated_at FROM sessions WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], |row| {
            Ok(Session {
                id: row.get(0)?,
                project_id: row.get(1)?,
                cwd: row.get(2)?,
                title: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }
}
