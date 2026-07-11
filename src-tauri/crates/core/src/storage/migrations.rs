//! Idempotent startup migrations. Plan §5.3 + D6.
//!
//! NON-NEGOTIABLE: every migration here runs on EVERY startup against real user
//! DBs, so each MUST be idempotent — guarded by `PRAGMA table_info` (missing
//! column) or by index existence. Re-running them is the normal case, not the
//! exception (in practice the column/index already exists and each is a no-op).

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::Result;

/// Run all twelve idempotent post-schema migrations in order. Call between
/// `apply_tables` and `apply_views` (views reference columns these add) — see
/// `super::init_db`, which also runs `dedup_project_to_paper` BEFORE apply_tables.
pub fn run_migrations(conn: &Connection) -> Result<()> {
    paper_roots_soft_delete(conn)?;
    paper_meta_provider(conn)?;
    search_state_sort_json(conn)?;
    tag_label_unique_index(conn)?;
    project_to_tag_unique_index(conn)?;
    project_to_paper_unique_index(conn)?;
    author_full_name_index(conn)?;
    annotation_table(conn)?;
    version_check_table(conn)?;
    notes_fts_backfill(conn)?;
    project_reading_list_flag(conn)?;
    paper_to_reading_cascade_fk(conn)?;
    Ok(())
}

// ── guards ────────────────────────────────────────────────────────────────

/// Column-present check via `PRAGMA table_info` (col 1 = name). Table names are
/// hardcoded constants below — PRAGMA cannot bind them, so format! is safe here.
fn has_column(conn: &Connection, table: &str, col: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(names.iter().any(|n| n.eq_ignore_ascii_case(col)))
}

fn index_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name = ?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Whether PAPER_TO_READING already has a FK referencing PROJECT_TO_PAPER
/// (column 2 of `PRAGMA foreign_key_list` is the referenced table name).
fn paper_to_reading_has_cascade_fk(conn: &Connection) -> Result<bool> {
    Ok(conn
        .prepare("PRAGMA foreign_key_list(PAPER_TO_READING)")?
        .query_map([], |r| r.get::<_, String>(2))?
        .collect::<rusqlite::Result<Vec<String>>>()?
        .iter()
        .any(|t| t.eq_ignore_ascii_case("PROJECT_TO_PAPER")))
}

// ── 1. PAPER_ROOTS soft-delete columns ──────────────────────────────────────

fn paper_roots_soft_delete(conn: &Connection) -> Result<()> {
    if !has_column(conn, "PAPER_ROOTS", "STATUS")? {
        conn.execute_batch(
            "ALTER TABLE PAPER_ROOTS ADD COLUMN STATUS TEXT NOT NULL DEFAULT 'active'",
        )?;
    }
    if !has_column(conn, "PAPER_ROOTS", "DELETED_AT")? {
        conn.execute_batch("ALTER TABLE PAPER_ROOTS ADD COLUMN DELETED_AT TIMESTAMP")?;
    }
    Ok(())
}

// ── 2. PAPER_META.PROVIDER ───────────────────────────────────────────────────

fn paper_meta_provider(conn: &Connection) -> Result<()> {
    if !has_column(conn, "PAPER_META", "PROVIDER")? {
        conn.execute_batch("ALTER TABLE PAPER_META ADD COLUMN PROVIDER TEXT DEFAULT 'arxiv'")?;
    }
    Ok(())
}

// ── 3. SEARCH_STATE.SORT_JSON ────────────────────────────────────────────────

fn search_state_sort_json(conn: &Connection) -> Result<()> {
    if !has_column(conn, "SEARCH_STATE", "SORT_JSON")? {
        conn.execute_batch("ALTER TABLE SEARCH_STATE ADD COLUMN SORT_JSON TEXT")?;
    }
    Ok(())
}

// ── 4. TAG case-insensitive unique label ─────────────────────────────────────

/// Collapse case-variant duplicate tags onto the canonical (lowest-TAG_FK) row,
/// remap the bridge tables, then enforce `UNIQUE (TAG COLLATE NOCASE)`.
fn tag_label_unique_index(conn: &Connection) -> Result<()> {
    if index_exists(conn, "idx_tag_label_unique")? {
        return Ok(());
    }
    // fetchall first; the remap loop issues writes that would invalidate a live cursor.
    let rows: Vec<(i64, Option<String>)> = conn
        .prepare("SELECT TAG_FK, TAG FROM TAG ORDER BY TAG_FK")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    // canonical = min TAG_FK per lowercased label (first seen wins; rows are FK-ordered).
    let mut canonical: HashMap<String, i64> = HashMap::new();
    for (fk, tag) in &rows {
        if let Some(t) = tag {
            canonical.entry(t.to_lowercase()).or_insert(*fk);
        }
    }

    let mut remapped = false;
    for (fk, tag) in &rows {
        let Some(t) = tag else { continue };
        let canon = canonical[&t.to_lowercase()];
        if canon != *fk {
            remapped = true;
            // UPDATE OR IGNORE absorbs the link onto the canonical FK; the DELETE
            // sweeps any link that could not move (canonical link already existed).
            conn.execute(
                "UPDATE OR IGNORE PROJECT_TO_TAG SET TAG_FK = ?1 WHERE TAG_FK = ?2",
                [canon, *fk],
            )?;
            conn.execute("DELETE FROM PROJECT_TO_TAG WHERE TAG_FK = ?1", [*fk])?;
            conn.execute(
                "UPDATE OR IGNORE PAPER_TO_TAG SET TAG_FK = ?1 WHERE TAG_FK = ?2",
                [canon, *fk],
            )?;
            conn.execute("DELETE FROM PAPER_TO_TAG WHERE TAG_FK = ?1", [*fk])?;
            conn.execute("DELETE FROM TAG WHERE TAG_FK = ?1", [*fk])?;
        }
    }
    if remapped {
        conn.execute_batch(
            "DELETE FROM PAPER_TO_TAG WHERE PTT_FK NOT IN (
                 SELECT MIN(PTT_FK) FROM PAPER_TO_TAG GROUP BY PAPER_ID, TAG_FK)",
        )?;
    }
    conn.execute_batch("CREATE UNIQUE INDEX idx_tag_label_unique ON TAG (TAG COLLATE NOCASE)")?;
    Ok(())
}

// ── 5. PROJECT_TO_TAG unique (PROJECT_FK, TAG_FK) ────────────────────────────

fn project_to_tag_unique_index(conn: &Connection) -> Result<()> {
    if index_exists(conn, "idx_project_to_tag_unique")? {
        return Ok(());
    }
    conn.execute_batch(
        "DELETE FROM PROJECT_TO_TAG WHERE PROJECT_TO_TAG_FK NOT IN (
             SELECT MIN(PROJECT_TO_TAG_FK) FROM PROJECT_TO_TAG GROUP BY PROJECT_FK, TAG_FK);
         CREATE UNIQUE INDEX idx_project_to_tag_unique ON PROJECT_TO_TAG (PROJECT_FK, TAG_FK);",
    )?;
    Ok(())
}

// ── 6. PROJECT_TO_PAPER unique (PROJECT_FK, SOURCE_FK) ───────────────────────

/// Creates the unique index whose duplicates `dedup_project_to_paper` (run BEFORE
/// apply_tables — see `init_db`) has already cleared, so this can never hit
/// `UNIQUE constraint failed` on a legacy DB. Deliberately NOT in
/// PROJECT_TO_PAPER.sql: apply_tables would run it before the dedup. MUST stay
/// before `paper_to_reading_cascade_fk`: that migration INSERTs into a table whose
/// composite FK needs this parent-key index to exist by then (SQLite checks it at
/// DML time, not CREATE TABLE time).
fn project_to_paper_unique_index(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_project_to_paper_unique ON PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK)",
    )?;
    Ok(())
}

/// Pre-schema dedup of PROJECT_TO_PAPER — the one migration that MUST run BEFORE
/// `apply_tables` (see `init_db`), not after. Once apply_tables has created the
/// current PAPER_TO_READING (composite FK on these two columns), ANY DML on
/// PROJECT_TO_PAPER while its parent key is unindexed fails with "foreign key
/// mismatch" — even a dedup DELETE on a legacy DB. Before apply_tables, a DB old
/// enough to hold duplicates has no such child table (PAPER_TO_READING postdates
/// the unique index), so the DELETE is legal. Idempotent: no-ops when the table
/// doesn't exist yet (fresh DB) or the unique index already does.
pub fn dedup_project_to_paper(conn: &Connection) -> Result<()> {
    let table_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='PROJECT_TO_PAPER'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 || index_exists(conn, "idx_project_to_paper_unique")? {
        return Ok(());
    }
    conn.execute_batch(
        "DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_TO_PAPER_FK NOT IN (
             SELECT MIN(PROJECT_TO_PAPER_FK) FROM PROJECT_TO_PAPER GROUP BY PROJECT_FK, SOURCE_FK)",
    )?;
    Ok(())
}

// ── 7. AUTHOR_FULL_NAME case-insensitive index (non-unique) ──────────────────

fn author_full_name_index(conn: &Connection) -> Result<()> {
    // CREATE INDEX IF NOT EXISTS is itself idempotent — no separate guard needed.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_author_full_name ON AUTHOR (AUTHOR_FULL_NAME COLLATE NOCASE)",
    )?;
    Ok(())
}

// ── 8. ANNOTATION table (PDF highlights) ─────────────────────────────────────

/// The PDF-annotation table was added after the initial schema, so it is created
/// here (not in TABLE_DDL) so existing user DBs gain it on startup. The DDL itself
/// is `CREATE TABLE IF NOT EXISTS`, so the whole step is an idempotent no-op once
/// the table exists. FK referents (PAPER_ROOTS, PROJECT) are created by
/// apply_tables, which runs before migrations — see `super::init_db`.
fn annotation_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../../sql/tables/ANNOTATION.sql"))?;
    Ok(())
}

// ── 9. VERSION_CHECK table (arXiv new-version monitoring) ──────────────────────

/// Per-root poll bookkeeping for the version monitor: LAST_CHECKED_AT drives the
/// stalest-first rotation, NEW_VERSION flags an un-acknowledged discovery. Added
/// after the initial schema, so created here like ANNOTATION.
fn version_check_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../../sql/tables/VERSION_CHECK.sql"))?;
    Ok(())
}

// ── 10. Backfill notes_fts for notes that predate the FTS table ────────────────

/// The notes_fts triggers (see notes_fts.sql) only index NOTE rows written after
/// the table existed, so index any pre-existing note. `NOT IN` skips rows
/// already indexed.
fn notes_fts_backfill(conn: &Connection) -> Result<()> {
    let note_count: i64 = conn.query_row("SELECT COUNT(*) FROM NOTE", [], |r| r.get(0))?;
    let fts_count: i64 = conn.query_row("SELECT COUNT(*) FROM notes_fts", [], |r| r.get(0))?;
    if note_count == fts_count {
        return Ok(());
    }
    conn.execute_batch(
        "INSERT INTO notes_fts(rowid, title, note, source_fk)
         SELECT NOTE_SK, TITLE, CAST(NOTE AS TEXT), SOURCE_FK FROM NOTE
         WHERE NOTE_SK NOT IN (SELECT rowid FROM notes_fts)",
    )?;
    Ok(())
}

// ── 11. PROJECT.IS_READING_LIST flag ─────────────────────────────────────────

/// Marks a project as a reading list (0 = normal project, 1 = reading list).
/// Per-paper reading status for such projects lives sparsely in PAPER_TO_READING.
fn project_reading_list_flag(conn: &Connection) -> Result<()> {
    if !has_column(conn, "PROJECT", "IS_READING_LIST")? {
        conn.execute_batch(
            "ALTER TABLE PROJECT ADD COLUMN IS_READING_LIST INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    Ok(())
}

// ── 12. PAPER_TO_READING → PROJECT_TO_PAPER composite cascade FK ────────────

/// Rebuilds PAPER_TO_READING with `FOREIGN KEY (PROJECT_FK, SOURCE_FK)
/// REFERENCES PROJECT_TO_PAPER(...) ON DELETE CASCADE` so a paper's reading
/// status is auto-dropped when it's removed from the project (previously only
/// PROJECT_TO_PAPER was cleaned up, leaving an orphaned row that resurrected on
/// re-add). SQLite has no `ALTER TABLE ADD CONSTRAINT`, so existing DBs need the
/// table rebuilt; fresh installs already get the new FK from PAPER_TO_READING.sql
/// (guard below sees it and no-ops). MUST run after `project_to_paper_unique_index`
/// — the new FK's parent key needs that unique index to already exist before any
/// row is written (it doesn't have to exist yet at CREATE TABLE time, only by the
/// time of the INSERT below). The `JOIN PROJECT_TO_PAPER` in the copy drops any
/// row that was already orphaned pre-migration, rather than carrying the bug forward.
fn paper_to_reading_cascade_fk(conn: &Connection) -> Result<()> {
    if paper_to_reading_has_cascade_fk(conn)? {
        return Ok(());
    }
    conn.execute_batch(
        "BEGIN;
         CREATE TABLE PAPER_TO_READING_NEW(
             PROJECT_FK  INTEGER NOT NULL,
             SOURCE_FK   INTEGER NOT NULL,
             STATUS      TEXT    NOT NULL CHECK (STATUS IN ('reading', 'read')),
             UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY (PROJECT_FK, SOURCE_FK),
             FOREIGN KEY (PROJECT_FK, SOURCE_FK) REFERENCES PROJECT_TO_PAPER(PROJECT_FK, SOURCE_FK) ON DELETE CASCADE
         );
         INSERT INTO PAPER_TO_READING_NEW (PROJECT_FK, SOURCE_FK, STATUS, UPDATED_AT)
             SELECT r.PROJECT_FK, r.SOURCE_FK, r.STATUS, r.UPDATED_AT
             FROM PAPER_TO_READING r
             JOIN PROJECT_TO_PAPER m ON m.PROJECT_FK = r.PROJECT_FK AND m.SOURCE_FK = r.SOURCE_FK;
         DROP TABLE PAPER_TO_READING;
         ALTER TABLE PAPER_TO_READING_NEW RENAME TO PAPER_TO_READING;
         CREATE INDEX IF NOT EXISTS idx_paper_to_reading_source_fk ON PAPER_TO_READING (SOURCE_FK);
         COMMIT;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::schema;

    #[test]
    fn migrations_are_idempotent() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        schema::apply_tables(&conn).unwrap();
        // First pass creates whatever TABLE_DDL doesn't (unique indexes, ANNOTATION,
        // VERSION_CHECK); the second must see every guard trip and be a silent no-op.
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
        assert!(has_column(&conn, "PROJECT", "IS_READING_LIST").unwrap());
        assert!(paper_to_reading_has_cascade_fk(&conn).unwrap());
        assert!(index_exists(&conn, "idx_tag_label_unique").unwrap());
        assert!(index_exists(&conn, "idx_project_to_paper_unique").unwrap());
        assert!(index_exists(&conn, "idx_author_full_name").unwrap());
        // The ANNOTATION table is created by the migration, not TABLE_DDL.
        let annotation_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ANNOTATION'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(annotation_tables, 1);
        assert!(index_exists(&conn, "idx_annotation_source_fk").unwrap());
        assert!(index_exists(&conn, "idx_annotation_project_fk").unwrap());
        let version_check_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='VERSION_CHECK'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version_check_tables, 1);
        schema::apply_views(&conn).unwrap();
    }

    /// `PRAGMA table_info(table)` rows as (name, type, notnull, dflt_value, pk),
    /// sorted so two schemas built via different paths can be diffed directly.
    fn table_info(
        conn: &Connection,
        table: &str,
    ) -> Vec<(String, String, i64, Option<String>, i64)> {
        let mut rows: Vec<_> = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        rows.sort();
        rows
    }

    /// `PRAGMA foreign_key_list(table)` rows as (referenced table, from-col,
    /// to-col, on_delete), sorted.
    fn foreign_key_list(conn: &Connection, table: &str) -> Vec<(String, String, String, String)> {
        let mut rows: Vec<_> = conn
            .prepare(&format!("PRAGMA foreign_key_list({table})"))
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        rows.sort();
        rows
    }

    /// A fresh install (schema::apply_tables → TABLE_DDL already has
    /// IS_READING_LIST and the PAPER_TO_READING→PROJECT_TO_PAPER cascade FK) must
    /// end up column-for-column and FK-for-FK identical to a pre-existing DB that
    /// only gained them via the two migrations (`project_reading_list_flag`,
    /// `paper_to_reading_cascade_fk`) — the whole point of folding a
    /// migration-only column/constraint into the base table def.
    #[test]
    fn fresh_install_matches_upgraded_via_migration() {
        let fresh = crate::storage::db::open_in_memory().unwrap();
        crate::storage::init_db(&fresh).unwrap();

        // Simulate a pre-fix DB: PROJECT without IS_READING_LIST, PAPER_TO_READING
        // with the old single-column (non-cascading-to-membership) FKs. Everything
        // else is created fresh by apply_tables below, same as any real upgrade.
        let legacy = crate::storage::db::open_in_memory().unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE PROJECT(
                     PROJECT_FK   INTEGER NOT NULL,
                     NAME         TEXT    NOT NULL,
                     DESCRIPTION  TEXT    DEFAULT '',
                     COLOR        INTEGER,
                     STATUS       TEXT    NOT NULL DEFAULT 'active',
                     CREATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                     UPDATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                     ARCHIVED_AT  TIMESTAMP,
                     PRIMARY KEY (PROJECT_FK)
                 );
                 CREATE TABLE PAPER_TO_READING(
                     PROJECT_FK  INTEGER NOT NULL,
                     SOURCE_FK   INTEGER NOT NULL,
                     STATUS      TEXT    NOT NULL CHECK (STATUS IN ('reading', 'read')),
                     UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                     PRIMARY KEY (PROJECT_FK, SOURCE_FK),
                     FOREIGN KEY (PROJECT_FK) REFERENCES PROJECT(PROJECT_FK) ON DELETE CASCADE,
                     FOREIGN KEY (SOURCE_FK)  REFERENCES PAPER_ROOTS(SOURCE_FK) ON DELETE CASCADE
                 );",
            )
            .unwrap();
        crate::storage::init_db(&legacy).unwrap();

        assert!(has_column(&legacy, "PROJECT", "IS_READING_LIST").unwrap());
        assert!(paper_to_reading_has_cascade_fk(&legacy).unwrap());
        assert_eq!(
            table_info(&fresh, "PROJECT"),
            table_info(&legacy, "PROJECT"),
            "fresh vs. migrated PROJECT schema must match column-for-column"
        );
        assert_eq!(
            table_info(&fresh, "PAPER_TO_READING"),
            table_info(&legacy, "PAPER_TO_READING"),
            "fresh vs. migrated PAPER_TO_READING schema must match column-for-column"
        );
        assert_eq!(
            foreign_key_list(&fresh, "PAPER_TO_READING"),
            foreign_key_list(&legacy, "PAPER_TO_READING"),
            "fresh vs. migrated PAPER_TO_READING must have the same FKs"
        );
    }

    /// A pre-migration-6 DB can hold duplicate (PROJECT_FK, SOURCE_FK) membership
    /// rows. Startup (init_db) must dedup them and then create the unique index —
    /// never fail with `UNIQUE constraint failed` and brick the app (which is what
    /// happens if the index creation sneaks into PROJECT_TO_PAPER.sql, since
    /// apply_tables runs before the dedup migration).
    #[test]
    fn legacy_db_with_duplicate_memberships_boots_and_dedups() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        // Legacy PROJECT_TO_PAPER shape (no unique index yet; FK clauses omitted —
        // irrelevant to the dedup under test, and their referents don't exist yet).
        conn.execute_batch(
            "CREATE TABLE PROJECT_TO_PAPER(
                 PROJECT_TO_PAPER_FK INTEGER NOT NULL,
                 PROJECT_FK  INTEGER NOT NULL,
                 SOURCE_FK   INTEGER NOT NULL,
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (PROJECT_TO_PAPER_FK)
             );
             INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK)
                 VALUES (1, 7, 42), (2, 7, 42), (3, 7, 43);",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(index_exists(&conn, "idx_project_to_paper_unique").unwrap());
        // The duplicate collapsed onto the lowest FK; the distinct row survived.
        let rows: Vec<i64> = conn
            .prepare("SELECT PROJECT_TO_PAPER_FK FROM PROJECT_TO_PAPER ORDER BY 1")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, vec![1, 3]);
    }

    #[test]
    fn notes_fts_backfill_indexes_pre_existing_notes() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        schema::apply_tables(&conn).unwrap();
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO NOTE (NOTE_SK, SOURCE_FK, TITLE, NOTE) VALUES (1, 1, 'Backfill Me', 'quantum entanglement')",
            [],
        )
        .unwrap();
        // Simulate a note that predates notes_fts: drop its row before backfill runs.
        conn.execute("DELETE FROM notes_fts WHERE rowid = 1", [])
            .unwrap();

        notes_fts_backfill(&conn).unwrap();

        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'entanglement'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);
    }
}
