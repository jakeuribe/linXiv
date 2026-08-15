-- FTS5 index over note TITLE + NOTE (content). Mirrors papers_fts: sync is done
-- with triggers, here on the NOTE table, there on PAPER_META.
-- source_fk is UNINDEXED — carried only so search can join back to the paper.
-- rowid == NOTE_SK so a note's index row is addressable for update/delete.
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(title, note, source_fk UNINDEXED);

CREATE TRIGGER IF NOT EXISTS notes_fts_ai AFTER INSERT ON NOTE BEGIN
    INSERT INTO notes_fts(rowid, title, note, source_fk)
    VALUES (new.NOTE_SK, new.TITLE, CAST(new.NOTE AS TEXT), new.SOURCE_FK);
END;

CREATE TRIGGER IF NOT EXISTS notes_fts_ad AFTER DELETE ON NOTE BEGIN
    DELETE FROM notes_fts WHERE rowid = old.NOTE_SK;
END;

CREATE TRIGGER IF NOT EXISTS notes_fts_au AFTER UPDATE ON NOTE
    WHEN old.TITLE IS NOT new.TITLE OR old.NOTE IS NOT new.NOTE OR old.SOURCE_FK IS NOT new.SOURCE_FK
BEGIN
    DELETE FROM notes_fts WHERE rowid = old.NOTE_SK;
    INSERT INTO notes_fts(rowid, title, note, source_fk)
    VALUES (new.NOTE_SK, new.TITLE, CAST(new.NOTE AS TEXT), new.SOURCE_FK);
END;
