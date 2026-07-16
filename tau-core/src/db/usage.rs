//! `usage` table operations.

use anyhow::Result;

use super::Db;
use super::domain::*;

impl Db {
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
            "INSERT INTO usage \
             (session_id, message_id, model, input_tokens, output_tokens, cached_tokens, created_at) \
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
}
