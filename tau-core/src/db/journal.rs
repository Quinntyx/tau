use super::Db;
use super::domain::now_ms;
use anyhow::{Context, Result};
use rusqlite::params;
use serde::{Serialize, de::DeserializeOwned};
use tau_proto::turn::{ArtifactReference, IdempotencyKey, SequencedEvent, TurnEvent};

#[derive(Debug, Clone)]
pub struct StoredArtifact {
    pub artifact_id: String,
    pub session_id: String,
    pub reference: ArtifactReference,
    pub created_at: i64,
}

impl Db {
    pub fn append_event(
        &self,
        session_id: &str,
        event: &TurnEvent,
        request_id: Option<&str>,
    ) -> Result<SequencedEvent> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let sequence: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM event_journal WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        let event_id = uuid::Uuid::new_v4().to_string();
        let payload = serde_json::to_string(event)?;
        let event_type = serde_json::to_value(event)?
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let occurred_at = now_ms();
        tx.execute("INSERT INTO event_journal(event_id,session_id,sequence,event_type,payload_json,request_id,occurred_at) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![event_id, session_id, sequence, event_type, payload, request_id, occurred_at])?;
        tx.commit()?;
        Ok(SequencedEvent {
            event_id,
            session_id: session_id.into(),
            sequence: sequence as u64,
            occurred_at,
            request_id: request_id.map(tau_proto::envelope::Id::string),
            event: event.clone(),
        })
    }

    pub fn replay_events(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: Option<u32>,
    ) -> Result<Vec<SequencedEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT event_id,sequence,payload_json,request_id,occurred_at FROM event_journal WHERE session_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3")?;
        let rows = stmt.query_map(
            params![
                session_id,
                after_sequence as i64,
                i64::from(limit.unwrap_or(1000))
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (event_id, sequence, payload, request_id, occurred_at) = row?;
            Ok(SequencedEvent {
                event_id,
                session_id: session_id.into(),
                sequence: sequence as u64,
                occurred_at,
                request_id: request_id.map(tau_proto::envelope::Id::string),
                event: serde_json::from_str(&payload)?,
            })
        })
        .collect()
    }

    pub fn remember_idempotency<T: Serialize>(
        &self,
        session_id: &str,
        key: &IdempotencyKey,
        request_hash: &str,
        result: &T,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let payload = serde_json::to_string(result)?;
        if conn.execute("INSERT INTO idempotency_keys(session_id,key,request_hash,result_json,created_at) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(session_id,key) DO NOTHING", params![session_id, key.0, request_hash, payload, now_ms()])? == 1 { return Ok(true); }
        let existing: String = conn.query_row(
            "SELECT request_hash FROM idempotency_keys WHERE session_id=?1 AND key=?2",
            params![session_id, key.0],
            |r| r.get(0),
        )?;
        if existing != request_hash {
            anyhow::bail!("idempotency key reused with different request")
        }
        Ok(false)
    }

    pub fn idempotent_result<T: DeserializeOwned>(
        &self,
        session_id: &str,
        key: &IdempotencyKey,
    ) -> Result<Option<T>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT result_json FROM idempotency_keys WHERE session_id=?1 AND key=?2")?;
        let mut rows = stmt.query(params![session_id, key.0])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&row.get::<_, String>(0)?)?))
    }

    pub fn create_artifact(
        &self,
        session_id: &str,
        reference: ArtifactReference,
    ) -> Result<StoredArtifact> {
        let conn = self.conn.lock().unwrap();
        let created_at = now_ms();
        conn.execute("INSERT INTO artifacts(artifact_id,session_id,media_type,size_bytes,content_hash,storage_ref,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![reference.artifact_id, session_id, reference.media_type, reference.size_bytes as i64, reference.content_hash, reference.storage_ref, created_at])?;
        Ok(StoredArtifact {
            artifact_id: reference.artifact_id.clone(),
            session_id: session_id.into(),
            reference,
            created_at,
        })
    }

    pub fn list_artifacts(&self, session_id: &str) -> Result<Vec<StoredArtifact>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT artifact_id,media_type,size_bytes,content_hash,storage_ref,created_at FROM artifacts WHERE session_id=?1 ORDER BY created_at")?;
        let rows = stmt.query_map([session_id], |r| {
            Ok(StoredArtifact {
                artifact_id: r.get(0)?,
                session_id: session_id.into(),
                reference: ArtifactReference {
                    artifact_id: r.get(0)?,
                    media_type: r.get(1)?,
                    size_bytes: r.get::<_, i64>(2)? as u64,
                    content_hash: r.get(3)?,
                    storage_ref: r.get(4)?,
                },
                created_at: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing artifacts")
    }
}
