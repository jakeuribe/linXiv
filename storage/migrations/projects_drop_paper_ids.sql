-- Migration: drop paper_ids JSON column from projects table
-- project_papers bridge table becomes the single source of truth.
--
-- The INSERT SELECT is executed by Python with a dynamic column list
-- generated from PRAGMA table_info(projects) so that any columns added
-- by future migrations are preserved.

CREATE TABLE projects_intermediate (
    id           INTEGER   PRIMARY KEY AUTOINCREMENT,
    name         TEXT      NOT NULL,
    description  TEXT,
    color        INTEGER,
    created_at   TIMESTAMP,
    updated_at   TIMESTAMP,
    archived_at  TIMESTAMP,
    project_tags LIST,
    status       TEXT      NOT NULL DEFAULT 'active'
);
