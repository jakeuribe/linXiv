//! Full-text storage and FTS index maintenance, plus the TeX-source backfill
//! candidate queries.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::error::Result;
use crate::models::{ARXIV_ID_PREFIX, ARXIV_PDF_MARKER};
use crate::storage::db::transaction;

// ── Writes (storage/db.py) ────────────────────────────────────────────────────
//
// papers_fts.paper_id holds the SOURCE_ID *string*, not the int PAPER_ID. The
// schema always creates papers_fts (init_db), so the Python `sqlite_master`
// existence guard is dropped — a DELETE/INSERT against it cannot miss.

/// Rows the backfill works on: latest-version active papers with no TeX source
/// yet that `service::paper::source_fetch_url` would accept — an `arxiv:`
/// source_id carrying a `/pdf/` link to derive the tarball URL from. Rows it
/// would reject never leave the list, so listing them makes the backlog readout
/// plateau above zero and puts them in front of the worker on every rebuild.
///
/// Both patterns are built from the constants `source_fetch_url` matches on, so
/// the two rules cannot drift apart. GLOB, not LIKE: LIKE is ASCII-case-
/// insensitive in SQLite, while the Rust-side rule is case-sensitive.
fn backfill_where() -> String {
    format!(
        "FROM latest_papers WHERE COALESCE(downloaded_source, 0) = 0 \
         AND source_id GLOB '{ARXIV_ID_PREFIX}*' AND url GLOB '*{ARXIV_PDF_MARKER}*'"
    )
}

/// SOURCE_IDs of those rows, oldest-published first. Returns ids ONLY:
/// `list_papers` would build a `PaperDetails` per row, so a backfill scan over a
/// large library would materialise the whole library just to filter it out. The
/// caller loads each paper individually instead.
pub fn full_text_backfill_candidates(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT source_id {} ORDER BY published ASC, source_id ASC",
        backfill_where()
    ))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// How many papers `full_text_backfill_candidates` would return, without
/// materialising an id per row — the backlog readout polls this.
pub fn full_text_backfill_count(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) {}", backfill_where()), [], |r| {
            r.get(0)
        })?,
    )
}

/// Re-derive a paper's FTS row from `paper_index_text`, dropping it when the view
/// yields nothing (no version holds text, or the root is soft-deleted). Byte-for-
/// byte the same two statements the PAPER_META triggers run — same view, so the
/// hand-called path and the automatic one cannot disagree about what is indexed.
///
/// Only for the writes the triggers cannot see: a SOURCE_ID rename (the index key
/// itself moves) and an undelete. Writers of FULL_TEXT need not call this — the
/// trigger has already run by the time their UPDATE returns.
pub(super) fn refresh_fts(tx: &Transaction, source_id: &str) -> Result<()> {
    tx.execute("DELETE FROM papers_fts WHERE paper_id = ?", [source_id])?;
    tx.execute(
        "INSERT INTO papers_fts(paper_id, full_text) \
         SELECT source_id, full_text FROM paper_index_text WHERE source_id = ?",
        [source_id],
    )?;
    Ok(())
}

/// `set_full_text` — store extracted TeX and mark DOWNLOADED_SOURCE. No-op if the
/// version does not exist. The FTS index follows by trigger, not from here.
///
/// Empty text marks the version fetched without taking the paper out of search:
/// `paper_index_text` falls back to whatever older version still holds a body. A
/// version bump whose tarball extracts empty (PDF-only or corrupt) is the common
/// way this happens, and the clobber guard (`has_full_text`) cannot see it — it
/// is handed one version.
pub fn set_full_text(
    conn: &mut Connection,
    source_id: &str,
    version: i64,
    full_text: Option<&str>,
) -> Result<()> {
    transaction(conn, |tx| {
        let pid: Option<i64> = tx
            .query_row(
                "SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? AND VERSION = ?",
                params![source_id, version],
                |r| r.get(0),
            )
            .optional()?;
        let Some(pid) = pid else { return Ok(()) };
        tx.execute(
            "UPDATE PAPER_META SET FULL_TEXT = ?, DOWNLOADED_SOURCE = 1 WHERE PAPER_ID = ?",
            params![full_text, pid],
        )?;
        Ok(())
    })
}

/// Whether this exact active version already stores a non-empty TeX body — the
/// commit-time guard that keeps an empty re-fetch from erasing an indexed one.
/// One column on purpose: the body itself can run to megabytes and no caller
/// wants it, only the answer.
pub fn has_full_text(conn: &Connection, source_id: &str, version: i64) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT full_text IS NOT NULL AND full_text != '' FROM papers \
             WHERE source_id = ? AND version = ?",
            params![source_id, version],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{count, meta};
    use super::super::*;
    use crate::storage::{db::open_in_memory, init_db};
    use rusqlite::{params, Connection};

    #[test]
    fn set_full_text_updates_meta_and_fts() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:ft", 1), None).unwrap();

        assert!(!has_full_text(&conn, "arxiv:ft", 1).unwrap());
        set_full_text(&mut conn, "arxiv:ft", 1, Some("the full tex body")).unwrap();
        let p = get_paper(&conn, "arxiv:ft", Some(1)).unwrap().unwrap();
        // `get_paper` blanks the body; the stored column answers through
        // `has_full_text`.
        assert_eq!(p.full_text, None);
        assert!(has_full_text(&conn, "arxiv:ft", 1).unwrap());
        assert!(p.downloaded_source);
        // FTS searchable under the SOURCE_ID string.
        let hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM papers_fts WHERE papers_fts MATCH 'tex' AND paper_id = ?",
                ["arxiv:ft"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1);

        // Refresh is DELETE+INSERT, so no duplicate rows accumulate.
        set_full_text(&mut conn, "arxiv:ft", 1, Some("rewritten")).unwrap();
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:ft"
            ),
            1
        );
    }

    /// THE INVARIANT: papers_fts is derived from FULL_TEXT, so a writer that
    /// stores text WITHOUT going through `set_full_text` still cannot desync
    /// search. Every write below is a raw statement of exactly the shape the
    /// index used to depend on nobody writing; drop either trigger (or the
    /// soft-delete gate) from `paper_index_text.sql` and this test goes red.
    #[test]
    fn raw_full_text_writes_cannot_desync_the_index() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:raw", 1), None).unwrap();
        let matches = |conn: &Connection, body: &str| {
            count(
                conn,
                "SELECT COUNT(*) FROM papers_fts WHERE papers_fts MATCH ? AND paper_id = 'arxiv:raw'",
                body,
            )
        };
        let set_text = |conn: &Connection, version: i64, text: Option<&str>| {
            conn.execute(
                "UPDATE PAPER_META SET FULL_TEXT = ? WHERE PAPER_ID IN \
                 (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = 'arxiv:raw' AND VERSION = ?)",
                params![text, version],
            )
            .unwrap();
        };

        // UPDATE: the body lands in the index without anyone asking it to.
        set_text(&conn, 1, Some("smuggled tex"));
        assert_eq!(matches(&conn, "smuggled"), 1);

        // INSERT: a v2 written with text takes over — one row per paper, newest wins.
        save_paper_metadata(&mut conn, &meta("arxiv:raw", 2), None).unwrap();
        let v2: i64 = conn
            .query_row(
                "SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = 'arxiv:raw' AND VERSION = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute("DELETE FROM PAPER_META WHERE PAPER_ID = ?", [v2])
            .unwrap();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED, FULL_TEXT) \
             VALUES (?, '2024-01-01', 'second tex')",
            [v2],
        )
        .unwrap();
        assert_eq!(matches(&conn, "second"), 1);
        assert_eq!(matches(&conn, "smuggled"), 0);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:raw"
            ),
            1
        );

        // Clearing the newest body falls back to the older version that still has one.
        set_text(&conn, 2, None);
        assert_eq!(matches(&conn, "smuggled"), 1);

        // ...and clearing every body takes the paper out of search entirely.
        set_text(&conn, 1, Some(""));
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:raw"
            ),
            0
        );

        // A soft-deleted paper keeps its FULL_TEXT, so re-deriving must NOT put it
        // back into search — the STATUS gate lives in `paper_index_text`.
        soft_delete_paper(&mut conn, "arxiv:raw").unwrap();
        set_text(&conn, 1, Some("resurrected tex"));
        assert_eq!(matches(&conn, "resurrected"), 0);
    }

    /// Dropping a version's meta row changes which body is newest, so the index
    /// has to follow. Delete `papers_fts_meta_ad` and this goes red: search keeps
    /// answering with a body whose row no longer exists.
    #[test]
    fn deleting_the_newest_meta_row_falls_back_to_an_older_body() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let matches = |conn: &Connection, body: &str| {
            count(
                conn,
                "SELECT COUNT(*) FROM papers_fts WHERE papers_fts MATCH ? AND paper_id = 'arxiv:del'",
                body,
            )
        };
        let set_text = |conn: &Connection, version: i64, text: &str| {
            conn.execute(
                "UPDATE PAPER_META SET FULL_TEXT = ? WHERE PAPER_ID IN \
                 (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = 'arxiv:del' AND VERSION = ?)",
                params![text, version],
            )
            .unwrap();
        };

        save_paper_metadata(&mut conn, &meta("arxiv:del", 1), None).unwrap();
        set_text(&conn, 1, "older body");
        save_paper_metadata(&mut conn, &meta("arxiv:del", 2), None).unwrap();
        set_text(&conn, 2, "newer body");
        assert_eq!(matches(&conn, "newer"), 1);
        assert_eq!(matches(&conn, "older"), 0);

        conn.execute(
            "DELETE FROM PAPER_META WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = 'arxiv:del' AND VERSION = 2)",
            [],
        )
        .unwrap();

        assert_eq!(matches(&conn, "older"), 1, "index did not fall back");
        assert_eq!(matches(&conn, "newer"), 0, "deleted body still searchable");
    }

    /// Re-adding a trashed paper whose version is already stored: `INSERT OR
    /// IGNORE` no-ops, so `write_paper_version_in_tx` returns before any
    /// PAPER_META write and no trigger fires. The un-delete in
    /// `ensure_paper_root_row` has to re-derive, or the paper comes back active,
    /// with a body, and permanently absent from search.
    #[test]
    fn readding_a_trashed_paper_returns_it_to_search() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let indexed = |conn: &Connection| {
            count(
                conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:trash",
            )
        };

        save_paper_metadata(&mut conn, &meta("arxiv:trash", 1), None).unwrap();
        conn.execute(
            "UPDATE PAPER_META SET FULL_TEXT = 'kept body' WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = 'arxiv:trash')",
            [],
        )
        .unwrap();
        assert_eq!(indexed(&conn), 1);

        soft_delete_paper(&mut conn, "arxiv:trash").unwrap();
        assert_eq!(indexed(&conn), 0, "trashed paper must leave the index");

        // Re-fetching the SAME version — the no-op path, not a new version.
        save_paper_metadata(&mut conn, &meta("arxiv:trash", 1), None).unwrap();
        assert_eq!(
            indexed(&conn),
            1,
            "re-added paper is active with a stored body but absent from search"
        );
    }
}
