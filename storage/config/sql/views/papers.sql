DROP VIEW IF EXISTS latest_papers;
DROP VIEW IF EXISTS papers;

CREATE VIEW papers AS
SELECT
    p.paper_id    AS paper_id,
    p.version     AS version,
    p.title       AS title,
    m.url         AS url,
    m.published   AS published,
    m.updated     AS updated,
    p.category    AS category,
    m.categories  AS categories,
    m.doi         AS doi,
    m.journal_ref AS journal_ref,
    m.comment     AS comment,
    m.summary     AS summary,
    m.authors     AS authors,
    m.tags        AS tags,
    p.has_pdf     AS has_pdf,
    m.source      AS source,
    m.pdf_path    AS pdf_path,
    m.full_text   AS full_text,
    m.downloaded_source AS downloaded_source,
    p.created_at  AS created_at,
    p.updated_at  AS updated_at
FROM PAPER p
JOIN PAPER_META m USING (paper_id, version);

CREATE VIEW latest_papers AS
SELECT * FROM papers v
WHERE version = (
    SELECT MAX(version) FROM PAPER x WHERE x.paper_id = v.paper_id
);
