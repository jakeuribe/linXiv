//! Author reads + writes. Rust port of `storage/authors.py` (+ the get-or-create
//! `_author_fk_for_name` from `storage/db.py`). Plan §5.3.
//!
//! No transaction wrappers here: every write is a single statement (matching the
//! Python, which relies on `with _connect()` autocommit). The get-or-create is a
//! SELECT-then-conditional-INSERT — no partial-inconsistent state to roll back.

use std::collections::HashSet;

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use crate::error::{CoreError, Result};
use crate::models::{AuthorPaperPreview, AuthorWithCount, BasicAuthorDetails};
use crate::storage::db;

// `AUTHOR.AUTHOR_*` columns are all nullable -> every field but the FK is Option.
fn row_to_basic(row: &Row) -> rusqlite::Result<BasicAuthorDetails> {
    Ok(BasicAuthorDetails {
        author_id: row.get("AUTHOR_FK")?,
        orcid: row.get("AUTHOR_ORCID")?,
        full_name: row.get("AUTHOR_FULL_NAME")?,
        first_name: row.get("AUTHOR_FIRST")?,
        last_name: row.get("AUTHOR_LAST")?,
    })
}

// ── reads ─────────────────────────────────────────────────────────────────

/// `authors.py::get_author` — one author by FK, or None.
pub fn get_author(conn: &Connection, author_id: i64) -> Result<Option<BasicAuthorDetails>> {
    Ok(conn
        .query_row(
            "SELECT AUTHOR_FK, AUTHOR_ORCID, AUTHOR_FULL_NAME, AUTHOR_FIRST, AUTHOR_LAST \
             FROM AUTHOR WHERE AUTHOR_FK = ?",
            params![author_id],
            row_to_basic,
        )
        .optional()?)
}

/// `authors.py::list_authors` (non-paper path). `name` Some -> exact match under
/// COLLATE NOCASE; None -> every author ordered by full name.
pub fn get_many(conn: &Connection, name: Option<&str>) -> Result<Vec<BasicAuthorDetails>> {
    let (sql, p): (&str, Vec<Value>) = match name {
        Some(n) => (
            "SELECT AUTHOR_FK, AUTHOR_ORCID, AUTHOR_FULL_NAME, AUTHOR_FIRST, AUTHOR_LAST \
             FROM AUTHOR WHERE AUTHOR_FULL_NAME = ? COLLATE NOCASE",
            vec![Value::Text(n.to_string())],
        ),
        None => (
            "SELECT AUTHOR_FK, AUTHOR_ORCID, AUTHOR_FULL_NAME, AUTHOR_FIRST, AUTHOR_LAST \
             FROM AUTHOR ORDER BY AUTHOR_FULL_NAME",
            vec![],
        ),
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(&p), row_to_basic)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// One author by exact ORCID (binary match, like the Rust `==` scan it replaced).
/// Ties broken by full name to mirror the old find-first-in-`get_many` order.
pub fn get_author_by_orcid(conn: &Connection, orcid: &str) -> Result<Option<BasicAuthorDetails>> {
    Ok(conn
        .query_row(
            "SELECT AUTHOR_FK, AUTHOR_ORCID, AUTHOR_FULL_NAME, AUTHOR_FIRST, AUTHOR_LAST \
             FROM AUTHOR WHERE AUTHOR_ORCID = ? ORDER BY AUTHOR_FULL_NAME LIMIT 1",
            params![orcid],
            row_to_basic,
        )
        .optional()?)
}

/// `authors.py::list_authors(paper_id=...)` via `_LIST_AUTHORS_FROM_PAPER_SQL` —
/// authors of one paper, ordered by their stored AUTHOR_INDEX.
pub fn get_paper_authors(conn: &Connection, paper_id: i64) -> Result<Vec<BasicAuthorDetails>> {
    let mut stmt = conn.prepare(
        "SELECT a.AUTHOR_FK, a.AUTHOR_ORCID, a.AUTHOR_FULL_NAME, a.AUTHOR_FIRST, a.AUTHOR_LAST \
         FROM AUTHOR a \
         JOIN PAPER_TO_AUTHOR pta ON pta.AUTHOR_FK = a.AUTHOR_FK \
         WHERE pta.PAPER_ID = ? ORDER BY pta.AUTHOR_INDEX",
    )?;
    let rows = stmt.query_map(params![paper_id], row_to_basic)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Each paper's author ORCIDs (AUTHOR_INDEX order, so index-aligned with the
/// paper's authors list), keyed by PAPER_ID. The batched sibling of
/// `get_paper_authors` for snapshot builders that would otherwise query once
/// per paper. Papers with no author links are absent from the map. Chunked to
/// stay under SQLite's bound-variable limit.
pub fn paper_author_orcids(
    conn: &Connection,
    paper_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<Option<String>>>> {
    let mut by_paper: std::collections::HashMap<i64, Vec<Option<String>>> =
        std::collections::HashMap::new();
    for chunk in paper_ids.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT pta.PAPER_ID, a.AUTHOR_ORCID \
             FROM AUTHOR a \
             JOIN PAPER_TO_AUTHOR pta ON pta.AUTHOR_FK = a.AUTHOR_FK \
             WHERE pta.PAPER_ID IN ({placeholders}) \
             ORDER BY pta.PAPER_ID, pta.AUTHOR_INDEX"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (pid, orcid) = row?;
            by_paper.entry(pid).or_default().push(orcid);
        }
    }
    Ok(by_paper)
}

/// `authors.py::list_authors_with_paper_count` — authors with their distinct
/// active-paper count (from the `author_paper_counts` view), `>= min_papers`,
/// ordered last-then-first with NULLs last.
pub fn list_with_paper_count(conn: &Connection, min_papers: i64) -> Result<Vec<AuthorWithCount>> {
    let mut stmt = conn.prepare(
        "SELECT a.AUTHOR_FK, a.AUTHOR_FULL_NAME, a.AUTHOR_FIRST, a.AUTHOR_LAST, a.AUTHOR_ORCID, \
                apc.paper_count AS paper_count \
         FROM AUTHOR a \
         JOIN author_paper_counts apc ON apc.author_fk = a.AUTHOR_FK \
         WHERE apc.paper_count >= ? \
         ORDER BY (a.AUTHOR_LAST IS NULL), a.AUTHOR_LAST, \
                  (a.AUTHOR_FIRST IS NULL), a.AUTHOR_FIRST",
    )?;
    let rows = stmt.query_map(params![min_papers], |row| {
        Ok(AuthorWithCount {
            base: row_to_basic(row)?,
            paper_count: row.get("paper_count")?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// `authors.py::get_author_paper_previews` — latest-version active papers linked
/// to an author, resolved via PAPER_ROOTS so a stale PAPER_TO_AUTHOR version does
/// not hide a newer version. Ordered by title with NULLs last.
pub fn get_paper_previews(conn: &Connection, author_id: i64) -> Result<Vec<AuthorPaperPreview>> {
    let mut stmt = conn.prepare(
        "SELECT lp.paper_id, lp.source_id, lp.source_fk, lp.version, lp.title \
         FROM latest_papers lp \
         WHERE lp.source_fk IN ( \
             SELECT DISTINCT p.SOURCE_FK FROM PAPER p \
             JOIN PAPER_TO_AUTHOR pta ON pta.PAPER_ID = p.PAPER_ID \
             WHERE pta.AUTHOR_FK = ? \
         ) \
         ORDER BY (lp.title IS NULL), lp.title",
    )?;
    let rows = stmt.query_map(params![author_id], |row| {
        Ok(AuthorPaperPreview {
            paper_id: row.get("paper_id")?,
            source_id: row.get("source_id")?,
            source_fk: row.get("source_fk")?,
            version: row.get("version")?,
            title: row.get("title")?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// `authors.py::count_author_paper_links` — distinct paper roots linked to this
/// author, regardless of paper status.
pub fn count_paper_links(conn: &Connection, author_id: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(DISTINCT p.SOURCE_FK) \
         FROM PAPER_TO_AUTHOR pta \
         JOIN PAPER p ON p.PAPER_ID = pta.PAPER_ID \
         WHERE pta.AUTHOR_FK = ?",
        params![author_id],
        |r| r.get(0),
    )?)
}

/// Other authors sharing `author_id`'s ORCID — a same-ORCID match is a near-certain
/// duplicate. Empty if the author has no ORCID (NULL never equals NULL in SQL).
pub fn orcid_merge_candidates(
    conn: &Connection,
    author_id: i64,
) -> Result<Vec<BasicAuthorDetails>> {
    let mut stmt = conn.prepare(
        "SELECT b.AUTHOR_FK, b.AUTHOR_ORCID, b.AUTHOR_FULL_NAME, b.AUTHOR_FIRST, b.AUTHOR_LAST \
         FROM AUTHOR b, AUTHOR a \
         WHERE a.AUTHOR_FK = ? AND b.AUTHOR_FK != a.AUTHOR_FK AND b.AUTHOR_ORCID = a.AUTHOR_ORCID",
    )?;
    let rows = stmt.query_map(params![author_id], row_to_basic)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// An ORCID-less author linked to a DOI-bearing paper, for `service::orcid_backfill`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct OrcidCandidate {
    pub author_id: i64,
    pub full_name: String,
    pub doi: String,
}

/// Authors with `AUTHOR_ORCID IS NULL` linked to a DOI-bearing active/latest
/// paper, one row per author, randomly ordered (both author and, for a
/// multi-DOI author, which DOI) so repeated passes don't wedge on one pair.
// ponytail: full scan + RANDOM(), upgrade to a last-attempted-at column if slow.
pub fn orcid_backfill_candidates(conn: &Connection, limit: i64) -> Result<Vec<OrcidCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT a.AUTHOR_FK, a.AUTHOR_FULL_NAME, lp.doi \
         FROM AUTHOR a \
         JOIN PAPER_TO_AUTHOR pta ON pta.AUTHOR_FK = a.AUTHOR_FK \
         JOIN latest_papers lp ON lp.paper_id = pta.PAPER_ID \
         WHERE a.AUTHOR_ORCID IS NULL AND lp.doi IS NOT NULL AND lp.doi != '' \
         ORDER BY RANDOM()",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(OrcidCandidate {
            author_id: r.get(0)?,
            full_name: r.get(1)?,
            doi: r.get(2)?,
        })
    })?;
    // De-dupe to one (randomly-ordered) row per author, then cap at `limit`.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        let cand = row?;
        if seen.insert(cand.author_id) {
            out.push(cand);
            if out.len() as i64 >= limit {
                break;
            }
        }
    }
    Ok(out)
}

/// Set `AUTHOR_ORCID` only if it's currently NULL — never overwrites a
/// manually-set or already-harvested value. Returns whether a row changed.
pub fn fill_orcid_if_null(conn: &Connection, author_id: i64, orcid: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE AUTHOR SET AUTHOR_ORCID = ? WHERE AUTHOR_FK = ? AND AUTHOR_ORCID IS NULL",
        params![orcid, author_id],
    )?;
    Ok(changed > 0)
}

// ── writes ────────────────────────────────────────────────────────────────

/// `authors.py::create_author` — plain INSERT (no dedup; the full-name index is
/// non-unique by design). Returns the new AUTHOR_FK.
pub fn create_author(
    conn: &Connection,
    full_name: &str,
    first_name: Option<&str>,
    last_name: Option<&str>,
    orcid: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO AUTHOR (AUTHOR_FULL_NAME, AUTHOR_FIRST, AUTHOR_LAST, AUTHOR_ORCID) \
         VALUES (?, ?, ?, ?)",
        params![full_name, first_name, last_name, orcid],
    )?;
    Ok(conn.last_insert_rowid())
}

/// `authors.py::update_author` — set only the provided fields. Python uses a
/// truthy check (`if full_name:`), so an empty string is treated as "not given"
/// and skipped; no provided field -> no-op.
pub fn update_author(
    conn: &Connection,
    author_id: i64,
    full_name: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
    orcid: Option<&str>,
) -> Result<()> {
    let mut sets: Vec<&str> = Vec::new();
    let mut p: Vec<Value> = Vec::new();
    for (col, val) in [
        ("AUTHOR_FULL_NAME", full_name),
        ("AUTHOR_FIRST", first_name),
        ("AUTHOR_LAST", last_name),
        ("AUTHOR_ORCID", orcid),
    ] {
        if let Some(v) = val.filter(|s| !s.is_empty()) {
            sets.push(col);
            p.push(Value::Text(v.to_string()));
        }
    }
    if sets.is_empty() {
        return Ok(());
    }
    let assigns = sets
        .iter()
        .map(|c| format!("{c} = ?"))
        .collect::<Vec<_>>()
        .join(", ");
    p.push(Value::Integer(author_id));
    conn.execute(
        &format!("UPDATE AUTHOR SET {assigns} WHERE AUTHOR_FK = ?"),
        params_from_iter(&p),
    )?;
    Ok(())
}

/// `authors.py::delete_author` — delete the AUTHOR row by FK. Fails on the FK
/// constraint if the author is still linked via PAPER_TO_AUTHOR; callers must
/// unlink first (see `unlink_author_from_paper`), so a merge can't silently
/// drop paper links it didn't mean to touch.
pub fn delete_author(conn: &Connection, author_id: i64) -> Result<()> {
    conn.execute("DELETE FROM AUTHOR WHERE AUTHOR_FK = ?", params![author_id])?;
    Ok(())
}

/// `authors.py::link_author_to_paper` — INSERT OR IGNORE the PAPER_TO_AUTHOR row
/// with an optional author_index (ordering within the paper's author list).
pub fn link_author_to_paper(
    conn: &Connection,
    author_fk: i64,
    paper_id: i64,
    author_index: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO PAPER_TO_AUTHOR (PAPER_ID, AUTHOR_FK, AUTHOR_INDEX) \
         VALUES (?, ?, ?)",
        params![paper_id, author_fk, author_index],
    )?;
    Ok(())
}

/// `authors.py::unlink_author_from_paper` — delete the link row for one
/// (author, paper) pair.
pub fn unlink_author_from_paper(conn: &Connection, author_fk: i64, paper_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM PAPER_TO_AUTHOR WHERE AUTHOR_FK = ? AND PAPER_ID = ?",
        params![author_fk, paper_id],
    )?;
    Ok(())
}

/// Merge `dup_ids` into `canonical_id`: resync PAPER_META.AUTHORS on every paper
/// touched by a duplicate, re-point every PAPER_TO_AUTHOR row off a duplicate onto
/// the canonical author, collapse the resulting double-links (no UNIQUE index
/// enforces one link per paper), then delete the duplicate AUTHOR rows that still
/// exist. All in one transaction. `canonical_id` itself is skipped if listed.
/// Returns the subset of `dup_ids` that actually existed and were merged.
pub fn merge_authors(
    conn: &mut Connection,
    canonical_id: i64,
    dup_ids: &[i64],
) -> Result<Vec<i64>> {
    let dups: Vec<i64> = dup_ids
        .iter()
        .copied()
        .filter(|&d| d != canonical_id)
        .collect();
    let placeholders = vec!["?"; dups.len()].join(",");
    db::transaction(conn, |tx| {
        let canonical_row: Option<Option<String>> = tx
            .query_row(
                "SELECT AUTHOR_FULL_NAME FROM AUTHOR WHERE AUTHOR_FK = ?",
                params![canonical_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(canonical_name) = canonical_row else {
            return Err(CoreError::NotFound(format!(
                "author {canonical_id} not found"
            )));
        };
        if dups.is_empty() {
            return Ok(Vec::new());
        }
        let dup_names: Vec<String> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT AUTHOR_FULL_NAME FROM AUTHOR WHERE AUTHOR_FK IN ({placeholders})"
            ))?;
            let rows = stmt.query_map(params_from_iter(dups.iter().copied()), |r| {
                r.get::<_, Option<String>>(0)
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect()
        };
        let affected_papers: Vec<i64> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT DISTINCT PAPER_ID FROM PAPER_TO_AUTHOR WHERE AUTHOR_FK IN ({placeholders})"
            ))?;
            let rows = stmt.query_map(params_from_iter(dups.iter().copied()), |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let dup_names_lower: HashSet<String> = dup_names.iter().map(|d| d.to_lowercase()).collect();
        let canonical_lower = canonical_name.as_deref().map(str::to_lowercase);
        let mut select_meta = tx.prepare("SELECT AUTHORS FROM PAPER_META WHERE PAPER_ID = ?")?;
        let mut update_meta = tx.prepare("UPDATE PAPER_META SET AUTHORS = ? WHERE PAPER_ID = ?")?;
        for pid in &affected_papers {
            let authors_json: Option<String> = select_meta
                .query_row(params![pid], |r| r.get(0))
                .optional()?
                .flatten();
            let Some(authors_json) = authors_json else {
                continue;
            };
            // Replace duplicate names with the canonical name, or drop them if the
            // canonical author has no name; then de-dupe case-insensitively.
            let mut merged: Vec<String> = Vec::new();
            let mut merged_lower: HashSet<String> = HashSet::new();
            for name in db::list_from_sql(&authors_json)? {
                let name_lower = name.to_lowercase();
                let (resolved, resolved_lower) = if dup_names_lower.contains(&name_lower) {
                    let (Some(cn), Some(cl)) = (&canonical_name, &canonical_lower) else {
                        continue;
                    };
                    (cn.clone(), cl.clone())
                } else {
                    (name, name_lower)
                };
                if merged_lower.insert(resolved_lower) {
                    merged.push(resolved);
                }
            }
            update_meta.execute(params![db::list_to_sql(&merged), pid])?;
        }
        tx.execute(
            &format!(
                "UPDATE PAPER_TO_AUTHOR SET AUTHOR_FK = ? WHERE AUTHOR_FK IN ({placeholders})"
            ),
            params_from_iter(std::iter::once(canonical_id).chain(dups.iter().copied())),
        )?;
        // Drop rows that now double-link a paper to the canonical author, keeping
        // the lowest PTA_FK per paper.
        tx.execute(
            "DELETE FROM PAPER_TO_AUTHOR WHERE AUTHOR_FK = ? AND PTA_FK NOT IN (\
                 SELECT MIN(PTA_FK) FROM PAPER_TO_AUTHOR WHERE AUTHOR_FK = ? GROUP BY PAPER_ID)",
            params![canonical_id, canonical_id],
        )?;
        let existing_dups: Vec<i64> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT AUTHOR_FK FROM AUTHOR WHERE AUTHOR_FK IN ({placeholders})"
            ))?;
            let rows = stmt.query_map(params_from_iter(dups.iter().copied()), |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.execute(
            &format!("DELETE FROM AUTHOR WHERE AUTHOR_FK IN ({placeholders})"),
            params_from_iter(dups.iter().copied()),
        )?;
        Ok(existing_dups)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};

    // Seeds one active paper root with two authors linked, returns (paper_id, a1, a2).
    fn seed(conn: &Connection) -> (i64, i64, i64) {
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1')", [])
            .unwrap();
        let fk = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('arxiv:1', 1, 'T1', ?)",
            params![fk],
        )
        .unwrap();
        let pid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?, '2024-01-01')",
            params![pid],
        )
        .unwrap();
        let a1 = create_author(conn, "Bob Stone", Some("Bob"), Some("Stone"), None).unwrap();
        let a2 = create_author(
            conn,
            "Alice Cole",
            Some("Alice"),
            Some("Cole"),
            Some("0000-1"),
        )
        .unwrap();
        link_author_to_paper(conn, a1, pid, Some(0)).unwrap();
        link_author_to_paper(conn, a2, pid, Some(1)).unwrap();
        (pid, a1, a2)
    }

    #[test]
    fn create_get_and_update() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let id = create_author(&conn, "Jane Doe", None, None, None).unwrap();
        assert!(id > 0);

        let a = get_author(&conn, id).unwrap().unwrap();
        assert_eq!(a.full_name.as_deref(), Some("Jane Doe"));
        assert_eq!(a.first_name, None);

        // empty string skipped; only orcid + last applied.
        update_author(&conn, id, Some(""), None, Some("Doe"), Some("0000-2")).unwrap();
        let a = get_author(&conn, id).unwrap().unwrap();
        assert_eq!(a.full_name.as_deref(), Some("Jane Doe")); // unchanged
        assert_eq!(a.last_name.as_deref(), Some("Doe"));
        assert_eq!(a.orcid.as_deref(), Some("0000-2"));

        // no-op update leaves the row intact.
        update_author(&conn, id, None, None, None, None).unwrap();
        assert_eq!(
            get_author(&conn, id).unwrap().unwrap().last_name.as_deref(),
            Some("Doe")
        );

        assert!(get_author(&conn, 9999).unwrap().is_none());
    }

    #[test]
    fn get_many_by_name_and_all() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn);
        // all, ordered by full name -> Alice before Bob.
        let all = get_many(&conn, None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].full_name.as_deref(), Some("Alice Cole"));
        // exact match under NOCASE.
        let hit = get_many(&conn, Some("bob stone")).unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].full_name.as_deref(), Some("Bob Stone"));
        assert!(get_many(&conn, Some("nobody")).unwrap().is_empty());
    }

    #[test]
    fn paper_authors_previews_and_counts() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (pid, a1, a2) = seed(&conn);

        // get_paper_authors ordered by AUTHOR_INDEX -> Bob(0) then Alice(1).
        let pa = get_paper_authors(&conn, pid).unwrap();
        assert_eq!(pa.len(), 2);
        assert_eq!(pa[0].full_name.as_deref(), Some("Bob Stone"));
        assert_eq!(pa[1].full_name.as_deref(), Some("Alice Cole"));

        // paper_author_orcids: same AUTHOR_INDEX order, keyed by paper; a paper
        // with no author links is absent, an unknown id is ignored.
        let orcids = paper_author_orcids(&conn, &[pid, 9_999]).unwrap();
        assert_eq!(orcids.len(), 1);
        assert_eq!(orcids[&pid], vec![None, Some("0000-1".to_string())]);
        assert!(paper_author_orcids(&conn, &[]).unwrap().is_empty());

        // previews: each author sees the one active latest paper.
        let prev = get_paper_previews(&conn, a1).unwrap();
        assert_eq!(prev.len(), 1);
        assert_eq!(prev[0].source_id, "arxiv:1");
        assert_eq!(prev[0].version, 1);
        assert_eq!(prev[0].title.as_deref(), Some("T1"));

        // counts.
        assert_eq!(count_paper_links(&conn, a1).unwrap(), 1);
        assert_eq!(count_paper_links(&conn, 9999).unwrap(), 0);

        // list_with_paper_count: both have 1 active paper; ordered by last name.
        let withc = list_with_paper_count(&conn, 1).unwrap();
        assert_eq!(withc.len(), 2);
        assert_eq!(withc[0].base.last_name.as_deref(), Some("Cole")); // Cole < Stone
        assert_eq!(withc[0].paper_count, 1);
        // min_papers above the max drops everyone.
        assert!(list_with_paper_count(&conn, 2).unwrap().is_empty());

        let _ = a2;
    }

    #[test]
    fn unlink_and_delete() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (pid, a1, _a2) = seed(&conn);

        unlink_author_from_paper(&conn, a1, pid).unwrap();
        assert_eq!(count_paper_links(&conn, a1).unwrap(), 0);
        assert_eq!(get_paper_authors(&conn, pid).unwrap().len(), 1); // only Alice left

        delete_author(&conn, a1).unwrap();
        assert!(get_author(&conn, a1).unwrap().is_none());
    }

    #[test]
    fn delete_author_still_linked_is_an_error() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (_pid, a1, _a2) = seed(&conn); // a1 still linked via PAPER_TO_AUTHOR

        assert!(delete_author(&conn, a1).is_err());
        assert!(get_author(&conn, a1).unwrap().is_some()); // row survives the failed delete
    }

    #[test]
    fn merge_resyncs_paper_meta_authors_case_insensitively() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (pid, a1, a2) = seed(&conn); // a1 = Bob Stone, a2 = Alice Cole
        conn.execute(
            "UPDATE PAPER_META SET AUTHORS = ? WHERE PAPER_ID = ?",
            params![
                db::list_to_sql(&[
                    "Bob Stone".to_string(),
                    "alice cole".to_string(), // case variant of the duplicate
                    "Someone Else".to_string(),
                ]),
                pid
            ],
        )
        .unwrap();

        merge_authors(&mut conn, a1, &[a2]).unwrap();

        let authors_json: String = conn
            .query_row(
                "SELECT AUTHORS FROM PAPER_META WHERE PAPER_ID = ?",
                params![pid],
                |r| r.get(0),
            )
            .unwrap();
        let authors = db::list_from_sql(&authors_json).unwrap();
        assert_eq!(
            authors,
            vec!["Bob Stone".to_string(), "Someone Else".to_string()]
        );
    }

    #[test]
    fn merge_errors_when_canonical_author_missing() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (_pid, _a1, a2) = seed(&conn);
        assert!(merge_authors(&mut conn, 9999, &[a2]).is_err());
    }

    #[test]
    fn orcid_merge_candidates_matches_same_orcid_only() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (_pid, bob, alice) = seed(&conn); // bob: no orcid, alice: "0000-1"
        let twin = create_author(&conn, "A. Cole", None, None, Some("0000-1")).unwrap();

        // Alice's twin shares her ORCID -> candidate; Bob has none -> no candidates.
        let candidates = orcid_merge_candidates(&conn, alice).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].author_id, twin);
        assert!(orcid_merge_candidates(&conn, bob).unwrap().is_empty());
    }

    #[test]
    fn orcid_backfill_candidates_and_fill_if_null() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1')", [])
            .unwrap();
        let fk = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('arxiv:1', 1, 'T1', ?)",
            params![fk],
        )
        .unwrap();
        let pid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED, DOI) VALUES (?, '2024-01-01', '10.1/x')",
            params![pid],
        )
        .unwrap();

        let no_orcid = create_author(&conn, "Bob Stone", None, None, None).unwrap();
        let has_orcid = create_author(&conn, "Alice Cole", None, None, Some("0000-1")).unwrap();
        let unlinked = create_author(&conn, "Ghost Author", None, None, None).unwrap();
        link_author_to_paper(&conn, no_orcid, pid, Some(0)).unwrap();
        link_author_to_paper(&conn, has_orcid, pid, Some(1)).unwrap();

        // Only the ORCID-less, DOI-linked author is a candidate.
        let candidates = orcid_backfill_candidates(&conn, 10).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].author_id, no_orcid);
        assert_eq!(candidates[0].full_name, "Bob Stone");
        assert_eq!(candidates[0].doi, "10.1/x");
        let _ = unlinked;

        assert!(fill_orcid_if_null(&conn, no_orcid, "0000-9").unwrap());
        assert_eq!(
            get_author(&conn, no_orcid)
                .unwrap()
                .unwrap()
                .orcid
                .as_deref(),
            Some("0000-9")
        );
        // Already-filled orcid is never overwritten.
        assert!(!fill_orcid_if_null(&conn, no_orcid, "0000-8").unwrap());
        assert_eq!(
            get_author(&conn, no_orcid)
                .unwrap()
                .unwrap()
                .orcid
                .as_deref(),
            Some("0000-9")
        );
    }

    #[test]
    fn orcid_backfill_candidates_dedupes_multi_doi_author_to_one_row() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let paper = |sid: &str, doi: &str| -> i64 {
            conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)", [sid])
                .unwrap();
            let fk = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES (?, 1, 'T', ?)",
                params![sid, fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED, DOI) VALUES (?, '2024-01-01', ?)",
                params![pid, doi],
            )
            .unwrap();
            pid
        };
        let pid_a = paper("arxiv:a", "10.1/a");
        let pid_b = paper("arxiv:b", "10.1/b");

        let author = create_author(&conn, "Bob Stone", None, None, None).unwrap();
        link_author_to_paper(&conn, author, pid_a, Some(0)).unwrap();
        link_author_to_paper(&conn, author, pid_b, Some(0)).unwrap();

        // Two DOI-bearing papers for the same author -> exactly one candidate
        // row, carrying one of the two DOIs (never both, never neither).
        let candidates = orcid_backfill_candidates(&conn, 10).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].author_id, author);
        assert!(["10.1/a", "10.1/b"].contains(&candidates[0].doi.as_str()));
    }

    #[test]
    fn orcid_backfill_candidates_respects_limit() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        for i in 0..5 {
            let sid = format!("arxiv:{i}");
            conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)", [&sid])
                .unwrap();
            let fk = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES (?, 1, 'T', ?)",
                params![sid, fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED, DOI) VALUES (?, '2024-01-01', ?)",
                params![pid, format!("10.1/{i}")],
            )
            .unwrap();
            let author = create_author(&conn, &format!("Author {i}"), None, None, None).unwrap();
            link_author_to_paper(&conn, author, pid, Some(0)).unwrap();
        }
        assert_eq!(orcid_backfill_candidates(&conn, 3).unwrap().len(), 3);
        assert_eq!(orcid_backfill_candidates(&conn, 100).unwrap().len(), 5);
    }
}
