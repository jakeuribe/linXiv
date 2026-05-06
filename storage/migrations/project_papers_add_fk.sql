-- Migration: add paper_id → paper_roots FK to project_papers table.
-- Preserves added_at and note columns from the live schema.
CREATE TABLE project_papers_intermediate (
    project_id INTEGER   NOT NULL REFERENCES projects(id)              ON DELETE CASCADE,
    paper_id   TEXT      NOT NULL REFERENCES paper_roots(paper_id)     ON DELETE CASCADE,
    position   INTEGER,
    added_at   TIMESTAMP,
    note       TEXT,
    PRIMARY KEY (project_id, paper_id)
);
