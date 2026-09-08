//! Idempotent startup migrations. Plan §5.3 + D6.
//!
//! NON-NEGOTIABLE: every migration here runs on EVERY startup against real user
//! DBs, so each MUST be idempotent — guarded by `PRAGMA table_info` (missing
//! column) or by index existence. Re-running them is the normal case, not the
//! exception (in practice the column/index already exists and each is a no-op).
//!
//! The migration SQL itself lives in `crates/core/sql/migrations/` (ground
//! truth) and is embedded here with `include_str!`; this file is guard logic
//! (has the column/index already been added?) plus call order, not SQL text.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::Result;

/// Run all idempotent post-schema migrations in order. Call between
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
    project_share_id(conn)?;
    note_uuid(conn)?;
    annotation_uuid(conn)?;
    rss_feed_tables(conn)?;
    rss_cache_entry_table(conn)?;
    paper_sort_indexes(conn)?;
    link_table_indexes(conn)?;
    paper_source_fk_index(conn)?;
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
        conn.execute_batch(include_str!(
            "../../sql/migrations/01_paper_roots_status.sql"
        ))?;
    }
    if !has_column(conn, "PAPER_ROOTS", "DELETED_AT")? {
        conn.execute_batch(include_str!(
            "../../sql/migrations/01_paper_roots_deleted_at.sql"
        ))?;
    }
    Ok(())
}

// ── 2. PAPER_META.PROVIDER ───────────────────────────────────────────────────

fn paper_meta_provider(conn: &Connection) -> Result<()> {
    if !has_column(conn, "PAPER_META", "PROVIDER")? {
        conn.execute_batch(include_str!(
            "../../sql/migrations/02_paper_meta_provider.sql"
        ))?;
    }
    Ok(())
}

// ── 3. SEARCH_STATE.SORT_JSON ────────────────────────────────────────────────

fn search_state_sort_json(conn: &Connection) -> Result<()> {
    if !has_column(conn, "SEARCH_STATE", "SORT_JSON")? {
        conn.execute_batch(include_str!(
            "../../sql/migrations/03_search_state_sort_json.sql"
        ))?;
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
                include_str!("../../sql/migrations/04_tag_label_remap_project_to_tag.sql"),
                [canon, *fk],
            )?;
            conn.execute(
                include_str!("../../sql/migrations/04_tag_label_delete_project_to_tag_orphan.sql"),
                [*fk],
            )?;
            conn.execute(
                include_str!("../../sql/migrations/04_tag_label_remap_paper_to_tag.sql"),
                [canon, *fk],
            )?;
            conn.execute(
                include_str!("../../sql/migrations/04_tag_label_delete_paper_to_tag_orphan.sql"),
                [*fk],
            )?;
            conn.execute(
                include_str!("../../sql/migrations/04_tag_label_delete_tag.sql"),
                [*fk],
            )?;
        }
    }
    if remapped {
        conn.execute_batch(include_str!(
            "../../sql/migrations/04_tag_label_dedup_paper_to_tag.sql"
        ))?;
    }
    conn.execute_batch(include_str!(
        "../../sql/migrations/04_tag_label_unique_index.sql"
    ))?;
    Ok(())
}

// ── 5. PROJECT_TO_TAG unique (PROJECT_FK, TAG_FK) ────────────────────────────

fn project_to_tag_unique_index(conn: &Connection) -> Result<()> {
    if index_exists(conn, "idx_project_to_tag_unique")? {
        return Ok(());
    }
    conn.execute_batch(include_str!(
        "../../sql/migrations/05_project_to_tag_unique_index.sql"
    ))?;
    Ok(())
}

// ── 6. PROJECT_TO_PAPER unique (PROJECT_FK, SOURCE_FK) ───────────────────────

/// Creates the unique index whose duplicates `dedup_project_to_paper` (run
/// BEFORE apply_tables) has already cleared. Deliberately NOT in
/// PROJECT_TO_PAPER.sql (apply_tables would run it before the dedup); MUST stay
/// before `paper_to_reading_cascade_fk`, whose INSERT needs this parent-key index.
fn project_to_paper_unique_index(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!(
        "../../sql/migrations/06_project_to_paper_unique_index.sql"
    ))?;
    Ok(())
}

/// Pre-schema dedup of PROJECT_TO_PAPER — the one migration that MUST run
/// BEFORE `apply_tables`: once PAPER_TO_READING's composite FK exists, ANY DML
/// on PROJECT_TO_PAPER with an unindexed parent key fails "foreign key
/// mismatch". Idempotent: no-ops when the table doesn't exist yet or the unique
/// index already does. Pinned by
/// `schema::tests::dedup_project_to_paper_must_run_before_apply_tables`.
pub fn dedup_project_to_paper(conn: &Connection) -> Result<()> {
    let table_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='PROJECT_TO_PAPER'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 || index_exists(conn, "idx_project_to_paper_unique")? {
        return Ok(());
    }
    conn.execute_batch(include_str!(
        "../../sql/migrations/00_dedup_project_to_paper.sql"
    ))?;
    Ok(())
}

// ── 7. AUTHOR_FULL_NAME case-insensitive index (non-unique) ──────────────────

fn author_full_name_index(conn: &Connection) -> Result<()> {
    // CREATE INDEX IF NOT EXISTS is itself idempotent — no separate guard needed.
    conn.execute_batch(include_str!(
        "../../sql/migrations/07_author_full_name_index.sql"
    ))?;
    Ok(())
}

// ── 8. ANNOTATION table (PDF highlights) ─────────────────────────────────────

/// Added after the initial schema, so created here (not TABLE_DDL) so existing
/// DBs gain it on startup; `CREATE TABLE IF NOT EXISTS` keeps it idempotent.
/// FK referents come from apply_tables, which runs before migrations.
fn annotation_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../../sql/tables/ANNOTATION.sql"))?;
    Ok(())
}

// ── 9. VERSION_CHECK table (arXiv new-version monitoring) ──────────────────────

/// Per-root poll bookkeeping for the version monitor: LAST_CHECKED_AT drives the
/// stalest-first rotation, NEW_VERSION flags an un-acknowledged discovery.
fn version_check_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!(
        "../../sql/migrations/09_version_check_table.sql"
    ))?;
    Ok(())
}

// ── 10. Backfill notes_fts for notes that predate the FTS table ────────────────

/// The notes_fts triggers only index NOTE rows written after the table existed;
/// index any pre-existing note (`NOT IN` skips rows already indexed).
fn notes_fts_backfill(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!(
        "../../sql/migrations/10_notes_fts_backfill.sql"
    ))?;
    Ok(())
}

// ── 11. PROJECT.IS_READING_LIST flag ─────────────────────────────────────────

/// Marks a project as a reading list (0 = normal project, 1 = reading list).
/// Per-paper reading status for such projects lives sparsely in PAPER_TO_READING.
fn project_reading_list_flag(conn: &Connection) -> Result<()> {
    if !has_column(conn, "PROJECT", "IS_READING_LIST")? {
        conn.execute_batch(include_str!(
            "../../sql/migrations/11_project_reading_list_flag.sql"
        ))?;
    }
    Ok(())
}

// ── 12. PAPER_TO_READING → PROJECT_TO_PAPER composite cascade FK ────────────

/// Rebuilds PAPER_TO_READING with a composite `ON DELETE CASCADE` FK to
/// PROJECT_TO_PAPER so reading status drops with project membership (SQLite has
/// no ADD CONSTRAINT; fresh installs get the FK from the table DDL and the
/// guard no-ops). MUST run after `project_to_paper_unique_index` — the INSERT
/// needs that parent-key index. The JOIN drops rows already orphaned pre-migration.
fn paper_to_reading_cascade_fk(conn: &Connection) -> Result<()> {
    if paper_to_reading_has_cascade_fk(conn)? {
        return Ok(());
    }
    conn.execute_batch(include_str!(
        "../../sql/migrations/12_paper_to_reading_cascade_fk.sql"
    ))?;
    Ok(())
}

// ── 13. PROJECT.SHARE_ID (persisted share identity, uuid v4) ─────────────────

/// Set lazily on first publish (`project::ensure_share_id`),
/// never at project creation.
fn project_share_id(conn: &Connection) -> Result<()> {
    if !has_column(conn, "PROJECT", "SHARE_ID")? {
        conn.execute_batch(include_str!(
            "../../sql/migrations/13_project_share_id_column.sql"
        ))?;
    }
    conn.execute_batch(include_str!(
        "../../sql/migrations/13_project_share_id_unique_index.sql"
    ))?;
    Ok(())
}

/// Backfill every NULL `col` with a fresh uuid v4, then enforce uniqueness.
/// Runs on every startup.
fn backfill_uuid_column(conn: &Connection, table: &str, col: &str, index: &str) -> Result<()> {
    let ids: Vec<i64> = conn
        .prepare(&format!("SELECT rowid FROM {table} WHERE {col} IS NULL"))?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(&format!("UPDATE {table} SET {col} = ?1 WHERE rowid = ?2"))?;
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        stmt.execute(rusqlite::params![uuid::Uuid::new_v4().to_string(), id])?;
    }
    tx.commit()?;
    conn.execute_batch(&format!(
        "CREATE UNIQUE INDEX IF NOT EXISTS {index} ON {table} ({col})"
    ))?;
    Ok(())
}

// ── 14. NOTE.NOTE_UUID (stable note identity) ────────────────────────────────

fn note_uuid(conn: &Connection) -> Result<()> {
    if !has_column(conn, "NOTE", "NOTE_UUID")? {
        conn.execute_batch(include_str!("../../sql/migrations/14_note_uuid_column.sql"))?;
    }
    backfill_uuid_column(conn, "NOTE", "NOTE_UUID", "idx_note_uuid_unique")
}

// ── 15. ANNOTATION.ANNOTATION_UUID (stable annotation identity) ──────────────

/// ANNOTATION itself is created by `annotation_table` (runs earlier);
/// the guard adds the column to DBs created before it existed.
fn annotation_uuid(conn: &Connection) -> Result<()> {
    if !has_column(conn, "ANNOTATION", "ANNOTATION_UUID")? {
        conn.execute_batch(include_str!(
            "../../sql/migrations/15_annotation_uuid_column.sql"
        ))?;
    }
    backfill_uuid_column(
        conn,
        "ANNOTATION",
        "ANNOTATION_UUID",
        "idx_annotation_uuid_unique",
    )
}

// ── 16. RSS feed tables (persisted home-feed items, dismiss state, filter rules) ──

/// Added after the initial schema, so created here like ANNOTATION. Order matters:
/// RSS_PAPER's FK needs RSS_PAPER_ROOTS to already exist.
fn rss_feed_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../../sql/tables/RSS_PAPER_ROOTS.sql"))?;
    conn.execute_batch(include_str!("../../sql/tables/RSS_PAPER.sql"))?;
    conn.execute_batch(include_str!("../../sql/tables/RSS_FILTER_RULE.sql"))?;
    Ok(())
}

// ── 17. RSS_CACHE_ENTRY table (persisted per-URL feed entries, replaces the
//        old in-memory last-response cache) ─────────────────────────────────

/// Added after the initial schema, same pattern as `rss_feed_tables` above.
fn rss_cache_entry_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../../sql/tables/RSS_CACHE_ENTRY.sql"))?;
    Ok(())
}

// ── 18. Library sort indexes (PAPER.TITLE/CREATED_AT, PAPER_META.PUBLISHED) ──

/// The `PaperSort` orderings. Live here (not table DDL) because TABLE_DDL runs
/// before the column-adding migrations; guarded on the column existing, since
/// skipping the purely-for-speed indexes on a stripped legacy DB is the harmless branch.
fn paper_sort_indexes(conn: &Connection) -> Result<()> {
    if has_column(conn, "PAPER_META", "PUBLISHED")? {
        conn.execute_batch(include_str!(
            "../../sql/migrations/18_paper_sort_indexes.sql"
        ))?;
    }
    Ok(())
}

// ── 19. Link-table lookup indexes (PAPER_TO_AUTHOR, PAPER_TO_TAG, NOTE, …) ──

/// Purely-for-speed indexes on the link tables' lookup columns (see the SQL file);
/// original-schema columns, so CREATE INDEX IF NOT EXISTS is the only guard needed.
fn link_table_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!(
        "../../sql/migrations/19_link_table_indexes.sql"
    ))?;
    Ok(())
}

// ── 20. PAPER(SOURCE_FK, VERSION) lookup index ──────────────────────────────

/// Purely-for-speed index on PAPER's root-FK column (see the SQL file);
/// CREATE INDEX IF NOT EXISTS is the only guard needed.
fn paper_source_fk_index(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!(
        "../../sql/migrations/20_paper_source_fk_index.sql"
    ))?;
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
        assert!(has_column(&conn, "PROJECT", "SHARE_ID").unwrap());
        assert!(has_column(&conn, "NOTE", "NOTE_UUID").unwrap());
        assert!(has_column(&conn, "ANNOTATION", "ANNOTATION_UUID").unwrap());
        assert!(index_exists(&conn, "idx_note_uuid_unique").unwrap());
        assert!(index_exists(&conn, "idx_annotation_uuid_unique").unwrap());
        assert!(index_exists(&conn, "idx_project_share_id_unique").unwrap());
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
        for table in [
            "RSS_PAPER_ROOTS",
            "RSS_PAPER",
            "RSS_FILTER_RULE",
            "RSS_CACHE_ENTRY",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{table} must exist after run_migrations");
        }
        assert!(index_exists(&conn, "IDX_RSS_PAPER_SOURCE_FK").unwrap());
        for idx in [
            "idx_paper_to_author_paper_id",
            "idx_paper_to_author_author_fk",
            "idx_paper_to_tag_paper_id",
            "idx_paper_to_tag_tag_fk",
            "idx_project_to_paper_source_fk",
            "idx_project_to_tag_tag_fk",
            "idx_note_source_fk",
            "idx_note_project_fk",
            "idx_note_paper_id_fk",
        ] {
            assert!(index_exists(&conn, idx).unwrap(), "{idx} must exist");
        }
        assert!(index_exists(&conn, "idx_paper_source_fk").unwrap());
        schema::apply_views(&conn).unwrap();
    }

    /// Existence isn't use: pin that the hottest link-table lookup (a paper's
    /// author list) goes through the new index rather than scanning PAPER_TO_AUTHOR.
    #[test]
    fn paper_author_lookup_uses_link_table_index() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        crate::storage::init_db(&conn).unwrap();
        let plan: Vec<String> = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT a.AUTHOR_FK FROM AUTHOR a \
                 JOIN PAPER_TO_AUTHOR pta ON pta.AUTHOR_FK = a.AUTHOR_FK \
                 WHERE pta.PAPER_ID = 1 ORDER BY pta.AUTHOR_INDEX",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|d| d.contains("idx_paper_to_author_paper_id")),
            "plan must use idx_paper_to_author_paper_id, got: {plan:?}"
        );
    }

    /// Same existence-vs-use pin for migration 20: deleted_papers' correlated
    /// MAX(VERSION) subquery must resolve via idx_paper_source_fk, not a full scan.
    #[test]
    fn deleted_papers_version_subquery_uses_paper_source_fk_index() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        crate::storage::init_db(&conn).unwrap();
        let plan: Vec<String> = conn
            .prepare("EXPLAIN QUERY PLAN SELECT source_fk, title FROM deleted_papers")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter().any(|d| d.contains("idx_paper_source_fk")),
            "plan must use idx_paper_source_fk, got: {plan:?}"
        );
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

    /// A fresh install must end up column-for-column and FK-for-FK identical to a
    /// DB that gained IS_READING_LIST + the cascade FK only via the two migrations.
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

    /// A DB predating SHARE_ID / NOTE_UUID / ANNOTATION_UUID gains the columns and
    /// unique indexes on startup; every pre-existing row is backfilled with a distinct uuid.
    #[test]
    fn legacy_db_without_share_and_uuid_columns_upgrades() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        // Legacy shapes without the three columns (FK clauses omitted — their
        // referents don't exist yet, as in the dedup test above).
        conn.execute_batch(
            "CREATE TABLE PROJECT(
                 PROJECT_FK      INTEGER NOT NULL,
                 NAME            TEXT    NOT NULL,
                 DESCRIPTION     TEXT    DEFAULT '',
                 COLOR           INTEGER,
                 STATUS          TEXT    NOT NULL DEFAULT 'active',
                 IS_READING_LIST INTEGER NOT NULL DEFAULT 0,
                 CREATED_AT      TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT      TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 ARCHIVED_AT     TIMESTAMP,
                 PRIMARY KEY (PROJECT_FK)
             );
             CREATE TABLE NOTE(
                 NOTE_SK     INTEGER NOT NULL,
                 SOURCE_FK   INTEGER NOT NULL,
                 PAPER_ID_FK INTEGER,
                 PROJECT_FK  INTEGER,
                 TITLE       TEXT,
                 NOTE        BLOB,
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (NOTE_SK)
             );
             CREATE TABLE ANNOTATION(
                 ANNOTATION_SK INTEGER NOT NULL,
                 SOURCE_FK     INTEGER NOT NULL,
                 PROJECT_FK    INTEGER,
                 ANCHOR        TEXT NOT NULL,
                 COMMENT       TEXT NOT NULL DEFAULT '',
                 CREATED_AT    TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT    TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (ANNOTATION_SK)
             );
             INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (1, 'p');
             INSERT INTO NOTE (NOTE_SK, SOURCE_FK, TITLE, NOTE)
                 VALUES (1, 1, 't1', 'c1'), (2, 1, 't2', 'c2');
             INSERT INTO ANNOTATION (ANNOTATION_SK, SOURCE_FK, ANCHOR)
                 VALUES (1, 1, '{}'), (2, 1, '{}');",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(has_column(&conn, "PROJECT", "SHARE_ID").unwrap());
        assert!(has_column(&conn, "NOTE", "NOTE_UUID").unwrap());
        assert!(has_column(&conn, "ANNOTATION", "ANNOTATION_UUID").unwrap());
        assert!(index_exists(&conn, "idx_project_share_id_unique").unwrap());
        assert!(index_exists(&conn, "idx_note_uuid_unique").unwrap());
        assert!(index_exists(&conn, "idx_annotation_uuid_unique").unwrap());

        for (table, col) in [("NOTE", "NOTE_UUID"), ("ANNOTATION", "ANNOTATION_UUID")] {
            let uuids: Vec<Option<String>> = conn
                .prepare(&format!("SELECT {col} FROM {table}"))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let uuids: Vec<String> = uuids
                .into_iter()
                .map(|u| u.unwrap_or_else(|| panic!("{table}.{col} left NULL")))
                .collect();
            assert_eq!(uuids.len(), 2);
            assert_ne!(
                uuids[0], uuids[1],
                "{table}.{col} backfill must be distinct"
            );
        }
    }

    /// A pre-migration-6 DB can hold duplicate (PROJECT_FK, SOURCE_FK) rows.
    /// init_db must dedup then create the unique index — never brick the app
    /// with `UNIQUE constraint failed`.
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
    fn uuid_backfill_fills_pre_existing_rows() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        schema::apply_tables(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1');
             INSERT INTO NOTE (NOTE_SK, SOURCE_FK, TITLE, NOTE) VALUES (1, 1, 't', 'c');",
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        let u: Option<String> = conn
            .query_row("SELECT NOTE_UUID FROM NOTE WHERE NOTE_SK = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let u = u.expect("backfilled");
        assert_eq!(u.len(), 36, "uuid v4 string form");
        run_migrations(&conn).unwrap();
        let again: Option<String> = conn
            .query_row("SELECT NOTE_UUID FROM NOTE WHERE NOTE_SK = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(again.as_deref(), Some(u.as_str()));
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

    // The tests below close the gap left by `migrations_are_idempotent`: that
    // test builds its DB via `schema::apply_tables`. For the column/FK guards,
    // TABLE_DDL already has the column (kept in sync deliberately for fresh
    // installs), so only the no-op branch runs. For the three index guards the
    // tests below target (idx_tag_label_unique, idx_project_to_tag_unique,
    // idx_author_full_name), TABLE_DDL has no such index, so the real CREATE
    // INDEX runs there too -- just against an empty table, so the two guards
    // with remap/dedup DML never exercise it there. Each test here hand-crafts
    // the genuinely pre-migration shape, seeded with rows
    // where relevant, so the real data-touching path actually runs once.

    #[test]
    fn paper_roots_soft_delete_backfills_legacy_rows() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        // Legacy PAPER_ROOTS shape predating STATUS/DELETED_AT.
        conn.execute_batch(
            "CREATE TABLE PAPER_ROOTS(
                 SOURCE_FK  INTEGER PRIMARY KEY AUTOINCREMENT,
                 SOURCE_ID  TEXT    NOT NULL UNIQUE,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1');",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(has_column(&conn, "PAPER_ROOTS", "STATUS").unwrap());
        assert!(has_column(&conn, "PAPER_ROOTS", "DELETED_AT").unwrap());
        let (status, deleted_at): (String, Option<String>) = conn
            .query_row(
                "SELECT STATUS, DELETED_AT FROM PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            status, "active",
            "ADD COLUMN...DEFAULT must backfill the pre-existing row"
        );
        assert_eq!(deleted_at, None);
    }

    #[test]
    fn paper_meta_provider_backfills_legacy_rows() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        // FK to PAPER omitted -- its referent doesn't exist until init_db runs below.
        conn.execute_batch(
            "CREATE TABLE PAPER_META(
                 PAPER_ID   INTEGER NOT NULL PRIMARY KEY,
                 URL        TEXT,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO PAPER_META (PAPER_ID, URL) VALUES (1, 'http://example.com');",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(has_column(&conn, "PAPER_META", "PROVIDER").unwrap());
        let provider: String = conn
            .query_row(
                "SELECT PROVIDER FROM PAPER_META WHERE PAPER_ID = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            provider, "arxiv",
            "ADD COLUMN...DEFAULT must backfill the pre-existing row"
        );
    }

    /// Checks the actual IS_READING_LIST backfill value on a pre-existing row
    /// (the schema shape alone is covered elsewhere).
    #[test]
    fn project_reading_list_flag_backfills_legacy_rows() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE PROJECT(
                 PROJECT_FK  INTEGER NOT NULL,
                 NAME        TEXT    NOT NULL,
                 DESCRIPTION TEXT    DEFAULT '',
                 COLOR       INTEGER,
                 STATUS      TEXT    NOT NULL DEFAULT 'active',
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 ARCHIVED_AT TIMESTAMP,
                 PRIMARY KEY (PROJECT_FK)
             );
             INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (1, 'p');",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(has_column(&conn, "PROJECT", "IS_READING_LIST").unwrap());
        let is_reading_list: i64 = conn
            .query_row(
                "SELECT IS_READING_LIST FROM PROJECT WHERE PROJECT_FK = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            is_reading_list, 0,
            "ADD COLUMN...DEFAULT 0 must backfill the pre-existing row"
        );
    }

    #[test]
    fn search_state_sort_json_preserves_legacy_row() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE SEARCH_STATE(
                 ID             INTEGER PRIMARY KEY CHECK (ID = 1),
                 CLAUSES_JSON   TEXT    NOT NULL,
                 SOURCE         TEXT    NOT NULL,
                 MAX_RESULTS    INTEGER NOT NULL DEFAULT 25,
                 RESULTS_JSON   TEXT    NOT NULL,
                 SAVED_IDS_JSON TEXT    NOT NULL,
                 UPDATED_AT     TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO SEARCH_STATE (ID, CLAUSES_JSON, SOURCE, RESULTS_JSON, SAVED_IDS_JSON)
                 VALUES (1, '[]', 'arxiv', '[]', '[]');",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(has_column(&conn, "SEARCH_STATE", "SORT_JSON").unwrap());
        let (source, sort_json): (String, Option<String>) = conn
            .query_row(
                "SELECT SOURCE, SORT_JSON FROM SEARCH_STATE WHERE ID = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            source, "arxiv",
            "pre-existing row must survive ADD COLUMN untouched"
        );
        assert_eq!(sort_json, None);
    }

    /// The remap-then-dedup DML only fires with real case-variant duplicates:
    /// TAG_FK 2 ('ml') collapses onto TAG_FK 1 ('ML'); bridge-table collisions
    /// are cleaned by the two dedup steps.
    #[test]
    fn tag_label_unique_index_merges_case_variant_duplicates() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        // FK clauses omitted -- PROJECT/PAPER don't exist yet at this point.
        conn.execute_batch(
            "CREATE TABLE TAG(
                 TAG_FK     INTEGER PRIMARY KEY AUTOINCREMENT,
                 TAG        TEXT,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE PROJECT_TO_TAG(
                 PROJECT_TO_TAG_FK INTEGER NOT NULL PRIMARY KEY,
                 PROJECT_FK INTEGER NOT NULL,
                 TAG_FK     INTEGER NOT NULL,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE PAPER_TO_TAG(
                 PTT_FK     INTEGER PRIMARY KEY AUTOINCREMENT,
                 PAPER_ID   INTEGER NOT NULL,
                 TAG_FK     INTEGER NOT NULL,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO TAG (TAG_FK, TAG) VALUES (1, 'ML'), (2, 'ml'), (3, 'NLP');
             INSERT INTO PROJECT_TO_TAG (PROJECT_TO_TAG_FK, PROJECT_FK, TAG_FK)
                 VALUES (1, 10, 1), (2, 10, 2), (3, 20, 3);
             INSERT INTO PAPER_TO_TAG (PTT_FK, PAPER_ID, TAG_FK)
                 VALUES (1, 100, 1), (2, 100, 2), (3, 200, 3);",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(index_exists(&conn, "idx_tag_label_unique").unwrap());

        let tags: Vec<(i64, String)> = conn
            .prepare("SELECT TAG_FK, TAG FROM TAG ORDER BY TAG_FK")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            tags,
            vec![(1, "ML".to_string()), (3, "NLP".to_string())],
            "the case-variant duplicate (TAG_FK 2) must be removed, canonical kept"
        );

        let ptt: Vec<(i64, i64)> = conn
            .prepare("SELECT PROJECT_FK, TAG_FK FROM PROJECT_TO_TAG ORDER BY PROJECT_FK")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            ptt,
            vec![(10, 1), (20, 3)],
            "both links must remap onto the canonical TAG_FK with no leftover duplicate"
        );

        let ptag: Vec<(i64, i64)> = conn
            .prepare("SELECT PAPER_ID, TAG_FK FROM PAPER_TO_TAG ORDER BY PAPER_ID")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(ptag, vec![(100, 1), (200, 3)]);
    }

    /// Duplicate (PROJECT_FK, TAG_FK) rows independent of any tag-label remap,
    /// mirroring the PROJECT_TO_PAPER dedup test but for PROJECT_TO_TAG.
    #[test]
    fn project_to_tag_unique_index_dedups_legacy_duplicates() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE PROJECT_TO_TAG(
                 PROJECT_TO_TAG_FK INTEGER NOT NULL PRIMARY KEY,
                 PROJECT_FK INTEGER NOT NULL,
                 TAG_FK     INTEGER NOT NULL,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO PROJECT_TO_TAG (PROJECT_TO_TAG_FK, PROJECT_FK, TAG_FK)
                 VALUES (1, 7, 42), (2, 7, 42), (3, 7, 43);",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(index_exists(&conn, "idx_project_to_tag_unique").unwrap());
        let rows: Vec<i64> = conn
            .prepare("SELECT PROJECT_TO_TAG_FK FROM PROJECT_TO_TAG ORDER BY 1")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, vec![1, 3]);
    }

    /// `idx_author_full_name` is non-unique (two people can share a name); what
    /// matters is that it's usable for a case-insensitive lookup.
    #[test]
    fn author_full_name_index_is_case_insensitive() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        // Legacy AUTHOR table predating the index, already holding case-variant names.
        conn.execute_batch(
            "CREATE TABLE AUTHOR(
                 AUTHOR_FK        INTEGER PRIMARY KEY AUTOINCREMENT,
                 AUTHOR_FULL_NAME TEXT,
                 AUTHOR_FIRST     TEXT,
                 AUTHOR_LAST      TEXT,
                 AUTHOR_ORCID     TEXT,
                 CREATED_AT       TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT       TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO AUTHOR (AUTHOR_FULL_NAME) VALUES ('Ada Lovelace'), ('ADA LOVELACE');",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(index_exists(&conn, "idx_author_full_name").unwrap());
        // `COLLATE NOCASE` in the query above would make the comparison
        // case-insensitive regardless of the index's own collation (or with no
        // index at all) -- that only proves SQLite can do case-insensitive
        // matching, not that this index is built for it. Assert the index's own
        // stored DDL instead: that's what makes it actually usable for the
        // case-insensitive lookups the app runs against it without a query-side
        // COLLATE override.
        let index_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'idx_author_full_name'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            index_sql.to_uppercase().contains("COLLATE NOCASE"),
            "idx_author_full_name must be built COLLATE NOCASE, got: {index_sql}"
        );

        // The DDL check above is necessary but not sufficient -- a NOCASE index
        // only makes a case-insensitive lookup fast, it doesn't prove SQLite will
        // actually pick it (a mismatched collation falls back to a full SCAN).
        // Query plan "detail" text names the index only when it's genuinely used;
        // this is what makes the two seeded case-variant rows earn their place.
        let plan: Vec<String> = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT * FROM AUTHOR \
                 WHERE AUTHOR_FULL_NAME = 'ada lovelace' COLLATE NOCASE",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter().any(|step| step.contains("idx_author_full_name")),
            "the case-insensitive lookup must actually use idx_author_full_name, got plan: {plan:?}"
        );
    }

    /// The only migration that rebuilds a table via `INSERT ... SELECT ... JOIN`.
    /// Seeds one valid row and one orphan; checks the upgrade cleanup and the
    /// FK's ongoing runtime cascade.
    #[test]
    fn paper_to_reading_cascade_fk_preserves_valid_rows_and_drops_orphans() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        // FK clauses omitted on both -- PROJECT/PAPER_ROOTS don't exist yet at this
        // point. PROJECT_TO_PAPER itself is the current (unchanged) shape; it just
        // needs to exist with rows before init_db runs apply_tables (IF NOT EXISTS).
        conn.execute_batch(
            "CREATE TABLE PROJECT_TO_PAPER(
                 PROJECT_TO_PAPER_FK INTEGER NOT NULL PRIMARY KEY,
                 PROJECT_FK  INTEGER NOT NULL,
                 SOURCE_FK   INTEGER NOT NULL,
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE PAPER_TO_READING(
                 PROJECT_FK  INTEGER NOT NULL,
                 SOURCE_FK   INTEGER NOT NULL,
                 STATUS      TEXT    NOT NULL CHECK (STATUS IN ('reading', 'read')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (PROJECT_FK, SOURCE_FK)
             );
             -- Membership exists for (1,10) only; (2,20) has no PROJECT_TO_PAPER row.
             INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK)
                 VALUES (1, 1, 10);
             INSERT INTO PAPER_TO_READING (PROJECT_FK, SOURCE_FK, STATUS, UPDATED_AT)
                 VALUES (1, 10, 'reading', '2020-01-01 00:00:00'),
                        (2, 20, 'read',    '2020-01-01 00:00:00');",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        assert!(paper_to_reading_has_cascade_fk(&conn).unwrap());
        assert!(index_exists(&conn, "idx_paper_to_reading_source_fk").unwrap());

        let rows: Vec<(i64, i64, String, String)> = conn
            .prepare(
                "SELECT PROJECT_FK, SOURCE_FK, STATUS, UPDATED_AT \
                 FROM PAPER_TO_READING ORDER BY PROJECT_FK",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(
                1,
                10,
                "reading".to_string(),
                "2020-01-01 00:00:00".to_string()
            )],
            "the orphaned (2,20) row must be dropped by the rebuild's JOIN, and the \
             valid row's STATUS/UPDATED_AT must survive the copy untouched"
        );

        // The migration's actual point: going forward, removing a paper from a
        // project must cascade-drop its reading status, not just clean up
        // pre-existing orphans at upgrade time.
        conn.execute(
            "DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_FK = 1 AND SOURCE_FK = 10",
            [],
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM PAPER_TO_READING", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "removing the PROJECT_TO_PAPER membership must cascade-delete PAPER_TO_READING"
        );
    }

    /// Checks ANNOTATION's FK to PAPER_ROOTS really cascades — it's never in
    /// fresh TABLE_DDL, so every install creates it via this guard for real.
    #[test]
    fn annotation_table_fk_cascade_works() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        crate::storage::init_db(&conn).unwrap();

        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1')", [])
            .unwrap();
        let source_fk: i64 = conn
            .query_row(
                "SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO ANNOTATION (SOURCE_FK, ANCHOR) VALUES (?1, '{}')",
            [source_fk],
        )
        .unwrap();

        conn.execute("DELETE FROM PAPER_ROOTS WHERE SOURCE_FK = ?1", [source_fk])
            .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM ANNOTATION", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "ANNOTATION must cascade-delete with its PAPER_ROOTS row"
        );
    }

    /// Checks VERSION_CHECK's actual shape (LAST_CHECKED_AT defaults) and that
    /// its FK to PAPER_ROOTS really cascades — it's never in fresh TABLE_DDL.
    #[test]
    fn version_check_table_shape_and_cascade() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        crate::storage::init_db(&conn).unwrap();

        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1')", [])
            .unwrap();
        let source_fk: i64 = conn
            .query_row(
                "SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO VERSION_CHECK (SOURCE_FK) VALUES (?1)",
            [source_fk],
        )
        .unwrap();
        let last_checked: Option<String> = conn
            .query_row(
                "SELECT LAST_CHECKED_AT FROM VERSION_CHECK WHERE SOURCE_FK = ?1",
                [source_fk],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            last_checked.is_some(),
            "LAST_CHECKED_AT must default on insert"
        );

        conn.execute("DELETE FROM PAPER_ROOTS WHERE SOURCE_FK = ?1", [source_fk])
            .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM VERSION_CHECK", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "VERSION_CHECK must cascade-delete with its PAPER_ROOTS row"
        );
    }

    /// RSS tables are never in fresh TABLE_DDL, and the FK creation-order
    /// dependency (RSS_PAPER needs RSS_PAPER_ROOTS) had no test inserting through it.
    #[test]
    fn rss_feed_tables_fk_cascade_works() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        crate::storage::init_db(&conn).unwrap();

        conn.execute(
            "INSERT INTO RSS_PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1')",
            [],
        )
        .unwrap();
        let source_fk: i64 = conn
            .query_row(
                "SELECT SOURCE_FK FROM RSS_PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO RSS_PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK)
                 VALUES ('arxiv:1', 1, 't', ?1)",
            [source_fk],
        )
        .unwrap();

        conn.execute(
            "DELETE FROM RSS_PAPER_ROOTS WHERE SOURCE_FK = ?1",
            [source_fk],
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM RSS_PAPER", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "RSS_PAPER must cascade-delete with its RSS_PAPER_ROOTS row"
        );
    }

    /// Replays the REAL last pre-port schema: table DDL captured verbatim from
    /// the v0.2.0 tag (commit a9b66700313e8dea67cea16babd88e320083715d), plus
    /// the 4 index migrations every v0.2.0 startup already ran. Views are not
    /// replayed — init_db DROP+CREATEs them unconditionally.
    ///
    /// Every DDL string below is FROZEN HISTORY, not a copy of the current
    /// `sql/` files — never "dedupe" against `sql/` or update on schema change.
    /// v0.1.0's flat lowercase `papers` model is out of scope: it was only ever
    /// upgraded by a manual one-off tool, never an automatic startup migration.
    #[test]
    fn real_v0_2_0_database_upgrades_cleanly() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS AUTHOR(
                 AUTHOR_FK INTEGER PRIMARY KEY AUTOINCREMENT,
                 AUTHOR_FULL_NAME TEXT,
                 AUTHOR_FIRST TEXT,
                 AUTHOR_LAST TEXT,
                 AUTHOR_ORCID TEXT,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE IF NOT EXISTS TAG(
                 TAG_FK INTEGER PRIMARY KEY AUTOINCREMENT,
                 TAG TEXT,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE IF NOT EXISTS PROJECT(
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
             CREATE TABLE IF NOT EXISTS PAPER_ROOTS (
                 SOURCE_FK  INTEGER   PRIMARY KEY AUTOINCREMENT,
                 SOURCE_ID  TEXT      NOT NULL UNIQUE,
                 STATUS     TEXT      NOT NULL DEFAULT 'active',
                 DELETED_AT TIMESTAMP,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE IF NOT EXISTS PAPER(
                 PAPER_ID    INTEGER PRIMARY KEY AUTOINCREMENT,
                 SOURCE_ID   TEXT    NOT NULL,
                 VERSION     INTEGER NOT NULL,
                 TITLE       TEXT    NOT NULL,
                 CATEGORY    TEXT,
                 HAS_PDF     BOOL NOT NULL DEFAULT 0,
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 SOURCE_FK   INTEGER NOT NULL,
                 UNIQUE (SOURCE_ID, VERSION),
                 FOREIGN KEY (SOURCE_FK) REFERENCES PAPER_ROOTS(SOURCE_FK) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS PAPER_META(
                 PAPER_ID          INTEGER NOT NULL,
                 URL               TEXT,
                 PUBLISHED         DATE,
                 UPDATED           DATE,
                 CATEGORIES        LIST,
                 DOI               TEXT,
                 JOURNAL_REF       TEXT,
                 COMMENT           TEXT,
                 SUMMARY           TEXT,
                 PROVIDER          TEXT DEFAULT 'arxiv',
                 PDF_PATH          TEXT,
                 FULL_TEXT         TEXT,
                 DOWNLOADED_SOURCE BOOL DEFAULT 0,
                 AUTHORS           LIST,
                 TAGS              LIST,
                 CREATED_AT        TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT        TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (PAPER_ID),
                 FOREIGN KEY (PAPER_ID) REFERENCES PAPER(PAPER_ID) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS PAPER_TO_AUTHOR(
                 PTA_FK       INTEGER PRIMARY KEY AUTOINCREMENT,
                 PAPER_ID     INTEGER NOT NULL,
                 AUTHOR_FK    INTEGER NOT NULL,
                 AUTHOR_INDEX INTEGER,
                 CREATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 FOREIGN KEY (PAPER_ID)  REFERENCES PAPER(PAPER_ID)   ON DELETE CASCADE,
                 FOREIGN KEY (AUTHOR_FK) REFERENCES AUTHOR(AUTHOR_FK)
             );
             CREATE TABLE IF NOT EXISTS PAPER_TO_TAG(
                 PTT_FK     INTEGER PRIMARY KEY AUTOINCREMENT,
                 PAPER_ID   INTEGER NOT NULL,
                 SOURCE_ID  TEXT,
                 VERSION    INTEGER,
                 TAG_FK     INTEGER NOT NULL,
                 CREATED_AT DATETIME NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT DATETIME NOT NULL DEFAULT (datetime('now')),
                 FOREIGN KEY (PAPER_ID)           REFERENCES PAPER(PAPER_ID)           ON DELETE CASCADE,
                 FOREIGN KEY (SOURCE_ID, VERSION) REFERENCES PAPER(SOURCE_ID, VERSION),
                 FOREIGN KEY (TAG_FK)             REFERENCES TAG(TAG_FK)
             );
             CREATE TABLE IF NOT EXISTS PROJECT_TO_PAPER(
                 PROJECT_TO_PAPER_FK INTEGER NOT NULL,
                 PROJECT_FK  INTEGER NOT NULL,
                 SOURCE_FK   INTEGER NOT NULL,
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (PROJECT_TO_PAPER_FK),
                 FOREIGN KEY (PROJECT_FK) REFERENCES PROJECT(PROJECT_FK),
                 FOREIGN KEY (SOURCE_FK) REFERENCES paper_roots(SOURCE_FK) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS PROJECT_TO_TAG(
                 PROJECT_TO_TAG_FK INTEGER NOT NULL,
                 PROJECT_FK INTEGER NOT NULL,
                 TAG_FK INTEGER NOT NULL,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (PROJECT_TO_TAG_FK),
                 FOREIGN KEY (PROJECT_FK) REFERENCES PROJECT(PROJECT_FK),
                 FOREIGN KEY (TAG_FK) REFERENCES TAG(TAG_FK)
             );
             CREATE TABLE IF NOT EXISTS NOTE(
                 NOTE_SK     INTEGER NOT NULL,
                 SOURCE_FK   INTEGER NOT NULL,
                 PAPER_ID_FK INTEGER,
                 PROJECT_FK  INTEGER,
                 TITLE       TEXT,
                 NOTE        BLOB,
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (NOTE_SK),
                 FOREIGN KEY (SOURCE_FK)   REFERENCES PAPER_ROOTS(SOURCE_FK) ON DELETE CASCADE,
                 FOREIGN KEY (PAPER_ID_FK) REFERENCES PAPER(PAPER_ID)        ON DELETE SET NULL,
                 FOREIGN KEY (PROJECT_FK)  REFERENCES PROJECT(PROJECT_FK)
             );
             -- Byte-identical to today's sql/tables/papers_fts.sql only by
             -- coincidence of history: this is the v0.2.0 shape, frozen. Do not
             -- retarget it at the sql/ file (see the doc comment above).
             CREATE VIRTUAL TABLE IF NOT EXISTS papers_fts USING fts5(paper_id, full_text);
             CREATE TABLE IF NOT EXISTS DB_VERSION (
                 VERSION_FK  INTEGER   PRIMARY KEY AUTOINCREMENT,
                 VERSION     TEXT      NOT NULL UNIQUE,
                 APPLIED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT OR IGNORE INTO DB_VERSION (VERSION) VALUES ('0.1.2');
             CREATE TABLE IF NOT EXISTS SEARCH_HISTORY (
                 HISTORY_ID INTEGER PRIMARY KEY AUTOINCREMENT,
                 TERM       TEXT      NOT NULL UNIQUE,
                 USE_COUNT  INTEGER   NOT NULL DEFAULT 1,
                 LAST_USED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             CREATE INDEX IF NOT EXISTS idx_search_history_term ON SEARCH_HISTORY (TERM);
             CREATE TABLE IF NOT EXISTS SEARCH_STATE (
                 ID           INTEGER   PRIMARY KEY CHECK (ID = 1),
                 CLAUSES_JSON TEXT      NOT NULL,
                 SOURCE       TEXT      NOT NULL,
                 MAX_RESULTS  INTEGER   NOT NULL DEFAULT 25,
                 RESULTS_JSON TEXT      NOT NULL,
                 SAVED_IDS_JSON TEXT    NOT NULL,
                 SORT_JSON    TEXT,
                 UPDATED_AT   TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             -- The 4 index-creation migrations apply_sql_schema also ran on every
             -- startup at v0.2.0 -- a real v0.2.0 DB already has these.
             CREATE UNIQUE INDEX idx_tag_label_unique ON TAG (TAG COLLATE NOCASE);
             CREATE UNIQUE INDEX idx_project_to_tag_unique ON PROJECT_TO_TAG (PROJECT_FK, TAG_FK);
             CREATE UNIQUE INDEX idx_project_to_paper_unique ON PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK);
             CREATE INDEX IF NOT EXISTS idx_author_full_name ON AUTHOR (AUTHOR_FULL_NAME COLLATE NOCASE);
             -- Representative real-world data across every table.
             INSERT INTO AUTHOR (AUTHOR_FK, AUTHOR_FULL_NAME) VALUES (1, 'Ada Lovelace');
             INSERT INTO TAG (TAG_FK, TAG) VALUES (1, 'ml');
             INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (1, 'My Project');
             INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:2101.00001');
             INSERT INTO PAPER (PAPER_ID, SOURCE_ID, VERSION, TITLE, SOURCE_FK)
                 VALUES (1, 'arxiv:2101.00001', 1, 'A Paper', 1);
             INSERT INTO PAPER_META (PAPER_ID, URL) VALUES (1, 'https://arxiv.org/abs/2101.00001');
             INSERT INTO PAPER_TO_AUTHOR (PAPER_ID, AUTHOR_FK) VALUES (1, 1);
             INSERT INTO PAPER_TO_TAG (PAPER_ID, TAG_FK) VALUES (1, 1);
             INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK) VALUES (1, 1, 1);
             INSERT INTO PROJECT_TO_TAG (PROJECT_TO_TAG_FK, PROJECT_FK, TAG_FK) VALUES (1, 1, 1);
             INSERT INTO NOTE (NOTE_SK, SOURCE_FK, TITLE, NOTE) VALUES (1, 1, 'My Note', 'quantum entanglement');",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        // New tables the Rust port added, none of which existed at v0.2.0.
        for table in [
            "ANNOTATION",
            "VERSION_CHECK",
            "PAPER_TO_READING",
            "RSS_PAPER_ROOTS",
            "RSS_PAPER",
            "RSS_FILTER_RULE",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','virtual table') AND name = ?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                n, 1,
                "{table} must exist after upgrading a real v0.2.0 database"
            );
        }

        // New columns the Rust port added via ALTER TABLE.
        assert!(has_column(&conn, "PROJECT", "IS_READING_LIST").unwrap());
        assert!(has_column(&conn, "PROJECT", "SHARE_ID").unwrap());
        assert!(has_column(&conn, "NOTE", "NOTE_UUID").unwrap());

        // The upgraded PROJECT table must be column-for-column identical to a
        // fresh install -- the same invariant `fresh_install_matches_upgraded_via_migration`
        // checks, now against the real historical shape instead of a hand-built one.
        let fresh = crate::storage::db::open_in_memory().unwrap();
        crate::storage::init_db(&fresh).unwrap();
        assert_eq!(
            table_info(&fresh, "PROJECT"),
            table_info(&conn, "PROJECT"),
            "PROJECT upgraded from a real v0.2.0 database must match a fresh install column-for-column"
        );

        // Every seeded row survives the upgrade untouched (title/name/label), and
        // the new columns are correctly backfilled -- an upgrade that silently
        // dropped or corrupted real user data would fail here.
        let (name, is_reading_list, share_id): (String, i64, Option<String>) = conn
            .query_row(
                "SELECT NAME, IS_READING_LIST, SHARE_ID FROM PROJECT WHERE PROJECT_FK = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "My Project");
        assert_eq!(is_reading_list, 0);
        assert_eq!(share_id, None);

        let (title, note_uuid): (String, Option<String>) = conn
            .query_row(
                "SELECT TITLE, NOTE_UUID FROM NOTE WHERE NOTE_SK = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "My Note");
        assert_eq!(
            note_uuid.map(|u| u.len()),
            Some(36),
            "NOTE_UUID must be backfilled"
        );

        let paper_title: String = conn
            .query_row("SELECT TITLE FROM PAPER WHERE PAPER_ID = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(paper_title, "A Paper");

        let tag: String = conn
            .query_row("SELECT TAG FROM TAG WHERE TAG_FK = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            tag, "ml",
            "the single non-duplicate tag must survive untouched"
        );

        // notes_fts_backfill must have indexed the pre-existing NOTE row (it
        // predates the notes_fts table, which didn't exist at v0.2.0 either).
        let fts_hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'entanglement'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts_hits, 1);
    }
}
