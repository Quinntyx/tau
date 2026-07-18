//! `sessions` table operations.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Db;
use super::domain::*;

/// Opaque identifier supplied by the daemon's project registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub project_id: ProjectId,
    pub cwd: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived_at: Option<i64>,
}

impl Db {
    pub fn create_session_for_project(
        &self,
        project_id: &ProjectId,
        cwd: &str,
    ) -> Result<SessionRecord> {
        if project_id.as_str().trim().is_empty() {
            anyhow::bail!("project_id is required for a managed session");
        }
        let now = now_ms();
        let record = SessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.clone(),
            cwd: cwd.to_owned(),
            title: None,
            created_at: now,
            updated_at: now,
            archived_at: None,
        };
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO sessions (id, project_id, cwd, title, created_at, updated_at, archived_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", rusqlite::params![record.id, record.project_id.as_str(), record.cwd, record.title, record.created_at, record.updated_at, record.archived_at])?;
        Ok(record)
    }

    pub fn list_sessions_for_project(
        &self,
        project_id: &ProjectId,
        include_archived: bool,
    ) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let sql = if include_archived {
            "SELECT id, project_id, cwd, title, created_at, updated_at, archived_at FROM sessions WHERE project_id = ?1 ORDER BY updated_at DESC, id DESC"
        } else {
            "SELECT id, project_id, cwd, title, created_at, updated_at, archived_at FROM sessions WHERE project_id = ?1 AND archived_at IS NULL ORDER BY updated_at DESC, id DESC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([project_id.as_str()], session_record_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn get_session_record(&self, id: &str) -> Result<Option<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, project_id, cwd, title, created_at, updated_at, archived_at FROM sessions WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], session_record_from_row)?;
        rows.next().transpose().map_err(Into::into)
    }

    pub fn get_session_record_for_project(
        &self,
        project_id: &ProjectId,
        id: &str,
    ) -> Result<Option<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, cwd, title, created_at, updated_at, archived_at \
             FROM sessions WHERE id = ?1 AND project_id = ?2",
        )?;
        let mut rows = stmt.query_map(
            rusqlite::params![id, project_id.as_str()],
            session_record_from_row,
        )?;
        rows.next().transpose().map_err(Into::into)
    }

    pub fn rename_session(&self, id: &str, title: &str) -> Result<bool> {
        self.update_session_state(id, "title", Some(title.to_owned()))
    }
    pub fn archive_session(&self, id: &str) -> Result<bool> {
        self.update_session_state(id, "archived_at", Some(now_ms().to_string()))
    }
    pub fn restore_session(&self, id: &str) -> Result<bool> {
        self.update_session_state(id, "archived_at", None)
    }

    pub fn rename_session_for_project(
        &self,
        project_id: &ProjectId,
        id: &str,
        title: &str,
    ) -> Result<Option<SessionRecord>> {
        self.update_session_state_for_project(project_id, id, "title", Some(title.to_owned()))
    }

    pub fn archive_session_for_project(
        &self,
        project_id: &ProjectId,
        id: &str,
    ) -> Result<Option<SessionRecord>> {
        self.update_session_state_for_project(
            project_id,
            id,
            "archived_at",
            Some(now_ms().to_string()),
        )
    }

    pub fn restore_session_for_project(
        &self,
        project_id: &ProjectId,
        id: &str,
    ) -> Result<Option<SessionRecord>> {
        self.update_session_state_for_project(project_id, id, "archived_at", None)
    }

    fn update_session_state(&self, id: &str, field: &str, value: Option<String>) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = match field {
            "title" => conn.execute(
                "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![value, now_ms(), id],
            )?,
            "archived_at" => conn.execute(
                "UPDATE sessions SET archived_at = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![value.map(|v| v.parse::<i64>().unwrap()), now_ms(), id],
            )?,
            _ => 0,
        };
        Ok(changed != 0)
    }

    fn update_session_state_for_project(
        &self,
        project_id: &ProjectId,
        id: &str,
        field: &str,
        value: Option<String>,
    ) -> Result<Option<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let changed = match field {
            "title" => conn.execute(
                "UPDATE sessions SET title = ?1, updated_at = ?2 \
                 WHERE id = ?3 AND project_id = ?4",
                rusqlite::params![value, now_ms(), id, project_id.as_str()],
            )?,
            "archived_at" => conn.execute(
                "UPDATE sessions SET archived_at = ?1, updated_at = ?2 \
                 WHERE id = ?3 AND project_id = ?4",
                rusqlite::params![
                    value.map(|v| v.parse::<i64>().unwrap()),
                    now_ms(),
                    id,
                    project_id.as_str()
                ],
            )?,
            _ => 0,
        };
        if changed == 0 {
            return Ok(None);
        }
        let mut stmt = conn.prepare(
            "SELECT id, project_id, cwd, title, created_at, updated_at, archived_at \
             FROM sessions WHERE id = ?1 AND project_id = ?2",
        )?;
        Ok(Some(stmt.query_row(
            rusqlite::params![id, project_id.as_str()],
            session_record_from_row,
        )?))
    }
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
}

fn session_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        project_id: ProjectId::new(row.get::<_, String>(1)?),
        cwd: row.get(2)?,
        title: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        archived_at: row.get(6)?,
    })
}
