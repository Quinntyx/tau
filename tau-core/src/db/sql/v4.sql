-- Durable interactive prompts. A prompt is never deleted on reply: its terminal
-- state is retained for replay and idempotent retries.
CREATE TABLE IF NOT EXISTS interactive_prompts (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    initiating_client_id TEXT NOT NULL,
    owner_client_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','resolved','rejected','interrupted')),
    idempotency_key TEXT,
    reply_idempotency_key TEXT,
    takeover_idempotency_key TEXT,
    reply_json TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_interactive_prompts_session ON interactive_prompts(session_id, created_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_interactive_prompts_idempotency ON interactive_prompts(session_id, idempotency_key) WHERE idempotency_key IS NOT NULL;

-- Durable decision history, scoped to a session or the global policy.  The
-- actor is retained so policy changes remain auditable across restarts.
CREATE TABLE IF NOT EXISTS policy_decisions (
    id TEXT PRIMARY KEY,
    session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
    scope TEXT NOT NULL CHECK(scope IN ('session','global')),
    actor TEXT NOT NULL,
    pattern TEXT NOT NULL,
    decision_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, scope, pattern)
);
CREATE INDEX IF NOT EXISTS idx_policy_decisions_scope ON policy_decisions(scope, session_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_policy_decisions_global_pattern ON policy_decisions(scope, pattern) WHERE scope='global';
