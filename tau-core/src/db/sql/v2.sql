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

CREATE TABLE IF NOT EXISTS plans (
    id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id),
    title TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
    active_revision INTEGER NOT NULL DEFAULT 1, revoked INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS plan_revisions (
    plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL, parent_revision INTEGER, payload TEXT NOT NULL,
    airtight INTEGER NOT NULL DEFAULT 0, revoked INTEGER NOT NULL DEFAULT 0,
    actor TEXT NOT NULL, created_at INTEGER NOT NULL,
    PRIMARY KEY(plan_id, revision)
);
CREATE TABLE IF NOT EXISTS plan_qa_citations (
    plan_id TEXT NOT NULL, revision INTEGER NOT NULL, step INTEGER NOT NULL,
    qa_id TEXT NOT NULL REFERENCES qa_records(id), quote TEXT NOT NULL,
    PRIMARY KEY(plan_id, revision, step, qa_id),
    FOREIGN KEY(plan_id, revision) REFERENCES plan_revisions(plan_id, revision)
);
CREATE TABLE IF NOT EXISTS steering_runs (
    id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id),
    mode TEXT NOT NULL CHECK(mode IN ('normal','super')),
    model TEXT NOT NULL, vision TEXT NOT NULL, authority TEXT NOT NULL,
    status TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_plans_session ON plans(session_id);
CREATE INDEX IF NOT EXISTS idx_steering_session ON steering_runs(session_id);
CREATE TABLE IF NOT EXISTS session_permissions (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL, pattern TEXT NOT NULL, decision TEXT NOT NULL,
    PRIMARY KEY(session_id, ordinal)
);
