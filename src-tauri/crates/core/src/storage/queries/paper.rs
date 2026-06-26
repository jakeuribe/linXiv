use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, Transaction};
use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::models::{PaperDetails, PaperMetadata};
use crate::storage::db::{
    bool_from_sql, date_from_sql, date_to_sql, list_from_sql, list_to_sql, timestamp_from_sql,
    transaction,
};

// Both functions select `*` from the `papers` / `latest_papers` views (same
// column set), so one row->model mapper serves both. LIST/DATE/BOOL columns go
// through the storage::db decltype converters — no inline re-parsing.
fn row_to_paper(row: &Row) -> Result<PaperDetails> {
    // LIST column (JSON TEXT) -> Vec<String>; NULL -> empty (model default).
    let list = |name: &str| -> Result<Vec<String>> {
        match row.get::<_, Option<String>>(name)? {
            Some(s) => list_from_sql(&s),
            None => Ok(Vec::new()),
        }
    };
    // DATE column (ISO TEXT) -> NaiveDate; NULL -> None.
    let date = |name: &str| -> Result<Option<NaiveDate>> {
        match row.get::<_, Option<String>>(name)? {
            Some(s) => Ok(Some(date_from_sql(&s)?)),
            None => Ok(None),
        }
    };
    Ok(PaperDetails {
        paper_id: row.get("paper_id")?,
        source_id: row.get("source_id")?,
        version: row.get("version")?,
        title: row.get("title")?,
        summary: row.get("summary")?,
        published: date("published")?,
        updated: date("updated")?,
        url: row.get("url")?,
        doi: row.get("doi")?,
        category: row.get("category")?,
        categories: list("categories")?,
        journal_ref: row.get("journal_ref")?,
        comment: row.get("comment")?,
        authors: list("authors")?,
        tags: list("tags")?,
        has_pdf: bool_from_sql(row.get::<_, i64>("has_pdf")?),
        pdf_path: row.get("pdf_path")?,
        source: row.get("source")?,
        full_text: row.get("full_text")?,
        downloaded_source: bool_from_sql(row.get::<_, Option<i64>>("downloaded_source")?.unwrap_or(0)),
        source_fk: row.get("source_fk")?,
    })
}

/// `storage/db.py::get_paper` — a specific version, or the latest if `None`.
/// `conn` is an opened storage::db connection (FK PRAGMA already ON).
pub fn get_paper(conn: &Connection, source_id: &str, version: Option<i64>) -> Result<Option<PaperDetails>> {
    // Python `if version:` treats 0 as falsy too -> fall through to latest.
    let (sql, params): (&str, Vec<Value>) = match version.filter(|v| *v != 0) {
        Some(v) => (
            "SELECT * FROM papers WHERE source_id = ? AND version = ?",
            vec![Value::Text(source_id.to_string()), Value::Integer(v)],
        ),
        None => (
            "SELECT * FROM latest_papers WHERE source_id = ?",
            vec![Value::Text(source_id.to_string())],
        ),
    };
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params_from_iter(&params))?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_paper(row)?)),
        None => Ok(None),
    }
}

/// `storage/db.py::list_papers` — latest version per paper by default.
/// Optional exact-category filter; limit/offset apply to the filtered result.
pub fn list_papers(
    conn: &Connection,
    latest_only: bool,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
) -> Result<Vec<PaperDetails>> {
    let mut sql = if latest_only {
        "SELECT * FROM latest_papers".to_string()
    } else {
        "SELECT * FROM papers".to_string()
    };
    let mut params: Vec<Value> = Vec::new();
    if let Some(cat) = category {
        sql.push_str(" WHERE category = ?");
        params.push(Value::Text(cat.to_string()));
    }
    sql.push_str(" ORDER BY published DESC");
    match limit {
        Some(l) => {
            sql.push_str(" LIMIT ? OFFSET ?");
            params.push(Value::Integer(l));
            params.push(Value::Integer(offset));
        }
        // No limit but a nonzero offset still needs LIMIT -1 (all rows) to skip.
        None if offset != 0 => {
            sql.push_str(" LIMIT -1 OFFSET ?");
            params.push(Value::Integer(offset));
        }
        None => {}
    }

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(&params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_paper(row)?);
    }
    Ok(out)
}

// ── Writes (storage/db.py) ────────────────────────────────────────────────────
//
// papers_fts.paper_id holds the SOURCE_ID *string*, not the int PAPER_ID. The
// schema always creates papers_fts (init_db), so the Python `sqlite_master`
// existence guard is dropped — a DELETE/INSERT against it cannot miss.

/// Nullable LIST column value: `Some([..])` -> JSON TEXT, `None` -> NULL.
fn opt_list_val(v: &Option<Vec<String>>) -> Value {
    match v {
        Some(x) => Value::Text(list_to_sql(x)),
        None => Value::Null,
    }
}

/// Nullable DATE column value.
fn opt_date_val(d: &Option<NaiveDate>) -> Value {
    match d {
        Some(d) => Value::Text(date_to_sql(*d)),
        None => Value::Null,
    }
}

/// `_ensure_paper_root_row` — INSERT OR IGNORE the root, then reactivate it if it
/// was soft-deleted. Returns (SOURCE_FK, was_restored). Runs in the caller's tx.
fn ensure_paper_root_row(tx: &Transaction, source_id: &str) -> Result<(i64, bool)> {
    tx.execute(
        "INSERT OR IGNORE INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)",
        [source_id],
    )?;
    let (fk, status): (i64, String) = tx.query_row(
        "SELECT SOURCE_FK, STATUS FROM PAPER_ROOTS WHERE SOURCE_ID = ?",
        [source_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let restored = status == "deleted";
    if restored {
        tx.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'active', DELETED_AT = NULL, \
             UPDATED_AT = datetime('now') WHERE SOURCE_ID = ?",
            [source_id],
        )?;
    }
    Ok((fk, restored))
}

/// `_author_fk_for_name` — find (case-insensitive) or create an AUTHOR row.
fn author_fk_for_name(tx: &Transaction, full_name: &str) -> Result<i64> {
    if let Some(fk) = tx
        .query_row(
            "SELECT AUTHOR_FK FROM AUTHOR WHERE AUTHOR_FULL_NAME = ? COLLATE NOCASE LIMIT 1",
            [full_name],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(fk);
    }
    tx.execute("INSERT INTO AUTHOR (AUTHOR_FULL_NAME) VALUES (?)", [full_name])?;
    Ok(tx.last_insert_rowid())
}

/// `_tag_fk_for_label` — find (case-insensitive) or create a TAG row.
fn tag_fk_for_label(tx: &Transaction, label: &str) -> Result<i64> {
    if let Some(fk) = tx
        .query_row(
            "SELECT TAG_FK FROM TAG WHERE TAG = ? COLLATE NOCASE LIMIT 1",
            [label],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(fk);
    }
    tx.execute("INSERT INTO TAG (TAG) VALUES (?)", [label])?;
    Ok(tx.last_insert_rowid())
}

/// `_sync_paper_authors` — relational half of dual author storage. Replaces the
/// PAPER_TO_AUTHOR rows for this paper, then garbage-collects AUTHOR rows that no
/// paper references any more (ADR-0009: hard-delete leaves orphans, this does not).
fn sync_paper_authors(tx: &Transaction, paper_id: i64, authors: &[String]) -> Result<()> {
    let old_fks: Vec<i64> = {
        let mut stmt =
            tx.prepare("SELECT AUTHOR_FK FROM PAPER_TO_AUTHOR WHERE PAPER_ID = ?")?;
        let rows = stmt.query_map([paper_id], |r| r.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    tx.execute("DELETE FROM PAPER_TO_AUTHOR WHERE PAPER_ID = ?", [paper_id])?;
    for (i, name) in authors.iter().enumerate() {
        let aid = author_fk_for_name(tx, name)?;
        tx.execute(
            "INSERT INTO PAPER_TO_AUTHOR (PAPER_ID, AUTHOR_FK, AUTHOR_INDEX) VALUES (?, ?, ?)",
            params![paper_id, aid, i as i64],
        )?;
    }
    for fk in old_fks {
        let still: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM PAPER_TO_AUTHOR WHERE AUTHOR_FK = ? LIMIT 1",
                [fk],
                |r| r.get(0),
            )
            .optional()?;
        if still.is_none() {
            tx.execute("DELETE FROM AUTHOR WHERE AUTHOR_FK = ?", [fk])?;
        }
    }
    Ok(())
}

/// `_sync_paper_tags` — relational half of dual tag storage. Replaces the
/// PAPER_TO_TAG rows (PAPER_ID + composite (SOURCE_ID, VERSION)) for this paper.
fn sync_paper_tags(
    tx: &Transaction,
    paper_id: i64,
    source_id: &str,
    version: i64,
    tags: Option<&[String]>,
) -> Result<()> {
    tx.execute("DELETE FROM PAPER_TO_TAG WHERE PAPER_ID = ?", [paper_id])?;
    let Some(tags) = tags else { return Ok(()) };
    for label in tags {
        if label.is_empty() {
            continue;
        }
        let tid = tag_fk_for_label(tx, label)?;
        tx.execute(
            "INSERT INTO PAPER_TO_TAG (PAPER_ID, SOURCE_ID, VERSION, TAG_FK) VALUES (?, ?, ?, ?)",
            params![paper_id, source_id, version, tid],
        )?;
    }
    Ok(())
}

/// `_insert_metadata`'s tag merge: union of the paper's own tags and the extra
/// tags, deduped. `None`/empty extra leaves the paper's tags (incl. `None`)
/// untouched — preserving the NULL-vs-`[]` distinction in PAPER_META.TAGS.
fn merge_tags(base: &Option<Vec<String>>, extra: Option<&[String]>) -> Option<Vec<String>> {
    match extra {
        Some(e) if !e.is_empty() => {
            let mut out: Vec<String> = base.clone().unwrap_or_default();
            for t in e {
                if !out.contains(t) {
                    out.push(t.clone());
                }
            }
            Some(out)
        }
        _ => base.clone(),
    }
}

/// `_write_paper_version` — INSERT OR IGNORE one PAPER version + its PAPER_META,
/// then sync the relational tag/author rows. A duplicate (SOURCE_ID, VERSION) is
/// a no-op (matches Python's early return). `pdf_path`/`full_text`/
/// `downloaded_source` are always NULL on this path; FTS is not touched here
/// (full_text is None) — set_full_text/repair/restore own the FTS index.
fn write_paper_version(
    tx: &Transaction,
    meta: &PaperMetadata,
    merged_tags: &Option<Vec<String>>,
) -> Result<()> {
    let (source_fk, _) = ensure_paper_root_row(tx, &meta.source_id)?;
    let changed = tx.execute(
        "INSERT OR IGNORE INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) \
         VALUES (?, ?, ?, ?, 0, ?)",
        params![meta.source_id, meta.version, meta.title, meta.category, source_fk],
    )?;
    if changed == 0 {
        return Ok(());
    }
    let paper_id = tx.last_insert_rowid();
    let source = meta.source.clone().unwrap_or_default();
    tx.execute(
        "INSERT INTO PAPER_META (\
            PAPER_ID, URL, PUBLISHED, UPDATED, CATEGORIES, DOI, JOURNAL_REF, \
            COMMENT, SUMMARY, PROVIDER, PDF_PATH, FULL_TEXT, DOWNLOADED_SOURCE, AUTHORS, TAGS\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, ?, ?)",
        params![
            paper_id,
            meta.url,
            date_to_sql(meta.published),
            opt_date_val(&meta.updated),
            opt_list_val(&meta.categories),
            meta.doi,
            meta.journal_ref,
            meta.comment,
            meta.summary,
            source,
            list_to_sql(&meta.authors),
            opt_list_val(merged_tags),
        ],
    )?;
    tx.execute(
        "UPDATE PAPER SET UPDATED_AT = date('now') WHERE PAPER_ID = ?",
        [paper_id],
    )?;
    sync_paper_authors(tx, paper_id, &meta.authors)?;
    sync_paper_tags(tx, paper_id, &meta.source_id, meta.version, merged_tags.as_deref())?;
    Ok(())
}

/// `save_paper_metadata` — persist one paper version atomically (PAPER +
/// PAPER_META + PAPER_ROOTS + dual tag/author sync). `extra_tags` are merged into
/// the paper's own tags. Returns (source_id, version).
pub fn save_paper_metadata(
    conn: &mut Connection,
    meta: &PaperMetadata,
    extra_tags: Option<&[String]>,
) -> Result<(String, i64)> {
    let merged = merge_tags(&meta.tags, extra_tags);
    transaction(conn, |tx| write_paper_version(tx, meta, &merged))?;
    Ok((meta.source_id.clone(), meta.version))
}

/// `db.add_paper_tags` — UNION `tags` onto a paper's existing tags across BOTH
/// halves of dual tag storage: the JSON `PAPER_META.TAGS` list (all versions) and
/// the relational `PAPER_TO_TAG` rows (re-synced per version). Dedup preserves
/// first-seen order (Python `dict.fromkeys`). Returns the merged tag list. Errors
/// if the paper has no latest version.
pub fn add_paper_tags(conn: &mut Connection, source_id: &str, tags: &[String]) -> Result<Vec<String>> {
    transaction(conn, |tx| {
        let current: Option<Option<String>> = tx
            .query_row(
                "SELECT tags FROM latest_papers WHERE source_id = ?",
                [source_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?;
        let Some(current_json) = current else {
            return Err(CoreError::NotFound(format!("paper {source_id:?} not found")));
        };
        let mut merged = match current_json {
            Some(s) => list_from_sql(&s)?,
            None => Vec::new(),
        };
        for t in tags {
            if !merged.contains(t) {
                merged.push(t.clone());
            }
        }
        tx.execute(
            "UPDATE PAPER_META SET TAGS = ? WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
            params![list_to_sql(&merged), source_id],
        )?;
        let versions: Vec<(i64, i64)> = {
            let mut stmt = tx.prepare("SELECT PAPER_ID, VERSION FROM PAPER WHERE SOURCE_ID = ?")?;
            let rows = stmt.query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (pid, ver) in versions {
            sync_paper_tags(tx, pid, source_id, ver, Some(&merged))?;
        }
        Ok(merged)
    })
}

/// `db.remove_paper_tags` — remove `tags` from a paper across BOTH halves of dual
/// tag storage: the JSON `PAPER_META.TAGS` list (all versions) and the relational
/// `PAPER_TO_TAG` rows (re-synced per version). Returns the remaining tag list.
/// Errors if the paper has no latest version. Symmetric with `add_paper_tags`.
pub fn remove_paper_tags(conn: &mut Connection, source_id: &str, tags: &[String]) -> Result<Vec<String>> {
    let remove: std::collections::HashSet<&str> = tags.iter().map(String::as_str).collect();
    transaction(conn, |tx| {
        let current: Option<Option<String>> = tx
            .query_row(
                "SELECT tags FROM latest_papers WHERE source_id = ?",
                [source_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?;
        let Some(current_json) = current else {
            return Err(CoreError::NotFound(format!("paper {source_id:?} not found")));
        };
        let updated: Vec<String> = match current_json {
            Some(s) => list_from_sql(&s)?
                .into_iter()
                .filter(|t| !remove.contains(t.as_str()))
                .collect(),
            None => Vec::new(),
        };
        tx.execute(
            "UPDATE PAPER_META SET TAGS = ? WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
            params![list_to_sql(&updated), source_id],
        )?;
        let versions: Vec<(i64, i64)> = {
            let mut stmt = tx.prepare("SELECT PAPER_ID, VERSION FROM PAPER WHERE SOURCE_ID = ?")?;
            let rows = stmt.query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (pid, ver) in versions {
            sync_paper_tags(tx, pid, source_id, ver, Some(&updated))?;
        }
        Ok(updated)
    })
}

/// `ensure_paper_root` — INSERT OR IGNORE the root (reactivating if deleted).
/// Returns its SOURCE_FK.
pub fn ensure_paper_root(conn: &mut Connection, source_id: &str) -> Result<i64> {
    transaction(conn, |tx| Ok(ensure_paper_root_row(tx, source_id)?.0))
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
/// dropping any that do not exist.
pub fn sfks_to_source_ids(conn: &Connection, source_fks: &[i64]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for &sfk in source_fks {
        if let Some(sid) = get_source_id(conn, sfk)? {
            out.push(sid);
        }
    }
    Ok(out)
}

/// `repair_paper` — in-place metadata repair keyed by the stable SOURCE_FK,
/// migrating SOURCE_ID if the full id changed.
///
/// Composite-FK ORDER is load-bearing: PAPER.SOURCE_ID is renamed BEFORE
/// PAPER_TO_TAG.SOURCE_ID (whose (SOURCE_ID, VERSION) FK references PAPER), then
/// the FTS row is moved (DELETE old id + INSERT new id). Wrong order = FK
/// violation or orphaned/duplicated FTS rows.
pub fn repair_paper(conn: &mut Connection, source_fk: i64, meta: &PaperMetadata) -> Result<()> {
    transaction(conn, |tx| {
        // Defer FK checks to commit: immediate FK rejects a parent-key rename
        // (PAPER.SOURCE_ID) while child rows (PAPER_TO_TAG) still reference the
        // old value, no matter the statement order. Deferring lets the documented
        // PAPER-first ordering land all renames before the single commit-time check.
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let old_id: Option<String> = tx
            .query_row(
                "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_FK = ?",
                [source_fk],
                |r| r.get(0),
            )
            .optional()?;
        let Some(old_id) = old_id else { return Ok(()) };
        let new_id = &meta.source_id;
        let renamed = *new_id != old_id;
        if renamed {
            // PAPER first (PAPER_TO_TAG's composite FK references it), then roots,
            // then the PAPER_TO_TAG rename.
            tx.execute(
                "UPDATE PAPER SET SOURCE_ID = ? WHERE SOURCE_FK = ?",
                params![new_id, source_fk],
            )?;
            tx.execute(
                "UPDATE PAPER_ROOTS SET SOURCE_ID = ? WHERE SOURCE_FK = ?",
                params![new_id, source_fk],
            )?;
            tx.execute(
                "UPDATE PAPER_TO_TAG SET SOURCE_ID = ? WHERE SOURCE_ID = ?",
                params![new_id, old_id],
            )?;
        }

        let row: Option<(i64, i64)> = tx
            .query_row(
                "SELECT PAPER_ID, VERSION FROM PAPER WHERE SOURCE_FK = ? ORDER BY VERSION DESC LIMIT 1",
                [source_fk],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((pid, ver)) = row else { return Ok(()) };

        if renamed {
            // FTS5 has no UPDATE: move the latest version's full_text entry from
            // old id to new id (paper_id column holds the SOURCE_ID string).
            let full_text: Option<String> = tx
                .query_row(
                    "SELECT FULL_TEXT FROM PAPER_META WHERE PAPER_ID = ?",
                    [pid],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            tx.execute("DELETE FROM papers_fts WHERE paper_id = ?", [&old_id])?;
            if let Some(ft) = full_text.filter(|s| !s.is_empty()) {
                tx.execute(
                    "INSERT INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
                    params![new_id, ft],
                )?;
            }
        }

        tx.execute(
            "UPDATE PAPER SET TITLE = ?, CATEGORY = ? WHERE PAPER_ID = ?",
            params![meta.title, meta.category, pid],
        )?;
        tx.execute(
            "UPDATE PAPER_META SET \
                AUTHORS = ?, PUBLISHED = ?, DOI = ?, URL = ?, \
                SUMMARY = ?, TAGS = ?, UPDATED_AT = datetime('now') \
             WHERE PAPER_ID = ?",
            params![
                list_to_sql(&meta.authors),
                date_to_sql(meta.published),
                meta.doi,
                meta.url,
                meta.summary,
                opt_list_val(&meta.tags),
                pid,
            ],
        )?;
        sync_paper_authors(tx, pid, &meta.authors)?;
        sync_paper_tags(tx, pid, new_id, ver, meta.tags.as_deref())?;
        Ok(())
    })
}

/// `set_has_pdf` — flip HAS_PDF for one paper version.
pub fn set_has_pdf(conn: &Connection, source_id: &str, version: i64, has: bool) -> Result<()> {
    conn.execute(
        "UPDATE PAPER SET HAS_PDF = ? WHERE SOURCE_ID = ? AND VERSION = ?",
        params![has as i64, source_id, version],
    )?;
    Ok(())
}

/// `set_pdf_path` — set PDF_PATH for one version, or every version when
/// `version` is None/0 (Python `if version:` treats 0 as falsy).
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

/// `mark_pdf_saved` — write PDF_PATH and HAS_PDF=1 for one version in a single
/// transaction so a crash cannot leave the two disagreeing. Errors if no matching
/// PAPER_META or PAPER row (0 rows updated).
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

/// `set_full_text` — store extracted TeX, mark DOWNLOADED_SOURCE, refresh the FTS
/// index (DELETE then INSERT). No-op if the version does not exist.
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
        tx.execute("DELETE FROM papers_fts WHERE paper_id = ?", [source_id])?;
        tx.execute(
            "INSERT INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
            params![source_id, full_text],
        )?;
        Ok(())
    })
}

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

/// `soft_delete_paper` — STATUS='deleted', drop the FTS entry. Returns the stored
/// PDF_PATH so the caller can unlink the file (filesystem side-effects + the
/// post-unlink HAS_PDF=0 reset are service-layer, not DB consistency).
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

/// `restore_paper` — STATUS='active', and if the latest version has full_text,
/// rebuild its FTS entry. Returns the stored PDF_PATH (the file may be gone).
pub fn restore_paper(conn: &mut Connection, source_id: &str) -> Result<Option<String>> {
    transaction(conn, |tx| {
        let row: Option<(Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT PDF_PATH, FULL_TEXT FROM PAPER_META WHERE PAPER_ID IN \
                 (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? ORDER BY VERSION DESC LIMIT 1)",
                [source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        tx.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'active', DELETED_AT = NULL, \
             UPDATED_AT = datetime('now') WHERE SOURCE_ID = ?",
            [source_id],
        )?;
        let mut path = None;
        if let Some((p, ft)) = row {
            path = p;
            if let Some(ft) = ft.filter(|s| !s.is_empty()) {
                tx.execute("DELETE FROM papers_fts WHERE paper_id = ?", [source_id])?;
                tx.execute(
                    "INSERT INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
                    params![source_id, ft],
                )?;
            }
        }
        Ok(path)
    })
}

/// `hard_delete_paper` — permanently delete the root; PAPER/PAPER_META/
/// PAPER_TO_TAG/PAPER_TO_AUTHOR/PROJECT_TO_PAPER cascade off the FK (PRAGMA ON).
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

/// `get_all_versions` — every stored (active) version, oldest-first.
pub fn get_all_versions(conn: &Connection, source_id: &str) -> Result<Vec<PaperDetails>> {
    let mut stmt =
        conn.prepare("SELECT * FROM papers WHERE source_id = ? ORDER BY version ASC")?;
    let mut rows = stmt.query([source_id])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_paper(row)?);
    }
    Ok(out)
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

fn opt_ts(s: Option<String>) -> Result<Option<NaiveDateTime>> {
    match s {
        Some(s) => Ok(Some(timestamp_from_sql(&s)?)),
        None => Ok(None),
    }
}

/// `get_paper_root` — the PAPER_ROOTS row for a source_id, or None.
pub fn get_paper_root(conn: &Connection, source_id: &str) -> Result<Option<PaperRoot>> {
    let raw: Option<(i64, String, String, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT SOURCE_FK, SOURCE_ID, STATUS, DELETED_AT, CREATED_AT, UPDATED_AT \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?",
            [source_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    match raw {
        Some((source_fk, source_id, status, del, cre, upd)) => Ok(Some(PaperRoot {
            source_fk,
            source_id,
            status,
            deleted_at: opt_ts(del)?,
            created_at: opt_ts(cre)?,
            updated_at: opt_ts(upd)?,
        })),
        None => Ok(None),
    }
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
    let mut stmt =
        conn.prepare("SELECT * FROM deleted_papers ORDER BY deleted_at DESC")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let authors = match row.get::<_, Option<String>>("authors")? {
            Some(s) => list_from_sql(&s)?,
            None => Vec::new(),
        };
        let published = match row.get::<_, Option<String>>("published")? {
            Some(s) => Some(date_from_sql(&s)?),
            None => None,
        };
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
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};
    use rusqlite::params;

    fn seed(conn: &Connection) {
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:2204.12985')", [])
            .unwrap();
        let fk = conn.last_insert_rowid();
        for (ver, title, pub_date) in [(1, "V1", "2024-01-01"), (2, "V2", "2024-03-05")] {
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) \
                 VALUES ('arxiv:2204.12985', ?1, ?2, 'cs.LG', 1, ?3)",
                params![ver, title, fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID, URL, PUBLISHED, CATEGORIES, SUMMARY, AUTHORS, TAGS, DOI) \
                 VALUES (?1, 'http://x', ?2, '[\"cs.LG\",\"cs.AI\"]', 'sum', '[\"Alice\",\"Bob\"]', '[\"ml\"]', '10.1/x')",
                params![pid, pub_date],
            )
            .unwrap();
        }
    }

    #[test]
    fn get_paper_latest_and_specific_version() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn);

        // None -> latest version via latest_papers view.
        let latest = get_paper(&conn, "arxiv:2204.12985", None).unwrap().unwrap();
        assert_eq!(latest.version, 2);
        assert_eq!(latest.title, "V2");
        assert_eq!(latest.published, NaiveDate::from_ymd_opt(2024, 3, 5));
        assert_eq!(latest.authors, vec!["Alice".to_string(), "Bob".to_string()]);
        assert_eq!(latest.categories, vec!["cs.LG".to_string(), "cs.AI".to_string()]);
        assert_eq!(latest.tags, vec!["ml".to_string()]);
        assert!(latest.has_pdf);
        assert!(!latest.downloaded_source);
        assert_eq!(latest.source.as_deref(), Some("arxiv")); // PROVIDER default

        // Some(1) -> that exact version via papers view.
        let v1 = get_paper(&conn, "arxiv:2204.12985", Some(1)).unwrap().unwrap();
        assert_eq!(v1.version, 1);
        assert_eq!(v1.title, "V1");

        assert!(get_paper(&conn, "arxiv:nope", None).unwrap().is_none());
    }

    #[test]
    fn list_papers_latest_only_and_category_filter() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn);

        // Default: latest version only -> one row.
        let latest = list_papers(&conn, true, None, 0, None).unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].version, 2);

        // latest_only=false -> both versions.
        let all = list_papers(&conn, false, None, 0, None).unwrap();
        assert_eq!(all.len(), 2);

        // Category filter passes the seeded category, misses on a wrong one.
        assert_eq!(list_papers(&conn, true, None, 0, Some("cs.LG")).unwrap().len(), 1);
        assert_eq!(list_papers(&conn, true, None, 0, Some("nope")).unwrap().len(), 0);

        // limit/offset apply to the (all-versions) filtered result.
        assert_eq!(list_papers(&conn, false, Some(1), 1, None).unwrap().len(), 1);
    }

    // ── Write tests ───────────────────────────────────────────────────────────

    fn meta(source_id: &str, version: i64) -> PaperMetadata {
        PaperMetadata {
            source_id: source_id.into(),
            version,
            title: "T".into(),
            authors: vec!["Alice".into(), "Bob".into()],
            published: NaiveDate::from_ymd_opt(2024, 3, 5).unwrap(),
            updated: None,
            summary: "sum".into(),
            category: Some("cs.LG".into()),
            categories: Some(vec!["cs.LG".into(), "cs.AI".into()]),
            doi: Some("10.1/x".into()),
            journal_ref: None,
            comment: None,
            url: Some("http://x".into()),
            tags: Some(vec!["ml".into()]),
            source: Some("arxiv".into()),
        }
    }

    fn count(conn: &Connection, sql: &str, sid: &str) -> i64 {
        conn.query_row(sql, [sid], |r| r.get(0)).unwrap()
    }

    #[test]
    fn save_paper_metadata_writes_dual_tag_author_storage() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let m = meta("arxiv:2204.12985", 1);
        let (sid, ver) = save_paper_metadata(&mut conn, &m, Some(&["extra".into(), "ml".into()]))
            .unwrap();
        assert_eq!((sid.as_str(), ver), ("arxiv:2204.12985", 1));

        // Re-read via the read path: relational + JSON both populated.
        let p = get_paper(&conn, "arxiv:2204.12985", None).unwrap().unwrap();
        assert_eq!(p.title, "T");
        assert_eq!(p.authors, vec!["Alice".to_string(), "Bob".to_string()]);
        assert_eq!(p.categories, vec!["cs.LG".to_string(), "cs.AI".to_string()]);
        // merge_tags: union, deduped ("ml" not doubled), order base-then-extra.
        assert_eq!(p.tags, vec!["ml".to_string(), "extra".to_string()]);
        assert_eq!(p.published, NaiveDate::from_ymd_opt(2024, 3, 5));
        assert!(!p.has_pdf);

        // Relational half: PAPER_TO_AUTHOR + PAPER_TO_TAG rows really exist.
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM PAPER_TO_AUTHOR pta JOIN PAPER p USING (PAPER_ID) WHERE p.SOURCE_ID = ?", "arxiv:2204.12985"),
            2
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM PAPER_TO_TAG WHERE SOURCE_ID = ?", "arxiv:2204.12985"),
            2 // ml + extra
        );

        // Re-saving the same (source_id, version) is a no-op (INSERT OR IGNORE).
        save_paper_metadata(&mut conn, &m, None).unwrap();
        assert_eq!(get_all_versions(&conn, "arxiv:2204.12985").unwrap().len(), 1);
    }

    #[test]
    fn add_then_remove_paper_tags_syncs_both_halves() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn); // 2 versions, PAPER_META.TAGS = ["ml"], no relational rows yet
        let sid = "arxiv:2204.12985";
        let label_rows = "SELECT COUNT(*) FROM PAPER_TO_TAG pt JOIN TAG t ON pt.TAG_FK = t.TAG_FK \
                          WHERE pt.SOURCE_ID = ? AND t.TAG = ";

        // add: union onto ["ml"], dedup first-seen order, across all versions + both halves.
        let after_add = add_paper_tags(&mut conn, sid, &["nlp".into(), "ml".into()]).unwrap();
        assert_eq!(after_add, vec!["ml".to_string(), "nlp".to_string()]);
        assert_eq!(get_paper(&conn, sid, None).unwrap().unwrap().tags, after_add); // JSON half
        // relational half synced for BOTH versions (2 rows per tag).
        assert_eq!(count(&conn, &format!("{label_rows}'ml'"), sid), 2);
        assert_eq!(count(&conn, &format!("{label_rows}'nlp'"), sid), 2);

        // remove: drop "ml" from both halves; "nlp" survives.
        let after_rm = remove_paper_tags(&mut conn, sid, &["ml".into()]).unwrap();
        assert_eq!(after_rm, vec!["nlp".to_string()]);
        assert_eq!(get_paper(&conn, sid, None).unwrap().unwrap().tags, vec!["nlp".to_string()]);
        assert_eq!(count(&conn, &format!("{label_rows}'ml'"), sid), 0); // relational row gone
        assert_eq!(count(&conn, &format!("{label_rows}'nlp'"), sid), 2);

        // a missing paper errors (no latest version), matching add_paper_tags.
        assert!(remove_paper_tags(&mut conn, "arxiv:nope", &["x".into()]).is_err());
    }

    #[test]
    fn save_then_repair_renames_and_moves_fts() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:OLD", 1), None).unwrap();
        let source_fk = ensure_paper_root(&mut conn, "arxiv:OLD").unwrap();

        // Put full_text into FTS under the old id.
        set_full_text(&mut conn, "arxiv:OLD", 1, Some("hello tex")).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?", "arxiv:OLD"), 1);

        let mut m2 = meta("arxiv:NEW", 1);
        m2.title = "Repaired".into();
        m2.tags = Some(vec!["t1".into(), "t2".into()]);
        m2.authors = vec!["Carol".into()];
        repair_paper(&mut conn, source_fk, &m2).unwrap();

        // SOURCE_ID migrated across PAPER_ROOTS, PAPER, PAPER_TO_TAG.
        assert!(get_paper(&conn, "arxiv:OLD", None).unwrap().is_none());
        let p = get_paper(&conn, "arxiv:NEW", None).unwrap().unwrap();
        assert_eq!(p.title, "Repaired");
        assert_eq!(p.authors, vec!["Carol".to_string()]);
        assert_eq!(p.tags, vec!["t1".to_string(), "t2".to_string()]);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM PAPER_TO_TAG WHERE SOURCE_ID = ?", "arxiv:NEW"),
            2
        );
        // FTS row moved old -> new, none left behind.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?", "arxiv:OLD"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?", "arxiv:NEW"), 1);
        // Old author GC'd (no paper references "Alice"/"Bob" any more).
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM AUTHOR", [], |r| r.get::<_, i64>(0)).unwrap(),
            1
        );
    }

    #[test]
    fn pdf_setters_and_mark_pdf_saved() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:p", 1), None).unwrap();

        set_has_pdf(&conn, "arxiv:p", 1, true).unwrap();
        assert!(get_paper(&conn, "arxiv:p", Some(1)).unwrap().unwrap().has_pdf);

        set_pdf_path(&conn, "arxiv:p", "/tmp/a.pdf", Some(1)).unwrap();
        assert_eq!(
            get_paper(&conn, "arxiv:p", Some(1)).unwrap().unwrap().pdf_path.as_deref(),
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

    #[test]
    fn set_full_text_updates_meta_and_fts() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:ft", 1), None).unwrap();

        set_full_text(&mut conn, "arxiv:ft", 1, Some("the full tex body")).unwrap();
        let p = get_paper(&conn, "arxiv:ft", Some(1)).unwrap().unwrap();
        assert_eq!(p.full_text.as_deref(), Some("the full tex body"));
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
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?", "arxiv:ft"), 1);
    }

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
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?", "arxiv:d"), 0);
        assert_eq!(list_deleted_papers(&conn).unwrap().len(), 1);
        assert_eq!(get_paper_root(&conn, "arxiv:d").unwrap().unwrap().status, "deleted");

        // Restore: active again, FTS rebuilt from stored full_text.
        restore_paper(&mut conn, "arxiv:d").unwrap();
        assert!(!is_paper_deleted(&conn, "arxiv:d").unwrap());
        assert!(get_paper(&conn, "arxiv:d", None).unwrap().is_some());
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?", "arxiv:d"), 1);

        // Hard delete: root gone, children cascade-deleted, FTS gone.
        hard_delete_paper(&mut conn, "arxiv:d").unwrap();
        assert!(get_paper_root(&conn, "arxiv:d").unwrap().is_none());
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM PAPER WHERE SOURCE_ID = ?", "arxiv:d"), 0);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM PAPER_TO_AUTHOR pta WHERE pta.PAPER_ID IN (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)", "arxiv:d"),
            0
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?", "arxiv:d"), 0);
    }

    #[test]
    fn root_helpers_and_versions() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:v", 1), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:v", 2), None).unwrap();

        let fk = ensure_paper_root(&mut conn, "arxiv:v").unwrap();
        assert_eq!(get_source_id(&conn, fk).unwrap().as_deref(), Some("arxiv:v"));
        assert_eq!(get_source_id(&conn, 999_999).unwrap(), None);
        assert_eq!(sfks_to_source_ids(&conn, &[fk, 999_999]).unwrap(), vec!["arxiv:v".to_string()]);

        let versions = get_all_versions(&conn, "arxiv:v").unwrap();
        assert_eq!(versions.iter().map(|p| p.version).collect::<Vec<_>>(), vec![1, 2]);

        // ensure_paper_root reactivates a soft-deleted root.
        soft_delete_paper(&mut conn, "arxiv:v").unwrap();
        assert!(is_paper_deleted(&conn, "arxiv:v").unwrap());
        ensure_paper_root(&mut conn, "arxiv:v").unwrap();
        assert!(!is_paper_deleted(&conn, "arxiv:v").unwrap());
    }
}
