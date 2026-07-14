-- Same use case as paper_roots, only specific to rss feeds, rss feeds are notoriously clunky, 
-- So we're going to try to use a little sql to make it better.
-- REMOVAL_TYPE; We give the users multiple options as to how they want to remove an item from their RSS feed
-- 'NOT' -- Not removed, currently in their RSS feed.
== 'VER' -- The of this specific paper version is removed, this can happen in multiple cases, 
-- One is that the user selected this option, and it was saved to this table. Not displayed in feed.
-- Another is that the user has this exact paper version from arxiv in their database
-- TODO: Add versions or DOI's to this?
-- TODO: Use check for other enums?
-- 'DOI': The user has selected that they want to see no versions of this table again.
CREATE TABLE IF NOT EXISTS RSS_PAPER_ROOTS (
    SOURCE_FK    INTEGER   PRIMARY KEY AUTOINCREMENT,
    SOURCE_ID    TEXT      NOT NULL UNIQUE,
    STATUS       TEXT      NOT NULL DEFAULT 'active',
    REMOVAL_TYPE TEXT      CHECK(REMOVAL_TYPE IN ('NOT','VER','DOI')) DEFAULT 'NOT',
    REMOVED_AT   TIMESTAMP,
    CREATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    UPDATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now'))
);
