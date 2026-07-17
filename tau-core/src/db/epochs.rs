use anyhow::Result;

use super::{Db, domain::ContextEpochRecord};

impl Db {
    /// Append an epoch record. Existing records are never updated or deleted.
    pub fn append_context_epoch(&self, record: &ContextEpochRecord) -> Result<ContextEpochRecord> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO context_epochs (session_id, epoch, summary, plan_context, provider, model, input_tokens, trigger, retry_marker, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![record.session_id, record.epoch, record.summary, record.plan_context, record.provider, record.model, record.input_tokens, record.trigger, record.retry_marker as i64, record.created_at],
        )?;
        let mut saved = record.clone();
        saved.id = conn.last_insert_rowid();
        Ok(saved)
    }

    pub fn context_epochs(&self, session_id: &str) -> Result<Vec<ContextEpochRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id,session_id,epoch,summary,plan_context,provider,model,input_tokens,trigger,retry_marker,created_at FROM context_epochs WHERE session_id=?1 ORDER BY epoch")?;
        let rows = stmt.query_map([session_id], |r| {
            Ok(ContextEpochRecord {
                id: r.get(0)?,
                session_id: r.get(1)?,
                epoch: r.get(2)?,
                summary: r.get(3)?,
                plan_context: r.get(4)?,
                provider: r.get(5)?,
                model: r.get(6)?,
                input_tokens: r.get(7)?,
                trigger: r.get(8)?,
                retry_marker: r.get::<_, i64>(9)? != 0,
                created_at: r.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn latest_context_epoch(&self, session_id: &str) -> Result<Option<ContextEpochRecord>> {
        Ok(self.context_epochs(session_id)?.pop())
    }
}
