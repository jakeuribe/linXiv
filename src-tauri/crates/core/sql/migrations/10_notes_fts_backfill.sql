INSERT INTO notes_fts(rowid, title, note, source_fk)
 SELECT NOTE_SK, TITLE, CAST(NOTE AS TEXT), SOURCE_FK FROM NOTE
 WHERE NOTE_SK NOT IN (SELECT rowid FROM notes_fts)
