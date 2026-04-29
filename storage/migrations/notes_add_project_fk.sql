-- Migration: add paper_roots and project_id FK constraints to notes table
-- Rebuilds the table atomically. Safe to run on any existing notes DB.

PRAGMA foreign_keys = OFF;

CREATE TABLE notes_intermediate (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    paper_id   TEXT    NOT NULL,
    project_id INTEGER,
    title      TEXT,
    content    TEXT,
    created_at TIMESTAMP,
    updated_at TIMESTAMP,
    FOREIGN KEY (paper_id)   REFERENCES paper_roots(paper_id) ON DELETE CASCADE,
    FOREIGN KEY (project_id) REFERENCES projects(id)          ON DELETE SET NULL
);

INSERT INTO notes_intermediate (id, paper_id, project_id, title, content, created_at, updated_at)
    SELECT id, paper_id, project_id, title, content, created_at, updated_at FROM notes;

DROP TABLE notes;

ALTER TABLE notes_intermediate RENAME TO notes;

CREATE INDEX IF NOT EXISTS idx_notes_paper_id   ON notes(paper_id);
CREATE INDEX IF NOT EXISTS idx_notes_project_id ON notes(project_id);
CREATE INDEX IF NOT EXISTS idx_notes_created_at ON notes(created_at);

PRAGMA foreign_keys = ON;
