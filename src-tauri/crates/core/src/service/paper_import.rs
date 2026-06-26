//! paper_import service — Rust port of `service/paper.py::import_pdf` (589-763).
//! Plan §5.2. The hardest fn in the paper service: a multi-branch rollback
//! matrix over interleaved DB + filesystem writes, serialized by a
//! process-global import-root lock.
//!
//! DI: `conn: &mut Connection` first, `pdf_dir: &Path` resolved by the caller —
//! never read from config here. Calls the existing storage layer
//! (`storage::queries::{paper, project}`); no raw SQL, no duplicated storage.
//!
//! INJECTED SEAM (`resolve`): PDF metadata extraction is Phase-3 work
//! (`sources/pdf_metadata.resolve_pdf_metadata` — reqwest/pdf crates, which must
//! NOT enter `core`). So `import_pdf` takes a resolver `Fn(&[u8]) -> Result<(meta,
//! external)>`. The full orchestration + rollback is ported and TESTED now with
//! a fake resolver; the real one plugs in at Phase 3. `external` is the optional
//! upstream identity `(source_id, version)`: `Some` ⇒ dedupe/adopt an existing
//! arxiv/DOI root, `None` ⇒ mint a fresh `local:<sha>` root (so the
//! "create new root" path is kept — do not drop the Option).
//!
//! Two PDF filename formats stay DISTINCT: on-disk `{safe}v{version}.pdf`
//! (`paper::pdf_on_disk_name`, used here) vs `.lxproj` archive
//! `{source_id}_v{version}.pdf`. Never unify.

use crate::error::{CoreError, Result};
use crate::models::{PaperMetadata, Status};
use crate::service::paper::pdf_on_disk_name;
use crate::storage::queries::{paper as store, project as proj_store};
use rusqlite::Connection;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Serializes the pre-existence check + insert in `import_pdf` so two concurrent
/// imports of the same upstream paper can't race on check-then-upsert. Mirrors
/// Python's `threading.Lock`.
// ponytail: process-global lock (single-process only, like the Python port).
// Multi-process deployments need external serialization; revisit only if core
// ever runs multi-process against one DB file.
static IMPORT_ROOT_LOCK: Mutex<()> = Mutex::new(());

/// Result of a successful `import_pdf` (Python `PaperImportResult`, defined in
/// `service/paper.py` — a service result, not a storage model).
#[derive(Debug, Clone, Serialize)]
pub struct PaperImportResult {
    pub source_id: String,
    pub title: String,
}

/// State threaded through the import body so the rollback matrix in `import_pdf`
/// can see exactly what was written. Each flag gates one rollback cell.
#[derive(Default)]
struct ImportState {
    /// Canonical on-disk path we wrote (or would have written) the PDF to.
    final_path: Option<PathBuf>,
    /// Root did NOT pre-exist → this import created it (rollback may hard-delete).
    inserted_new_root: bool,
    /// This exact version did NOT pre-exist → a PAPER row is new.
    inserted_new_version: bool,
    /// We adopted a soft-deleted root; `save_paper_metadata` auto-restored it, so
    /// rollback must re-soft-delete to restore prior state.
    restored_deleted_root: bool,
    /// Adopting + the version's PDF already sits at the canonical path → preserve
    /// the user's copy, don't overwrite (no file write happened).
    pre_existing_pdf_on_disk: bool,
    /// We actually wrote a fresh file at `final_path` (distinct from "inserted a
    /// row": a same-version adopt with NULL PDF_PATH writes a file but no new row).
    wrote_final_path: bool,
    source_id: Option<String>,
    version: Option<i64>,
}

/// Save a PDF to disk, extract its metadata (via `resolve`), persist to the DB,
/// and optionally link it to a project. Returns the imported `(source_id, title)`.
///
/// Dedupe: when `resolve` returns an `external` identity already in PAPER_ROOTS,
/// the import ADOPTS it instead of minting a new `local:<sha>` root. Adopting a
/// soft-deleted root auto-restores it; a later failure re-soft-deletes so a
/// failed import never permanently un-trashes a root.
///
/// Rollback policy on storage/FS failure (the matrix):
///   - Brand-new paper (root didn't exist): hard_delete under the import lock,
///     but only if no concurrent import has since written a pdf_path for this
///     version (re-check under the lock).
///   - New version of an existing paper: leave the orphan PAPER row in place
///     (deleting it could destroy pre-existing versions).
///   - Re-import of an existing version: row left as-is; the orphan file written
///     by the failed import is unlinked (the preserve-existing branch writes
///     nothing, so there's nothing to clean).
///   - Adopted into a soft-deleted root: re-soft-delete to restore prior state.
///
/// Project linking: the membership guard runs before any import work (missing →
/// `ProjectNotFound`, deleted → `ProjectDeleted`), and again at link time — a
/// project deleted mid-import surfaces there as `PaperLink`, with the paper
/// already saved.
pub fn import_pdf<R>(
    conn: &mut Connection,
    pdf_dir: &Path,
    content: &[u8],
    project_id: Option<i64>,
    resolve: R,
) -> Result<PaperImportResult>
where
    R: Fn(&[u8]) -> Result<(PaperMetadata, Option<(String, i64)>)>,
{
    // Pre-import membership guard: fail before mutating the library.
    if let Some(pid) = project_id {
        ensure_membership_writable(conn, pid)?;
    }

    fs::create_dir_all(pdf_dir)
        .map_err(|e| CoreError::Internal(format!("import_pdf: mkdir {pdf_dir:?} failed: {e}")))?;
    let tmp_path = pdf_dir.join(format!("_upload_{}.pdf", unique_token()));
    fs::write(&tmp_path, content).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        CoreError::Internal(format!("import_pdf: write temp PDF failed: {e}"))
    })?;

    // Metadata extraction (Python: caught + re-raised as PdfImportError). This is
    // BEFORE any DB write, so its only cleanup is the temp file — distinct from
    // the rollback matrix below.
    let (meta, external) = match resolve(content) {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(CoreError::PdfImport(e.to_string()));
        }
    };
    let title = meta.title.clone();

    let mut st = ImportState::default();
    let sid = match import_body(conn, pdf_dir, &tmp_path, meta, external, &mut st) {
        Ok(sid) => sid,
        Err(e) => {
            rollback(conn, &tmp_path, &st);
            return Err(e);
        }
    };

    if let Some(pid) = project_id {
        if let Err(e) = link_imported(conn, pid, &sid) {
            return Err(match e {
                CoreError::ProjectNotFound | CoreError::ProjectDeleted(_) => CoreError::PaperLink(
                    format!("paper {sid} was imported but could not be linked to project {pid}: {e}"),
                ),
                other => other,
            });
        }
    }

    Ok(PaperImportResult { source_id: sid, title })
}

/// `import_pdf` with the production resolver wired in. Async because the resolver
/// (`sources::pdf_metadata::resolve_pdf_metadata`) hits the network for arXiv/DOI/
/// CrossRef enrichment; `data_dir` threads to arXiv's rate-limit file. We resolve
/// FIRST, then hand the already-resolved `(meta, external)` to the sync `import_pdf`
/// via a closure (its bytes arg is ignored) — so `import_pdf`'s seam, and the
/// rollback-matrix tests over it, stay exactly as they are.
pub async fn import_pdf_default(
    conn: &mut Connection,
    pdf_dir: &Path,
    content: &[u8],
    project_id: Option<i64>,
    data_dir: &Path,
) -> Result<PaperImportResult> {
    let resolved = crate::sources::pdf_metadata::resolve_pdf_metadata(content, data_dir).await?;
    import_pdf(conn, pdf_dir, content, project_id, |_| Ok(resolved.clone()))
}

/// The DB + FS write section. Any error here triggers the rollback matrix in
/// `import_pdf`, so `st` is filled incrementally to record exactly what happened.
/// Returns the resolved source_id on success.
fn import_body(
    conn: &mut Connection,
    pdf_dir: &Path,
    tmp_path: &Path,
    mut meta: PaperMetadata,
    external: Option<(String, i64)>,
    st: &mut ImportState,
) -> Result<String> {
    {
        // Serialize check-then-upsert against concurrent imports of the same paper.
        let _guard = IMPORT_ROOT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        // If enrichment matched an upstream root already in PAPER_ROOTS, adopt
        // that identity instead of creating a new local:<sha> root.
        if let Some((ext_id, ext_version)) = external {
            if let Some(existing) = store::get_paper_root(conn, &ext_id)? {
                if existing.status == "deleted" {
                    // save_paper_metadata's ensure_paper_root will auto-restore;
                    // record it so rollback can re-trash.
                    st.restored_deleted_root = true;
                }
                meta.source_id = ext_id;
                meta.version = ext_version;
            }
        }

        let pre_existing_root = store::get_paper_root(conn, &meta.source_id)?.is_some();
        let pre_existing_version =
            store::get_paper(conn, &meta.source_id, Some(meta.version))?.is_some();
        // Adopting + the canonical PDF already on disk → preserve the user's copy.
        st.pre_existing_pdf_on_disk = pre_existing_version
            && pdf_dir
                .join(pdf_on_disk_name(&meta.source_id, meta.version))
                .exists();

        let (sid, ver) = store::save_paper_metadata(conn, &meta, None)?;
        st.source_id = Some(sid);
        st.version = Some(ver);
        st.inserted_new_root = !pre_existing_root;
        st.inserted_new_version = !pre_existing_version;
    }

    let sid = st.source_id.clone().expect("set under lock above");
    let ver = st.version.expect("set under lock above");
    let final_path = pdf_dir.join(pdf_on_disk_name(&sid, ver));
    st.final_path = Some(final_path.clone());

    if st.pre_existing_pdf_on_disk {
        // Dedupe: keep the existing PDF, drop the upload.
        let _ = fs::remove_file(tmp_path);
    } else {
        fs::rename(tmp_path, &final_path)
            .map_err(|e| CoreError::Internal(format!("import_pdf: move PDF into place failed: {e}")))?;
        st.wrote_final_path = true;
        store::mark_pdf_saved(conn, &sid, &final_path.to_string_lossy(), ver)?;
    }
    Ok(sid)
}

/// Roll back the right rows/files after a failed import (the matrix). Best-effort:
/// each step ignores its own error (Python logs and continues — a failed rollback
/// must not mask the original failure).
fn rollback(conn: &mut Connection, tmp_path: &Path, st: &ImportState) {
    let _ = fs::remove_file(tmp_path);

    // restored_deleted_root and inserted_new_root are mutually exclusive: the
    // former needs the root to have pre-existed (deleted), the latter needs it
    // not to have existed at all.
    if st.restored_deleted_root {
        if let Some(sid) = &st.source_id {
            // Adopt auto-restored a trashed root, then the import failed — re-trash.
            let _ = store::soft_delete_paper(conn, sid);
        }
    } else if st.inserted_new_root {
        if let Some(sid) = &st.source_id {
            // Re-check under the lock: only delete file + row if no concurrent
            // import has since committed a pdf_path. Same guard for both keeps
            // file and row from splitting brain.
            let _guard = IMPORT_ROOT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            if let Ok(Some(row)) = store::get_paper(conn, sid, st.version) {
                if row.pdf_path.is_none() {
                    if let Some(fp) = &st.final_path {
                        let _ = fs::remove_file(fp);
                    }
                    let _ = store::hard_delete_paper(conn, sid);
                }
            }
        }
    }

    // Independent file cleanup: if we wrote a fresh file and the inserted_new_root
    // branch above isn't handling it (it has its own lock-guarded unlink), remove
    // the orphan. Catches the restored_deleted_root + same-version cell where
    // PDF_PATH was NULL — the re-soft-delete won't unlink, but the file is real.
    if st.wrote_final_path && !st.inserted_new_root {
        if let Some(fp) = &st.final_path {
            let _ = fs::remove_file(fp);
        }
    }
    // Python also logs an orphan-row warning when inserted_new_version &&
    // !inserted_new_root (a stranded new PAPER row left to protect sibling
    // versions). No logger is wired into core yet, so it's elided — the row IS
    // deliberately left in place either way.
}

// ── project-membership guards ────────────────────────────────────────────────
// Ports of service/project.py::{ensure_membership_writable, link_imported}.
// The Rust project SERVICE is still an empty stub, so these are composed from the
// project STORAGE layer here rather than reaching into a service that doesn't
// exist. They are private and import-only.
// ponytail: inline until service::project lands; then import_pdf should call
// service::project::{ensure_membership_writable, link_imported} and these go away.

/// Apply the membership-write guards without writing: missing → ProjectNotFound,
/// soft-deleted → ProjectDeleted.
fn ensure_membership_writable(conn: &Connection, project_fk: i64) -> Result<()> {
    match proj_store::get_project(conn, project_fk, false)? {
        None => Err(CoreError::ProjectNotFound),
        Some(p) if p.status == Status::Deleted => {
            Err(CoreError::ProjectDeleted("cannot update a deleted project".into()))
        }
        Some(_) => Ok(()),
    }
}

/// Link a just-imported paper to a project (same write path as add_papers).
/// Re-applies the guard (the project may have been deleted since the pre-import
/// check). An id that doesn't resolve to a root is a no-op (Python logs a warning;
/// no logger here, and the caller has no user to report it to).
fn link_imported(conn: &mut Connection, project_fk: i64, source_id: &str) -> Result<()> {
    ensure_membership_writable(conn, project_fk)?;
    if let Some(root) = store::get_paper_root(conn, source_id)? {
        proj_store::add_papers(conn, project_fk, &[root.source_fk])?;
    }
    Ok(())
}

/// Process-unique token for the temp upload filename. Avoids a `uuid` dependency.
// ponytail: pid+nanos+counter is collision-free within a process; swap for uuid
// only if cross-process temp-name collisions ever matter (they don't — same dir,
// same process owns the rename).
fn unique_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}_{}_{}", std::process::id(), nanos, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::paper;
    use crate::storage::{db::open_in_memory, init_db};
    use chrono::NaiveDate;
    use tempfile::tempdir;

    fn mem() -> Connection {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    fn meta(source_id: &str, version: i64) -> PaperMetadata {
        PaperMetadata {
            source_id: source_id.into(),
            version,
            title: format!("Title of {source_id} v{version}"),
            authors: vec!["Alice".into()],
            published: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            updated: None,
            summary: "s".into(),
            category: Some("cs.LG".into()),
            categories: Some(vec!["cs.LG".into()]),
            doi: None,
            journal_ref: None,
            comment: None,
            url: None,
            tags: None,
            source: Some("arxiv".into()),
        }
    }

    /// Resolver that hands back a fixed (meta, external).
    fn resolver(
        m: PaperMetadata,
        external: Option<(String, i64)>,
    ) -> impl Fn(&[u8]) -> Result<(PaperMetadata, Option<(String, i64)>)> {
        move |_| Ok((m.clone(), external.clone()))
    }

    #[test]
    fn happy_path_new_paper_writes_db_and_file() {
        let mut conn = mem();
        let dir = tempdir().unwrap();
        let res = import_pdf(
            &mut conn,
            dir.path(),
            b"%PDF-1.4 fake",
            None,
            resolver(meta("local:abc", 1), None),
        )
        .unwrap();

        assert_eq!(res.source_id, "local:abc");
        assert_eq!(res.title, "Title of local:abc v1");

        let p = paper::get(&conn, &paper::Paper { source_id: Some("local:abc".into()), version: Some(1), ..Default::default() })
            .unwrap()
            .unwrap();
        assert!(p.has_pdf);
        let final_path = dir.path().join("local_abcv1.pdf");
        assert_eq!(p.pdf_path.as_deref(), Some(final_path.to_string_lossy().as_ref()));
        assert!(final_path.exists());
        // Temp upload cleaned up; only the canonical file remains.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().starts_with("_upload_"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn resolve_failure_is_pdf_import_and_leaves_nothing() {
        let mut conn = mem();
        let dir = tempdir().unwrap();
        let bad = |_: &[u8]| Err(CoreError::Internal("corrupt pdf".into()));
        let err = import_pdf(&mut conn, dir.path(), b"junk", None, bad).unwrap_err();
        assert!(matches!(err, CoreError::PdfImport(_)));
        // No paper saved, temp file cleaned up (dir empty).
        assert!(paper::list_papers(&conn, false, None, 0, None).unwrap().is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn adopt_existing_active_root_dedupes_identity() {
        let mut conn = mem();
        let dir = tempdir().unwrap();
        // Pre-existing arxiv root + v1.
        paper::save_paper_metadata(&mut conn, &meta("arxiv:2204.0001", 1), None).unwrap();

        // Import a PDF whose local meta is local:xyz but external matches the arxiv root.
        let res = import_pdf(
            &mut conn,
            dir.path(),
            b"pdf",
            None,
            resolver(meta("local:xyz", 1), Some(("arxiv:2204.0001".into(), 1))),
        )
        .unwrap();
        // Adopted the arxiv identity, not the local one.
        assert_eq!(res.source_id, "arxiv:2204.0001");
        // No local:xyz root was created.
        assert!(store::get_paper_root(&conn, "local:xyz").unwrap().is_none());
        // The arxiv version now has a PDF on disk.
        assert!(dir.path().join("arxiv_2204.0001v1.pdf").exists());
    }

    #[test]
    fn reimport_preserves_existing_pdf_on_disk() {
        let mut conn = mem();
        let dir = tempdir().unwrap();
        paper::save_paper_metadata(&mut conn, &meta("arxiv:keep", 1), None).unwrap();
        // A PDF already sits at the canonical path with known bytes.
        let canonical = dir.path().join("arxiv_keepv1.pdf");
        fs::write(&canonical, b"ORIGINAL").unwrap();

        import_pdf(
            &mut conn,
            dir.path(),
            b"NEW UPLOAD",
            None,
            resolver(meta("arxiv:keep", 1), Some(("arxiv:keep".into(), 1))),
        )
        .unwrap();

        // Existing copy preserved, upload dropped (not overwritten).
        assert_eq!(fs::read(&canonical).unwrap(), b"ORIGINAL");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    // Force a post-save failure by pre-creating a DIRECTORY at the canonical PDF
    // path: `fs::rename(tmp -> dir)` fails. Valid only when pre_existing_version
    // is false (else the preserve-existing branch short-circuits the write).
    fn block_final_path(dir: &Path, name: &str) {
        fs::create_dir(dir.join(name)).unwrap();
    }

    #[test]
    fn rollback_brand_new_root_hard_deletes() {
        let mut conn = mem();
        let dir = tempdir().unwrap();
        block_final_path(dir.path(), "local_newv1.pdf");

        let err = import_pdf(
            &mut conn,
            dir.path(),
            b"pdf",
            None,
            resolver(meta("local:new", 1), None),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Internal(_)));

        // Brand-new root with NULL pdf_path → hard-deleted: paper + root gone.
        assert!(paper::get(&conn, &paper::Paper { source_id: Some("local:new".into()), ..Default::default() }).unwrap().is_none());
        assert!(store::get_paper_root(&conn, "local:new").unwrap().is_none());
        // Temp upload cleaned up.
        let uploads = fs::read_dir(dir.path()).unwrap().filter(|e| {
            e.as_ref().unwrap().file_name().to_string_lossy().starts_with("_upload_")
        });
        assert_eq!(uploads.count(), 0);
    }

    #[test]
    fn rollback_restored_deleted_root_re_soft_deletes() {
        let mut conn = mem();
        let dir = tempdir().unwrap();
        // A soft-deleted root with NO version (so pre_existing_version is false
        // and the failure-injection dir doesn't trip preserve-existing).
        paper::ensure_paper_root(&mut conn, "arxiv:dead").unwrap();
        store::soft_delete_paper(&mut conn, "arxiv:dead").unwrap();
        assert!(paper::is_paper_deleted(&conn, "arxiv:dead").unwrap());

        block_final_path(dir.path(), "arxiv_deadv1.pdf");

        let err = import_pdf(
            &mut conn,
            dir.path(),
            b"pdf",
            None,
            resolver(meta("local:ignored", 1), Some(("arxiv:dead".into(), 1))),
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Internal(_)));

        // save_paper_metadata auto-restored the root; rollback re-trashed it.
        assert!(paper::is_paper_deleted(&conn, "arxiv:dead").unwrap());
    }

    #[tokio::test]
    async fn import_pdf_default_uses_real_resolver_and_mints_local_root() {
        // Wiring check: the convenience entry resolves first (junk bytes extract
        // no arXiv/DOI/title, so enrichment makes no network call), then mints a
        // deterministic local:<sha> identity (None external).
        let mut conn = mem();
        let dir = tempdir().unwrap();
        let data_dir = tempdir().unwrap();
        let res = import_pdf_default(&mut conn, dir.path(), b"%PDF-1.4 junk", None, data_dir.path())
            .await
            .unwrap();
        assert!(res.source_id.starts_with("local:"));
        let p = paper::get(
            &conn,
            &paper::Paper { source_id: Some(res.source_id.clone()), ..Default::default() },
        )
        .unwrap()
        .unwrap();
        assert!(p.has_pdf);
    }

    #[test]
    fn project_link_membership_guards() {
        let mut conn = mem();
        let dir = tempdir().unwrap();

        // Missing project → ProjectNotFound, before any import work.
        let err = import_pdf(&mut conn, dir.path(), b"pdf", Some(999), resolver(meta("local:p1", 1), None)).unwrap_err();
        assert!(matches!(err, CoreError::ProjectNotFound));
        assert!(paper::list_papers(&conn, false, None, 0, None).unwrap().is_empty());

        // Deleted project → ProjectDeleted, also before import work.
        conn.execute("INSERT INTO PROJECT (NAME, STATUS) VALUES ('Gone', 'deleted')", []).unwrap();
        let pid = conn.last_insert_rowid();
        let err = import_pdf(&mut conn, dir.path(), b"pdf", Some(pid), resolver(meta("local:p2", 1), None)).unwrap_err();
        assert!(matches!(err, CoreError::ProjectDeleted(_)));
        assert!(paper::list_papers(&conn, false, None, 0, None).unwrap().is_empty());

        // Active project → paper imported AND linked.
        conn.execute("INSERT INTO PROJECT (NAME, STATUS) VALUES ('Live', 'active')", []).unwrap();
        let pid = conn.last_insert_rowid();
        let res = import_pdf(&mut conn, dir.path(), b"pdf", Some(pid), resolver(meta("local:p3", 1), None)).unwrap();
        let root = store::get_paper_root(&conn, &res.source_id).unwrap().unwrap();
        assert_eq!(proj_store::get_paper_project_fks(&conn, root.source_fk).unwrap(), vec![pid]);
    }
}
