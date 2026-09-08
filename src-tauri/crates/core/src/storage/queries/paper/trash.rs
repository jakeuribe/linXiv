//! Soft/hard delete and the trash lifecycle over PAPER_ROOTS.

use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::Serialize;

use crate::error::Result;
use crate::storage::db::{bool_from_sql, date_from_sql, list_from_sql, transaction};

use super::fts::refresh_fts;
use super::roots::opt_ts;

/// Latest-version PDF_PATH for a paper (helper for the delete fns).
fn latest_pdf_path(tx: &Transaction, source_id: &str) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT PDF_PATH FROM PAPER_META WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? ORDER BY VERSION DESC LIMIT 1)",
            [source_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// STATUS='deleted', drop the FTS entry. Returns the stored PDF_PATH so the
/// caller can unlink the file (filesystem side-effects are service-layer).
pub fn soft_delete_paper(conn: &mut Connection, source_id: &str) -> Result<Option<String>> {
    transaction(conn, |tx| {
        let path = latest_pdf_path(tx, source_id)?;
        tx.execute("DELETE FROM papers_fts WHERE paper_id = ?", [source_id])?;
        tx.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'deleted', DELETED_AT = datetime('now'), \
             UPDATED_AT = datetime('now') WHERE SOURCE_ID = ?",
            [source_id],
        )?;
        Ok(path)
    })
}

/// `restore_paper` — STATUS='active', and rebuild the FTS entry that
/// `soft_delete_paper` dropped. Returns the stored PDF_PATH (the file may be gone).
pub fn restore_paper(conn: &mut Connection, source_id: &str) -> Result<Option<String>> {
    transaction(conn, |tx| {
        let path: Option<Option<String>> = tx
            .query_row(
                "SELECT PDF_PATH FROM PAPER_META WHERE PAPER_ID IN \
                 (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? ORDER BY VERSION DESC LIMIT 1)",
                [source_id],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'active', DELETED_AT = NULL, \
             UPDATED_AT = datetime('now') WHERE SOURCE_ID = ?",
            [source_id],
        )?;
        refresh_fts(tx, source_id)?;
        Ok(path.flatten())
    })
}

/// Permanently delete the root; dependent tables cascade off the FK (PRAGMA ON).
/// AUTHOR orphans are intentionally NOT cleaned (ADR-0009). Returns the latest
/// PDF_PATH for the caller to unlink.
pub fn hard_delete_paper(conn: &mut Connection, source_id: &str) -> Result<Option<String>> {
    transaction(conn, |tx| {
        let path = latest_pdf_path(tx, source_id)?;
        tx.execute("DELETE FROM papers_fts WHERE paper_id = ?", [source_id])?;
        tx.execute("DELETE FROM PAPER_ROOTS WHERE SOURCE_ID = ?", [source_id])?;
        Ok(path)
    })
}

/// `is_paper_deleted` — true if a PAPER_ROOTS row exists with STATUS='deleted'.
pub fn is_paper_deleted(conn: &Connection, source_id: &str) -> Result<bool> {
    let row: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM PAPER_ROOTS WHERE SOURCE_ID = ? AND STATUS = 'deleted'",
            [source_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(row.is_some())
}

/// Which of `source_ids` are soft-deleted — the batched sibling of
/// [`is_paper_deleted`] for callers checking many ids (e.g. share import).
/// Chunked like the other bulk id queries; unknown ids are simply absent.
pub fn deleted_source_ids(
    conn: &Connection,
    source_ids: &[String],
) -> Result<std::collections::HashSet<String>> {
    let mut out = std::collections::HashSet::new();
    for chunk in source_ids.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE STATUS = 'deleted' \
             AND SOURCE_ID IN ({placeholders})"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |r| r.get::<_, String>(0))?;
        for row in rows {
            out.insert(row?);
        }
    }
    Ok(out)
}

/// A soft-deleted paper from the `deleted_papers` view. Local struct (no model;
/// models.rs out of scope this phase).
#[derive(Debug, Clone, Serialize)]
pub struct DeletedPaper {
    pub source_fk: i64,
    pub source_id: String,
    pub deleted_at: Option<NaiveDateTime>,
    pub title: String,
    pub authors: Vec<String>,
    pub published: Option<NaiveDate>,
    pub pdf_path: Option<String>,
    pub had_pdf: bool,
}

/// `list_deleted_papers` — all soft-deleted papers, newest-deleted first.
pub fn list_deleted_papers(conn: &Connection) -> Result<Vec<DeletedPaper>> {
    let mut stmt = conn.prepare("SELECT * FROM deleted_papers ORDER BY deleted_at DESC")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let authors = row
            .get::<_, Option<String>>("authors")?
            .map_or(Ok(Vec::new()), |s| list_from_sql(&s))?;
        let published = row
            .get::<_, Option<String>>("published")?
            .as_deref()
            .map(date_from_sql)
            .transpose()?;
        out.push(DeletedPaper {
            source_fk: row.get("source_fk")?,
            source_id: row.get("source_id")?,
            deleted_at: opt_ts(row.get::<_, Option<String>>("deleted_at")?)?,
            title: row.get("title")?,
            authors,
            published,
            pdf_path: row.get("pdf_path")?,
            had_pdf: bool_from_sql(row.get::<_, i64>("had_pdf")?),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{count, meta};
    use super::super::*;
    use crate::storage::{db::open_in_memory, init_db};

    #[test]
    fn soft_delete_restore_and_hard_delete() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:d", 1), None).unwrap();
        set_full_text(&mut conn, "arxiv:d", 1, Some("body")).unwrap();

        // Soft delete: hidden from active view, marked deleted, FTS dropped.
        soft_delete_paper(&mut conn, "arxiv:d").unwrap();
        assert!(get_paper(&conn, "arxiv:d", None).unwrap().is_none());
        assert!(is_paper_deleted(&conn, "arxiv:d").unwrap());
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:d"
            ),
            0
        );
        assert_eq!(list_deleted_papers(&conn).unwrap().len(), 1);
        assert_eq!(
            get_paper_root(&conn, "arxiv:d").unwrap().unwrap().status,
            "deleted"
        );

        // Restore: active again, FTS rebuilt from stored full_text.
        restore_paper(&mut conn, "arxiv:d").unwrap();
        assert!(!is_paper_deleted(&conn, "arxiv:d").unwrap());
        assert!(get_paper(&conn, "arxiv:d", None).unwrap().is_some());
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:d"
            ),
            1
        );

        // Hard delete: root gone, children cascade-deleted, FTS gone.
        hard_delete_paper(&mut conn, "arxiv:d").unwrap();
        assert!(get_paper_root(&conn, "arxiv:d").unwrap().is_none());
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER WHERE SOURCE_ID = ?",
                "arxiv:d"
            ),
            0
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM PAPER_TO_AUTHOR pta WHERE pta.PAPER_ID IN (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)", "arxiv:d"),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:d"
            ),
            0
        );
    }

    #[test]
    fn deleted_source_ids_matches_per_id_checks() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:kept", 1), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:gone", 1), None).unwrap();
        soft_delete_paper(&mut conn, "arxiv:gone").unwrap();

        let found = deleted_source_ids(
            &conn,
            &[
                "arxiv:kept".into(),    // active -> absent
                "arxiv:gone".into(),    // trashed -> present
                "arxiv:unknown".into(), // never saved -> absent
            ],
        )
        .unwrap();
        assert_eq!(found, ["arxiv:gone".to_string()].into_iter().collect());
        assert!(deleted_source_ids(&conn, &[]).unwrap().is_empty());
    }
}
