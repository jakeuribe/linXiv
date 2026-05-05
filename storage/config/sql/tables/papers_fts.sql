CREATE VIRTUAL TABLE IF NOT EXISTS papers_fts USING fts5(paper_id, full_text);
