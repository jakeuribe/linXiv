-- Migration: add project_id FK constraint to notes table
-- Rebuilds the table atomically. Safe to run on any existing notes DB.

PRAGMA foreign_keys = OFF;

CREATE TABLE notes_new (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    paper_id   TEXT    NOT NULL,
    project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    title      TEXT,
    content    TEXT,
    created_at TIMESTAMP,
    updated_at TIMESTAMP
);

INSERT INTO notes_new (id, paper_id, project_id, title, content, created_at, updated_at)
    SELECT id, paper_id, project_id, title, content, created_at, updated_at FROM notes;

DROP TABLE notes;

ALTER TABLE notes_new RENAME TO notes;

CREATE INDEX IF NOT EXISTS idx_notes_paper_id   ON notes(paper_id);
CREATE INDEX IF NOT EXISTS idx_notes_project_id ON notes(project_id);
CREATE INDEX IF NOT EXISTS idx_notes_created_at ON notes(created_at);

PRAGMA foreign_keys = ON;
