-- Persisted per-URL RSS/Atom feed entries -- replaces the old ephemeral
-- in-memory last-response cache so a re-visit doesn't lose yesterday's
-- entries just because today's upstream fetch came back empty (e.g. arXiv
-- publishes nothing on weekends).
-- DEDUP_KEY: arxiv source_id+version when the entry has one, else its link
-- (falls back to title if even that's missing) -- see feed.rs::to_cache_entry.
-- SOURCE_ID: arxiv:{id} (no version) when present -- carried alongside the
-- entry purely for debugging/future use; the actual durable dismissal lives
-- in RSS_PAPER_ROOTS, which annotate_and_filter checks directly.
CREATE TABLE IF NOT EXISTS RSS_CACHE_ENTRY (
    ENTRY_FK     INTEGER PRIMARY KEY AUTOINCREMENT,
    FEED_URL     TEXT      NOT NULL,
    DEDUP_KEY    TEXT      NOT NULL,
    SOURCE_ID    TEXT,
    ENTRY_JSON   TEXT      NOT NULL,
    PUBLISHED_AT TIMESTAMP,
    FETCHED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    -- Also serves as the FEED_URL lookup index for the load/prune queries below
    -- (leftmost-prefix match) -- no separate index on FEED_URL alone needed.
    UNIQUE (FEED_URL, DEDUP_KEY)
);
