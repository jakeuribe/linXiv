-- Lookup indexes for the link tables, which previously had none beyond their
-- PKs/unique indexes — so every per-paper author list, tag lookup, note listing,
-- and reverse membership lookup was a full table scan (and every FK cascade from
-- PAPER/PAPER_ROOTS scanned the child table). Purely for speed; every query on
-- these predicates carries its own ORDER BY, so plans may change but results don't.
CREATE INDEX IF NOT EXISTS idx_paper_to_author_paper_id ON PAPER_TO_AUTHOR (PAPER_ID);
CREATE INDEX IF NOT EXISTS idx_paper_to_author_author_fk ON PAPER_TO_AUTHOR (AUTHOR_FK);
CREATE INDEX IF NOT EXISTS idx_paper_to_tag_paper_id ON PAPER_TO_TAG (PAPER_ID);
CREATE INDEX IF NOT EXISTS idx_paper_to_tag_tag_fk ON PAPER_TO_TAG (TAG_FK);
CREATE INDEX IF NOT EXISTS idx_project_to_paper_source_fk ON PROJECT_TO_PAPER (SOURCE_FK);
CREATE INDEX IF NOT EXISTS idx_project_to_tag_tag_fk ON PROJECT_TO_TAG (TAG_FK);
CREATE INDEX IF NOT EXISTS idx_note_source_fk ON NOTE (SOURCE_FK);
CREATE INDEX IF NOT EXISTS idx_note_project_fk ON NOTE (PROJECT_FK);
CREATE INDEX IF NOT EXISTS idx_note_paper_id_fk ON NOTE (PAPER_ID_FK);
