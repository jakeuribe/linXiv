//! Idempotent startup migrations. Rust port of the seven `_migrate_*` helpers in
//! `storage/config/core.py`. Plan §5.3 + D6.
//!
//! NON-NEGOTIABLE: every migration here runs on EVERY startup against real user
//! DBs, so each MUST be idempotent — guarded by `PRAGMA table_info` (missing
//! column) or by index existence. Re-running them is the normal case, not the
//! exception (in practice the column/index already exists and each is a no-op).

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::Result;

/// Run all seven idempotent migrations in order. Call between `apply_tables`
/// and `apply_views` (views reference columns these add) — see `super::init_db`.
pub fn run_migrations(conn: &Connection) -> Result<()> {
    paper_roots_soft_delete(conn)?;
    paper_meta_provider(conn)?;
    search_state_sort_json(conn)?;
    tag_label_unique_index(conn)?;
    project_to_tag_unique_index(conn)?;
    project_to_paper_unique_index(conn)?;
    author_full_name_index(conn)?;
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
            conn.execute("UPDATE OR IGNORE PROJECT_TO_TAG SET TAG_FK = ?1 WHERE TAG_FK = ?2", [canon, *fk])?;
            conn.execute("DELETE FROM PROJECT_TO_TAG WHERE TAG_FK = ?1", [*fk])?;
            conn.execute("UPDATE OR IGNORE PAPER_TO_TAG SET TAG_FK = ?1 WHERE TAG_FK = ?2", [canon, *fk])?;
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

fn project_to_paper_unique_index(conn: &Connection) -> Result<()> {
    if index_exists(conn, "idx_project_to_paper_unique")? {
        return Ok(());
    }
    conn.execute_batch(
        "DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_TO_PAPER_FK NOT IN (
             SELECT MIN(PROJECT_TO_PAPER_FK) FROM PROJECT_TO_PAPER GROUP BY PROJECT_FK, SOURCE_FK);
         CREATE UNIQUE INDEX idx_project_to_paper_unique ON PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK);",
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

// ponytail: the one-shot blue→green data migration (sql/migrations/migrate_data.sql
//   — ATTACH old DB, namespace SOURCE_IDs, populate AUTHOR_FIRST/AUTHOR_LAST by a
//   Python-side name split SQLite can't do) is NOT a startup migration and is not
//   ported here. Port it as a one-time CLI subcommand when an upgrade path off the
//   legacy `papers`/`projects`/`notes` schema is actually needed.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::schema;

    #[test]
    fn migrations_are_idempotent() {
        let conn = crate::storage::db::open_in_memory().unwrap();
        schema::apply_tables(&conn).unwrap();
        // fresh schema already has every column/index → all guards skip cleanly,
        // and a second pass must also be a clean no-op.
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();
        assert!(index_exists(&conn, "idx_tag_label_unique").unwrap());
        assert!(index_exists(&conn, "idx_author_full_name").unwrap());
        schema::apply_views(&conn).unwrap();
    }
}
