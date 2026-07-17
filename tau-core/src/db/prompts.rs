use super::{Db, InteractivePrompt};
use anyhow::{Result, bail};
use rusqlite::OptionalExtension;
use uuid::Uuid;

impl Db {
    pub fn create_interactive_prompt(
        &self,
        session_id: &str,
        turn_id: &str,
        kind: &str,
        payload_json: &str,
        client_id: &str,
        idempotency_key: Option<&str>,
    ) -> Result<InteractivePrompt> {
        let id = Uuid::new_v4().to_string();
        let now = super::domain::now_ms();
        let conn = self.conn.lock().unwrap();
        if let Some(key) = idempotency_key {
            let existing = conn
                .query_row(
                    "SELECT id FROM interactive_prompts WHERE session_id=?1 AND idempotency_key=?2",
                    rusqlite::params![session_id, key],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            if let Some(id) = existing {
                drop(conn);
                return self.get_interactive_prompt(&id);
            }
        }
        // The unique index is the final arbiter of concurrent retries.  INSERT
        // OR IGNORE makes the loser of that race replay the durable record.
        conn.execute("INSERT OR IGNORE INTO interactive_prompts (id,session_id,turn_id,kind,payload_json,initiating_client_id,owner_client_id,status,idempotency_key,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?6,'pending',?7,?8,?8)", rusqlite::params![id,session_id,turn_id,kind,payload_json,client_id,idempotency_key,now])?;
        if let Some(key) = idempotency_key {
            if let Some(existing) = conn
                .query_row(
                    "SELECT id FROM interactive_prompts WHERE session_id=?1 AND idempotency_key=?2",
                    rusqlite::params![session_id, key],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                drop(conn);
                return self.get_interactive_prompt(&existing);
            }
        }
        drop(conn);
        self.get_interactive_prompt(&id)
    }
    pub fn get_interactive_prompt(&self, id: &str) -> Result<InteractivePrompt> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT id,session_id,turn_id,kind,payload_json,initiating_client_id,owner_client_id,status,idempotency_key,reply_idempotency_key,takeover_idempotency_key,reply_json,created_at,updated_at FROM interactive_prompts WHERE id=?1", [id], row)?)
    }
    pub fn list_pending_interactive_prompts(&self) -> Result<Vec<InteractivePrompt>> {
        self.list_interactive_prompts("pending")
    }
    pub fn list_interactive_prompts(&self, status: &str) -> Result<Vec<InteractivePrompt>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id,session_id,turn_id,kind,payload_json,initiating_client_id,owner_client_id,status,idempotency_key,reply_idempotency_key,takeover_idempotency_key,reply_json,created_at,updated_at FROM interactive_prompts WHERE status=?1 ORDER BY created_at")?;
        let rows = stmt.query_map([status], row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn resolve_interactive_prompt(
        &self,
        id: &str,
        client_id: &str,
        reply_json: &str,
        idempotency_key: &str,
        owner_required: bool,
    ) -> Result<InteractivePrompt> {
        let conn = self.conn.lock().unwrap();
        let current: (String, String, Option<String>, Option<String>) = conn.query_row("SELECT status,owner_client_id,reply_idempotency_key,reply_json FROM interactive_prompts WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        if current.0 != "pending" {
            if current.0 == "resolved"
                && current.2.as_deref() == Some(idempotency_key)
                && current.3.as_deref() == Some(reply_json)
            {
                drop(conn);
                return self.get_interactive_prompt(id);
            }
            bail!("prompt has already reached a terminal state");
        }
        if owner_required && client_id != current.1 {
            bail!("permission prompt is owned by another client")
        }
        let changed = conn.execute("UPDATE interactive_prompts SET status='resolved',reply_json=?2,reply_idempotency_key=?3,updated_at=?4 WHERE id=?1 AND status='pending' AND (?5=0 OR owner_client_id=?6)", rusqlite::params![id,reply_json,idempotency_key,super::domain::now_ms(), owner_required as i64, client_id])?;
        drop(conn);
        if changed == 0 {
            let existing = self.get_interactive_prompt(id)?;
            if existing.status == "resolved"
                && existing.reply_idempotency_key.as_deref() == Some(idempotency_key)
                && existing.reply_json.as_deref() == Some(reply_json)
            {
                return Ok(existing);
            }
            bail!("prompt resolution lost a concurrent race")
        }
        self.get_interactive_prompt(id)
    }

    /// Persist an explicit negative decision without deleting the request.
    pub fn reject_interactive_prompt(
        &self,
        id: &str,
        client_id: &str,
        reply_json: &str,
        idempotency_key: &str,
        owner_required: bool,
    ) -> Result<InteractivePrompt> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute("UPDATE interactive_prompts SET status='rejected',reply_json=?2,reply_idempotency_key=?3,updated_at=?4 WHERE id=?1 AND status='pending' AND (?5=0 OR owner_client_id=?6)", rusqlite::params![id, reply_json, idempotency_key, super::domain::now_ms(), owner_required as i64, client_id])?;
        if changed == 0 {
            let existing = conn.query_row("SELECT id,session_id,turn_id,kind,payload_json,initiating_client_id,owner_client_id,status,idempotency_key,reply_idempotency_key,takeover_idempotency_key,reply_json,created_at,updated_at FROM interactive_prompts WHERE id=?1", [id], row)?;
            if existing.status == "rejected"
                && existing.reply_idempotency_key.as_deref() == Some(idempotency_key)
                && existing.reply_json.as_deref() == Some(reply_json)
            {
                drop(conn);
                return Ok(existing);
            }
            if existing.status == "pending" && owner_required {
                bail!("permission prompt is owned by another client");
            }
            bail!("prompt has already reached a terminal state");
        }
        drop(conn);
        self.get_interactive_prompt(id)
    }
    pub fn take_over_interactive_prompt(
        &self,
        id: &str,
        client_id: &str,
        idempotency_key: &str,
    ) -> Result<InteractivePrompt> {
        let conn = self.conn.lock().unwrap();
        let current: (String, Option<String>, String) = conn.query_row("SELECT status,takeover_idempotency_key,owner_client_id FROM interactive_prompts WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        if current.0 != "pending" {
            bail!("prompt has already reached a terminal state")
        }
        if current.1.as_deref() == Some(idempotency_key) && current.2 == client_id {
            drop(conn);
            return self.get_interactive_prompt(id);
        }
        let changed = conn.execute("UPDATE interactive_prompts SET owner_client_id=?2,takeover_idempotency_key=?3,updated_at=?4 WHERE id=?1 AND status='pending'", rusqlite::params![id,client_id,idempotency_key,super::domain::now_ms()])?;
        if changed == 0 {
            bail!("prompt has already been taken over")
        }
        drop(conn);
        self.get_interactive_prompt(id)
    }
    pub fn interrupt_interactive_prompts(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("UPDATE interactive_prompts SET status='interrupted',updated_at=?1 WHERE status='pending'", [super::domain::now_ms()])?)
    }
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<InteractivePrompt> {
    Ok(InteractivePrompt {
        id: r.get(0)?,
        session_id: r.get(1)?,
        turn_id: r.get(2)?,
        kind: r.get(3)?,
        payload_json: r.get(4)?,
        initiating_client_id: r.get(5)?,
        owner_client_id: r.get(6)?,
        status: r.get(7)?,
        idempotency_key: r.get(8)?,
        reply_idempotency_key: r.get(9)?,
        takeover_idempotency_key: r.get(10)?,
        reply_json: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_are_replayable_and_permission_owned() {
        let db = Db::open_in_memory().unwrap();
        let session = db.create_session("/tmp").unwrap();
        let prompt = db
            .create_interactive_prompt(&session.id, "turn", "permission", "{}", "client-a", None)
            .unwrap();
        assert_eq!(db.list_pending_interactive_prompts().unwrap().len(), 1);
        assert!(
            db.resolve_interactive_prompt(&prompt.id, "client-b", "{}", "k", true)
                .is_err()
        );
        let resolved = db
            .resolve_interactive_prompt(&prompt.id, "client-a", "{}", "k", true)
            .unwrap();
        assert_eq!(resolved.status, "resolved");
        assert_eq!(
            db.get_interactive_prompt(&prompt.id)
                .unwrap()
                .reply_json
                .as_deref(),
            Some("{}")
        );
    }

    #[test]
    fn prompt_replies_and_takeovers_are_idempotent_and_terminal() {
        let db = Db::open_in_memory().unwrap();
        let session = db.create_session("/tmp").unwrap();
        let prompt = db
            .create_interactive_prompt(
                &session.id,
                "turn",
                "question",
                "{}",
                "client-a",
                Some("create-key"),
            )
            .unwrap();
        let replay = db
            .create_interactive_prompt(
                &session.id,
                "other-turn",
                "question",
                "{\"stale\":true}",
                "client-b",
                Some("create-key"),
            )
            .unwrap();
        assert_eq!(prompt.id, replay.id);

        let taken = db
            .take_over_interactive_prompt(&prompt.id, "client-b", "takeover-key")
            .unwrap();
        assert_eq!(taken.owner_client_id, "client-b");
        let same_takeover = db
            .take_over_interactive_prompt(&prompt.id, "client-b", "takeover-key")
            .unwrap();
        assert_eq!(same_takeover.owner_client_id, "client-b");

        let resolved = db
            .resolve_interactive_prompt(
                &prompt.id,
                "client-b",
                "{\"answer\":\"yes\"}",
                "reply-key",
                false,
            )
            .unwrap();
        let replayed = db
            .resolve_interactive_prompt(
                &prompt.id,
                "client-b",
                "{\"answer\":\"yes\"}",
                "reply-key",
                false,
            )
            .unwrap();
        assert_eq!(resolved, replayed);
        assert!(
            db.resolve_interactive_prompt(
                &prompt.id,
                "client-b",
                "{\"answer\":\"no\"}",
                "different-key",
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn restart_interrupts_pending_prompts_without_deleting_them() {
        let db = Db::open_in_memory().unwrap();
        let session = db.create_session("/tmp").unwrap();
        let prompt = db
            .create_interactive_prompt(&session.id, "turn", "plan", "{}", "client-a", None)
            .unwrap();
        assert_eq!(db.interrupt_interactive_prompts().unwrap(), 1);
        assert_eq!(
            db.get_interactive_prompt(&prompt.id).unwrap().status,
            "interrupted"
        );
        assert!(db.list_pending_interactive_prompts().unwrap().is_empty());
    }
}
