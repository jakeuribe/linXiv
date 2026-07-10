-- =============================================================================
-- Data migration: old (blue) -> new (green)
-- Run this AGAINST the new DB, AFTER the DDL files have created the schema.
-- The old DB must be ATTACHed as `old` before running.
--
-- Assumes (from your clarifications):
--   * old.papers.paper_id is a bare natural string id (e.g. '2401.12345')
--   * (paper_id, version) is unique in old.papers
--   * old.papers.authors / tags are JSON arrays of strings
--   * old.papers.source identifies the backend ('arxiv', 'openalex', 'crossref',
--     'semanticscholar', 'pdf', NULL)
--   * old.projects.paper_ids is a JSON array of old paper_id strings
--   * old.projects.project_tags is a JSON array of tag strings
--   * old.notes carries no version info -> migrate as paper-level (PAPER_ID_FK NULL)
--   * source string in old.papers (e.g. 'arxiv') becomes PAPER_META.PROVIDER
--
-- SOURCE_ID in new schema is NAMESPACED: 'arxiv:2204.12985', 'openalex:W3123456789',
-- 'doi:10.48550/...', 'local:{hash}'. Bare old paper_ids are prefixed here.
--
-- Namespace prefix rules (applied consistently throughout):
--   old.source = 'arxiv'                         -> 'arxiv:'   || paper_id
--   old.source = 'openalex'                      -> 'openalex:'|| paper_id
--   old.source IN ('crossref','semanticscholar')  -> 'doi:'     || paper_id
--   old.source = 'pdf'                            -> 'local:'   || paper_id
--     (if old paper_id starts with 'pdf:' the prefix is replaced, not doubled)
--   old.source IS NULL                            -> 'linxiv:'  || paper_id (unknown source)
--   old.source = 'arxiv' or anything unrecognised -> source     || ':' || paper_id
-- =============================================================================

PRAGMA foreign_keys = OFF;
BEGIN;

-- ---------------------------------------------------------------------------
-- Scratch mapping tables (dropped at end).
-- ---------------------------------------------------------------------------

CREATE TEMP TABLE map_paper (
    old_paper_id TEXT NOT NULL,
    old_version  INTEGER NOT NULL,
    new_paper_id INTEGER NOT NULL,   -- the integer surrogate from PAPER
    source_fk    INTEGER NOT NULL,
    PRIMARY KEY (old_paper_id, old_version)
);

-- ---------------------------------------------------------------------------
-- Helper: one row per distinct old paper_id with its namespaced SOURCE_ID.
-- Groups by paper_id so each distinct paper gets exactly one mapping.
-- ---------------------------------------------------------------------------

CREATE TEMP TABLE map_source_id (
    old_paper_id TEXT NOT NULL PRIMARY KEY,
    new_source_id TEXT NOT NULL
);

INSERT INTO map_source_id (old_paper_id, new_source_id)
SELECT
    paper_id,
    CASE
        WHEN COALESCE(source, 'arxiv') = 'openalex'
            THEN 'openalex:' || paper_id
        WHEN COALESCE(source, 'arxiv') IN ('crossref', 'semanticscholar', 'doi')
            THEN 'doi:' || paper_id
        WHEN COALESCE(source, 'arxiv') = 'pdf'
            THEN CASE
                     WHEN paper_id LIKE 'pdf:%'   THEN 'local:' || substr(paper_id, 5)
                     WHEN paper_id LIKE 'local:%' THEN paper_id
                     ELSE 'local:' || paper_id
                 END
        WHEN source IS NULL THEN 'linxiv:' || paper_id   -- no source recorded; use app namespace
        ELSE COALESCE(source, 'linxiv') || ':' || paper_id  -- pass unknown sources through as-is
    END
FROM old.papers
GROUP BY paper_id;

-- ---------------------------------------------------------------------------
-- 1. PAPER_ROOTS: one row per distinct old paper_id, SOURCE_ID namespaced.
--    All migrated papers are active (old schema had no soft-delete).
-- ---------------------------------------------------------------------------

INSERT INTO PAPER_ROOTS (SOURCE_ID, STATUS, DELETED_AT)
SELECT new_source_id, 'active', NULL
FROM map_source_id
ORDER BY new_source_id;

-- ---------------------------------------------------------------------------
-- 2. AUTHOR: one row per distinct author name across all papers.
--    AUTHOR_FULL_NAME only; AUTHOR_FIRST / AUTHOR_LAST are populated by the
--    Python wrapper (SQLite has no reverse()/rinstr(), so a SQL-only split
--    is brittle).
-- ---------------------------------------------------------------------------

INSERT INTO AUTHOR (AUTHOR_FULL_NAME)
SELECT DISTINCT TRIM(j.value)
FROM old.papers p, json_each(p.authors) j
WHERE TRIM(j.value) <> ''
ORDER BY TRIM(j.value);

-- ---------------------------------------------------------------------------
-- 3. TAG: one row per distinct tag string across all papers AND projects
-- ---------------------------------------------------------------------------

INSERT INTO TAG (TAG)
SELECT DISTINCT TRIM(t) AS tag FROM (
    SELECT j.value AS t FROM old.papers p, json_each(p.tags) j
    WHERE TRIM(j.value) <> ''
    UNION
    SELECT j.value AS t FROM old.projects pr, json_each(pr.project_tags) j
    WHERE TRIM(j.value) <> ''
);

-- ---------------------------------------------------------------------------
-- 4. PROJECT: copy from old.projects, preserving id as PROJECT_FK
-- ---------------------------------------------------------------------------

INSERT INTO PROJECT (PROJECT_FK, NAME, DESCRIPTION, COLOR, STATUS,
                     CREATED_AT, UPDATED_AT, ARCHIVED_AT)
SELECT
    id,
    name,
    COALESCE(description, ''),
    color,
    COALESCE(status, 'active'),
    COALESCE(created_at, datetime('now')),
    COALESCE(updated_at, datetime('now')),
    archived_at
FROM old.projects;

-- ---------------------------------------------------------------------------
-- 5. PAPER: one row per (paper_id, version) in old.papers
--    SOURCE_ID is the namespaced form; SOURCE_FK from PAPER_ROOTS
-- ---------------------------------------------------------------------------

INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK)
SELECT
    ms.new_source_id,
    p.version,
    COALESCE(p.title, ''),
    p.category,
    COALESCE(p.has_pdf, 0),
    pr.SOURCE_FK
FROM old.papers p
JOIN map_source_id ms ON ms.old_paper_id = p.paper_id
JOIN PAPER_ROOTS pr ON pr.SOURCE_ID = ms.new_source_id;

-- Populate the mapping table for downstream FK lookups.
-- Join back through map_source_id to recover the bare OLD paper_id,
-- which is what old.papers.paper_id uses in steps 6–8.
INSERT INTO map_paper (old_paper_id, old_version, new_paper_id, source_fk)
SELECT ms.old_paper_id, p.VERSION, p.PAPER_ID, p.SOURCE_FK
FROM PAPER p
JOIN map_source_id ms ON ms.new_source_id = p.SOURCE_ID;

-- ---------------------------------------------------------------------------
-- 6. PAPER_META: one row per PAPER row
--    AUTHORS / TAGS / CATEGORIES stored as JSON strings (the LIST columns)
-- ---------------------------------------------------------------------------

INSERT INTO PAPER_META (
    PAPER_ID, URL, PUBLISHED, UPDATED, CATEGORIES, DOI, JOURNAL_REF,
    COMMENT, SUMMARY, PROVIDER, PDF_PATH, FULL_TEXT, DOWNLOADED_SOURCE,
    AUTHORS, TAGS
)
SELECT
    m.new_paper_id,
    p.url,
    p.published,
    p.updated,
    p.categories,
    p.doi,
    p.journal_ref,
    p.comment,
    p.summary,
    COALESCE(p.source, 'arxiv'),
    p.pdf_path,
    p.full_text,
    COALESCE(p.downloaded_source, 0),
    p.authors,
    p.tags
FROM old.papers p
JOIN map_paper m
  ON m.old_paper_id = p.paper_id AND m.old_version = p.version;

-- ---------------------------------------------------------------------------
-- 7. PAPER_TO_AUTHOR: explode old.papers.authors JSON array
--    AUTHOR_INDEX preserved from JSON ordering (json_each.key gives 0-based idx)
-- ---------------------------------------------------------------------------

INSERT INTO PAPER_TO_AUTHOR (PAPER_ID, AUTHOR_FK, AUTHOR_INDEX)
SELECT
    m.new_paper_id,
    a.AUTHOR_FK,
    j.key AS author_index
FROM old.papers p
JOIN map_paper m
  ON m.old_paper_id = p.paper_id AND m.old_version = p.version
JOIN json_each(p.authors) j
JOIN AUTHOR a ON a.AUTHOR_FULL_NAME = TRIM(j.value)
WHERE TRIM(j.value) <> '';

-- ---------------------------------------------------------------------------
-- 8. PAPER_TO_TAG: explode old.papers.tags JSON array
--    SOURCE_ID stored in the namespaced form
-- ---------------------------------------------------------------------------

INSERT INTO PAPER_TO_TAG (PAPER_ID, SOURCE_ID, VERSION, TAG_FK)
SELECT
    m.new_paper_id,
    ms.new_source_id,
    p.version,
    t.TAG_FK
FROM old.papers p
JOIN map_source_id ms ON ms.old_paper_id = p.paper_id
JOIN map_paper m
  ON m.old_paper_id = p.paper_id AND m.old_version = p.version
JOIN json_each(p.tags) j
JOIN TAG t ON t.TAG = TRIM(j.value)
WHERE TRIM(j.value) <> '';

-- ---------------------------------------------------------------------------
-- 9. PROJECT_TO_PAPER: explode old.projects.paper_ids JSON array
--    Maps each old bare paper_id -> SOURCE_FK in PAPER_ROOTS (via namespaced id)
--    OR IGNORE: legacy paper_ids arrays may contain duplicates, which would
--    otherwise abort under idx_project_to_paper_unique.
-- ---------------------------------------------------------------------------

INSERT OR IGNORE INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK)
SELECT
    row_number() OVER (ORDER BY pr.id, j.key) AS pk,
    pr.id,
    roots.SOURCE_FK
FROM old.projects pr
JOIN json_each(pr.paper_ids) j
JOIN map_source_id ms ON ms.old_paper_id = TRIM(j.value)
JOIN PAPER_ROOTS roots ON roots.SOURCE_ID = ms.new_source_id
WHERE TRIM(j.value) <> '';

-- ---------------------------------------------------------------------------
-- 10. PROJECT_TO_TAG: explode old.projects.project_tags JSON array
--     OR IGNORE: duplicate legacy tags would abort under
--     idx_project_to_tag_unique.
-- ---------------------------------------------------------------------------

INSERT OR IGNORE INTO PROJECT_TO_TAG (PROJECT_TO_TAG_FK, PROJECT_FK, TAG_FK)
SELECT
    row_number() OVER (ORDER BY pr.id, j.key) AS pk,
    pr.id,
    t.TAG_FK
FROM old.projects pr
JOIN json_each(pr.project_tags) j
JOIN TAG t ON t.TAG = TRIM(j.value)
WHERE TRIM(j.value) <> '';

-- ---------------------------------------------------------------------------
-- 11. NOTE: copy from old.notes.
--     Old schema has no version on notes -> SOURCE_FK only, PAPER_ID_FK NULL.
--     NOTE_SK reuses old.notes.id.
--     Requires joining through old.papers to resolve the namespaced SOURCE_ID.
-- ---------------------------------------------------------------------------

INSERT INTO NOTE (NOTE_SK, SOURCE_FK, PAPER_ID_FK, PROJECT_FK,
                  TITLE, NOTE, CREATED_AT, UPDATED_AT)
SELECT
    n.id,
    roots.SOURCE_FK,
    NULL,                              -- paper-level, not version-pinned
    n.project_id,
    n.title,
    n.content,
    COALESCE(n.created_at, datetime('now')),
    COALESCE(n.updated_at, datetime('now'))
FROM old.notes n
JOIN map_source_id ms ON ms.old_paper_id = n.paper_id
JOIN PAPER_ROOTS roots ON roots.SOURCE_ID = ms.new_source_id;

-- Notes with no paper_id (project-level notes): keep them, but they need a
-- SOURCE_FK which is NOT NULL in the new schema. Surface them for the wrapper.

CREATE TEMP TABLE _orphan_notes_count AS
SELECT COUNT(*) AS n FROM old.notes WHERE paper_id IS NULL;

-- ---------------------------------------------------------------------------
-- 12. Rebuild papers_fts from PAPER_META.FULL_TEXT
--     paper_id column holds the namespaced SOURCE_ID so that
--     search_full_text's JOIN (p.source_id = fts.paper_id) resolves correctly.
-- ---------------------------------------------------------------------------

INSERT INTO papers_fts (rowid, paper_id, full_text)
SELECT
    pm.PAPER_ID,
    p.SOURCE_ID,
    pm.FULL_TEXT
FROM PAPER_META pm
JOIN PAPER p ON p.PAPER_ID = pm.PAPER_ID
WHERE pm.FULL_TEXT IS NOT NULL AND pm.FULL_TEXT <> '';

-- ---------------------------------------------------------------------------
-- Cleanup
-- ---------------------------------------------------------------------------

DROP TABLE map_paper;
DROP TABLE map_source_id;

COMMIT;
PRAGMA foreign_keys = ON;
