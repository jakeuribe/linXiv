//! Saving paper versions and repairing stored metadata, including the dual
//! tag/author storage sync (JSON columns AND relational join tables).

use chrono::NaiveDate;
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::error::{CoreError, Result};
use crate::models::PaperMetadata;
use crate::storage::db::{date_to_sql, list_from_sql, list_to_sql, transaction};

use super::fts::refresh_fts;
use super::roots::ensure_paper_root_row;

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
    tx.execute(
        "INSERT INTO AUTHOR (AUTHOR_FULL_NAME) VALUES (?)",
        [full_name],
    )?;
    Ok(tx.last_insert_rowid())
}

/// `_sync_paper_authors` — relational half of dual author storage. Replaces the
/// PAPER_TO_AUTHOR rows for this paper, then garbage-collects AUTHOR rows that no
/// paper references any more (ADR-0009: hard-delete leaves orphans, this does not).
/// `author_orcids`, index-aligned with `authors` when present, fills a NULL
/// ORCID only (never overwrites); inherits `author_fk_for_name`'s name-collision ceiling.
fn sync_paper_authors(
    tx: &Transaction,
    paper_id: i64,
    authors: &[String],
    author_orcids: Option<&[Option<String>]>,
) -> Result<()> {
    let old_fks: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT AUTHOR_FK FROM PAPER_TO_AUTHOR WHERE PAPER_ID = ?")?;
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
        if let Some(orcid) = author_orcids
            .and_then(|v| v.get(i))
            .and_then(|o| o.as_deref())
        {
            tx.execute(
                "UPDATE AUTHOR SET AUTHOR_ORCID = ? WHERE AUTHOR_FK = ? AND AUTHOR_ORCID IS NULL",
                params![orcid, aid],
            )?;
        }
    }
    for chunk in old_fks.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        tx.execute(
            &format!(
                "DELETE FROM AUTHOR WHERE AUTHOR_FK IN ({placeholders}) \
                 AND NOT EXISTS (SELECT 1 FROM PAPER_TO_AUTHOR \
                                 WHERE AUTHOR_FK = AUTHOR.AUTHOR_FK)"
            ),
            rusqlite::params_from_iter(chunk.iter()),
        )?;
    }
    Ok(())
}

/// `_sync_paper_tags` — relational half of dual tag storage. Replaces the
/// PAPER_TO_TAG rows (PAPER_ID + composite (SOURCE_ID, VERSION)) for this paper.
pub(super) fn sync_paper_tags(
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
        let tid = super::super::tag::tag_fk_for_label(tx, label)?;
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
/// `extra_tags` are merged into the paper's own tags. tx-level, for callers that
/// need the write to share a transaction with other statements of their own
/// (e.g. version_monitor saving a new version and flagging it as checked as one
/// atomic unit instead of two ordered-but-separate writes).
pub(crate) fn write_paper_version_in_tx(
    tx: &Transaction,
    meta: &PaperMetadata,
    extra_tags: Option<&[String]>,
) -> Result<()> {
    let merged_tags = merge_tags(&meta.tags, extra_tags);
    let source_fk = ensure_paper_root_row(tx, &meta.source_id)?;
    // UPDATED_AT is date('now'), not the column's datetime('now') default: the
    // Python-era post-INSERT UPDATE stored date-only strings, kept for parity.
    let changed = tx.execute(
        "INSERT OR IGNORE INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK, UPDATED_AT) \
         VALUES (?, ?, ?, ?, 0, ?, date('now'))",
        params![
            meta.source_id,
            meta.version,
            meta.title,
            meta.category,
            source_fk
        ],
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
            opt_list_val(&merged_tags),
        ],
    )?;
    sync_paper_authors(tx, paper_id, &meta.authors, meta.author_orcids.as_deref())?;
    sync_paper_tags(
        tx,
        paper_id,
        &meta.source_id,
        meta.version,
        merged_tags.as_deref(),
    )?;
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
    transaction(conn, |tx| write_paper_version_in_tx(tx, meta, extra_tags))?;
    Ok((meta.source_id.clone(), meta.version))
}

/// `save_papers_metadata` — persist many paper versions in ONE transaction
/// (bulk import/search-save paths pay one IMMEDIATE tx per batch instead of one
/// per paper). All-or-nothing: an error rolls back the whole batch. Returns the
/// source_ids in input order (duplicates included; a dup version is a no-op).
pub fn save_papers_metadata(conn: &mut Connection, metas: &[PaperMetadata]) -> Result<Vec<String>> {
    if metas.is_empty() {
        return Ok(Vec::new());
    }
    transaction(conn, |tx| {
        for m in metas {
            write_paper_version_in_tx(tx, m, None)?;
        }
        Ok(metas.iter().map(|m| m.source_id.clone()).collect())
    })
}

/// `db.add_paper_tags` — UNION `tags` onto a paper's existing tags across BOTH
/// halves of dual tag storage: the JSON `PAPER_META.TAGS` list (all versions) and
/// the relational `PAPER_TO_TAG` rows (re-synced per version). Dedup preserves
/// first-seen order (Python `dict.fromkeys`). Returns the merged tag list. Errors
/// if the paper has no latest version.
pub fn add_paper_tags(
    conn: &mut Connection,
    source_id: &str,
    tags: &[String],
) -> Result<Vec<String>> {
    transaction(conn, |tx| {
        let current: Option<Option<String>> = tx
            .query_row(
                "SELECT tags FROM latest_papers WHERE source_id = ?",
                [source_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?;
        let Some(current_json) = current else {
            return Err(CoreError::NotFound(format!(
                "paper {source_id:?} not found"
            )));
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
pub fn remove_paper_tags(
    conn: &mut Connection,
    source_id: &str,
    tags: &[String],
) -> Result<Vec<String>> {
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
            return Err(CoreError::NotFound(format!(
                "paper {source_id:?} not found"
            )));
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
            // The FTS row's key IS the source_id, so a rename is a delete under
            // the old id plus a rebuild under the new one.
            tx.execute("DELETE FROM papers_fts WHERE paper_id = ?", [&old_id])?;
            refresh_fts(tx, new_id)?;
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
        sync_paper_authors(tx, pid, &meta.authors, meta.author_orcids.as_deref())?;
        sync_paper_tags(tx, pid, new_id, ver, meta.tags.as_deref())?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{count, meta, seed};
    use super::super::*;
    use crate::storage::{db::open_in_memory, init_db};
    use chrono::NaiveDate;
    use rusqlite::Connection;

    #[test]
    fn save_paper_metadata_writes_dual_tag_author_storage() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let m = meta("arxiv:2204.12985", 1);
        let (sid, ver) =
            save_paper_metadata(&mut conn, &m, Some(&["extra".into(), "ml".into()])).unwrap();
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
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_TO_TAG WHERE SOURCE_ID = ?",
                "arxiv:2204.12985"
            ),
            2 // ml + extra
        );

        // Re-saving the same (source_id, version) is a no-op (INSERT OR IGNORE).
        save_paper_metadata(&mut conn, &m, None).unwrap();
        assert_eq!(
            get_all_versions(&conn, "arxiv:2204.12985").unwrap().len(),
            1
        );
    }

    #[test]
    fn save_papers_metadata_bulk_saves_all_in_input_order() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Duplicate (source_id, version) inside the batch: id echoed, row not doubled.
        let ids = save_papers_metadata(
            &mut conn,
            &[meta("arxiv:A", 1), meta("arxiv:B", 1), meta("arxiv:A", 1)],
        )
        .unwrap();
        assert_eq!(ids, vec!["arxiv:A", "arxiv:B", "arxiv:A"]);
        assert_eq!(get_all_versions(&conn, "arxiv:A").unwrap().len(), 1);

        // Same stored shape as the per-paper path (dual author storage populated).
        let p = get_paper(&conn, "arxiv:B", None).unwrap().unwrap();
        assert_eq!(p.authors, vec!["Alice".to_string(), "Bob".to_string()]);

        // UPDATED_AT keeps its Python-era date-only precision (now set in the
        // INSERT rather than a follow-up UPDATE).
        let updated_at: String = conn
            .query_row("SELECT UPDATED_AT FROM PAPER LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(updated_at.len(), 10, "date-only, got {updated_at:?}");

        assert!(save_papers_metadata(&mut conn, &[]).unwrap().is_empty());
    }

    #[test]
    fn sync_paper_authors_fills_null_orcid_but_never_overwrites() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let mut m = meta("arxiv:2204.12985", 1);
        m.author_orcids = Some(vec![Some("0000-1".to_string()), None]);
        save_paper_metadata(&mut conn, &m, None).unwrap();

        fn orcid(conn: &Connection, name: &str) -> Option<String> {
            conn.query_row(
                "SELECT AUTHOR_ORCID FROM AUTHOR WHERE AUTHOR_FULL_NAME = ?",
                [name],
                |r| r.get(0),
            )
            .unwrap()
        }
        assert_eq!(orcid(&conn, "Alice").as_deref(), Some("0000-1"));
        assert_eq!(orcid(&conn, "Bob"), None);

        // A second paper reusing the same author names (matched by name, not FK)
        // with a *different* orcid for Alice must not clobber the one already
        // stored, while still filling Bob's still-NULL orcid.
        let mut m2 = meta("arxiv:9999.99999", 1);
        m2.author_orcids = Some(vec![Some("0000-9".to_string()), Some("0000-2".to_string())]);
        save_paper_metadata(&mut conn, &m2, None).unwrap();
        assert_eq!(orcid(&conn, "Alice").as_deref(), Some("0000-1")); // unchanged
        assert_eq!(orcid(&conn, "Bob").as_deref(), Some("0000-2")); // filled, was NULL
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
        assert_eq!(
            get_paper(&conn, sid, None).unwrap().unwrap().tags,
            after_add
        ); // JSON half
           // relational half synced for BOTH versions (2 rows per tag).
        assert_eq!(count(&conn, &format!("{label_rows}'ml'"), sid), 2);
        assert_eq!(count(&conn, &format!("{label_rows}'nlp'"), sid), 2);

        // remove: drop "ml" from both halves; "nlp" survives.
        let after_rm = remove_paper_tags(&mut conn, sid, &["ml".into()]).unwrap();
        assert_eq!(after_rm, vec!["nlp".to_string()]);
        assert_eq!(
            get_paper(&conn, sid, None).unwrap().unwrap().tags,
            vec!["nlp".to_string()]
        );
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
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:OLD"
            ),
            1
        );

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
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_TO_TAG WHERE SOURCE_ID = ?",
                "arxiv:NEW"
            ),
            2
        );
        // FTS row moved old -> new, none left behind.
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:OLD"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = ?",
                "arxiv:NEW"
            ),
            1
        );
        // Old author GC'd (no paper references "Alice"/"Bob" any more).
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM AUTHOR", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
