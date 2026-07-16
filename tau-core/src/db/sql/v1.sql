-- M2 initial schema: sessions, messages, content_blocks, usage, qa_records.

CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT PRIMARY KEY,
    cwd        TEXT NOT NULL,
    title      TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role       TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    seq        INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, seq)
);

CREATE TABLE IF NOT EXISTS content_blocks (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id       INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    seq              INTEGER NOT NULL,
    block_type       TEXT NOT NULL CHECK (block_type IN ('text', 'tool_use', 'tool_result')),
    text             TEXT,
    tool_call_id     TEXT,
    tool_name        TEXT,
    tool_args_json   TEXT,
    tool_result_json TEXT,
    tool_is_error    INTEGER,
    UNIQUE(message_id, seq)
);

CREATE TABLE IF NOT EXISTS usage (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL REFERENCES sessions(id),
    message_id    INTEGER REFERENCES messages(id),
    model         TEXT NOT NULL,
    input_tokens  INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cached_tokens INTEGER,
    created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS qa_records (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    question   TEXT NOT NULL,
    answer     TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, seq);
CREATE INDEX IF NOT EXISTS idx_content_blocks_message ON content_blocks(message_id, seq);
CREATE INDEX IF NOT EXISTS idx_usage_session ON usage(session_id);
CREATE INDEX IF NOT EXISTS idx_qa_records_session ON qa_records(session_id);
