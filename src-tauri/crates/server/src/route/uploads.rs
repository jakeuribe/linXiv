//! Binary upload routes (attach/import PDF, bibtex, project preview/commit).
//! WIRE: Tauri `invoke` cannot carry multipart, so file bytes ride as base64 in
//! `file_b64`; a malformed `file_b64` is a 400 before core runs.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::Deserialize;
use serde_json::Value;

use linxiv_core::config;
use linxiv_core::error::CoreError;
use linxiv_core::service::export_import::{self, OnConflict};
use linxiv_core::service::paper::{self as svc_paper, pdf_on_disk_name};
use linxiv_core::service::paper_import;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

const MAX_PDF_BYTES: usize = 100 * 1024 * 1024;

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("PUT", ["api", "papers", id, "pdf"]) => Some(attach_pdf(state, id, ctx)),
        ("POST", ["api", "papers", "import", "pdf"]) => Some(import_pdf(state, ctx).await),
        ("POST", ["api", "papers", "import", "bibtex"]) => Some(import_bibtex(state, ctx)),
        ("POST", ["api", "projects", "import", "preview"]) => Some(import_preview(ctx)),
        ("POST", ["api", "projects", "import", "commit"]) => Some(import_commit(state, ctx)),
        _ => None,
    }
}

/// Decode the `file_b64` field's bytes; a bad base64 string is a 400 (the byte
/// payload is malformed before any handler logic runs).
fn decode_b64(s: &str) -> Result<Vec<u8>, ApiError> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| ApiError::new(400, format!("Invalid base64: {e}")))
}

/// Reject an over-limit upload from the base64 length BEFORE decoding, so a
/// malicious IPC caller can't force hundreds of MB to be decoded into memory just
/// to fail the post-decode size check. `len/4*3` is the decoded-size upper bound.
fn reject_oversized_b64(file_b64: &str, msg: &str) -> Result<(), ApiError> {
    if file_b64.len() / 4 * 3 > MAX_PDF_BYTES {
        return Err(ApiError::new(413, msg.to_string()));
    }
    Ok(())
}

#[derive(Deserialize, ts_rs::TS)]
pub struct UploadPdfBody {
    pub file_b64: String,
}

/// `PUT /api/papers/{id}/pdf` — store PDF bytes the client already fetched for a
/// saved paper. Check order: 404 paper → 413 size → 400 magic.
fn attach_pdf(state: &AppState, source_id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: UploadPdfBody = ctx.parse_body()?;
    reject_oversized_b64(&b.file_b64, "PDF exceeds size limit")?;
    let content = decode_b64(&b.file_b64)?;
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| -> Result<Value, ApiError> {
        let paper = svc_paper::get(conn, &sid_key(source_id))?
            .ok_or_else(|| CoreError::PaperNotFound(source_id.to_string()))?;
        if !content.starts_with(b"%PDF") {
            return Err(ApiError::new(400, "Not a valid PDF"));
        }
        let ver = paper.version;
        let dest = pdf_dir.join(pdf_on_disk_name(source_id, ver));
        // Containment guard: the on-disk name is sanitized so the join stays a
        // direct child of pdf_dir; a separator slipping through would change
        // the parent → 400.
        if dest.parent() != Some(pdf_dir.as_path()) {
            return Err(ApiError::new(400, "Invalid source_id"));
        }
        std::fs::create_dir_all(&pdf_dir)
            .map_err(|e| ApiError::new(500, format!("{}: {e}", pdf_dir.display())))?;
        std::fs::write(&dest, &content)
            .map_err(|e| ApiError::new(500, format!("{}: {e}", dest.display())))?;
        linxiv_core::service::files::note_pdf_written(&dest, content.len() as u64);
        let dest_str = dest.to_string_lossy().into_owned();
        if let Err(e) = svc_paper::mark_pdf_saved(conn, source_id, &dest_str, ver) {
            linxiv_core::service::files::remove_pdf_counted(&dest);
            return Err(ApiError::new(500, e.to_string()));
        }
        crate::route::to_value(&linxiv_core::models::OkReceipt { ok: true })
    })
}

#[derive(Deserialize, ts_rs::TS)]
#[ts(optional_fields = nullable)]
pub struct ImportPdfBody {
    pub file_b64: String,
    pub filename: Option<String>,
}

/// `POST /api/papers/import/pdf` — resolve metadata (network) OUTSIDE the DB
/// lock, then run the sync DB+FS import under it.
async fn import_pdf(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: ImportPdfBody = ctx.parse_body()?;
    // project_id query param links the imported paper to a project (core runs
    // the membership guard + link).
    let project_id = ctx.q_i64("project_id");
    let filename = b.filename.unwrap_or_default();
    if !filename.to_lowercase().ends_with(".pdf") {
        return Err(ApiError::new(400, "File must be a PDF"));
    }
    reject_oversized_b64(
        &b.file_b64,
        "Upload rejected: file size exceeds 100 MB limit",
    )?;
    let content = decode_b64(&b.file_b64)?;
    if !content.starts_with(b"%PDF") {
        return Err(ApiError::new(400, "File does not appear to be a valid PDF"));
    }
    let pdf_dir = state.pdf_dir.clone();
    // `pdf_save_limit_mb` — a user-configurable TOTAL-storage cap, layered under
    // the fixed 100 MB per-upload ceiling above (DI'd here; core never reads config).
    let max_pdf_bytes = config::UserSettings::load()?.pdf_save_limit_bytes();
    // ProjectNotFound → 404, ProjectDeleted/PaperLink → 400 flow through `?`. NOTE:
    // resolve_pdf_metadata degrades a pdfium extraction failure to empty metadata
    // (it never errors), so a %PDF-but-corrupt file saves a minimal paper + 200
    // rather than a 422; surfacing PdfImport from the resolver is deferred.
    // Fail-fast: a bad project_id is rejected before the network resolve; the
    // commit lock re-checks (project can vanish in between).
    state.with_conn(|conn| paper_import::precheck_import_pdf(conn, project_id))?;
    let resolved =
        paper_import::resolve_import_pdf(&pdf_dir, &content, max_pdf_bytes, &config::data_dir())
            .await?;
    let result = state.with_conn(|conn| {
        paper_import::commit_import_pdf(
            conn,
            &pdf_dir,
            &content,
            project_id,
            max_pdf_bytes,
            resolved,
        )
    })?;
    crate::route::to_value(&result)
}

#[derive(Deserialize, ts_rs::TS)]
#[ts(optional_fields = nullable)]
pub struct ImportBibtexBody {
    pub file_b64: String,
    pub project_id: Option<i64>,
}

/// `POST /api/papers/import/bibtex` — `service::paper_import` owns guard order,
/// parse and link; `?` maps ProjectNotFound → 404, the other refusals → 400.
fn import_bibtex(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: ImportBibtexBody = ctx.parse_body()?;
    let content = decode_b64(&b.file_b64)?;
    let text = String::from_utf8_lossy(&content).into_owned();
    let receipt = state.with_conn(|conn| paper_import::import_bibtex(conn, &text, b.project_id))?;
    crate::route::to_value(&receipt)
}

#[derive(Deserialize, ts_rs::TS)]
pub struct ImportPreviewBody {
    pub file_b64: String,
}

/// `POST /api/projects/import/preview` — spill the decoded bytes to a temp
/// `.lxproj` (preview takes a path); any error is a 400. Temp removed on every path.
fn import_preview(ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: ImportPreviewBody = ctx.parse_body()?;
    let content = decode_b64(&b.file_b64)?;
    let tmp = write_temp_lxproj(&content)?;
    let res = export_import::preview_import(&tmp);
    std::fs::remove_file(&tmp).ok();
    let p = res.map_err(|e| ApiError::new(400, e.to_string()))?;
    crate::route::to_value(&export_import::ImportPreviewResponse::from(p))
}

#[derive(Deserialize, ts_rs::TS)]
#[ts(optional_fields = nullable)]
pub struct ImportCommitBody {
    pub file_b64: String,
    pub on_conflict: Option<String>,
}

/// `POST /api/projects/import/commit` — `on_conflict` defaults to "merge";
/// a `ProjectImportError` is a 422, any other failure a 400.
fn import_commit(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: ImportCommitBody = ctx.parse_body()?;
    let on_conflict = match b.on_conflict.as_deref() {
        None | Some("merge") => OnConflict::Merge,
        Some("overwrite") => OnConflict::Overwrite,
        Some(_) => {
            return Err(ApiError::new(
                422,
                "on_conflict must be 'merge' or 'overwrite'",
            ))
        }
    };
    let content = decode_b64(&b.file_b64)?;
    let pdf_dir = state.pdf_dir.clone();
    let tmp = write_temp_lxproj(&content)?;
    let res =
        state.with_conn(|conn| export_import::commit_import(conn, &tmp, on_conflict, &pdf_dir));
    std::fs::remove_file(&tmp).ok();
    let project_fk = res.map_err(|e| match e {
        CoreError::ProjectImport(m) => ApiError::new(422, m),
        other => ApiError::new(400, other.to_string()),
    })?;
    crate::route::to_value(&export_import::ImportedProject {
        project_id: project_fk,
    })
}

/// Spill upload bytes to a temp `.lxproj`. Created O_EXCL (`create_new`) so a
/// pre-planted symlink at the path can't be followed and clobbered (the `api`
/// command is IPC-reachable) — on collision, retry with a fresh name. Callers
/// remove it on every path.
fn write_temp_lxproj(content: &[u8]) -> Result<PathBuf, ApiError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir();
    for _ in 0..16 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let uniq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "linxiv_import_{}_{}_{}.lxproj",
            std::process::id(),
            nanos,
            uniq
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(content)
                    .map_err(|e| ApiError::new(500, e.to_string()))?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(ApiError::new(500, e.to_string())),
        }
    }
    Err(ApiError::new(500, "could not create a unique temp file"))
}

use crate::route::papers::sid_key;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::models::PaperMetadata;
    use linxiv_core::storage;
    use serde_json::json;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn post(st: &AppState, path: &str, body: Value) -> Result<Value, ApiError> {
        route(
            st,
            ApiRequest {
                method: "POST".into(),
                path: path.into(),
                body: Some(body),
            },
        )
        .await
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// Synthetic PaperMetadata via serde (the app crate has no chrono dep).
    fn meta(source_id: &str) -> PaperMetadata {
        serde_json::from_value(json!({
            "source_id": source_id,
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-01",
            "summary": "S",
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn attach_non_pdf_is_400() {
        let st = state();
        // A paper must exist (the 404 check precedes the magic-byte check).
        let sid = st
            .with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta("arxiv:2204.99999"), None))
            .unwrap()
            .0;
        let path = format!("/api/papers/{}/pdf", sid);
        let err = route(
            &st,
            ApiRequest {
                method: "PUT".into(),
                path,
                body: Some(json!({ "file_b64": b64(b"not a pdf") })),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
        assert_eq!(err.detail, "Not a valid PDF");
    }

    /// Regression: pdf_dir may not exist yet on a fresh install (nothing creates
    /// it at startup) — attach must create it, not 500 with os error 2.
    #[tokio::test]
    async fn attach_creates_missing_pdf_dir() {
        let dir = tempfile::tempdir().unwrap();
        let pdf_dir = dir.path().join("pdfs");
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let st = AppState::from_parts(conn, pdf_dir.clone(), std::env::temp_dir());
        let sid = st
            .with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta("arxiv:2204.99998"), None))
            .unwrap()
            .0;
        let path = format!("/api/papers/{}/pdf", sid);
        let ok = route(
            &st,
            ApiRequest {
                method: "PUT".into(),
                path,
                body: Some(json!({ "file_b64": b64(b"%PDF-1.4 test") })),
            },
        )
        .await
        .unwrap();
        assert_eq!(ok, json!({ "ok": true }));
        assert!(pdf_dir
            .join(pdf_on_disk_name("arxiv:2204.99998", 1))
            .is_file());
    }

    #[tokio::test]
    async fn attach_missing_paper_is_404() {
        let st = state();
        let err = route(
            &st,
            ApiRequest {
                method: "PUT".into(),
                path: "/api/papers/arxiv:ghost/pdf".into(),
                body: Some(json!({ "file_b64": b64(b"%PDF-1.4\n") })),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper arxiv:ghost not found");
    }

    #[tokio::test]
    async fn import_bibtex_one_entry_saves_one() {
        let bib = b"@article{k, title={A Title}, author={Ada Lovelace}, year={1843}}";
        let out = post(
            &state(),
            "/api/papers/import/bibtex",
            json!({ "file_b64": b64(bib) }),
        )
        .await
        .unwrap();
        assert_eq!(out["saved_count"], json!(1));
        assert_eq!(out["source_ids"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn import_bibtex_unknown_project_is_404() {
        let bib = b"@article{k, title={T}, author={A}, year={2020}}";
        let err = post(
            &state(),
            "/api/papers/import/bibtex",
            json!({ "file_b64": b64(bib), "project_id": 9999 }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project 9999 not found");
    }

    #[tokio::test]
    async fn bad_base64_is_400() {
        let err = post(
            &state(),
            "/api/papers/import/bibtex",
            json!({ "file_b64": "!!! not valid base64 !!!" }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[tokio::test]
    async fn import_preview_bad_archive_is_400() {
        // Valid base64, but the bytes are not a .lxproj zip → preview_import errors → 400.
        let err = post(
            &state(),
            "/api/projects/import/preview",
            json!({ "file_b64": b64(b"not a zip") }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[tokio::test]
    async fn import_commit_bad_on_conflict_is_422() {
        let err = post(
            &state(),
            "/api/projects/import/commit",
            json!({ "file_b64": b64(b"x"), "on_conflict": "replace" }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
    }
}
