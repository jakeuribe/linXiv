-- Swap projects_new into place after the dynamic INSERT SELECT has run.
DROP TABLE projects;
ALTER TABLE projects_new RENAME TO projects;
CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status);
