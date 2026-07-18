-- SOURCE_FK references paper_roots, so a project membership row covers all versions of a paper.
CREATE TABLE IF NOT EXISTS PROJECT_TO_PAPER(
    PROJECT_TO_PAPER_FK INTEGER NOT NULL,
    PROJECT_FK  INTEGER NOT NULL,
    SOURCE_FK   INTEGER NOT NULL,
    CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (PROJECT_TO_PAPER_FK),
    FOREIGN KEY (PROJECT_FK) REFERENCES PROJECT(PROJECT_FK),
    FOREIGN KEY (SOURCE_FK) REFERENCES paper_roots(SOURCE_FK) ON DELETE CASCADE
);
-- idx_project_to_paper_unique on (PROJECT_FK, SOURCE_FK) is deliberately NOT created
-- here: a pre-existing DB can hold duplicate membership rows, and creating the index
-- from apply_tables would abort startup with "UNIQUE constraint failed" before the
-- dedup could run. Instead init_db runs migrations::dedup_project_to_paper BEFORE
-- apply_tables (it must precede this file too — see that fn: once PAPER_TO_READING's
-- composite FK exists, dedup DML on the unindexed parent key is itself an error),
-- and the project_to_paper_unique_index migration creates the index afterwards.
-- That's safe for PAPER_TO_READING's composite FK on these two columns: SQLite only
-- requires the parent key to be uniquely indexed by the time of DML, not at
-- CREATE TABLE time — and migrations run before any reading-status write.
