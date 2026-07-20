-- REMOVAL_TYPE: per-version dismiss state.
-- 'NOT' -- Not removed, currently shown in the feed (default -- a new version
--          is a new row, so it surfaces even if an earlier version was 'VER').
-- 'VER' -- This exact version was dismissed by the user.
CREATE TABLE IF NOT EXISTS RSS_PAPER(
    PAPER_ID    INTEGER PRIMARY KEY AUTOINCREMENT,
    SOURCE_ID   TEXT    NOT NULL,
    VERSION     INTEGER NOT NULL,
    TITLE       TEXT    NOT NULL,
    CATEGORY    TEXT,
    HAS_PDF     BOOL NOT NULL DEFAULT 0,
    REMOVAL_TYPE TEXT      CHECK(REMOVAL_TYPE IN ('NOT','VER')) DEFAULT 'NOT',
    REMOVED_AT   TIMESTAMP,
    CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    SOURCE_FK   INTEGER NOT NULL,
    UNIQUE (SOURCE_ID, VERSION),
    FOREIGN KEY (SOURCE_FK) REFERENCES RSS_PAPER_ROOTS(SOURCE_FK) ON DELETE CASCADE
);

-- Every feed GET scans for 'VER' rows (queries::rss::dismissed_versions).
CREATE INDEX IF NOT EXISTS IDX_RSS_PAPER_REMOVAL_TYPE ON RSS_PAPER(REMOVAL_TYPE);

-- Unindexed FK child columns force a full table scan per parent delete (SQLite
-- foreign key docs 4.1) -- needed by both the ON DELETE CASCADE and prune_dismissed's
-- orphan-root NOT EXISTS check.
CREATE INDEX IF NOT EXISTS IDX_RSS_PAPER_SOURCE_FK ON RSS_PAPER(SOURCE_FK);
