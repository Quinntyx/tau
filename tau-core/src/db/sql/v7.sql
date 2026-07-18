-- Session lifecycle index. Columns are ensured after project v6 and before
-- this migration so S1's project_id foreign key remains authoritative.
CREATE INDEX IF NOT EXISTS idx_sessions_project_updated
    ON sessions(project_id, updated_at DESC);
