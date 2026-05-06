-- SOURCE_FK is required; a note always belongs to a paper across all versions by default.
-- PAPER_ID_FK is optional: when present, the note is pinned to a specific version of the paper.
-- PROJECT_FK is optional: when present, the note is scoped to a project rather than the whole library.
CREATE TABLE IF NOT EXISTS LIBRARY_NOTE(
    NOTE_SK     INTEGER NOT NULL,
    SOURCE_FK   INTEGER NOT NULL,
    PAPER_ID_FK INTEGER,
    PROJECT_FK  INTEGER,
    TITLE       TEXT,
    NOTE        BLOB,
    CREATED_AT  TEXT NOT NULL DEFAULT (date('now')),
    UPDATED_AT  TEXT NOT NULL DEFAULT (date('now')),
    PRIMARY KEY (NOTE_SK),
    FOREIGN KEY (SOURCE_FK)   REFERENCES PAPER_ROOTS(SOURCE_FK) ON DELETE CASCADE,
    FOREIGN KEY (PAPER_ID_FK) REFERENCES PAPER(PAPER_ID)        ON DELETE SET NULL,
    FOREIGN KEY (PROJECT_FK)  REFERENCES PROJECT(PROJECT_FK)
);
