//! PDF path/flag bookkeeping on PAPER / PAPER_META.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{CoreError, Result};
use crate::storage::db::transaction;

/// `set_has_pdf` — flip HAS_PDF for one paper version.
pub fn set_has_pdf(conn: &Connection, source_id: &str, version: i64, has: bool) -> Result<()> {
    conn.execute(
        "UPDATE PAPER SET HAS_PDF = ? WHERE SOURCE_ID = ? AND VERSION = ?",
        params![has as i64, source_id, version],
    )?;
    Ok(())
}

/// Set PDF_PATH for one version, or every version when `version` is None/0
/// (0 is treated as unset).
pub fn set_pdf_path(
    conn: &Connection,
    source_id: &str,
    path: &str,
    version: Option<i64>,
) -> Result<()> {
    match version.filter(|v| *v != 0) {
        Some(v) => conn.execute(
            "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? AND VERSION = ?)",
            params![path, source_id, v],
        )?,
        None => conn.execute(
            "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
            params![path, source_id],
        )?,
    };
    Ok(())
}

/// Stored PDF_PATH for one exact (root, version) row, or None (row absent or
/// path NULL). Post-commit merge cleanup keys on this.
pub fn pdf_path_for_version(
    conn: &Connection,
    source_fk: i64,
    version: i64,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT m.PDF_PATH FROM PAPER p \
             JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
             WHERE p.SOURCE_FK = ?1 AND p.VERSION = ?2",
            params![source_fk, version],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// Resolved (version, stored PDF_PATH) for one active (source_id, version) row,
/// or None — the two-column sibling of `get_paper` (same version-0-means-latest
/// fallthrough), without materializing the row.
pub fn pdf_path_for_source(
    conn: &Connection,
    source_id: &str,
    version: Option<i64>,
) -> Result<Option<(i64, Option<String>)>> {
    let row = match version.filter(|v| *v != 0) {
        Some(v) => conn.query_row(
            "SELECT version, pdf_path FROM papers WHERE source_id = ?1 AND version = ?2",
            params![source_id, v],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
        ),
        None => conn.query_row(
            "SELECT version, pdf_path FROM latest_papers WHERE source_id = ?1",
            params![source_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
        ),
    };
    Ok(row.optional()?)
}

/// Write PDF_PATH and HAS_PDF=1 for one version in a single transaction so a
/// crash cannot leave the two disagreeing. Errors if no row matched.
pub fn mark_pdf_saved(
    conn: &mut Connection,
    source_id: &str,
    path: &str,
    version: i64,
) -> Result<()> {
    transaction(conn, |tx| {
        let meta_rows = tx.execute(
            "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? AND VERSION = ?)",
            params![path, source_id, version],
        )?;
        if meta_rows == 0 {
            return Err(CoreError::Internal(format!(
                "mark_pdf_saved: no PAPER or PAPER_META row for source_id={source_id:?} version={version}"
            )));
        }
        let paper_rows = tx.execute(
            "UPDATE PAPER SET HAS_PDF = 1 WHERE SOURCE_ID = ? AND VERSION = ?",
            params![source_id, version],
        )?;
        if paper_rows == 0 {
            return Err(CoreError::Internal(format!(
                "mark_pdf_saved: no PAPER row for source_id={source_id:?} version={version}"
            )));
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::super::testutil::meta;
    use super::super::*;
    use crate::storage::{db::open_in_memory, init_db};

    #[test]
    fn pdf_setters_and_mark_pdf_saved() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:p", 1), None).unwrap();

        set_has_pdf(&conn, "arxiv:p", 1, true).unwrap();
        assert!(
            get_paper(&conn, "arxiv:p", Some(1))
                .unwrap()
                .unwrap()
                .has_pdf
        );

        set_pdf_path(&conn, "arxiv:p", "/tmp/a.pdf", Some(1)).unwrap();
        assert_eq!(
            get_paper(&conn, "arxiv:p", Some(1))
                .unwrap()
                .unwrap()
                .pdf_path
                .as_deref(),
            Some("/tmp/a.pdf")
        );

        // mark_pdf_saved sets both path and has_pdf atomically.
        set_has_pdf(&conn, "arxiv:p", 1, false).unwrap();
        mark_pdf_saved(&mut conn, "arxiv:p", "/tmp/b.pdf", 1).unwrap();
        let p = get_paper(&conn, "arxiv:p", Some(1)).unwrap().unwrap();
        assert!(p.has_pdf);
        assert_eq!(p.pdf_path.as_deref(), Some("/tmp/b.pdf"));

        // Missing version -> error, nothing partially written.
        let err = mark_pdf_saved(&mut conn, "arxiv:p", "/tmp/c.pdf", 99);
        assert!(err.is_err());
    }
}
