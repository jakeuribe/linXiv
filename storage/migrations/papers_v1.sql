-- Migration v1: add columns absent from the original papers schema
ALTER TABLE papers ADD COLUMN updated           DATE;
ALTER TABLE papers ADD COLUMN categories        LIST;
ALTER TABLE papers ADD COLUMN journal_ref       TEXT;
ALTER TABLE papers ADD COLUMN comment           TEXT;
ALTER TABLE papers ADD COLUMN tags              LIST;
ALTER TABLE papers ADD COLUMN has_pdf           BOOL NOT NULL DEFAULT 0;
ALTER TABLE papers ADD COLUMN source            TEXT DEFAULT 'arxiv';
ALTER TABLE papers ADD COLUMN pdf_path          TEXT DEFAULT NULL;
ALTER TABLE papers ADD COLUMN full_text         TEXT;
ALTER TABLE papers ADD COLUMN downloaded_source BOOL DEFAULT 0;
