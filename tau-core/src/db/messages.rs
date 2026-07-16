//! `messages` + `content_blocks` table operations.

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::Db;
use super::domain::*;

impl Db {
    pub fn append_message(
        &self,
        session_id: &str,
        role: &str,
        blocks: Vec<ContentBlock>,
    ) -> Result<Message> {
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .context("computing next message seq")?;
        tx.execute(
            "INSERT INTO messages (session_id, role, seq, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, role, seq, now],
        )?;
        let message_id = tx.last_insert_rowid();
        for (i, block) in blocks.iter().enumerate() {
            insert_content_block(&tx, message_id, i as i64, block)?;
        }
        tx.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, session_id],
        )?;
        tx.commit()?;
        Ok(Message {
            id: message_id,
            session_id: session_id.to_string(),
            role: role.to_string(),
            seq,
            created_at: now,
            blocks,
        })
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, seq, created_at \
             FROM messages WHERE session_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let (id, sid, role, seq, created_at) = row?;
            let blocks = load_content_blocks(&conn, id)?;
            messages.push(Message {
                id,
                session_id: sid,
                role,
                seq,
                created_at,
                blocks,
            });
        }
        Ok(messages)
    }
}

fn insert_content_block(
    tx: &rusqlite::Transaction,
    message_id: i64,
    seq: i64,
    block: &ContentBlock,
) -> Result<()> {
    match block {
        ContentBlock::Text { text } => {
            tx.execute(
                "INSERT INTO content_blocks (message_id, seq, block_type, text) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![message_id, seq, "text", text],
            )?;
        }
        ContentBlock::ToolUse {
            call_id,
            name,
            args_json,
        } => {
            tx.execute(
                "INSERT INTO content_blocks \
                 (message_id, seq, block_type, tool_call_id, tool_name, tool_args_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![message_id, seq, "tool_use", call_id, name, args_json],
            )?;
        }
        ContentBlock::ToolResult {
            call_id,
            result_json,
            is_error,
        } => {
            tx.execute(
                "INSERT INTO content_blocks \
                 (message_id, seq, block_type, tool_call_id, tool_result_json, tool_is_error) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    message_id,
                    seq,
                    "tool_result",
                    call_id,
                    result_json,
                    *is_error as i64,
                ],
            )?;
        }
    }
    Ok(())
}

fn load_content_blocks(conn: &Connection, message_id: i64) -> Result<Vec<ContentBlock>> {
    let mut stmt = conn.prepare(
        "SELECT block_type, text, tool_call_id, tool_name, tool_args_json, \
         tool_result_json, tool_is_error \
         FROM content_blocks WHERE message_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map([message_id], |row| {
        let block_type: String = row.get(0)?;
        match block_type.as_str() {
            "text" => Ok(ContentBlock::Text { text: row.get(1)? }),
            "tool_use" => Ok(ContentBlock::ToolUse {
                call_id: row.get(2)?,
                name: row.get(3)?,
                args_json: row.get(4)?,
            }),
            "tool_result" => {
                let is_error_val: Option<i64> = row.get(6)?;
                Ok(ContentBlock::ToolResult {
                    call_id: row.get(2)?,
                    result_json: row.get(5)?,
                    is_error: is_error_val.unwrap_or(0) != 0,
                })
            }
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("unknown block_type: {other}").into(),
            )),
        }
    })?;
let mut blocks = Vec::new();
    for r in rows {
        blocks.push(r?);
    }
    Ok(blocks)
}
