//! PAPER_ROOTS lifecycle: ensuring/reactivating the root row and resolving
//! between SOURCE_FK and SOURCE_ID.

use chrono::NaiveDateTime;
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::Serialize;

use crate::error::Result;
use crate::storage::db::{timestamp_from_sql, transaction};

use super::fts::refresh_fts;

/// `_ensure_paper_root_row` — INSERT OR IGNORE the root, then reactivate it if it
/// was soft-deleted. Returns SOURCE_FK. Runs in the caller's tx.
pub(super) fn ensure_paper_root_row(tx: &Transaction, source_id: &str) -> Result<i64> {
    tx.execute(
        "INSERT OR IGNORE INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)",
        [source_id],
    )?;
    let (fk, status): (i64, String) = tx.query_row(
        "SELECT SOURCE_FK, STATUS FROM PAPER_ROOTS WHERE SOURCE_ID = ?",
        [source_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if status == "deleted" {
        tx.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'active', DELETED_AT = NULL, \
             UPDATED_AT = datetime('now') WHERE SOURCE_ID = ?",
            [source_id],
        )?;
        // Soft delete drops the FTS row but keeps FULL_TEXT, and STATUS lives on
        // PAPER_ROOTS — no PAPER_META write happens here, so no trigger fires.
        // Re-deriving is the un-delete's job. Without this, re-adding a trashed
        // paper whose version is already stored leaves it active, with a body,
        // and absent from search permanently.
        refresh_fts(tx, source_id)?;
    }
    Ok(fk)
}

/// `ensure_paper_root` — INSERT OR IGNORE the root (reactivating if deleted).
/// Returns its SOURCE_FK.
pub fn ensure_paper_root(conn: &mut Connection, source_id: &str) -> Result<i64> {
    transaction(conn, |tx| ensure_paper_root_row(tx, source_id))
}

/// `get_source_id` — SOURCE_ID for a SOURCE_FK, or None.
pub fn get_source_id(conn: &Connection, source_fk: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_FK = ?",
            [source_fk],
            |r| r.get(0),
        )
        .optional()?)
}

/// `service/paper.py::sfks_to_source_ids` — resolve SOURCE_FKs to SOURCE_IDs,
/// dropping any that do not exist. Input order is preserved.
///
/// Batched: project listings resolve every paper of every project through
/// here, so this must not be a query per fk. Chunked to stay under SQLite's
/// bound-variable limit.
pub fn sfks_to_source_ids(conn: &Connection, source_fks: &[i64]) -> Result<Vec<String>> {
    let by_fk = source_ids_by_fk(conn, source_fks)?;
    Ok(source_fks
        .iter()
        .filter_map(|fk| by_fk.get(fk).cloned())
        .collect())
}

/// SOURCE_FK → SOURCE_ID map for a set of fks; nonexistent fks are simply
/// absent. The batched sibling of `get_source_id` for callers that resolve
/// many rows (e.g. share snapshots) — chunked like `sfks_to_source_ids`.
pub fn source_ids_by_fk(
    conn: &Connection,
    source_fks: &[i64],
) -> Result<std::collections::HashMap<i64, String>> {
    let mut by_fk = std::collections::HashMap::with_capacity(source_fks.len());
    for chunk in source_fks.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT SOURCE_FK, SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_FK IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (fk, sid) = row?;
            by_fk.insert(fk, sid);
        }
    }
    Ok(by_fk)
}

/// SOURCE_ID → SOURCE_FK map for a set of ids; unknown ids are simply absent.
/// The inverse of [`source_ids_by_fk`], for callers resolving many ids at once
/// (e.g. bulk project membership ops) — chunked the same way. No status filter,
/// matching `get_paper_root`: trashed papers resolve.
pub fn source_fks_by_id(
    conn: &Connection,
    source_ids: &[&str],
) -> Result<std::collections::HashMap<String, i64>> {
    let mut by_id = std::collections::HashMap::with_capacity(source_ids.len());
    for chunk in source_ids.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT SOURCE_ID, SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (sid, fk) = row?;
            by_id.insert(sid, fk);
        }
    }
    Ok(by_id)
}

/// PAPER_ROOTS row. No model exists (PAPER_ROOTS is storage-internal) and
/// models.rs is out of scope this phase, so this local struct carries the row.
#[derive(Debug, Clone, Serialize)]
pub struct PaperRoot {
    pub source_fk: i64,
    pub source_id: String,
    pub status: String,
    pub deleted_at: Option<NaiveDateTime>,
    pub created_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
}

pub(super) fn opt_ts(s: Option<String>) -> Result<Option<NaiveDateTime>> {
    s.as_deref().map(timestamp_from_sql).transpose()
}

/// `get_paper_root` — the PAPER_ROOTS row for a source_id, or None.
pub fn get_paper_root(conn: &Connection, source_id: &str) -> Result<Option<PaperRoot>> {
    conn.query_row(
        "SELECT SOURCE_FK, SOURCE_ID, STATUS, DELETED_AT, CREATED_AT, UPDATED_AT \
         FROM PAPER_ROOTS WHERE SOURCE_ID = ?",
        [source_id],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        },
    )
    .optional()?
    .map(|(source_fk, source_id, status, del, cre, upd)| {
        Ok(PaperRoot {
            source_fk,
            source_id,
            status,
            deleted_at: opt_ts(del)?,
            created_at: opt_ts(cre)?,
            updated_at: opt_ts(upd)?,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::super::testutil::meta;
    use super::super::*;
    use crate::storage::{db::open_in_memory, init_db};

    #[test]
    fn root_helpers_and_versions() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:v", 1), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:v", 2), None).unwrap();

        let fk = ensure_paper_root(&mut conn, "arxiv:v").unwrap();
        assert_eq!(
            get_source_id(&conn, fk).unwrap().as_deref(),
            Some("arxiv:v")
        );
        assert_eq!(get_source_id(&conn, 999_999).unwrap(), None);
        assert_eq!(
            sfks_to_source_ids(&conn, &[fk, 999_999]).unwrap(),
            vec!["arxiv:v".to_string()]
        );
        let by_id = source_fks_by_id(&conn, &["arxiv:v", "ghost"]).unwrap();
        assert_eq!(by_id.get("arxiv:v"), Some(&fk));
        assert!(!by_id.contains_key("ghost"));
        assert!(source_fks_by_id(&conn, &[]).unwrap().is_empty());

        let versions = get_all_versions(&conn, "arxiv:v").unwrap();
        assert_eq!(
            versions.iter().map(|p| p.version).collect::<Vec<_>>(),
            vec![1, 2]
        );

        // ensure_paper_root reactivates a soft-deleted root.
        soft_delete_paper(&mut conn, "arxiv:v").unwrap();
        assert!(is_paper_deleted(&conn, "arxiv:v").unwrap());
        // No status filter: trashed papers still resolve, like get_paper_root.
        assert_eq!(
            source_fks_by_id(&conn, &["arxiv:v"]).unwrap()["arxiv:v"],
            fk
        );
        ensure_paper_root(&mut conn, "arxiv:v").unwrap();
        assert!(!is_paper_deleted(&conn, "arxiv:v").unwrap());
    }
}
