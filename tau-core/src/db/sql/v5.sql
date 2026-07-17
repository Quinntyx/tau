-- Additional append-only context epoch metadata used by configured compaction
-- agents and restart-safe overflow retries.
ALTER TABLE context_epochs ADD COLUMN parent_epoch INTEGER;
ALTER TABLE context_epochs ADD COLUMN estimated_tokens INTEGER;
ALTER TABLE context_epochs ADD COLUMN terminal_status TEXT;
ALTER TABLE context_epochs ADD COLUMN is_baseline INTEGER NOT NULL DEFAULT 0;
ALTER TABLE context_epochs ADD COLUMN system_message TEXT;
CREATE INDEX IF NOT EXISTS idx_context_epochs_parent ON context_epochs(session_id, parent_epoch);
