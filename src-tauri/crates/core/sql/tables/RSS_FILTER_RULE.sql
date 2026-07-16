-- Auto-filter rules applied to home RSS feed entries before they reach the client.
-- Each rule matches KEYWORDS (comma-separated, ALL must match = AND) against one FIELD.
-- An entry is hidden if any enabled DENY rule matches it, UNLESS an enabled ALLOW
-- rule also matches -- e.g. DENY "AI,quantum computing" on TITLE, ALLOW "ML" on SUMMARY
-- lets a quantum-computing/AI paper back in if its abstract mentions ML.
CREATE TABLE IF NOT EXISTS RSS_FILTER_RULE (
    RULE_ID     INTEGER PRIMARY KEY AUTOINCREMENT,
    FIELD       TEXT    NOT NULL CHECK(FIELD IN ('TITLE','SUMMARY','AUTHOR')),
    KEYWORDS    TEXT    NOT NULL,
    ACTION      TEXT    NOT NULL CHECK(ACTION IN ('DENY','ALLOW')) DEFAULT 'DENY',
    ENABLED     BOOL    NOT NULL DEFAULT 1,
    CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now'))
);
