//! Merge one paper root into another — the DB half of the paper/PDF dedupe.
//!
//! [`merge_plan`] classifies the loser's versions read-only; the service does
//! the (reversible) PDF renames; [`merge_paper_roots`] is the one transaction
//! that re-points every dependent row and deletes the loser root. Duplicate
//! files are unlinked by the service only after commit.
//!
//! The winner's metadata is canonical — never field-merged. Children, tags,
//! missing versions and PDFs move; use Paper Repair afterwards for metadata.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::storage::db::{list_from_sql, list_to_sql, transaction};

use super::fts::refresh_fts;
use super::write::sync_paper_tags_for_versions;

/// What happens to one loser version, decided by [`merge_plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionAction {
    /// Winner has no row with this VERSION: the loser PAPER row is re-keyed to
    /// the winner (same VERSION), and its PDF file (if any) renamed with it.
    Transplant {
        version: i64,
        loser_pdf_path: Option<String>,
    },
    /// Winner's same-numbered version exists but has no PDF while the loser's
    /// does: the winner row adopts the loser's file; the loser row collapses.
    AdoptPdf {
        version: i64,
        loser_pdf_path: String,
    },
    /// Winner's same-numbered version wins outright; the loser row collapses
    /// and its PDF (if any) is a duplicate to delete post-commit.
    Collapse {
        version: i64,
        duplicate_pdf_path: Option<String>,
    },
}

/// Read-only merge plan: identities plus the per-version classification the
/// service turns into filesystem renames.
#[derive(Debug, Clone)]
pub struct MergePlan {
    pub winner_fk: i64,
    pub winner_id: String,
    pub loser_fk: i64,
    pub loser_id: String,
    pub actions: Vec<VersionAction>,
    /// Both roots' (VERSION, stored PDF_PATH) at plan time (ascending);
    /// re-checked in the transaction so post-plan drift — a version appearing
    /// or a PDF attached concurrently — becomes a Conflict, not a surprise.
    pub winner_snapshot: Vec<(i64, Option<String>)>,
    pub loser_snapshot: Vec<(i64, Option<String>)>,
}

/// Row counts of what the merge transaction actually moved — the DB half of
/// the receipt (the service adds the file-op counts).
#[derive(Debug, Clone, Default, Serialize)]
pub struct MergeStats {
    pub notes_moved: usize,
    pub annotations_moved: usize,
    pub memberships_moved: usize,
    pub memberships_collapsed: usize,
    pub reading_statuses_moved: usize,
    pub versions_transplanted: usize,
    pub versions_collapsed: usize,
    pub tags_added: Vec<String>,
}

/// A root's (fk, id, status), or a typed miss.
fn require_root(conn: &Connection, source_fk: i64) -> Result<(String, String)> {
    conn.query_row(
        "SELECT SOURCE_ID, STATUS FROM PAPER_ROOTS WHERE SOURCE_FK = ?",
        [source_fk],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| CoreError::PaperNotFound(source_fk.to_string()))
}

/// Non-empty stored PDF path for a version, if any. Treats NULL and `''` alike:
/// both mean "no PDF" everywhere else in the schema.
fn stored_pdf(path: Option<String>) -> Option<String> {
    path.filter(|p| !p.is_empty())
}

/// Share docs key papers by source_id (CRDT), so a merged-away id in a peer's
/// doc would resurrect via the next sync import — refuse instead. Runs at plan
/// time and again in the transaction (the import lock is in-process only).
fn ensure_loser_not_shared(conn: &Connection, loser_fk: i64, loser_id: &str) -> Result<()> {
    // Any status: a trashed project keeps its SHARE_ID and can be restored,
    // so its peers' docs still carry the member list.
    let loser_shared: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM PROJECT_TO_PAPER ptp \
         JOIN PROJECT pr ON pr.PROJECT_FK = ptp.PROJECT_FK \
         WHERE ptp.SOURCE_FK = ? AND pr.SHARE_ID IS NOT NULL AND pr.SHARE_ID != '')",
        [loser_fk],
        |r| r.get(0),
    )?;
    if loser_shared {
        return Err(CoreError::Conflict(format!(
            "Cannot merge {loser_id:?} away: it belongs to a shared project, and peers' \
             sync docs would re-create it. Remove it from the shared project first."
        )));
    }
    Ok(())
}

/// A root's (VERSION, PDF_PATH) rows, ascending — the drift check's unit.
fn version_snapshot(conn: &Connection, source_fk: i64) -> Result<Vec<(i64, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT p.VERSION, m.PDF_PATH FROM PAPER p \
         LEFT JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
         WHERE p.SOURCE_FK = ? ORDER BY p.VERSION ASC",
    )?;
    let rows = stmt.query_map([source_fk], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Classify every loser version against the winner's version set. Read-only.
///
/// Guards: both roots must exist and be `active`, and be distinct. A trashed
/// root can't be merged in either direction — restore it first; merging is a
/// deliberate act on live papers, not a trash operation.
///
/// `pdf_exists` is an injected file check (storage stays FS-free): whether the
/// winner "has a PDF" must be judged by a real file, not the PDF_PATH string —
/// a ghost pointer must lose to a real loser file, not delete it as a dup.
pub fn merge_plan(
    conn: &Connection,
    winner_fk: i64,
    loser_fk: i64,
    pdf_exists: impl Fn(&str) -> bool,
) -> Result<MergePlan> {
    if winner_fk == loser_fk {
        return Err(CoreError::Conflict(
            "Cannot merge a paper into itself.".into(),
        ));
    }
    let (winner_id, w_status) = require_root(conn, winner_fk)?;
    let (loser_id, l_status) = require_root(conn, loser_fk)?;
    for (label, status) in [("winner", w_status.as_str()), ("loser", l_status.as_str())] {
        if status != "active" {
            return Err(CoreError::Conflict(format!(
                "Cannot merge: the {label} paper is in the trash (status {status:?}); restore it first."
            )));
        }
    }

    ensure_loser_not_shared(conn, loser_fk, &loser_id)?;

    // (version -> pdf_path) for both roots; classifies collisions and is the
    // drift baseline the transaction re-checks.
    let winner_versions = version_snapshot(conn, winner_fk)?;
    let loser_versions = version_snapshot(conn, loser_fk)?;

    let actions = loser_versions
        .clone()
        .into_iter()
        .map(|(version, l_path)| {
            let l_path = stored_pdf(l_path);
            match winner_versions.iter().find(|(wv, _)| *wv == version) {
                None => VersionAction::Transplant {
                    version,
                    loser_pdf_path: l_path,
                },
                Some((_, w_path)) => {
                    // File-backed, not string-backed (see fn docs).
                    let winner_pdf = stored_pdf(w_path.clone()).filter(|p| pdf_exists(p.as_str()));
                    match (winner_pdf, l_path) {
                        (None, Some(p)) => VersionAction::AdoptPdf {
                            version,
                            loser_pdf_path: p,
                        },
                        (_, l) => VersionAction::Collapse {
                            version,
                            duplicate_pdf_path: l,
                        },
                    }
                }
            }
        })
        .collect();

    Ok(MergePlan {
        winner_fk,
        winner_id,
        loser_fk,
        loser_id,
        actions,
        winner_snapshot: winner_versions,
        loser_snapshot: loser_versions,
    })
}

/// The merge transaction. `version_pdf_paths` carries, per version the service
/// renamed a file for (transplants and adoptions), the NEW absolute path; a
/// version absent from the slice means its file was missing on disk, so the
/// row's PDF columns are cleared instead of pointing at a ghost.
///
/// Everything below runs in ONE deferred-FK transaction, ordered so parent-key
/// moves land before the commit-time check (same technique as `repair_paper`):
///
/// 1. Version-pinned notes on collapsing loser rows → the winner's
///    same-numbered PAPER_ID (beats `ON DELETE SET NULL` unpinning).
/// 2. NOTE / ANNOTATION root re-point.
/// 3. Reading status: statuses in overlap projects move only where the winner
///    has none (winner's wins); leftovers die with the loser membership row.
/// 4. Memberships: overlap rows deleted (cascade may only reach loser-keyed
///    status rows, all handled in 3), disjoint rows re-keyed — child status
///    rows first, then the parent membership (composite FK, deferred).
/// 5. Transplants: PAPER re-keyed to the winner (SOURCE_ID + SOURCE_FK, same
///    VERSION), PDF columns set from the rename outcome.
/// 6. Adoptions: winner rows take the renamed file (PDF_PATH + HAS_PDF).
/// 7. Tags: union of both roots' latest tag lists written across every
///    surviving version (JSON half), then relational rows re-synced per
///    version — transplanted rows pick up the winner SOURCE_ID here.
/// 8. FTS: loser row dropped, winner rebuilt (now includes transplanted text).
/// 9. Loser root deleted — collapsing PAPER rows and their PAPER_META /
///    PAPER_TO_AUTHOR / PAPER_TO_TAG go by cascade.
pub fn merge_paper_roots(
    conn: &mut Connection,
    plan: &MergePlan,
    version_pdf_paths: &[(i64, String)],
) -> Result<MergeStats> {
    let new_path = |version: i64| -> Option<&str> {
        version_pdf_paths
            .iter()
            .find(|(v, _)| *v == version)
            .map(|(_, p)| p.as_str())
    };
    transaction(conn, |tx| {
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let mut stats = MergeStats::default();
        let (w, l) = (plan.winner_fk, plan.loser_fk);

        // Re-check every plan-time premise: the plan ran outside this
        // transaction, and the import lock doesn't cover other processes.
        for (fk, id) in [(w, &plan.winner_id), (l, &plan.loser_id)] {
            let (cur_id, status) = require_root(tx, fk)?;
            if cur_id != *id || status != "active" {
                return Err(CoreError::Conflict(format!(
                    "Paper {cur_id:?} changed while the merge was being prepared; retry."
                )));
            }
        }
        ensure_loser_not_shared(tx, l, &plan.loser_id)?;
        if version_snapshot(tx, l)? != plan.loser_snapshot
            || version_snapshot(tx, w)? != plan.winner_snapshot
        {
            return Err(CoreError::Conflict(format!(
                "Paper {:?} or {:?} gained or lost versions (or a PDF was attached) while \
                 the merge was being prepared; retry.",
                plan.winner_id, plan.loser_id
            )));
        }

        // 1. Re-pin notes pinned to collapsing loser versions.
        for action in &plan.actions {
            let (VersionAction::AdoptPdf { version, .. } | VersionAction::Collapse { version, .. }) =
                action
            else {
                continue;
            };
            let ids: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT lp.PAPER_ID, wp.PAPER_ID FROM PAPER lp, PAPER wp \
                     WHERE lp.SOURCE_FK = ?1 AND wp.SOURCE_FK = ?2 \
                       AND lp.VERSION = ?3 AND wp.VERSION = ?3",
                    params![l, w, version],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((loser_pid, winner_pid)) = ids {
                tx.execute(
                    "UPDATE NOTE SET PAPER_ID_FK = ? WHERE PAPER_ID_FK = ?",
                    params![winner_pid, loser_pid],
                )?;
            }
        }

        // 2. Root-level children.
        stats.notes_moved =
            tx.execute("UPDATE NOTE SET SOURCE_FK = ? WHERE SOURCE_FK = ?", [w, l])?;
        stats.annotations_moved = tx.execute(
            "UPDATE ANNOTATION SET SOURCE_FK = ? WHERE SOURCE_FK = ?",
            [w, l],
        )?;

        // 3 + 4. Memberships and reading status.
        stats.reading_statuses_moved = tx.execute(
            // Re-key only; the status is unchanged, so UPDATED_AT stays.
            "UPDATE PAPER_TO_READING SET SOURCE_FK = ?1 \
             WHERE SOURCE_FK = ?2 \
               AND PROJECT_FK IN (SELECT PROJECT_FK FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?1) \
               AND PROJECT_FK NOT IN (SELECT PROJECT_FK FROM PAPER_TO_READING WHERE SOURCE_FK = ?1)",
            [w, l],
        )?;
        stats.memberships_collapsed = tx.execute(
            "DELETE FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?2 \
             AND PROJECT_FK IN (SELECT PROJECT_FK FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?1)",
            [w, l],
        )?;
        // Disjoint projects: status child first, then the membership parent.
        stats.reading_statuses_moved += tx.execute(
            "UPDATE PAPER_TO_READING SET SOURCE_FK = ?1 WHERE SOURCE_FK = ?2",
            [w, l],
        )?;
        stats.memberships_moved = tx.execute(
            "UPDATE PROJECT_TO_PAPER SET SOURCE_FK = ?1, UPDATED_AT = datetime('now') \
             WHERE SOURCE_FK = ?2",
            [w, l],
        )?;

        // Tag lists must be read BEFORE the loser loses its rows.
        let read_tags = |fk: i64| -> Result<Vec<String>> {
            let json: Option<Option<String>> = tx
                .query_row(
                    "SELECT m.TAGS FROM PAPER p JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
                     WHERE p.SOURCE_FK = ? ORDER BY p.VERSION DESC LIMIT 1",
                    [fk],
                    |r| r.get(0),
                )
                .optional()?;
            match json.flatten() {
                Some(s) => list_from_sql(&s),
                None => Ok(Vec::new()),
            }
        };
        // Case-insensitive dedup: TAG.TAG is COLLATE NOCASE, so "ML" and "ml"
        // resolve to one TAG_FK — treating them as distinct would write
        // duplicate PAPER_TO_TAG rows for the same logical tag.
        let mut merged_tags = read_tags(w)?;
        for t in read_tags(l)? {
            if !merged_tags.iter().any(|m| m.eq_ignore_ascii_case(&t)) {
                stats.tags_added.push(t.clone());
                merged_tags.push(t);
            }
        }

        // 5. Transplants.
        for action in &plan.actions {
            let VersionAction::Transplant { version, .. } = action else {
                continue;
            };
            tx.execute(
                "UPDATE PAPER SET SOURCE_ID = ?1, SOURCE_FK = ?2, UPDATED_AT = datetime('now') \
                 WHERE SOURCE_FK = ?3 AND VERSION = ?4",
                params![plan.winner_id, w, l, version],
            )?;
            match new_path(*version) {
                Some(p) => tx.execute(
                    "UPDATE PAPER_META SET PDF_PATH = ?1 WHERE PAPER_ID IN \
                     (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ?2 AND VERSION = ?3)",
                    params![p, w, version],
                )?,
                // File was missing on disk: clear the stale pointer.
                None => {
                    tx.execute(
                        "UPDATE PAPER_META SET PDF_PATH = NULL WHERE PAPER_ID IN \
                         (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ?1 AND VERSION = ?2)",
                        params![w, version],
                    )?;
                    tx.execute(
                        "UPDATE PAPER SET HAS_PDF = 0 WHERE SOURCE_FK = ?1 AND VERSION = ?2",
                        params![w, version],
                    )?
                }
            };
            stats.versions_transplanted += 1;
        }

        // 6. Adoptions. No rename for this version means the loser's file was
        // gone too — clear the winner's (ghost or NULL) pointer, mirroring the
        // Transplant branch, instead of leaving a stale path behind.
        for action in &plan.actions {
            let VersionAction::AdoptPdf { version, .. } = action else {
                continue;
            };
            match new_path(*version) {
                Some(p) => {
                    tx.execute(
                        "UPDATE PAPER_META SET PDF_PATH = ?1 WHERE PAPER_ID IN \
                         (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ?2 AND VERSION = ?3)",
                        params![p, w, version],
                    )?;
                    tx.execute(
                        "UPDATE PAPER SET HAS_PDF = 1, UPDATED_AT = datetime('now') \
                         WHERE SOURCE_FK = ?1 AND VERSION = ?2",
                        params![w, version],
                    )?;
                }
                None => {
                    tx.execute(
                        "UPDATE PAPER_META SET PDF_PATH = NULL WHERE PAPER_ID IN \
                         (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ?1 AND VERSION = ?2)",
                        params![w, version],
                    )?;
                    tx.execute(
                        "UPDATE PAPER SET HAS_PDF = 0 WHERE SOURCE_FK = ?1 AND VERSION = ?2",
                        params![w, version],
                    )?;
                }
            }
        }
        stats.versions_collapsed = plan
            .actions
            .iter()
            .filter(|a| !matches!(a, VersionAction::Transplant { .. }))
            .count();

        // 7. Tag union across every surviving version (both halves of dual
        // tag storage; transplanted rows get the winner SOURCE_ID re-synced).
        tx.execute(
            "UPDATE PAPER_META SET TAGS = ?1 WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ?2)",
            params![list_to_sql(&merged_tags), w],
        )?;
        let winner_rows: Vec<(i64, i64)> = {
            let mut stmt = tx.prepare("SELECT PAPER_ID, VERSION FROM PAPER WHERE SOURCE_FK = ?")?;
            let rows = stmt.query_map([w], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        sync_paper_tags_for_versions(tx, &winner_rows, &plan.winner_id, Some(&merged_tags))?;

        // VERSION_CHECK: the loser's row dies with its root (cascade), but the
        // winner's NEW_VERSION was computed against a version series the
        // transplants may just have changed — drop it and let the next check
        // re-derive rather than surface a stale "new version" pill.
        tx.execute("DELETE FROM VERSION_CHECK WHERE SOURCE_FK = ?", [w])?;

        // SEARCH_STATE saved-ids (the Search page's "already saved" checkmarks)
        // durably hold source_id strings with no FK: rewrite the loser's id to
        // the winner's so the mark survives the merge instead of dangling.
        let saved: Option<String> = tx
            .query_row(
                "SELECT SAVED_IDS_JSON FROM SEARCH_STATE WHERE ID = 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(json) = saved {
            if let Ok(serde_json::Value::Array(ids)) = serde_json::from_str(&json) {
                let mut out: Vec<serde_json::Value> = Vec::with_capacity(ids.len());
                for v in ids {
                    let mapped = match v.as_str() {
                        Some(s) if s == plan.loser_id => {
                            serde_json::Value::String(plan.winner_id.clone())
                        }
                        _ => v,
                    };
                    if !out.contains(&mapped) {
                        out.push(mapped);
                    }
                }
                tx.execute(
                    "UPDATE SEARCH_STATE SET SAVED_IDS_JSON = ? WHERE ID = 1",
                    [serde_json::Value::Array(out).to_string()],
                )?;
            }
        }

        // 8 + 9. FTS, then the loser root (cascade collapses its leftover rows).
        tx.execute(
            "DELETE FROM papers_fts WHERE paper_id = ?",
            [&plan.loser_id],
        )?;
        tx.execute("DELETE FROM PAPER_ROOTS WHERE SOURCE_FK = ?", [l])?;
        refresh_fts(tx, &plan.winner_id)?;
        tx.execute(
            "UPDATE PAPER_ROOTS SET UPDATED_AT = datetime('now') WHERE SOURCE_FK = ?",
            [w],
        )?;
        Ok(stats)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};
    use rusqlite::params;

    fn db() -> Connection {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    fn root(conn: &Connection, sid: &str) -> i64 {
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)", [sid])
            .unwrap();
        conn.last_insert_rowid()
    }

    /// A PAPER + PAPER_META pair; `pdf` sets both PDF_PATH and HAS_PDF.
    fn version(conn: &Connection, fk: i64, sid: &str, v: i64, pdf: Option<&str>) -> i64 {
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, HAS_PDF, SOURCE_FK) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![sid, v, format!("{sid} v{v}"), pdf.is_some(), fk],
        )
        .unwrap();
        let pid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PDF_PATH, AUTHORS, TAGS) \
             VALUES (?1, ?2, '[\"Alice\"]', NULL)",
            params![pid, pdf],
        )
        .unwrap();
        pid
    }

    fn project(conn: &Connection, name: &str, share_id: Option<&str>) -> i64 {
        conn.execute(
            "INSERT INTO PROJECT (NAME, SHARE_ID) VALUES (?1, ?2)",
            params![name, share_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn member(conn: &Connection, proj: i64, fk: i64) {
        conn.execute(
            "INSERT INTO PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK) VALUES (?1, ?2)",
            params![proj, fk],
        )
        .unwrap();
    }

    fn reading(conn: &Connection, proj: i64, fk: i64, status: &str) {
        conn.execute(
            "INSERT INTO PAPER_TO_READING (PROJECT_FK, SOURCE_FK, STATUS) VALUES (?1, ?2, ?3)",
            params![proj, fk, status],
        )
        .unwrap();
    }

    fn note(conn: &Connection, fk: i64, pinned: Option<i64>) -> i64 {
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PAPER_ID_FK, TITLE, NOTE) VALUES (?1, ?2, 't', 'b')",
            params![fk, pinned],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn count(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> i64 {
        conn.query_row(sql, p, |r| r.get(0)).unwrap()
    }

    // ── plan classification ─────────────────────────────────────────────────

    #[test]
    fn plan_classifies_transplant_adopt_and_collapse() {
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, Some("/p/wv1.pdf"));
        version(&conn, w, "arxiv:W", 2, None);
        version(&conn, l, "local:L", 1, Some("/p/lv1.pdf")); // collides, W has PDF → dup
        version(&conn, l, "local:L", 2, Some("/p/lv2.pdf")); // collides, W lacks PDF → adopt
        version(&conn, l, "local:L", 3, Some("/p/lv3.pdf")); // W lacks v3 → transplant
        version(&conn, l, "local:L", 4, None); // transplant, no file

        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        assert_eq!(plan.winner_id, "arxiv:W");
        assert_eq!(plan.loser_id, "local:L");
        assert_eq!(
            plan.actions,
            vec![
                VersionAction::Collapse {
                    version: 1,
                    duplicate_pdf_path: Some("/p/lv1.pdf".into())
                },
                VersionAction::AdoptPdf {
                    version: 2,
                    loser_pdf_path: "/p/lv2.pdf".into()
                },
                VersionAction::Transplant {
                    version: 3,
                    loser_pdf_path: Some("/p/lv3.pdf".into())
                },
                VersionAction::Transplant {
                    version: 4,
                    loser_pdf_path: None
                },
            ]
        );
    }

    #[test]
    fn plan_treats_empty_pdf_path_as_no_pdf() {
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, Some("")); // '' == no PDF
        version(&conn, l, "local:L", 1, Some("/p/l.pdf"));
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        assert_eq!(
            plan.actions,
            vec![VersionAction::AdoptPdf {
                version: 1,
                loser_pdf_path: "/p/l.pdf".into()
            }]
        );
    }

    /// A ghost winner pointer (stored path, file gone) must classify as
    /// AdoptPdf, not mark the loser's real file a deletable duplicate.
    #[test]
    fn plan_ghost_winner_pdf_loses_to_a_real_loser_file() {
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, Some("/p/ghost.pdf"));
        version(&conn, l, "local:L", 1, Some("/p/real.pdf"));

        // The injected check says the winner's file is gone, the loser's real.
        let plan = merge_plan(&conn, w, l, |p| p == "/p/real.pdf").unwrap();
        assert_eq!(
            plan.actions,
            vec![VersionAction::AdoptPdf {
                version: 1,
                loser_pdf_path: "/p/real.pdf".into()
            }]
        );
    }

    // ── plan guards ─────────────────────────────────────────────────────────

    #[test]
    fn plan_rejects_self_merge_missing_roots_and_trashed_roots() {
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");

        assert!(matches!(
            merge_plan(&conn, w, w, |_| true),
            Err(CoreError::Conflict(_))
        ));
        assert!(matches!(
            merge_plan(&conn, 999, l, |_| true),
            Err(CoreError::PaperNotFound(_))
        ));
        assert!(matches!(
            merge_plan(&conn, w, 999, |_| true),
            Err(CoreError::PaperNotFound(_))
        ));

        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'deleted' WHERE SOURCE_FK = ?",
            [l],
        )
        .unwrap();
        let err = merge_plan(&conn, w, l, |_| true).unwrap_err();
        assert!(err.to_string().contains("loser"), "{err}");
        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'active' WHERE SOURCE_FK = ?",
            [l],
        )
        .unwrap();
        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'deleted' WHERE SOURCE_FK = ?",
            [w],
        )
        .unwrap();
        let err = merge_plan(&conn, w, l, |_| true).unwrap_err();
        assert!(err.to_string().contains("winner"), "{err}");
    }

    #[test]
    fn plan_rejects_loser_in_shared_project_but_not_winner() {
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, None);
        version(&conn, l, "local:L", 1, None);
        let shared = project(&conn, "shared", Some("share-uuid"));
        member(&conn, shared, w);
        // Winner in a shared project is fine — its id survives.
        assert!(merge_plan(&conn, w, l, |_| true).is_ok());

        member(&conn, shared, l);
        let err = merge_plan(&conn, w, l, |_| true).unwrap_err();
        assert!(
            err.to_string().contains("shared project"),
            "expected the share guard, got: {err}"
        );

        // A trashed shared project still blocks: it keeps its SHARE_ID and
        // can be restored, so peers' docs still list the member.
        conn.execute(
            "UPDATE PROJECT SET STATUS = 'deleted' WHERE PROJECT_FK = ?",
            [shared],
        )
        .unwrap();
        assert!(merge_plan(&conn, w, l, |_| true).is_err());

        // Clearing the SHARE_ID genuinely un-shares and unblocks.
        conn.execute(
            "UPDATE PROJECT SET SHARE_ID = NULL WHERE PROJECT_FK = ?",
            [shared],
        )
        .unwrap();
        assert!(merge_plan(&conn, w, l, |_| true).is_ok());
    }

    // ── the transaction ─────────────────────────────────────────────────────

    /// One fully-loaded fixture: overlapping + disjoint projects, reading
    /// statuses in every configuration, pinned + unpinned notes, annotations,
    /// tags on both sides, full text on a transplanted version.
    fn loaded_fixture(conn: &Connection) -> (i64, i64) {
        let w = root(conn, "arxiv:W");
        let l = root(conn, "local:L");
        version(conn, w, "arxiv:W", 1, Some("/p/wv1.pdf"));
        version(conn, l, "local:L", 1, Some("/p/lv1.pdf")); // collapse (dup)
        version(conn, l, "local:L", 2, None); // transplant
        conn.execute(
            "UPDATE PAPER_META SET TAGS = '[\"ml\",\"shared\"]' WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ?)",
            [w],
        )
        .unwrap();
        conn.execute(
            "UPDATE PAPER_META SET TAGS = '[\"shared\",\"vision\"]' WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ?)",
            [l],
        )
        .unwrap();
        (w, l)
    }

    #[test]
    fn merge_moves_notes_and_repins_version_pinned_notes() {
        let conn = db();
        let (w, l) = loaded_fixture(&conn);
        let w_v1: i64 = count(
            &conn,
            "SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ? AND VERSION = 1",
            [w],
        );
        let l_v1: i64 = count(
            &conn,
            "SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ? AND VERSION = 1",
            [l],
        );
        let l_v2: i64 = count(
            &conn,
            "SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ? AND VERSION = 2",
            [l],
        );
        let unpinned = note(&conn, l, None);
        let pinned_colliding = note(&conn, l, Some(l_v1));
        let pinned_transplanted = note(&conn, l, Some(l_v2));
        let winner_note = note(&conn, w, Some(w_v1));

        let mut conn = conn;
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        let stats = merge_paper_roots(&mut conn, &plan, &[]).unwrap();
        assert_eq!(stats.notes_moved, 3);

        let pin = |id: i64| -> (i64, Option<i64>) {
            conn.query_row(
                "SELECT SOURCE_FK, PAPER_ID_FK FROM NOTE WHERE NOTE_SK = ?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(pin(unpinned), (w, None));
        // Pinned to the collapsing loser v1 → re-pinned to the winner's v1 row.
        assert_eq!(pin(pinned_colliding), (w, Some(w_v1)));
        // Pinned to the transplanted v2 → the row survived, pin unchanged.
        assert_eq!(pin(pinned_transplanted), (w, Some(l_v2)));
        assert_eq!(pin(winner_note), (w, Some(w_v1)));
    }

    #[test]
    fn merge_moves_annotations() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        conn.execute(
            "INSERT INTO ANNOTATION (SOURCE_FK, ANCHOR) VALUES (?, '{}')",
            [l],
        )
        .unwrap();
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        let stats = merge_paper_roots(&mut conn, &plan, &[]).unwrap();
        assert_eq!(stats.annotations_moved, 1);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM ANNOTATION WHERE SOURCE_FK = ?",
                [w]
            ),
            1
        );
    }

    #[test]
    fn merge_memberships_and_reading_statuses_across_all_configurations() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        // overlap_a: both members, loser has status, winner none → status moves.
        let overlap_a = project(&conn, "overlap-a", None);
        member(&conn, overlap_a, w);
        member(&conn, overlap_a, l);
        reading(&conn, overlap_a, l, "read");
        // overlap_b: both members, both have statuses → winner's wins.
        let overlap_b = project(&conn, "overlap-b", None);
        member(&conn, overlap_b, w);
        member(&conn, overlap_b, l);
        reading(&conn, overlap_b, w, "reading");
        reading(&conn, overlap_b, l, "read");
        // disjoint: loser only, with a status → membership + status move.
        let disjoint = project(&conn, "disjoint", None);
        member(&conn, disjoint, l);
        reading(&conn, disjoint, l, "reading");

        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        let stats = merge_paper_roots(&mut conn, &plan, &[]).unwrap();
        assert_eq!(stats.memberships_collapsed, 2);
        assert_eq!(stats.memberships_moved, 1);
        assert_eq!(stats.reading_statuses_moved, 2); // overlap_a + disjoint

        // Loser has no rows anywhere.
        for table in ["PROJECT_TO_PAPER", "PAPER_TO_READING"] {
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT COUNT(*) FROM {table} WHERE SOURCE_FK = ?"),
                    [l]
                ),
                0,
                "{table} still references the loser"
            );
        }
        let status = |proj: i64| -> Option<String> {
            conn.query_row(
                "SELECT STATUS FROM PAPER_TO_READING WHERE PROJECT_FK = ? AND SOURCE_FK = ?",
                params![proj, w],
                |r| r.get(0),
            )
            .ok()
        };
        assert_eq!(status(overlap_a).as_deref(), Some("read")); // moved
        assert_eq!(status(overlap_b).as_deref(), Some("reading")); // winner's kept
        assert_eq!(status(disjoint).as_deref(), Some("reading")); // moved with membership
                                                                  // Exactly one membership row per project for the winner.
        for proj in [overlap_a, overlap_b, disjoint] {
            assert_eq!(
                count(
                    &conn,
                    "SELECT COUNT(*) FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ? AND SOURCE_FK = ?",
                    params![proj, w],
                ),
                1
            );
        }
    }

    #[test]
    fn merge_transplants_versions_and_collapses_collisions() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        let stats = merge_paper_roots(&mut conn, &plan, &[]).unwrap();
        assert_eq!(stats.versions_transplanted, 1);
        assert_eq!(stats.versions_collapsed, 1);

        // Winner owns v1 (its own) and v2 (transplanted), all under its id.
        let versions: Vec<(String, i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT SOURCE_ID, VERSION, TITLE FROM PAPER ORDER BY VERSION")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        assert_eq!(
            versions,
            vec![
                ("arxiv:W".to_string(), 1, "arxiv:W v1".to_string()),
                ("arxiv:W".to_string(), 2, "local:L v2".to_string()),
            ]
        );
        // The loser root and its collapsed v1's meta are gone.
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_ROOTS WHERE SOURCE_FK = ?",
                [l]
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_META WHERE PAPER_ID NOT IN (SELECT PAPER_ID FROM PAPER)",
                [],
            ),
            0,
            "orphaned PAPER_META rows"
        );
    }

    #[test]
    fn merge_unions_tags_across_both_halves_of_dual_storage() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        let stats = merge_paper_roots(&mut conn, &plan, &[]).unwrap();
        assert_eq!(stats.tags_added, vec!["vision".to_string()]);

        // JSON half: every surviving version carries the union, winner-first.
        let jsons: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT m.TAGS FROM PAPER p JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
                     WHERE p.SOURCE_FK = ? ORDER BY p.VERSION",
                )
                .unwrap();
            let rows = stmt.query_map([w], |r| r.get(0)).unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        assert_eq!(jsons, vec![r#"["ml","shared","vision"]"#; 2]);

        // Relational half: rows per version, keyed by the winner's SOURCE_ID.
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_TO_TAG WHERE SOURCE_ID = 'arxiv:W'",
                [],
            ),
            6 // 3 tags × 2 versions
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_TO_TAG WHERE SOURCE_ID = 'local:L'",
                []
            ),
            0
        );
    }

    #[test]
    fn merge_moves_fts_to_the_winner_and_drops_the_loser_row() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        // Give the loser's transplanted v2 the only full text in the merge.
        conn.execute(
            "UPDATE PAPER_META SET FULL_TEXT = 'quantum entanglement' WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ? AND VERSION = 2)",
            [l],
        )
        .unwrap();
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = 'local:L'",
                []
            ),
            1
        );

        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        merge_paper_roots(&mut conn, &plan, &[]).unwrap();

        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = 'local:L'",
                []
            ),
            0
        );
        // The transplanted text is now indexed under the winner's id.
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = 'arxiv:W' \
                 AND full_text MATCH 'entanglement'",
                [],
            ),
            1
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM papers_fts", []), 1);
    }

    #[test]
    fn merge_resets_version_check_for_both_roots() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        conn.execute(
            "INSERT INTO VERSION_CHECK (SOURCE_FK, NEW_VERSION) VALUES (?, 9)",
            [w],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO VERSION_CHECK (SOURCE_FK, NEW_VERSION) VALUES (?, 9)",
            [l],
        )
        .unwrap();
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        merge_paper_roots(&mut conn, &plan, &[]).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM VERSION_CHECK", []), 0);
    }

    #[test]
    fn merge_rewrites_search_state_saved_ids_with_dedup() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        conn.execute(
            "INSERT INTO SEARCH_STATE (ID, CLAUSES_JSON, SOURCE, MAX_RESULTS, RESULTS_JSON, SAVED_IDS_JSON) \
             VALUES (1, '[]', 'arxiv', 10, '[]', '[\"local:L\",\"arxiv:W\",\"arxiv:other\"]')",
            [],
        )
        .unwrap();
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        merge_paper_roots(&mut conn, &plan, &[]).unwrap();
        let saved: String = conn
            .query_row(
                "SELECT SAVED_IDS_JSON FROM SEARCH_STATE WHERE ID = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // loser rewritten to winner, then deduped against the existing entry.
        assert_eq!(saved, r#"["arxiv:W","arxiv:other"]"#);
    }

    #[test]
    fn merge_applies_pdf_paths_from_the_rename_slice_and_clears_missing() {
        let mut conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, None); // adoption target
        version(&conn, l, "local:L", 1, Some("/p/lv1.pdf")); // adopt
        version(&conn, l, "local:L", 2, Some("/p/lv2.pdf")); // transplant, file missing

        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        // Service renamed only v1; v2's file was missing on disk.
        merge_paper_roots(&mut conn, &plan, &[(1, "/p/arxiv_Wv1.pdf".to_string())]).unwrap();

        let row = |v: i64| -> (Option<String>, bool) {
            conn.query_row(
                "SELECT m.PDF_PATH, p.HAS_PDF FROM PAPER p \
                 JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
                 WHERE p.SOURCE_FK = ?1 AND p.VERSION = ?2",
                params![w, v],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(row(1), (Some("/p/arxiv_Wv1.pdf".to_string()), true));
        assert_eq!(row(2), (None, false), "missing file must clear the pointer");
    }

    #[test]
    fn merge_is_atomic_when_the_plan_went_stale() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        let proj = project(&conn, "p", None);
        member(&conn, proj, l);
        note(&conn, l, None);
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();

        // The loser gets trashed between plan and commit.
        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'deleted' WHERE SOURCE_FK = ?",
            [l],
        )
        .unwrap();
        let err = merge_paper_roots(&mut conn, &plan, &[]).unwrap_err();
        assert!(matches!(err, CoreError::Conflict(_)), "{err}");

        // Nothing moved: root, membership, note, versions all intact.
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_ROOTS WHERE SOURCE_FK = ?",
                [l]
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?",
                [l]
            ),
            1
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM NOTE WHERE SOURCE_FK = ?", [l]),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER WHERE SOURCE_ID = 'local:L'",
                []
            ),
            2
        );
    }

    /// An adoption whose loser file also went missing (no rename executed)
    /// must clear the winner's ghost pointer, mirroring the Transplant branch.
    #[test]
    fn merge_clears_the_winner_ghost_when_the_adoption_file_is_missing_too() {
        let mut conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, Some("/p/ghost.pdf"));
        version(&conn, l, "local:L", 1, Some("/p/also-gone.pdf"));

        // pdf_exists=false for the winner's path -> AdoptPdf; empty rename
        // slice -> the loser's file was missing on disk as well.
        let plan = merge_plan(&conn, w, l, |_| false).unwrap();
        assert!(matches!(plan.actions[0], VersionAction::AdoptPdf { .. }));
        merge_paper_roots(&mut conn, &plan, &[]).unwrap();

        let (path, has): (Option<String>, bool) = conn
            .query_row(
                "SELECT m.PDF_PATH, p.HAS_PDF FROM PAPER p \
                 JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
                 WHERE p.SOURCE_FK = ?1 AND p.VERSION = 1",
                [w],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((path, has), (None, false), "ghost pointer must be cleared");
    }

    /// "ML" and "ml" are one tag (TAG is COLLATE NOCASE): the union must not
    /// produce a case-duplicate JSON entry or a second PAPER_TO_TAG row.
    #[test]
    fn merge_tag_union_is_case_insensitive() {
        let mut conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, None);
        version(&conn, l, "local:L", 2, None);
        for (fk, tags) in [(w, r#"["ML"]"#), (l, r#"["ml","vision"]"#)] {
            conn.execute(
                "UPDATE PAPER_META SET TAGS = ?1 WHERE PAPER_ID IN \
                 (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ?2)",
                params![tags, fk],
            )
            .unwrap();
        }
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        let stats = merge_paper_roots(&mut conn, &plan, &[]).unwrap();
        assert_eq!(stats.tags_added, vec!["vision".to_string()]);
        // One PAPER_TO_TAG row per (version, logical tag): 2 versions x 2 tags.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM PAPER_TO_TAG", []), 4);
        let json: String = conn
            .query_row(
                "SELECT m.TAGS FROM PAPER p JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
                 WHERE p.SOURCE_FK = ? AND p.VERSION = 1",
                [w],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            json, r#"["ML","vision"]"#,
            "winner casing wins, no case-dup"
        );
    }

    /// A PDF attached to either root between plan and commit changes the
    /// snapshot the classification relied on — the merge must refuse.
    #[test]
    fn merge_refuses_when_a_pdf_was_attached_after_the_plan() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        conn.execute(
            "UPDATE PAPER_META SET PDF_PATH = '/p/new.pdf' WHERE PAPER_ID IN \
             (SELECT PAPER_ID FROM PAPER WHERE SOURCE_FK = ? AND VERSION = 1)",
            [w],
        )
        .unwrap();
        let err = merge_paper_roots(&mut conn, &plan, &[]).unwrap_err();
        assert!(err.to_string().contains("PDF was attached"), "{err}");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_ROOTS WHERE SOURCE_FK = ?",
                [l]
            ),
            1
        );
    }

    /// Plan-time premises are re-checked in the transaction (other processes
    /// aren't covered by the in-process lock).
    #[test]
    fn merge_rechecks_the_share_guard_inside_the_transaction() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();

        // Another process links the loser into a shared project post-plan.
        let shared = project(&conn, "shared", Some("share-uuid"));
        member(&conn, shared, l);

        let err = merge_paper_roots(&mut conn, &plan, &[]).unwrap_err();
        assert!(err.to_string().contains("shared project"), "{err}");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER_ROOTS WHERE SOURCE_FK = ?",
                [l]
            ),
            1,
            "loser must survive a refused merge"
        );
    }

    #[test]
    fn merge_refuses_when_a_version_appeared_after_the_plan() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();

        // A concurrent import lands a new loser version the plan never saw —
        // committing anyway would cascade-delete it silently with the root.
        version(&conn, l, "local:L", 9, None);

        let err = merge_paper_roots(&mut conn, &plan, &[]).unwrap_err();
        assert!(err.to_string().contains("gained or lost versions"), "{err}");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM PAPER WHERE SOURCE_ID = 'local:L'",
                []
            ),
            3,
            "no loser rows may be touched"
        );

        // Same drift on the WINNER side (e.g. a colliding version fetched
        // in-between) is refused too, not surfaced as a UNIQUE violation.
        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        version(&conn, w, "arxiv:W", 2, None);
        let err = merge_paper_roots(&mut conn, &plan, &[]).unwrap_err();
        assert!(err.to_string().contains("gained or lost versions"), "{err}");
    }

    #[test]
    fn merge_leaves_no_loser_references_anywhere() {
        let mut conn = db();
        let (w, l) = loaded_fixture(&conn);
        let proj = project(&conn, "p", None);
        member(&conn, proj, l);
        reading(&conn, proj, l, "read");
        note(&conn, l, None);
        conn.execute(
            "INSERT INTO ANNOTATION (SOURCE_FK, ANCHOR) VALUES (?, '{}')",
            [l],
        )
        .unwrap();

        let plan = merge_plan(&conn, w, l, |_| true).unwrap();
        merge_paper_roots(&mut conn, &plan, &[]).unwrap();

        for (table, col) in [
            ("PAPER_ROOTS", "SOURCE_FK"),
            ("PAPER", "SOURCE_FK"),
            ("NOTE", "SOURCE_FK"),
            ("ANNOTATION", "SOURCE_FK"),
            ("PROJECT_TO_PAPER", "SOURCE_FK"),
            ("PAPER_TO_READING", "SOURCE_FK"),
            ("VERSION_CHECK", "SOURCE_FK"),
        ] {
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT COUNT(*) FROM {table} WHERE {col} = ?"),
                    [l]
                ),
                0,
                "{table}.{col} still references the loser fk"
            );
        }
        for (table, col) in [("PAPER", "SOURCE_ID"), ("PAPER_TO_TAG", "SOURCE_ID")] {
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT COUNT(*) FROM {table} WHERE {col} = 'local:L'"),
                    [],
                ),
                0,
                "{table}.{col} still holds the loser id"
            );
        }
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM papers_fts WHERE paper_id = 'local:L'",
                []
            ),
            0
        );
        // FK integrity of everything that remains.
        let violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(violations, 0, "foreign_key_check found dangling rows");
    }
}
