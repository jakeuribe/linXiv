-- Same use case as PAPER_ROOTS, only specific to RSS feeds.
-- REMOVAL_TYPE: root-level dismiss state, ignores version.
-- 'NOT' -- Not removed, currently shown in the feed.
-- 'DOI' -- The user dismissed the paper permanently; no version of it (past or
--          future) should reappear in the feed.
-- Per-version dismiss ('VER') lives on RSS_PAPER instead -- see that table.
CREATE TABLE IF NOT EXISTS RSS_PAPER_ROOTS (
    SOURCE_FK    INTEGER   PRIMARY KEY AUTOINCREMENT,
    SOURCE_ID    TEXT      NOT NULL UNIQUE,
    STATUS       TEXT      NOT NULL DEFAULT 'active',
    REMOVAL_TYPE TEXT      CHECK(REMOVAL_TYPE IN ('NOT','DOI')) DEFAULT 'NOT',
    REMOVED_AT   TIMESTAMP,
    CREATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    UPDATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now'))
);

-- Every feed GET scans for 'DOI' rows (queries::rss::blocked_source_ids).
CREATE INDEX IF NOT EXISTS IDX_RSS_PAPER_ROOTS_REMOVAL_TYPE ON RSS_PAPER_ROOTS(REMOVAL_TYPE);
