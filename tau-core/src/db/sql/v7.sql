-- Sessions cannot outlive the active project they belong to.  Old databases
-- may contain unattached sessions; those records are intentionally discarded.
PRAGMA foreign_keys = OFF;
CREATE TABLE sessions_new (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    cwd TEXT NOT NULL,
    title TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived_at INTEGER
);
INSERT INTO sessions_new (id, project_id, cwd, title, created_at, updated_at)
    SELECT s.id, s.project_id, s.cwd, s.title, s.created_at, s.updated_at
    FROM sessions s JOIN projects p ON p.id = s.project_id AND p.active = 1;
DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;
CREATE INDEX sessions_project_id ON sessions(project_id);
CREATE INDEX idx_sessions_project_updated ON sessions(project_id, updated_at DESC);
PRAGMA foreign_keys = ON;

CREATE TRIGGER sessions_project_active_insert
BEFORE INSERT ON sessions
WHEN NOT EXISTS (SELECT 1 FROM projects WHERE id = NEW.project_id AND active = 1)
BEGIN
    SELECT RAISE(ABORT, 'session project must be active');
END;
CREATE TRIGGER sessions_project_active_update
BEFORE UPDATE OF project_id ON sessions
WHEN NOT EXISTS (SELECT 1 FROM projects WHERE id = NEW.project_id AND active = 1)
BEGIN
    SELECT RAISE(ABORT, 'session project must be active');
END;
