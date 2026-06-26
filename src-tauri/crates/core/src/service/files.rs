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

use crate::error::Result;

/// Characters Python's `_UNSAFE_FNAME_RE = [/\:*?"<>|]` strips from a paper id before
/// it becomes a filename stem. Old-style arXiv ids (`math.GT/0309136`) contain `/`.
const UNSAFE_FNAME_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Sanitise a paper id into a filename-safe stem (each unsafe char → `_`).
fn safe_name(paper_id: &str) -> String {
    paper_id
        .chars()
        .map(|c| if UNSAFE_FNAME_CHARS.contains(&c) { '_' } else { c })
        .collect()
}

/// Standard managed PDF location for a (paper_id, version): `<pdf_dir>/<safe_id>v<n>.pdf`.
fn pdf_file(pdf_dir: &Path, paper_id: &str, version: i64) -> PathBuf {
    pdf_dir.join(format!("{}v{}.pdf", safe_name(paper_id), version))
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

/// Total size of all managed `*.pdf` files in `pdf_dir`, in MB. `0.0` if the dir is
/// absent. Files that vanish mid-scan are skipped (Python ignores `FileNotFoundError`).
/// Port of `files.pdf_storage_mb`.
pub fn pdf_storage_mb(pdf_dir: &Path) -> f64 {
    let entries = match std::fs::read_dir(pdf_dir) {
        Ok(e) => e,
        Err(_) => return 0.0, // missing dir (or unreadable) → no managed storage
    };
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with(".pdf") {
            if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total as f64 / (1024.0 * 1024.0)
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
pub async fn download_pdf(
    pdf_dir: &Path,
    paper_id: &str,
    version: i64,
    url: &str,
) -> Result<PathBuf> {
    let dest = pdf_file(pdf_dir, paper_id, version);
    crate::sources::download::download_pdf(&dest, url).await
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

    #[test]
    fn safe_name_strips_unsafe_chars() {
        assert_eq!(safe_name("2204.00001"), "2204.00001"); // dots are safe
        assert_eq!(safe_name("math.GT/0309136"), "math.GT_0309136");
        assert_eq!(safe_name(r#"a:b*c?d"e<f>g|h\i"#), "a_b_c_d_e_f_g_h_i");
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
        assert!(delete_pdf(&pdf_dir, pdf_dir.join("gone.pdf").to_str().unwrap()));

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
        let out = download_pdf(pdf_dir, "2204.00001", 3, "http://example.com/x.pdf")
            .await
            .unwrap();
        assert_eq!(out, pdf_dir.join("2204.00001v3.pdf"));
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
        let err = download_pdf(pdf_dir, "2204.00002", 1, &url).await.unwrap_err();
        assert!(
            matches!(err, crate::error::CoreError::Validation(ref m) if m.contains("disallowed")),
            "loopback host must be refused by the SSRF guard, got {err}"
        );
        assert!(!pdf_dir.join("2204.00002v1.pdf").exists(), "no file on a refused download");
    }
}
