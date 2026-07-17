-- Context lifecycle metadata is durable across restarts.
CREATE TABLE IF NOT EXISTS context_state (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL DEFAULT 0,
    plan_context TEXT,
    retry_marker INTEGER NOT NULL DEFAULT 0 CHECK (retry_marker IN (0, 1)),
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
