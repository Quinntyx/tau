CREATE TABLE IF NOT EXISTS event_journal (
    event_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    request_id TEXT,
    occurred_at INTEGER NOT NULL,
    UNIQUE(session_id, sequence)
);
CREATE INDEX IF NOT EXISTS idx_event_journal_replay ON event_journal(session_id, sequence);
CREATE TABLE IF NOT EXISTS idempotency_keys (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    key TEXT NOT NULL, request_hash TEXT NOT NULL, result_json TEXT NOT NULL,
    created_at INTEGER NOT NULL, PRIMARY KEY(session_id, key)
);
CREATE TABLE IF NOT EXISTS artifacts (
    artifact_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    media_type TEXT NOT NULL, size_bytes INTEGER NOT NULL, content_hash TEXT NOT NULL,
    storage_ref TEXT NOT NULL, created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_artifacts_session ON artifacts(session_id, created_at);
