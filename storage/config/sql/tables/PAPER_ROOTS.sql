CREATE TABLE IF NOT EXISTS PAPER_ROOTS (
    paper_id    TEXT PRIMARY KEY,
    created_at  DATE NOT NULL DEFAULT (date('now')),
    updated_at  DATE NOT NULL DEFAULT (date('now'))
);
