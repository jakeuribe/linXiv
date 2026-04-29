-- Migration: rebuild papers table to add paper_roots FK constraint
CREATE TABLE papers_new (
    paper_id    TEXT    NOT NULL,
    version     INTEGER NOT NULL,
    title       TEXT    NOT NULL,
    url         TEXT,
    published   DATE,
    updated     DATE,
    category    TEXT,
    categories  LIST,
    doi         TEXT,
    journal_ref TEXT,
    comment     TEXT,
    summary     TEXT,
    authors     LIST,
    tags        LIST,
    has_pdf     BOOL NOT NULL DEFAULT 0,
    source      TEXT DEFAULT 'arxiv',
    pdf_path    TEXT DEFAULT NULL,
    full_text   TEXT,
    downloaded_source BOOL DEFAULT 0,
    PRIMARY KEY (paper_id, version),
    FOREIGN KEY (paper_id) REFERENCES paper_roots(paper_id) ON DELETE CASCADE
);
