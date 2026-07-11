CREATE TABLE IF NOT EXISTS PROJECT(
    PROJECT_FK      INTEGER NOT NULL,
    NAME            TEXT    NOT NULL,
    DESCRIPTION     TEXT    DEFAULT '',
    COLOR           INTEGER,
    STATUS          TEXT    NOT NULL DEFAULT 'active',
    -- 0 = normal project, 1 = reading list (per-paper status lives in PAPER_TO_READING).
    -- Also added via an idempotent migration (project_reading_list_flag) so
    -- pre-existing DBs upgrade in place; that migration is a no-op here.
    IS_READING_LIST INTEGER NOT NULL DEFAULT 0,
    CREATED_AT      TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    UPDATED_AT      TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    ARCHIVED_AT     TIMESTAMP,
    PRIMARY KEY (PROJECT_FK)
);
