DROP VIEW IF EXISTS latest_papers;
DROP VIEW IF EXISTS papers;

CREATE VIEW papers AS
SELECT
    p.PAPER_ID          AS paper_id,
    p.SOURCE_ID         AS source_id,
    p.VERSION           AS version,
    p.TITLE             AS title,
    m.URL               AS url,
    m.PUBLISHED         AS published,
    m.UPDATED           AS updated,
    p.CATEGORY          AS category,
    m.CATEGORIES        AS categories,
    m.DOI               AS doi,
    m.JOURNAL_REF       AS journal_ref,
    m.COMMENT           AS comment,
    m.SUMMARY           AS summary,
    m.AUTHORS           AS authors,
    m.TAGS              AS tags,
    p.HAS_PDF           AS has_pdf,
    m.SOURCE            AS source,
    m.PDF_PATH          AS pdf_path,
    m.FULL_TEXT         AS full_text,
    m.DOWNLOADED_SOURCE AS downloaded_source,
    p.CREATED_AT        AS created_at,
    p.UPDATED_AT        AS updated_at
FROM PAPER p
JOIN PAPER_META m USING (PAPER_ID);

CREATE VIEW latest_papers AS
SELECT * FROM papers v
WHERE version = (
    SELECT MAX(VERSION) FROM PAPER x WHERE x.SOURCE_ID = v.source_id
);
