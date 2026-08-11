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

use std::path::{Path, PathBuf};

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

/// Total size of all managed `*.pdf` files in `pdf_dir`, in bytes. `0` if the dir is
/// absent. Files that vanish mid-scan are skipped (Python ignores `FileNotFoundError`).
/// Also the basis of the `pdf_save_limit_mb` total-storage cap (see
/// `paper_import::check_pdf_storage_quota` and `download_pdf` below).
pub fn pdf_storage_bytes(pdf_dir: &Path) -> u64 {
    std::fs::read_dir(pdf_dir).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".pdf"))
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    })
}

/// `pdf_storage_bytes` in MB. Port of `files.pdf_storage_mb`.
pub fn pdf_storage_mb(pdf_dir: &Path) -> f64 {
    pdf_storage_bytes(pdf_dir) as f64 / (1024.0 * 1024.0)
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
    let _ = std::fs::remove_file(&target);
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
    crate::sources::download::download_pdf(&dest, url, remaining).await
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

        // Empty dir → 0.0.
        assert_eq!(pdf_storage_mb(pdf_dir), 0.0);

        // 1 MB + 0.5 MB of pdf, plus a non-pdf that must be ignored.
        write_pdf(pdf_dir, "a v1.pdf", 1024 * 1024);
        write_pdf(pdf_dir, "bv1.pdf", 512 * 1024);
        write_pdf(pdf_dir, "notes.txt", 9_000_000);
        let mb = pdf_storage_mb(pdf_dir);
        assert!((mb - 1.5).abs() < 1e-9, "expected ~1.5 MB, got {mb}");
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
