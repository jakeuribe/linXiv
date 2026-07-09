-- Per-paper reading status inside a reading-list PROJECT, stored SPARSELY:
-- the default status (unread) is the ABSENCE of a row, so only deviations
-- (reading / read) ever get persisted. Setting a paper back to unread deletes
-- its row. STATUS holds only the non-default values 'reading' | 'read'.
-- Keyed on (PROJECT_FK, SOURCE_FK): one status per paper per reading list,
-- covering all versions of the paper (SOURCE_FK → PAPER_ROOTS, like membership).
CREATE TABLE IF NOT EXISTS PAPER_TO_READING(
    PROJECT_FK  INTEGER NOT NULL,
    SOURCE_FK   INTEGER NOT NULL,
    STATUS      TEXT    NOT NULL CHECK (STATUS IN ('reading', 'read')),
    UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (PROJECT_FK, SOURCE_FK),
    FOREIGN KEY (PROJECT_FK) REFERENCES PROJECT(PROJECT_FK) ON DELETE CASCADE,
    FOREIGN KEY (SOURCE_FK)  REFERENCES PAPER_ROOTS(SOURCE_FK) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_paper_to_reading_source_fk ON PAPER_TO_READING (SOURCE_FK);
