-- Sessions must always have a project.  Keep rows whose project still exists
-- (including inactive history), while discarding unattached/unknown records.
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
INSERT INTO sessions_new (id, project_id, cwd, title, created_at, updated_at, archived_at)
    SELECT s.id, s.project_id, s.cwd, s.title, s.created_at, s.updated_at, NULL
    FROM sessions s JOIN projects p ON p.id = s.project_id;
-- Foreign keys are disabled during the rebuild, so explicitly remove rows
-- owned by sessions that the migration discarded.
DELETE FROM content_blocks WHERE message_id IN
    (SELECT id FROM messages WHERE session_id NOT IN (SELECT id FROM sessions_new));
DELETE FROM plan_qa_citations WHERE plan_id IN
    (SELECT id FROM plans WHERE session_id NOT IN (SELECT id FROM sessions_new))
    OR qa_id IN (SELECT id FROM qa_records WHERE session_id NOT IN (SELECT id FROM sessions_new));
DELETE FROM plan_revisions WHERE plan_id IN
    (SELECT id FROM plans WHERE session_id NOT IN (SELECT id FROM sessions_new));
DELETE FROM messages WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM usage WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM qa_records WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM context_state WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM interactive_prompts WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM policy_decisions WHERE session_id IS NOT NULL
    AND session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM event_journal WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM idempotency_keys WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM artifacts WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM plans WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM steering_runs WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM session_permissions WHERE session_id NOT IN (SELECT id FROM sessions_new);
DELETE FROM context_epochs WHERE session_id NOT IN (SELECT id FROM sessions_new);
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
