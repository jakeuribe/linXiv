//! Schema DDL. Rust port of `storage/config/core.py::apply_sql_schema`.
//! Plan §5.3 + D6.
//!
//! The `.sql` files are COPIED into `crates/core/sql/` and embedded with
//! include_str! — copying (not referencing ../../../storage) makes the crate
//! self-contained so Phase-6 Python deletion can't break the build.

use rusqlite::Connection;

use crate::error::Result;

// Table DDL in FK-dependency order (the exact `_TABLE_DDL_ORDER` from core.py).
// Only the canonical lowercase `.sql` variants are embedded; the stale
// case-duplicate files (NOTES.SQL=LIBRARY_NOTE, TAG.SQL, PROJECT_TO_TAG.SQL)
// are NOT applied by the Python loader and were not copied.
const TABLE_DDL: &[&str] = &[
    include_str!("../../sql/tables/AUTHOR.sql"),
    include_str!("../../sql/tables/TAG.sql"),
    include_str!("../../sql/tables/PROJECT.sql"),
    include_str!("../../sql/tables/PAPER_ROOTS.sql"),
    include_str!("../../sql/tables/PAPER.sql"),
    include_str!("../../sql/tables/PAPER_META.sql"),
    include_str!("../../sql/tables/PAPER_TO_AUTHOR.sql"),
    include_str!("../../sql/tables/PAPER_TO_TAG.sql"),
    include_str!("../../sql/tables/PROJECT_TO_PAPER.sql"),
    include_str!("../../sql/tables/PAPER_TO_READING.sql"),
    include_str!("../../sql/tables/PROJECT_TO_TAG.sql"),
    include_str!("../../sql/tables/NOTE.sql"),
    include_str!("../../sql/tables/papers_fts.sql"),
    // After NOTE: its sync triggers reference the NOTE table.
    include_str!("../../sql/tables/notes_fts.sql"),
    include_str!("../../sql/tables/DB_VERSION.sql"),
    include_str!("../../sql/tables/SEARCH_HISTORY.sql"),
    include_str!("../../sql/tables/SEARCH_STATE.sql"),
];

// Views are DROP-then-CREATE (idempotent); each references columns added by the
// 7 migrations, so on a legacy DB run_migrations MUST precede this — see init_db.
const VIEW_DDL: &[&str] = &[
    include_str!("../../sql/views/author_paper_counts.sql"),
    include_str!("../../sql/views/papers.sql"),
    // After papers.sql: paper_index_text selects from `papers`. Carries the
    // papers_fts sync triggers, which read that view — see the file header for
    // why they cannot ride along with the table in TABLE_DDL.
    include_str!("../../sql/views/paper_index_text.sql"),
];

/// Create all bundled tables (FK-safe order). FTS5 + JSON1 are compiled in via
/// rusqlite's `bundled` feature, so `papers_fts` and `json_each` are available.
pub fn apply_tables(conn: &Connection) -> Result<()> {
    Ok(TABLE_DDL
        .iter()
        .try_for_each(|ddl| conn.execute_batch(ddl))?)
}

/// (Re)create the `papers` / `latest_papers` / `deleted_papers` and
/// `author_paper_counts` views.
pub fn apply_views(conn: &Connection) -> Result<()> {
    Ok(VIEW_DDL
        .iter()
        .try_for_each(|ddl| conn.execute_batch(ddl))?)
}

// No tables+views shortcut: the sole init path is `super::init_db`, which runs
// run_migrations *between* tables and views. A tables-then-views shortcut would
// skip the four unique indexes the migrations create — never reintroduce one.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{db, migrations};

    /// A DB predating `paper_meta_provider` (migration 02) and
    /// `paper_roots_soft_delete` (migration 01) — the two migrations whose
    /// columns `papers` / `deleted_papers` select. `apply_tables` cannot repair
    /// these: their DDL is `CREATE TABLE IF NOT EXISTS`, so it sees the tables
    /// and no-ops.
    fn legacy_conn() -> rusqlite::Connection {
        let conn = db::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE PAPER_ROOTS(
                 SOURCE_FK  INTEGER PRIMARY KEY AUTOINCREMENT,
                 SOURCE_ID  TEXT    NOT NULL UNIQUE,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE PAPER_META(
                 PAPER_ID   INTEGER NOT NULL PRIMARY KEY,
                 URL        TEXT,
                 PUBLISHED  DATE,
                 UPDATED    DATE,
                 CATEGORIES LIST,
                 DOI        TEXT,
                 JOURNAL_REF TEXT,
                 COMMENT    TEXT,
                 SUMMARY    TEXT,
                 PDF_PATH   TEXT,
                 FULL_TEXT  TEXT,
                 DOWNLOADED_SOURCE BOOL DEFAULT 0,
                 AUTHORS    LIST,
                 TAGS       LIST,
                 CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        conn
    }

    fn has_column(conn: &rusqlite::Connection, table: &str, col: &str) -> bool {
        conn.prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap()
            .iter()
            .any(|n| n.eq_ignore_ascii_case(col))
    }

    /// `apply_tables` runs table DDL and NOTHING else: every `.sql` file is
    /// `CREATE TABLE IF NOT EXISTS`, so a table that already exists is left
    /// exactly as-is — columns added to the base def since are NOT reconciled
    /// onto it.
    ///
    /// This is the contract behind "any column added to a base `.sql` def needs a
    /// companion guarded migration". If someone adds a column to PAPER_META.sql
    /// and skips the migration, fresh installs get it and every existing user DB
    /// silently doesn't — which is what the next test's failure mode looks like.
    #[test]
    fn apply_tables_does_not_reconcile_columns_onto_existing_tables() {
        let conn = legacy_conn();
        assert!(!has_column(&conn, "PAPER_META", "PROVIDER"));

        apply_tables(&conn).unwrap();

        assert!(
            !has_column(&conn, "PAPER_META", "PROVIDER"),
            "apply_tables must not be mistaken for a column reconciler"
        );
        assert!(!has_column(&conn, "PAPER_ROOTS", "STATUS"));
        // It did still create the tables that were genuinely missing.
        assert!(has_column(&conn, "PAPER", "SOURCE_FK"));
    }

    /// The papers_fts sync triggers need no migration of their own: unlike a
    /// column added to an existing table (which `CREATE TABLE IF NOT EXISTS`
    /// cannot reconcile — see `apply_tables_does_not_reconcile_columns_onto_
    /// existing_tables`), a trigger is its own `CREATE ... IF NOT EXISTS`
    /// statement, here in paper_index_text.sql, and apply_views runs on EVERY
    /// open. This pins that: a DB that predates them has them after one init_db,
    /// or search silently stops tracking the library on every existing install.
    #[test]
    fn existing_db_gains_the_papers_fts_sync_triggers() {
        let legacy = legacy_conn();
        crate::storage::init_db(&legacy).unwrap();

        let triggers: Vec<String> = legacy
            .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'papers_fts%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            triggers,
            [
                "papers_fts_meta_ad",
                "papers_fts_meta_ai",
                "papers_fts_meta_au"
            ]
        );
    }

    /// Existing installs must get an *edited* trigger body, not just their first
    /// one. `CREATE TRIGGER IF NOT EXISTS` no-ops against a trigger that already
    /// exists, so it would pin every shipped DB to whatever body it saw first —
    /// silently, and only on upgraded installs. `paper_index_text.sql` drops
    /// before creating, exactly like the view beside it; swap that back to
    /// `IF NOT EXISTS` and this goes red.
    #[test]
    fn an_edited_trigger_body_reaches_an_already_initialised_db() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        crate::storage::init_db(&conn).unwrap();

        // Stand in for "the shipped body changed": replace it with a decoy.
        conn.execute_batch(
            "DROP TRIGGER papers_fts_meta_ai;
             CREATE TRIGGER papers_fts_meta_ai AFTER INSERT ON PAPER_META
             BEGIN SELECT 'stale body'; END;",
        )
        .unwrap();

        crate::storage::init_db(&conn).unwrap();

        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'papers_fts_meta_ai'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !sql.contains("stale body"),
            "re-init left the decoy body in place: {sql}"
        );
        assert!(
            sql.contains("paper_index_text"),
            "re-init did not restore the shipped body: {sql}"
        );
    }

    /// THE ORDERING INVARIANT: tables → migrations → views.
    ///
    /// The views select `PAPER_META.PROVIDER` and `PAPER_ROOTS.STATUS`, columns
    /// only the migrations put on a legacy table. Running tables → views (the
    /// "shortcut" the comment at the bottom of this file forbids) must fail;
    /// slotting run_migrations in between must succeed.
    ///
    /// Reorder `init_db` to tables → views → migrations, or add a
    /// tables-and-views convenience fn, and this test goes red.
    #[test]
    fn views_require_migrations_to_have_run_first() {
        let conn = legacy_conn();
        apply_tables(&conn).unwrap();

        // SQLite resolves a view body lazily, so the failure can surface either at
        // CREATE VIEW or at first SELECT — either way the phase order is wrong.
        let skipped_migrations = apply_views(&conn).and_then(|()| {
            Ok(conn.query_row("SELECT COUNT(*) FROM papers", [], |r| r.get::<_, i64>(0))?)
        });
        let err = skipped_migrations
            .expect_err("views must not build against a DB the migrations haven't touched")
            .to_string();
        assert!(
            err.contains("PROVIDER") || err.contains("STATUS"),
            "expected a missing-migration-column error, got: {err}"
        );

        // Same DB, correct order: migrations first, then the views build and run.
        migrations::run_migrations(&conn).unwrap();
        apply_views(&conn).unwrap();
        conn.query_row("SELECT COUNT(*) FROM papers", [], |r| r.get::<_, i64>(0))
            .unwrap();
        conn.query_row("SELECT COUNT(*) FROM deleted_papers", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    }

    /// The other half of the ordering rule, at the front: `dedup_project_to_paper`
    /// is `pub` precisely so `init_db` can call it OUTSIDE `run_migrations`,
    /// BEFORE `apply_tables`. Once apply_tables has created PAPER_TO_READING with
    /// its composite FK on (PROJECT_FK, SOURCE_FK), any DML on the still-unindexed
    /// parent key is a hard "foreign key mismatch" — including the dedup DELETE
    /// itself. Move the call into the migration list and a legacy DB with
    /// duplicate memberships stops booting.
    #[test]
    fn dedup_project_to_paper_must_run_before_apply_tables() {
        let conn = db::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE PROJECT_TO_PAPER(
                 PROJECT_TO_PAPER_FK INTEGER NOT NULL PRIMARY KEY,
                 PROJECT_FK  INTEGER NOT NULL,
                 SOURCE_FK   INTEGER NOT NULL,
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK)
                 VALUES (1, 7, 42), (2, 7, 42);",
        )
        .unwrap();

        // Wrong order: tables first.
        apply_tables(&conn).unwrap();
        let err = migrations::dedup_project_to_paper(&conn)
            .expect_err("dedup DML after apply_tables must not silently work")
            .to_string();
        assert!(
            err.contains("foreign key mismatch"),
            "expected the composite-FK mismatch that forces the pre-schema call, got: {err}"
        );

        // Right order (what init_db does) on the same legacy shape: boots clean.
        let conn = db::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE PROJECT_TO_PAPER(
                 PROJECT_TO_PAPER_FK INTEGER NOT NULL PRIMARY KEY,
                 PROJECT_FK  INTEGER NOT NULL,
                 SOURCE_FK   INTEGER NOT NULL,
                 CREATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now')),
                 UPDATED_AT  TIMESTAMP NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK)
                 VALUES (1, 7, 42), (2, 7, 42);",
        )
        .unwrap();
        crate::storage::init_db(&conn).unwrap();
    }

    /// Views are DROP-then-CREATE, so `apply_views` is safe to re-run on every
    /// startup (it is: init_db runs unconditionally). A plain CREATE VIEW here
    /// would fail the second call with "table papers already exists".
    #[test]
    fn apply_views_is_idempotent_and_creates_every_view() {
        let conn = db::open_in_memory().unwrap();
        crate::storage::init_db(&conn).unwrap();
        apply_views(&conn).unwrap();

        let mut views: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'view'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        views.sort();
        assert_eq!(
            views,
            vec![
                "author_paper_counts",
                "deleted_papers",
                "latest_papers",
                "paper_index_text",
                "papers"
            ]
        );
    }
}
