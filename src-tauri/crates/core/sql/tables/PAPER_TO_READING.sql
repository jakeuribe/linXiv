-- Per-paper reading status inside a reading-list PROJECT, stored SPARSELY:
-- the default status (unread) is the ABSENCE of a row, so only deviations
-- (reading / read) ever get persisted. Setting a paper back to unread deletes
-- its row. STATUS holds only the non-default values 'reading' | 'read'.
-- Keyed on (PROJECT_FK, SOURCE_FK): one status per paper per reading list,
-- covering all versions of the paper (SOURCE_FK → PAPER_ROOTS, like membership).
-- The composite FK ties a status row to the paper's PROJECT_TO_PAPER membership
-- row (not directly to PROJECT/PAPER_ROOTS): removing a paper from a project
-- deletes its PROJECT_TO_PAPER row, which cascades here too, so reading status
-- can't outlive membership and silently resurrect on re-add. Needs
-- idx_project_to_paper_unique on PROJECT_TO_PAPER(PROJECT_FK, SOURCE_FK) by the
-- time of any DML on either table (not at CREATE TABLE time) — init_db dedups
-- pre-schema, then the project_to_paper_unique_index migration creates it before
-- any write — and needs the removal to be a genuine delete,
-- not a blanket delete+reinsert of unchanged rows in one statement (that would
-- cascade-clear this table too; see save_source_fks).
CREATE TABLE IF NOT EXISTS PAPER_TO_READING(
    PROJECT_FK  INTEGER NOT NULL,
    SOURCE_FK   INTEGER NOT NULL,
    STATUS      TEXT    NOT NULL CHECK (STATUS IN ('reading', 'read')),
    UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (PROJECT_FK, SOURCE_FK),
    FOREIGN KEY (PROJECT_FK, SOURCE_FK) REFERENCES PROJECT_TO_PAPER(PROJECT_FK, SOURCE_FK) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_paper_to_reading_source_fk ON PAPER_TO_READING (SOURCE_FK);
