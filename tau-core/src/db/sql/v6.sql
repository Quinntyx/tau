ALTER TABLE sessions ADD COLUMN project_id TEXT NOT NULL DEFAULT '';
ALTER TABLE sessions ADD COLUMN archived_at INTEGER;
CREATE INDEX IF NOT EXISTS idx_sessions_project_updated ON sessions(project_id, updated_at DESC);
