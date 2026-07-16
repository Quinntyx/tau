//! `qa_records` table operations.

use anyhow::Result;
use uuid::Uuid;

use super::Db;
use super::domain::*;

impl Db {
    pub fn record_qa(&self, session_id: &str, question: &str, answer: &str) -> Result<QaRecord> {
        let now = now_ms();
        let id = Uuid::new_v4().to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO qa_records (id, session_id, question, answer, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
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
            "SELECT id, session_id, question, answer, created_at \
             FROM qa_records WHERE session_id = ?1 ORDER BY created_at",
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
