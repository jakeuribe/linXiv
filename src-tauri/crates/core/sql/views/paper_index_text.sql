-- papers_fts is derived from PAPER_META.FULL_TEXT. This file is the derivation:
-- one view saying what belongs in the index, and the triggers that keep the
-- index equal to it.
--
-- `paper_index_text` is the ONE definition of "the body a paper is searchable
-- by": the newest version that has any text, keyed by SOURCE_ID (papers_fts's
-- key, not PAPER_ID) and gated on the root still being active — inherited from
-- `papers`, so a soft-deleted paper yields no row and cannot be indexed. Rust's
-- `refresh_fts` runs the same two statements against the same view, so the
-- automatic path and the hand-called one cannot disagree.
--
-- Applied in the VIEWS phase, after the migrations, and the triggers ship with
-- the view rather than with the table: SQLite compiles a trigger body when it
-- prepares the DML that fires it, so a trigger that reads this view must not
-- exist during a phase where the view doesn't (a migration that touches
-- PAPER_META would fail to prepare at all).
DROP VIEW IF EXISTS paper_index_text;

CREATE VIEW paper_index_text AS
SELECT
    v.source_id AS source_id,
    v.full_text AS full_text
FROM papers v
WHERE COALESCE(v.full_text, '') != ''
  AND v.version = (
      SELECT MAX(x.version) FROM papers x
      WHERE x.source_id = v.source_id AND COALESCE(x.full_text, '') != ''
  );

-- fts5 has no UPDATE, hence DELETE then INSERT. The INSERT selects from the
-- view, so it writes nothing when the paper no longer belongs in the index
-- (text cleared, or the root soft-deleted while its FULL_TEXT is still stored).
CREATE TRIGGER IF NOT EXISTS papers_fts_meta_ai AFTER INSERT ON PAPER_META
    WHEN COALESCE(new.FULL_TEXT, '') != ''
BEGIN
    DELETE FROM papers_fts
     WHERE paper_id = (SELECT SOURCE_ID FROM PAPER WHERE PAPER_ID = new.PAPER_ID);
    INSERT INTO papers_fts (paper_id, full_text)
    SELECT source_id, full_text FROM paper_index_text
     WHERE source_id = (SELECT SOURCE_ID FROM PAPER WHERE PAPER_ID = new.PAPER_ID);
END;

CREATE TRIGGER IF NOT EXISTS papers_fts_meta_au AFTER UPDATE OF FULL_TEXT ON PAPER_META
    WHEN old.FULL_TEXT IS NOT new.FULL_TEXT
BEGIN
    DELETE FROM papers_fts
     WHERE paper_id = (SELECT SOURCE_ID FROM PAPER WHERE PAPER_ID = new.PAPER_ID);
    INSERT INTO papers_fts (paper_id, full_text)
    SELECT source_id, full_text FROM paper_index_text
     WHERE source_id = (SELECT SOURCE_ID FROM PAPER WHERE PAPER_ID = new.PAPER_ID);
END;
