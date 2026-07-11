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
