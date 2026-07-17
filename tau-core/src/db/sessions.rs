//! `sessions` table operations.

use anyhow::Result;
use uuid::Uuid;

use super::Db;
use super::domain::*;

impl Db {
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
            "INSERT INTO sessions (id, cwd, title, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
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
            "SELECT id, cwd, title, created_at, updated_at \
             FROM sessions ORDER BY created_at DESC",
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
        for r in rows {
            sessions.push(r?);
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

    pub fn get_session_by_cwd(&self, cwd: &str) -> Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, cwd, title, created_at, updated_at FROM sessions \
             WHERE cwd = ?1 ORDER BY updated_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([cwd], |row| {
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
}
