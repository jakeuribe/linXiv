CREATE TABLE IF NOT EXISTS PAPER(
    paper_id    TEXT    NOT NULL,
    version     INTEGER NOT NULL,
    title       TEXT    NOT NULL,
    category    TEXT,
    has_pdf     BOOL NOT NULL DEFAULT 0,
    created_at  DATE NOT NULL DEFAULT (date('now')),
    updated_at  DATE NOT NULL DEFAULT (date('now')),
    PRIMARY KEY (paper_id, version),
    FOREIGN KEY (paper_id) REFERENCES paper_roots(paper_id) ON DELETE CASCADE
);
