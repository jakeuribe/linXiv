-- SOURCE_FK is required; a note always belongs to a paper across all versions by default.
-- PAPER_ID_FK is optional: when present, the note is pinned to a specific version of the paper.
-- PROJECT_FK is optional: when present, the note is scoped to a project rather than the whole library.
CREATE TABLE IF NOT EXISTS NOTE(
    NOTE_SK     INTEGER NOT NULL,
    SOURCE_FK   INTEGER NOT NULL,
    PAPER_ID_FK INTEGER,
    PROJECT_FK  INTEGER,
    TITLE       TEXT,
    NOTE        BLOB,
    -- Optional playback position for media-linked notes. Milliseconds keeps the
    -- value provider-neutral for export/import and P2P sharing.
    MEDIA_TIME_MS INTEGER CHECK (MEDIA_TIME_MS IS NULL OR MEDIA_TIME_MS >= 0),
    -- Optional item inside a media collection (for example, a video in a playlist).
    MEDIA_ITEM_ID TEXT,
    -- Stable identity across export/import + share (uuid v4). Backfilled + made
    -- UNIQUE by the note_uuid migration; new rows get one at insert.
    NOTE_UUID   TEXT,
    CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (NOTE_SK),
    FOREIGN KEY (SOURCE_FK)   REFERENCES PAPER_ROOTS(SOURCE_FK) ON DELETE CASCADE,
    FOREIGN KEY (PAPER_ID_FK) REFERENCES PAPER(PAPER_ID)        ON DELETE SET NULL,
    FOREIGN KEY (PROJECT_FK)  REFERENCES PROJECT(PROJECT_FK)
);
