//! files service — Rust port of `service/files.py` (pure-FS parts). Plan §5.2.
//!
//! DI: every fn takes the resolved managed `pdf_dir: &Path` as a parameter; this
//! module NEVER reads `config::pdf_dir()` itself — the binary layer resolves it and
//! passes it in (mirrors Python's `storage.paths.pdf_dir()`, but injected for testing).
//!
//! `managed_pdf_dir()` from Python is dropped: under DI the caller already holds the
//! resolved path, so the wrapper is a redundant identity (D17 — no forwarding wrappers).
//!
//! `download_pdf` (the SSRF-safe HTTP downloader) resolves the managed dest under the DI'd
//! `pdf_dir` and delegates the network/SSRF work to `sources::download`. See below.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::error::{CoreError, Result};

/// Standard managed PDF location for a (paper_id, version): `<pdf_dir>/<safe_id>v<n>.pdf`.
fn pdf_file(pdf_dir: &Path, paper_id: &str, version: i64) -> PathBuf {
    pdf_dir.join(crate::service::paper::pdf_on_disk_name(paper_id, version))
}

/// "Where is this paper's PDF" wire envelope — `pdf path`/`pdf download` (CLI),
/// `get_pdf_path`/`download_pdf` (MCP), and `GET /api/papers/{id}/pdf-path` all
/// emit this shape.
#[derive(Debug, serde::Serialize)]
pub struct PdfLocation {
    pub source_id: String,
    pub version: i64,
    pub path: Option<PathBuf>,
}

/// Local path to a paper's PDF if it exists, else `None`. Checks `custom_path` first
/// (the value stored on the paper row), then the standard managed location.
/// Port of `files.pdf_path` — returns the path only when the file is actually present.
pub fn pdf_path(
    pdf_dir: &Path,
    paper_id: &str,
    version: i64,
    custom_path: Option<&str>,
) -> Option<PathBuf> {
    if let Some(c) = custom_path {
        let p = Path::new(c);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    let std = pdf_file(pdf_dir, paper_id, version);
    std.is_file().then_some(std)
}

/// Cached size of every managed `*.pdf` per `pdf_dir`, keyed by the dir path exactly
/// as the caller passes it (DI resolves one spelling per process). Lazily seeded by a
/// full walk on first read, then kept current by [`note_pdf_written`]/
/// [`note_pdf_removed`] on every write/delete seam that core (and the server's two
/// direct-write routes) own. Per-file, not a running delta: a re-write of the same
/// dest replaces its entry, so overlapping writes can't double-count the total.
static PDF_STORAGE: std::sync::LazyLock<
    std::sync::Mutex<HashMap<PathBuf, HashMap<std::ffi::OsString, u64>>>,
> = std::sync::LazyLock::new(Default::default);

/// The full scan behind the cache: name → size of all `*.pdf` files directly in
/// `pdf_dir`, empty if the dir is absent. Files that vanish mid-scan are skipped
/// (Python ignores `FileNotFoundError`).
pub(crate) fn walk_pdf_files(pdf_dir: &Path) -> HashMap<std::ffi::OsString, u64> {
    std::fs::read_dir(pdf_dir).map_or_else(
        |_| HashMap::new(),
        |entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".pdf"))
                .filter_map(|e| Some((e.file_name(), e.metadata().ok()?.len())))
                .collect()
        },
    )
}

#[cfg(test)]
fn walk_pdf_storage_bytes(pdf_dir: &Path) -> u64 {
    walk_pdf_files(pdf_dir).values().sum()
}

/// Total size of all managed `*.pdf` files in `pdf_dir`, in bytes — the basis of the
/// `pdf_save_limit_mb` total-storage cap (see `paper_import::check_pdf_storage_quota`
/// and `download_pdf` below). Walks the dir ONCE per process (lazy init), then serves
/// the running total maintained by the write/delete seams.
///
/// ponytail: files changed outside this process's seams (manual deletes in the
/// folder, crash orphans, an init walk racing a concurrent write, and writes by a
/// sibling linxiv process — CLI/MCP against the same library) drift the total until
/// process restart, when the next walk re-seeds it; move the total into the DB if
/// cross-process accuracy ever matters.
pub fn pdf_storage_bytes(pdf_dir: &Path) -> u64 {
    let mut cache = PDF_STORAGE.lock().unwrap_or_else(|p| p.into_inner());
    cache
        .entry(pdf_dir.to_path_buf())
        .or_insert_with(|| walk_pdf_files(pdf_dir))
        .values()
        .sum()
}

/// Record a managed PDF write: `dest` (built as `pdf_dir.join(name)`, so its parent
/// keeps the readers' `pdf_dir` spelling) now holds `size` bytes. Sets the file's
/// cache entry — last writer wins, never accumulates. A no-op until the dir's cache
/// has been lazily seeded (the eventual first walk sees the file anyway) and for
/// non-`.pdf` names (the walk wouldn't count them).
pub fn note_pdf_written(dest: &Path, size: u64) {
    let (Some(dir), Some(name)) = (dest.parent(), dest.file_name()) else {
        return;
    };
    if !name.to_string_lossy().ends_with(".pdf") {
        return;
    }
    if let Some(files) = PDF_STORAGE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_mut(dir)
    {
        files.insert(name.to_os_string(), size);
    }
}

/// Forget `name` from `pdf_dir`'s cache after a managed delete.
fn forget_pdf(pdf_dir: &Path, name: &std::ffi::OsStr) {
    if let Some(files) = PDF_STORAGE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_mut(pdf_dir)
    {
        files.remove(name);
    }
}

/// Best-effort remove of a managed PDF built as `pdf_dir.join(name)`, dropping it
/// from the cached storage total on success. For cleanup paths that hold the exact
/// dest path (import rollback, failed attach/share saves).
pub fn remove_pdf_counted(path: &Path) {
    if std::fs::remove_file(path).is_ok() {
        if let (Some(dir), Some(name)) = (path.parent(), path.file_name()) {
            forget_pdf(dir, name);
        }
    }
}

/// Rename a managed PDF, moving its cache entry with it: the old name is
/// forgotten and the new one recorded, under ONE lock acquisition so a
/// concurrent `pdf_storage_bytes` (quota check) never observes the
/// forgotten-but-not-yet-recorded gap. Size is re-stat'd post-rename, falling
/// back to the old cached size (a rename preserves length) if the stat fails.
/// Cache keys use the caller's `pdf_dir` spelling — not the raw parents of
/// `from`/`to`, which may spell the same dir differently (DB-stored paths,
/// symlinks) and would strand a phantom entry (same convention as
/// `delete_pdf`). Both files must live in `pdf_dir`; callers gate on that.
/// For seams that re-home a managed file (paper merge and its undo).
pub fn rename_pdf_counted(pdf_dir: &Path, from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)?;
    let stat_size = std::fs::metadata(to).ok().map(|m| m.len());
    let mut cache = PDF_STORAGE.lock().unwrap_or_else(|p| p.into_inner());
    let mut old_size = None;
    if let (Some(files), Some(name)) = (cache.get_mut(pdf_dir), from.file_name()) {
        old_size = files.remove(name);
    }
    if let Some(name) = to.file_name() {
        if name.to_string_lossy().ends_with(".pdf") && cache.contains_key(pdf_dir) {
            match stat_size.or(old_size) {
                Some(size) => {
                    cache
                        .get_mut(pdf_dir)
                        .expect("checked contains_key above")
                        .insert(name.to_os_string(), size);
                }
                // Size unknowable (stat failed, old name never cached): drop
                // the dir's cache so the next quota check re-walks instead of
                // silently missing this file until restart.
                None => {
                    cache.remove(pdf_dir);
                }
            }
        }
    }
    Ok(())
}

/// `pdf_storage_bytes` in MB. Port of `files.pdf_storage_mb`.
pub fn pdf_storage_mb(pdf_dir: &Path) -> f64 {
    pdf_storage_bytes(pdf_dir) as f64 / (1024.0 * 1024.0)
}

/// Saved-PDF listing rows from `paper::list_pdf_papers` output: stat each paper's
/// on-disk PDF (dropping rows whose file is missing), sorted size desc then
/// source_id asc — matches app.py (`_LIST_PDFS_SQL` orders by source_id, then a
/// stable sort by size_bytes desc keeps that as the tiebreak). Uncapped; the
/// route and MCP cap at 200 for the UI, the CLI lists everything. Backs
/// `GET /api/pdfs`, CLI `pdf list`, and MCP `list_pdfs`.
pub fn saved_pdf_sizes(pdf_dir: &Path, papers: Vec<crate::models::PaperDetails>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for p in papers {
        let Some(path) = pdf_path(pdf_dir, &p.source_id, p.version, p.pdf_path.as_deref()) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        out.push(json!({
            "source_id": p.source_id,
            "source_fk": p.source_fk,
            "title": p.title,
            "version": p.version,
            "size_bytes": meta.len(),
        }));
    }
    out.sort_by(|a, b| {
        b["size_bytes"]
            .as_u64()
            .cmp(&a["size_bytes"].as_u64())
            .then_with(|| a["source_id"].as_str().cmp(&b["source_id"].as_str()))
    });
    out
}

/// Delete a PDF only if it resolves to a location inside the managed `pdf_dir`. Returns
/// `true` if the path is inside the managed dir (deleting it if present; a missing file
/// is a no-op success, matching Python's `unlink(missing_ok=True)`), `false` if the path
/// escapes the managed dir. SECURITY BOUNDARY — port of `files.delete_pdf`: never let a
/// caller-supplied path delete a file outside `pdf_dir`.
pub fn delete_pdf(pdf_dir: &Path, path: &str) -> bool {
    // Canonicalize the managed root (resolves symlinks + `..`). If it can't be resolved
    // (dir absent), nothing is managed → refuse. Conservative for a trust boundary.
    let managed = match std::fs::canonicalize(pdf_dir) {
        Ok(m) => m,
        Err(_) => return false,
    };
    // Resolve the target the same way. std::fs::canonicalize requires existence, so for a
    // not-yet-existing file we resolve its parent and re-attach the name (Python's
    // Path.resolve() resolves lexically without requiring the file to exist).
    let target = match std::fs::canonicalize(path) {
        Ok(t) => t,
        Err(_) => {
            let p = Path::new(path);
            match (p.parent(), p.file_name()) {
                (Some(parent), Some(name)) => match std::fs::canonicalize(parent) {
                    Ok(cp) => cp.join(name),
                    Err(_) => return false, // parent unresolvable → not provably inside
                },
                _ => return false,
            }
        }
    };
    if !target.starts_with(&managed) {
        return false;
    }
    // Inside the boundary: remove if present, ignore a missing file (idempotent delete).
    // A removed direct child comes out of the cached storage total, keyed by `pdf_dir`
    // as the readers spell it (the canonicalized parent may not match); nested or
    // non-.pdf targets were never in it.
    if std::fs::remove_file(&target).is_ok() && target.parent() == Some(managed.as_path()) {
        if let Some(name) = target.file_name() {
            forget_pdf(pdf_dir, name);
        }
    }
    true
}

/// SSRF-safe HTTP downloader: resolve the managed dest under the DI'd `pdf_dir`, then hand off
/// to `sources::download::download_pdf` (scheme allowlist, host-resolves-to-public check, per-hop
/// redirect re-check, content-type + size caps, atomic tmp→dest rename). Port of
/// `files.download_pdf`.
///
/// `max_total_bytes` is the caller-resolved `pdf_save_limit_mb` cap (`config::UserSettings::
/// pdf_save_limit_bytes`, DI'd — this module never reads config itself): a TOTAL-storage cap
/// across every managed PDF, not a per-file one. The allowance handed to the downloader is
/// whatever the PDFs already in `pdf_dir` leave of it; the fixed `sources::download`
/// SSRF/memory ceiling still applies on top (the smaller of the two wins).
/// An already-downloaded dest is returned as-is — re-fetching writes nothing new, so the
/// quota never blocks it.
pub async fn download_pdf(
    pdf_dir: &Path,
    paper_id: &str,
    version: i64,
    url: &str,
    max_total_bytes: u64,
) -> Result<PathBuf> {
    let dest = pdf_file(pdf_dir, paper_id, version);
    if dest.exists() {
        return Ok(dest); // idempotent re-return (mirrors sources::download) — no quota check
    }
    let existing = pdf_storage_bytes(pdf_dir);
    let remaining = max_total_bytes.saturating_sub(existing);
    if remaining == 0 {
        return Err(CoreError::PdfTooLarge(format!(
            "PDF storage is full: {existing} bytes already saved of the {max_total_bytes} byte total limit (pdf_save_limit_mb)."
        )));
    }
    let out = crate::sources::download::download_pdf(&dest, url, remaining).await?;
    note_pdf_written(&out, std::fs::metadata(&out).map_or(0, |m| m.len()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_pdf(dir: &Path, name: &str, bytes: usize) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, vec![0u8; bytes]).unwrap();
        p
    }

    /// Wire-shape pin: `{"source_id", "version", "path"}`, path nullable.
    #[test]
    fn pdf_location_wire_shape() {
        let loc = PdfLocation {
            source_id: "arxiv:1".into(),
            version: 2,
            path: None,
        };
        assert_eq!(
            serde_json::to_string(&loc).unwrap(),
            r#"{"source_id":"arxiv:1","version":2,"path":null}"#
        );
    }

    #[test]
    fn pdf_path_prefers_custom_then_standard_then_none() {
        let dir = tempfile::tempdir().unwrap();
        let pdf_dir = dir.path();

        // Nothing on disk yet → None.
        assert!(pdf_path(pdf_dir, "2204.00001", 1, None).is_none());

        // Standard managed file present → returned.
        let std = write_pdf(pdf_dir, "2204.00001v1.pdf", 10);
        assert_eq!(pdf_path(pdf_dir, "2204.00001", 1, None), Some(std.clone()));

        // custom_path takes priority when it is an existing file.
        let custom = write_pdf(pdf_dir, "elsewhere.pdf", 5);
        let custom_s = custom.to_str().unwrap();
        assert_eq!(
            pdf_path(pdf_dir, "2204.00001", 1, Some(custom_s)),
            Some(custom.clone())
        );

        // A custom_path that does not exist falls back to the standard file.
        assert_eq!(
            pdf_path(pdf_dir, "2204.00001", 1, Some("/no/such/file.pdf")),
            Some(std)
        );

        // Old-style id with a slash maps to the sanitised filename.
        write_pdf(pdf_dir, "math.GT_0309136v2.pdf", 3);
        assert!(pdf_path(pdf_dir, "math.GT/0309136", 2, None).is_some());
    }

    #[test]
    fn pdf_storage_mb_sums_only_pdfs() {
        let dir = tempfile::tempdir().unwrap();
        let pdf_dir = dir.path();

        // Missing dir → 0.0.
        assert_eq!(pdf_storage_mb(&pdf_dir.join("nope")), 0.0);

        // Empty dir → 0.0 (own dir: the total is cached per dir on first read).
        assert_eq!(pdf_storage_mb(tempfile::tempdir().unwrap().path()), 0.0);

        // 1 MB + 0.5 MB of pdf, plus a non-pdf that must be ignored.
        write_pdf(pdf_dir, "a v1.pdf", 1024 * 1024);
        write_pdf(pdf_dir, "bv1.pdf", 512 * 1024);
        write_pdf(pdf_dir, "notes.txt", 9_000_000);
        let mb = pdf_storage_mb(pdf_dir);
        assert!((mb - 1.5).abs() < 1e-9, "expected ~1.5 MB, got {mb}");
    }

    /// The cached total walks once, then tracks the service add/delete seams
    /// without re-walking — proven by an out-of-band file the cache must not see.
    #[test]
    fn pdf_storage_total_tracks_service_mutations_without_rewalk() {
        let dir = tempfile::tempdir().unwrap();
        let pdf_dir = dir.path();
        let a = write_pdf(pdf_dir, "av1.pdf", 100);
        write_pdf(pdf_dir, "bv1.pdf", 50);

        // Lazy seed walk.
        assert_eq!(pdf_storage_bytes(pdf_dir), 150);

        // Delete through the service seam → decrement, still equal to a fresh walk.
        assert!(delete_pdf(pdf_dir, a.to_str().unwrap()));
        assert_eq!(pdf_storage_bytes(pdf_dir), 50);
        assert_eq!(pdf_storage_bytes(pdf_dir), walk_pdf_storage_bytes(pdf_dir));

        // Counted cleanup helper (rollback/attach failure paths) → same.
        let c = write_pdf(pdf_dir, "cv1.pdf", 30);
        note_pdf_written(&c, 30); // as the import/attach write seams do
        assert_eq!(pdf_storage_bytes(pdf_dir), 80);
        // Re-noting the same dest replaces its entry — overlapping writes to one
        // file can never double-count the total.
        note_pdf_written(&c, 30);
        assert_eq!(pdf_storage_bytes(pdf_dir), 80);
        remove_pdf_counted(&c);
        assert_eq!(pdf_storage_bytes(pdf_dir), 50);
        assert_eq!(pdf_storage_bytes(pdf_dir), walk_pdf_storage_bytes(pdf_dir));

        // An out-of-band file is invisible to the cache (no re-walk happens)…
        write_pdf(pdf_dir, "sneakyv1.pdf", 7);
        assert_eq!(pdf_storage_bytes(pdf_dir), 50);
        // …which is exactly the ponytail drift ceiling: a fresh walk sees it.
        assert_eq!(walk_pdf_storage_bytes(pdf_dir), 57);
    }

    #[test]
    fn delete_pdf_only_inside_managed_dir() {
        let dir = tempfile::tempdir().unwrap();
        let pdf_dir = dir.path().join("pdfs");
        fs::create_dir_all(&pdf_dir).unwrap();

        // Inside the managed dir → deleted, returns true.
        let inside = write_pdf(&pdf_dir, "2204.00001v1.pdf", 4);
        assert!(delete_pdf(&pdf_dir, inside.to_str().unwrap()));
        assert!(!inside.exists());

        // A missing file *inside* the managed dir is an idempotent success.
        assert!(delete_pdf(
            &pdf_dir,
            pdf_dir.join("gone.pdf").to_str().unwrap()
        ));

        // A file OUTSIDE the managed dir is refused and left intact.
        let outside = write_pdf(dir.path(), "secret.pdf", 4);
        assert!(!delete_pdf(&pdf_dir, outside.to_str().unwrap()));
        assert!(outside.exists());

        // `..` traversal escaping the managed dir is refused, sibling untouched.
        let escape = format!("{}/../secret.pdf", pdf_dir.display());
        assert!(!delete_pdf(&pdf_dir, &escape));
        assert!(outside.exists());
    }

    #[tokio::test]
    async fn download_pdf_returns_managed_dest_when_present() {
        // Valid PDF already at the managed (pdf_dir, paper_id, version) location → returned with
        // no network call, proving the dest mapping. The full network happy-path lives in
        // sources::download's wiremock tests; the public-IP SSRF guard rejects loopback, so a
        // wiremock host can't drive the real guarded download without weakening that guard.
        let dir = tempfile::tempdir().unwrap();
        let pdf_dir = dir.path();
        let body = b"%PDF-1.7 ok".to_vec();
        fs::write(pdf_dir.join("2204.00001v3.pdf"), &body).unwrap();
        let out = download_pdf(
            pdf_dir,
            "2204.00001",
            3,
            "http://example.com/x.pdf",
            1024 * 1024 * 1024,
        )
        .await
        .unwrap();
        assert_eq!(out, pdf_dir.join("2204.00001v3.pdf"));
        assert_eq!(fs::read(&out).unwrap(), body);
    }

    #[tokio::test]
    async fn download_pdf_rejects_when_total_storage_full_before_any_network() {
        // Existing PDFs already meet the pdf_save_limit_mb quota → early PdfTooLarge,
        // proven offline: the unresolvable URL would error differently if fetched.
        let dir = tempfile::tempdir().unwrap();
        let pdf_dir = dir.path();
        write_pdf(pdf_dir, "seedv1.pdf", 100);
        let err = download_pdf(
            pdf_dir,
            "2204.00003",
            1,
            "http://example.invalid/x.pdf",
            100,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, crate::error::CoreError::PdfTooLarge(ref m) if m.contains("full")),
            "expected storage-full rejection, got {err}"
        );
        assert!(!pdf_dir.join("2204.00003v1.pdf").exists());

        // An already-downloaded dest is still returned even at a full quota.
        let body = b"%PDF-1.7 ok".to_vec();
        fs::write(pdf_dir.join("2204.00004v1.pdf"), &body).unwrap();
        let out = download_pdf(
            pdf_dir,
            "2204.00004",
            1,
            "http://example.invalid/x.pdf",
            100,
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&out).unwrap(), body);
    }

    #[tokio::test]
    async fn download_pdf_refuses_ssrf_and_leaves_no_file() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        // wiremock binds 127.0.0.1; the SSRF public-IP guard must refuse it before any body lands.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/evil.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_string("x"),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let pdf_dir = dir.path();
        let url = format!("{}/evil.pdf", server.uri());
        let err = download_pdf(pdf_dir, "2204.00002", 1, &url, 1024 * 1024 * 1024)
            .await
            .unwrap_err();
        assert!(
            matches!(err, crate::error::CoreError::Validation(ref m) if m.contains("disallowed")),
            "loopback host must be refused by the SSRF guard, got {err}"
        );
        assert!(
            !pdf_dir.join("2204.00002v1.pdf").exists(),
            "no file on a refused download"
        );
    }
}
