//! paper_merge service — the paper/PDF dedupe workflow's front door.
//!
//! Orchestrates [`storage::queries::paper::merge_plan`] /
//! [`merge_paper_roots`] around the filesystem work, in the same shape as
//! `paper_import`'s rollback philosophy:
//!
//! 1. Plan (read-only DB classification of the loser's versions).
//! 2. FS phase: rename loser PDFs to the winner's on-disk names — reversible.
//! 3. DB transaction: every dependent row re-pointed, loser root deleted.
//!    On failure the renames are undone (best-effort) and the error surfaces.
//! 4. Post-commit only: unlink duplicate loser PDFs — the one irreversible
//!    filesystem step goes last, and a failure there can no longer corrupt
//!    state (worst case: an orphan file in the PDF dir).
//!
//! Serialized under `paper_import::IMPORT_ROOT_LOCK`: a concurrent PDF import
//! resolving to the loser's identity must not race the root's deletion.
//!
//! Accepted crash window (same class as `paper_import`'s rename-then-commit):
//! a process kill between the FS renames and the DB commit leaves loser rows
//! pointing at moved files. Nothing is lost — the files sit under the winner's
//! on-disk names, and readers fall back from the stored path to the recomputed
//! managed name — but a re-run is needed to reconcile the pointers.

use crate::error::{CoreError, Result};
use crate::service::paper::{pdf_on_disk_name, resolve_source_id, PaperRef};
use crate::service::paper_import::IMPORT_ROOT_LOCK;
use crate::storage::queries::paper as store;
use crate::storage::queries::paper::{MergeStats, VersionAction};
use rusqlite::Connection;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// What a completed merge did — the receipt all three surfaces emit.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct MergeReceipt {
    pub winner_source_fk: i64,
    pub winner_source_id: String,
    /// The id that no longer exists after the merge.
    pub merged_source_id: String,
    pub notes_moved: usize,
    pub annotations_moved: usize,
    pub memberships_moved: usize,
    /// Loser memberships dropped because the winner was already in the project.
    pub memberships_collapsed: usize,
    pub reading_statuses_moved: usize,
    /// Loser versions the winner lacked, re-keyed under the winner (same
    /// version numbers).
    pub versions_transplanted: usize,
    /// Loser versions collapsed into the winner's same-numbered versions.
    pub versions_collapsed: usize,
    /// Tags the loser carried that the winner didn't (now unioned in).
    pub tags_added: Vec<String>,
    /// Loser PDFs renamed to winner on-disk names in the managed dir.
    pub pdfs_renamed: usize,
    /// Loser PDFs that filled a PDF-less winner version — renamed in, or
    /// pointed at in place when stored outside the managed dir.
    pub pdfs_adopted: usize,
    /// Duplicate loser PDFs unlinked after commit.
    pub pdfs_deleted: usize,
    /// Duplicate loser PDFs left on disk because they live outside the
    /// managed PDF dir (never deleted there).
    pub pdfs_kept_external: usize,
    /// Loser versions whose stored PDF path had no file behind it
    /// (transplants/adoptions found gone pre-rename, duplicates found gone
    /// post-commit).
    pub pdfs_missing: usize,
}

/// Resolve a ref to its root's SOURCE_FK (trashed roots included — the plan
/// wants to reject those with a 409, not a 404).
fn resolve_source_fk(conn: &Connection, paper: &PaperRef) -> Result<i64> {
    match paper {
        PaperRef::SourceFk(sfk) => Ok(*sfk),
        _ => {
            let sid = resolve_source_id(conn, paper)?.ok_or_else(|| {
                // Plain id in the message, matching every other surface.
                CoreError::PaperNotFound(match paper {
                    PaperRef::Id(pid) => pid.to_string(),
                    PaperRef::SourceFk(sfk) => sfk.to_string(),
                    PaperRef::Source { source_id, .. } => source_id.clone(),
                })
            })?;
            crate::service::paper::resolve_source_fk(conn, &sid)
        }
    }
}

/// One executed (reversible) rename, kept so a failed DB phase can undo it.
struct DoneRename {
    from: PathBuf,
    to: PathBuf,
}

/// Merge the `loser` paper root into the `winner`: winner's metadata is
/// canonical; the loser's notes, annotations, project memberships, reading
/// statuses, tags, missing versions, and PDFs move over; the loser root is
/// deleted. See the storage module for the exact row-level contract.
pub fn merge_papers(
    conn: &mut Connection,
    pdf_dir: &Path,
    winner: &PaperRef,
    loser: &PaperRef,
) -> Result<MergeReceipt> {
    let _guard = IMPORT_ROOT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let winner_fk = resolve_source_fk(conn, winner)?;
    let loser_fk = resolve_source_fk(conn, loser)?;
    // "Winner has a PDF" is judged by a real file, not the stored string.
    let plan = store::merge_plan(conn, winner_fk, loser_fk, |p: &str| Path::new(p).is_file())?;

    // ── FS phase: reversible renames only ───────────────────────────────────
    let mut renames: Vec<(i64, String)> = Vec::new();
    let mut done: Vec<DoneRename> = Vec::new();
    let mut pdfs_adopted = 0usize;
    let mut pdfs_missing = 0usize;

    // Never move a file that lives outside the managed PDF dir (a hand-linked
    // or legacy path): keep it where it is and let the DB keep pointing at it.
    let managed = fs::canonicalize(pdf_dir).ok();
    let mut rename_for = |version: i64,
                          from_str: &str,
                          adopt: bool,
                          done: &mut Vec<DoneRename>,
                          renames: &mut Vec<(i64, String)>|
     -> Result<()> {
        let from = Path::new(from_str);
        let to = pdf_dir.join(pdf_on_disk_name(&plan.winner_id, version));
        if !from.is_file() {
            pdfs_missing += 1;
            return Ok(()); // absent from `renames` → DB clears the pointer
        }
        let inside_managed = managed.as_deref().is_some_and(|m| {
            from.parent()
                .and_then(|p| fs::canonicalize(p).ok())
                .is_some_and(|p| p == m)
        });
        // Keep the file where it is (DB points at it) when it must not be
        // moved: it lives outside the managed dir, or an unrelated file (e.g.
        // an orphan from a crashed import) already occupies the destination —
        // rename() would silently destroy that file's bytes.
        if !inside_managed || (from != to.as_path() && to.exists()) {
            renames.push((version, from_str.to_owned()));
            if adopt {
                pdfs_adopted += 1;
            }
            return Ok(());
        }
        if from != to.as_path() {
            crate::service::files::rename_pdf_counted(pdf_dir, from, &to).map_err(|e| {
                CoreError::Internal(format!(
                    "merge_papers: renaming PDF {from:?} -> {to:?} failed: {e}"
                ))
            })?;
            done.push(DoneRename {
                from: from.to_path_buf(),
                to: to.clone(),
            });
        }
        renames.push((version, to.to_string_lossy().into_owned()));
        if adopt {
            pdfs_adopted += 1;
        }
        Ok(())
    };

    let mut fs_result: Result<()> = Ok(());
    for action in &plan.actions {
        let r = match action {
            VersionAction::Transplant {
                version,
                loser_pdf_path: Some(p),
            } => rename_for(*version, p, false, &mut done, &mut renames),
            VersionAction::AdoptPdf {
                version,
                loser_pdf_path,
            } => rename_for(*version, loser_pdf_path, true, &mut done, &mut renames),
            _ => Ok(()),
        };
        if let Err(e) = r {
            fs_result = Err(e);
            break;
        }
    }
    if let Err(e) = fs_result {
        return Err(fold_undo_failures(e, undo_renames(pdf_dir, &done)));
    }

    // ── DB phase ────────────────────────────────────────────────────────────
    let stats: MergeStats = match store::merge_paper_roots(conn, &plan, &renames) {
        Ok(s) => s,
        Err(e) => {
            return Err(fold_undo_failures(e, undo_renames(pdf_dir, &done)));
        }
    };

    // ── Post-commit: irreversible deletions last, best-effort ───────────────
    let mut pdfs_deleted = 0usize;
    let mut pdfs_kept_external = 0usize;
    for action in &plan.actions {
        if let VersionAction::Collapse {
            version,
            duplicate_pdf_path: Some(p),
        } = action
        {
            // Delete only while the winner's copy verifiably exists as a
            // DIFFERENT file — the plan-time snapshot may have gone stale, and
            // both rows can point at one shared file. Skipping leaves at worst
            // an orphan in the PDF dir; deleting wrongly loses the last copy.
            let winner_path = store::pdf_path_for_version(conn, plan.winner_fk, *version)?;
            let winner_has_other_copy = winner_path
                .as_deref()
                .is_some_and(|wp| wp != p.as_str() && Path::new(wp).is_file());
            if !Path::new(p).is_file() {
                pdfs_missing += 1;
            } else if winner_has_other_copy {
                if crate::service::files::delete_pdf(pdf_dir, p) {
                    pdfs_deleted += 1;
                } else {
                    // Outside the managed dir: never deleted, reported instead.
                    pdfs_kept_external += 1;
                }
            }
        }
    }

    Ok(MergeReceipt {
        winner_source_fk: plan.winner_fk,
        winner_source_id: plan.winner_id,
        merged_source_id: plan.loser_id,
        notes_moved: stats.notes_moved,
        annotations_moved: stats.annotations_moved,
        memberships_moved: stats.memberships_moved,
        memberships_collapsed: stats.memberships_collapsed,
        reading_statuses_moved: stats.reading_statuses_moved,
        versions_transplanted: stats.versions_transplanted,
        versions_collapsed: stats.versions_collapsed,
        tags_added: stats.tags_added,
        pdfs_renamed: done.len(),
        pdfs_adopted,
        pdfs_deleted,
        pdfs_kept_external,
        pdfs_missing,
    })
}

/// Reverse the FS phase, reporting what could NOT be restored (the caller
/// folds failures into its error instead of silently losing them).
fn undo_renames(pdf_dir: &Path, done: &[DoneRename]) -> Vec<String> {
    let mut failed = Vec::new();
    for r in done.iter().rev() {
        if let Err(e) = crate::service::files::rename_pdf_counted(pdf_dir, &r.to, &r.from) {
            failed.push(format!("{:?} -> {:?}: {e}", r.to, r.from));
        }
    }
    failed
}

/// A failed undo leaves files under winner names with the DB unchanged — an
/// inconsistency the caller must hear about, upgraded to Internal so nobody
/// blindly retries against the skewed filesystem.
fn fold_undo_failures(e: CoreError, failed: Vec<String>) -> CoreError {
    if failed.is_empty() {
        e
    } else {
        CoreError::Internal(format!(
            "{e}; additionally {} PDF rename(s) could not be undone: {}",
            failed.len(),
            failed.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;
    use rusqlite::params;
    use tempfile::tempdir;

    fn root(conn: &Connection, sid: &str) -> i64 {
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)", [sid])
            .unwrap();
        conn.last_insert_rowid()
    }

    fn version(conn: &Connection, fk: i64, sid: &str, v: i64, pdf: Option<&str>) {
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, HAS_PDF, SOURCE_FK) \
             VALUES (?1, ?2, 'T', ?3, ?4)",
            params![sid, v, pdf.is_some(), fk],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PDF_PATH, AUTHORS) VALUES (?1, ?2, '[\"A\"]')",
            params![conn.last_insert_rowid(), pdf],
        )
        .unwrap();
    }

    /// Winner arxiv:W v1 (own PDF) + v2 (no PDF); loser local:L v1 (duplicate
    /// PDF), v2 (adoptable PDF), v3 (transplantable PDF). Files really exist.
    fn seeded(pdf_dir: &Path) -> (Connection, i64, i64) {
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        let path = |name: &str| pdf_dir.join(name).to_string_lossy().into_owned();
        for (name, content) in [
            ("arxiv_Wv1.pdf", "winner-v1"),
            ("local_Lv1.pdf", "dup"),
            ("local_Lv2.pdf", "adopt-me"),
            ("local_Lv3.pdf", "transplant-me"),
        ] {
            fs::write(pdf_dir.join(name), content).unwrap();
        }
        version(&conn, w, "arxiv:W", 1, Some(&path("arxiv_Wv1.pdf")));
        version(&conn, w, "arxiv:W", 2, None);
        version(&conn, l, "local:L", 1, Some(&path("local_Lv1.pdf")));
        version(&conn, l, "local:L", 2, Some(&path("local_Lv2.pdf")));
        version(&conn, l, "local:L", 3, Some(&path("local_Lv3.pdf")));
        (conn, w, l)
    }

    #[test]
    fn merge_end_to_end_renames_adopts_and_deletes_files() {
        let dir = tempdir().unwrap();
        let (mut conn, w, _l) = seeded(dir.path());

        let r = merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::source("local:L".into()),
        )
        .unwrap();

        assert_eq!(r.winner_source_id, "arxiv:W");
        assert_eq!(r.merged_source_id, "local:L");
        assert_eq!(r.versions_transplanted, 1);
        assert_eq!(r.versions_collapsed, 2);
        assert_eq!(r.pdfs_renamed, 2); // adopt v2 + transplant v3
        assert_eq!(r.pdfs_adopted, 1);
        assert_eq!(r.pdfs_deleted, 1); // duplicate v1
        assert_eq!(r.pdfs_missing, 0);

        // Filesystem: winner names exist with the loser's bytes; loser names gone.
        let read = |n: &str| fs::read_to_string(dir.path().join(n)).unwrap();
        assert_eq!(read("arxiv_Wv1.pdf"), "winner-v1");
        assert_eq!(read("arxiv_Wv2.pdf"), "adopt-me");
        assert_eq!(read("arxiv_Wv3.pdf"), "transplant-me");
        for gone in ["local_Lv1.pdf", "local_Lv2.pdf", "local_Lv3.pdf"] {
            assert!(!dir.path().join(gone).exists(), "{gone} should be gone");
        }

        // DB pointers follow the renames.
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
        let expect = |n: &str| Some(dir.path().join(n).to_string_lossy().into_owned());
        assert_eq!(row(1), (expect("arxiv_Wv1.pdf"), true));
        assert_eq!(row(2), (expect("arxiv_Wv2.pdf"), true));
        assert_eq!(row(3), (expect("arxiv_Wv3.pdf"), true));
    }

    /// The rename must move the storage-cache entry with the file: a phantom
    /// entry under the loser's name has the same size as the renamed file, so
    /// the total only skews once the renamed PDF is deleted — assert there.
    #[test]
    fn merge_renames_keep_the_pdf_storage_cache_in_sync() {
        use crate::service::files::{delete_pdf, pdf_storage_bytes};
        let dir = tempdir().unwrap();
        let (mut conn, w, _l) = seeded(dir.path());
        assert!(pdf_storage_bytes(dir.path()) > 0); // seed the cache pre-merge

        merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::source("local:L".into()),
        )
        .unwrap();

        let renamed = dir.path().join("arxiv_Wv2.pdf");
        assert!(delete_pdf(dir.path(), &renamed.to_string_lossy()));
        // Fresh walk with the production filter (.pdf files only), so the
        // comparison holds the real counting semantics, not read_dir's.
        let walk: u64 = crate::service::files::walk_pdf_files(dir.path())
            .values()
            .sum();
        assert_eq!(pdf_storage_bytes(dir.path()), walk);
    }

    #[test]
    fn merge_counts_missing_files_and_skips_their_pointers() {
        let dir = tempdir().unwrap();
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, None);
        // Both loser versions claim PDFs that don't exist on disk.
        version(&conn, l, "local:L", 1, Some("/nonexistent/lv1.pdf")); // adopt attempt
        version(&conn, l, "local:L", 2, Some("/nonexistent/lv2.pdf")); // transplant

        let mut conn = conn;
        let r = merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::SourceFk(l),
        )
        .unwrap();
        assert_eq!(r.pdfs_missing, 2);
        assert_eq!(r.pdfs_renamed, 0);
        assert_eq!(r.pdfs_adopted, 0);

        // Adoption skipped: winner v1 still has no PDF. Transplanted v2's stale
        // pointer cleared.
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
        assert_eq!(row(1), (None, false));
        assert_eq!(row(2), (None, false));
    }

    #[test]
    fn merge_restores_renamed_files_when_the_db_phase_fails() {
        let dir = tempdir().unwrap();
        let (mut conn, w, l) = seeded(dir.path());
        // Fault injection: the merge transaction reads SEARCH_STATE; dropping
        // the table makes the DB phase fail AFTER the FS phase renamed files.
        conn.execute_batch("DROP TABLE SEARCH_STATE").unwrap();

        let err = merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::SourceFk(l),
        )
        .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("search_state"),
            "{err}"
        );

        // Every file is back under its original name, no winner-name leftovers.
        for (name, content) in [
            ("arxiv_Wv1.pdf", "winner-v1"),
            ("local_Lv1.pdf", "dup"),
            ("local_Lv2.pdf", "adopt-me"),
            ("local_Lv3.pdf", "transplant-me"),
        ] {
            assert_eq!(
                fs::read_to_string(dir.path().join(name)).unwrap(),
                content,
                "{name} not restored"
            );
        }
        for leftover in ["arxiv_Wv2.pdf", "arxiv_Wv3.pdf"] {
            assert!(
                !dir.path().join(leftover).exists(),
                "{leftover} left behind"
            );
        }
        // And the DB rows are untouched.
        let loser_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM PAPER WHERE SOURCE_ID = 'local:L'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(loser_rows, 3);
    }

    /// Winner claims a PDF whose file is gone; the loser holds the only real
    /// copy — it must be adopted, never deleted as a duplicate.
    #[test]
    fn merge_rescues_the_loser_pdf_when_the_winner_pointer_is_a_ghost() {
        let dir = tempdir().unwrap();
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        let ghost = dir.path().join("arxiv_Wv1.pdf"); // stored, never written
        let real = dir.path().join("local_Lv1.pdf");
        fs::write(&real, "the-only-real-copy").unwrap();
        version(&conn, w, "arxiv:W", 1, Some(&ghost.to_string_lossy()));
        version(&conn, l, "local:L", 1, Some(&real.to_string_lossy()));

        let mut conn = conn;
        let r = merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::SourceFk(l),
        )
        .unwrap();
        assert_eq!(r.pdfs_adopted, 1);
        assert_eq!(r.pdfs_deleted, 0, "the only real PDF must not be deleted");

        let path: Option<String> = conn
            .query_row(
                "SELECT m.PDF_PATH FROM PAPER p JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
                 WHERE p.SOURCE_FK = ?1 AND p.VERSION = 1",
                params![w],
                |r| r.get(0),
            )
            .unwrap();
        let expected = dir.path().join("arxiv_Wv1.pdf");
        assert_eq!(path.as_deref(), Some(&*expected.to_string_lossy()));
        assert_eq!(fs::read_to_string(&expected).unwrap(), "the-only-real-copy");
    }

    /// An unrelated file already at the destination name (orphan of a crashed
    /// import) must never be overwritten: the loser file stays put and the DB
    /// points at it in place.
    #[test]
    fn merge_never_overwrites_an_orphan_at_the_destination_name() {
        let dir = tempdir().unwrap();
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        let orphan = dir.path().join("arxiv_Wv2.pdf");
        let loser_file = dir.path().join("local_Lv2.pdf");
        fs::write(&orphan, "orphan-bytes").unwrap();
        fs::write(&loser_file, "loser-bytes").unwrap();
        version(&conn, w, "arxiv:W", 1, None);
        version(&conn, l, "local:L", 2, Some(&loser_file.to_string_lossy()));

        let mut conn = conn;
        merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::SourceFk(l),
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&orphan).unwrap(), "orphan-bytes");
        assert_eq!(fs::read_to_string(&loser_file).unwrap(), "loser-bytes");
        let path: Option<String> = conn
            .query_row(
                "SELECT m.PDF_PATH FROM PAPER p JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
                 WHERE p.SOURCE_FK = ?1 AND p.VERSION = 2",
                params![w],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(path.as_deref(), Some(&*loser_file.to_string_lossy()));
    }

    /// Undo failures are reported, and a caller folding them keeps both the
    /// original failure and the un-restored files in one message.
    #[test]
    fn undo_failures_are_reported_and_folded_into_the_error() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        fs::write(&a, "x").unwrap();
        let restorable = DoneRename {
            from: dir.path().join("orig.pdf"),
            to: a.clone(),
        };
        let gone = DoneRename {
            from: dir.path().join("orig2.pdf"),
            to: dir.path().join("never-existed.pdf"),
        };
        let failed = undo_renames(dir.path(), &[restorable, gone]);
        assert_eq!(failed.len(), 1);
        assert!(dir.path().join("orig.pdf").is_file());

        let e = fold_undo_failures(CoreError::Conflict("boom".into()), failed);
        let msg = e.to_string();
        assert!(matches!(e, CoreError::Internal(_)));
        assert!(
            msg.contains("boom") && msg.contains("could not be undone"),
            "{msg}"
        );
        // No failures -> the original error passes through untouched.
        assert!(matches!(
            fold_undo_failures(CoreError::Conflict("boom".into()), Vec::new()),
            CoreError::Conflict(_)
        ));
    }

    /// An external duplicate is never deleted (delete_pdf is dir-gated) and
    /// the receipt says so instead of staying silent.
    #[test]
    fn merge_reports_external_duplicates_it_leaves_on_disk() {
        let pdf_dir = tempdir().unwrap();
        let elsewhere = tempdir().unwrap();
        let w_pdf = pdf_dir.path().join("arxiv_Wv1.pdf");
        let ext_dup = elsewhere.path().join("copy.pdf");
        fs::write(&w_pdf, "winner").unwrap();
        fs::write(&ext_dup, "dup").unwrap();

        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, Some(&w_pdf.to_string_lossy()));
        version(&conn, l, "local:L", 1, Some(&ext_dup.to_string_lossy()));

        let mut conn = conn;
        let r = merge_papers(
            &mut conn,
            pdf_dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::SourceFk(l),
        )
        .unwrap();
        assert_eq!(r.pdfs_deleted, 0);
        assert_eq!(r.pdfs_kept_external, 1);
        assert!(ext_dup.is_file());
    }

    /// Winner and loser rows can point at ONE shared file; the post-commit
    /// duplicate delete must recognize that and leave it alone.
    #[test]
    fn merge_never_deletes_a_file_the_winner_also_points_at() {
        let dir = tempdir().unwrap();
        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        let shared = dir.path().join("shared.pdf");
        fs::write(&shared, "one-copy").unwrap();
        version(&conn, w, "arxiv:W", 1, Some(&shared.to_string_lossy()));
        version(&conn, l, "local:L", 1, Some(&shared.to_string_lossy()));

        let mut conn = conn;
        let r = merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::SourceFk(l),
        )
        .unwrap();
        assert_eq!(r.pdfs_deleted, 0, "the shared file is not a duplicate");
        assert_eq!(fs::read_to_string(&shared).unwrap(), "one-copy");
    }

    /// A PDF stored OUTSIDE the managed dir (hand-linked / legacy path) is
    /// never moved: the winner's row simply points at it where it lives.
    #[test]
    fn merge_leaves_external_pdfs_in_place() {
        let pdf_dir = tempdir().unwrap();
        let elsewhere = tempdir().unwrap();
        let external = elsewhere.path().join("my-copy.pdf");
        fs::write(&external, "external").unwrap();

        let conn = db();
        let w = root(&conn, "arxiv:W");
        let l = root(&conn, "local:L");
        version(&conn, w, "arxiv:W", 1, None); // adoption target
        version(&conn, l, "local:L", 1, Some(&external.to_string_lossy()));

        let mut conn = conn;
        let r = merge_papers(
            &mut conn,
            pdf_dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::SourceFk(l),
        )
        .unwrap();
        assert_eq!(r.pdfs_adopted, 1);
        assert_eq!(r.pdfs_renamed, 0, "external files must not be moved");
        assert!(external.is_file(), "external file must stay where it was");

        let (path, has): (Option<String>, bool) = conn
            .query_row(
                "SELECT m.PDF_PATH, p.HAS_PDF FROM PAPER p \
                 JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
                 WHERE p.SOURCE_FK = ?1 AND p.VERSION = 1",
                params![w],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(path.as_deref(), Some(&*external.to_string_lossy()));
        assert!(has);
    }

    #[test]
    fn merge_rejects_two_refs_to_the_same_root() {
        let dir = tempdir().unwrap();
        let mut conn = db();
        let w = root(&conn, "arxiv:W");
        version(&conn, w, "arxiv:W", 1, None);
        let err = merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::source("arxiv:W".into()),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Conflict(_)), "{err}");
    }

    #[test]
    fn merge_surfaces_not_found_for_unknown_refs() {
        let dir = tempdir().unwrap();
        let mut conn = db();
        let w = root(&conn, "arxiv:W");
        version(&conn, w, "arxiv:W", 1, None);
        let err = merge_papers(
            &mut conn,
            dir.path(),
            &PaperRef::SourceFk(w),
            &PaperRef::source("arxiv:nope".into()),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::PaperNotFound(_)), "{err}");
    }
}
