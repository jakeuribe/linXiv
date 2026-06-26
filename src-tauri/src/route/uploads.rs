//! Binary upload routes — `api/app.py` `api_attach_pdf` (292–318),
//! `api_import_pdf` (1376–1406), `api_import_bibtex` (1336–1367),
//! `api_import_preview` (1243–1261), `api_import_commit` (1264–1282).
//!
//! WIRE: the webview cannot send a multipart body through Tauri `invoke`, so every
//! upload's file bytes arrive as a base64 string `file_b64` in the JSON request
//! body. A malformed `file_b64` is a 400 (the upload never reaches core). All other
//! status codes + `detail` strings byte-match the Python handlers above.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::config;
use linxiv_core::error::CoreError;
use linxiv_core::formats::bibtex_import;
use linxiv_core::service::export_import::{self, OnConflict};
use linxiv_core::service::paper::{self as svc_paper, pdf_on_disk_name, Paper};
use linxiv_core::service::paper_import;
use linxiv_core::service::project as svc_project;
use linxiv_core::sources::pdf_metadata::resolve_pdf_metadata;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

/// `_MAX_PDF_BYTES` (app.py 1372) = 100 MB.
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

#[derive(Deserialize)]
struct FileBody {
    file_b64: String,
}

/// `PUT /api/papers/{id}/pdf` — `api_attach_pdf`. Store PDF bytes the client already
/// fetched for a saved paper. Order matches app.py: 404 paper → 413 size → 400 magic.
fn attach_pdf(state: &AppState, source_id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: FileBody = ctx.parse_body()?;
    let content = decode_b64(&b.file_b64)?;
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| -> Result<Value, ApiError> {
        let paper = svc_paper::get(conn, &sid_key(source_id))?
            .ok_or_else(|| ApiError::new(404, "Paper not found"))?;
        if content.len() > MAX_PDF_BYTES {
            return Err(ApiError::new(413, "PDF exceeds size limit"));
        }
        if !content.starts_with(b"%PDF") {
            return Err(ApiError::new(400, "Not a valid PDF"));
        }
        let ver = paper.version;
        let dest = pdf_dir.join(pdf_on_disk_name(source_id, ver));
        // app.py's `dest.resolve().relative_to(PDF_DIR)` containment guard. The
        // on-disk name is sanitized so the join stays a direct child of pdf_dir;
        // a separator slipping through would change the parent → 400.
        if dest.parent() != Some(pdf_dir.as_path()) {
            return Err(ApiError::new(400, "Invalid source_id"));
        }
        std::fs::write(&dest, &content).map_err(|e| ApiError::new(500, e.to_string()))?;
        let dest_str = dest.to_string_lossy().into_owned();
        if let Err(e) = svc_paper::mark_pdf_saved(conn, source_id, &dest_str, ver) {
            std::fs::remove_file(&dest).ok();
            return Err(ApiError::new(500, e.to_string()));
        }
        Ok(json!({ "ok": true }))
    })
}

/// `POST /api/papers/import/pdf` — `api_import_pdf`. Resolve metadata (network)
/// OUTSIDE the DB lock, then run the sync DB+FS import under it (mirrors
/// `import_pdf_default` without holding the mutex across the await).
async fn import_pdf(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        file_b64: String,
        #[serde(default)]
        filename: Option<String>,
    }
    let b: Body = ctx.parse_body()?;
    // app.py: `project_id: int | None = Query(...)` — links the imported paper to
    // a project (core's import_pdf runs the membership guard + link).
    let project_id = ctx.q_i64("project_id");
    let filename = b.filename.unwrap_or_default();
    if !filename.to_lowercase().ends_with(".pdf") {
        return Err(ApiError::new(400, "File must be a PDF"));
    }
    let content = decode_b64(&b.file_b64)?;
    if content.len() > MAX_PDF_BYTES {
        return Err(ApiError::new(
            413,
            "Upload rejected: file size exceeds 100 MB limit",
        ));
    }
    if !content.starts_with(b"%PDF") {
        return Err(ApiError::new(400, "File does not appear to be a valid PDF"));
    }
    let pdf_dir = state.pdf_dir.clone();
    // ponytail: data_dir is a config read here (not in AppState) — the resolver
    // needs it for arXiv's rate-limit file, same as the sources arms.
    let data_dir = config::data_dir();
    // PdfImport → 422, ProjectNotFound → 404, ProjectDeleted/PaperLink → 400 all
    // flow through `?` (CoreError::http_status + Display match app.py's details).
    let resolved = resolve_pdf_metadata(&content, &data_dir).await?;
    let result = state.with_conn(|conn| {
        paper_import::import_pdf(conn, &pdf_dir, &content, project_id, |_| Ok(resolved.clone()))
    })?;
    Ok(json!({ "source_id": result.source_id, "title": result.title }))
}

/// `POST /api/papers/import/bibtex` — `api_import_bibtex`. Optional `project_id`
/// links the imported papers; the project guard runs before parsing/saving.
fn import_bibtex(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        file_b64: String,
        #[serde(default)]
        project_id: Option<i64>,
    }
    let b: Body = ctx.parse_body()?;
    let content = decode_b64(&b.file_b64)?;
    let text = String::from_utf8_lossy(&content).into_owned();
    state.with_conn(|conn| -> Result<Value, ApiError> {
        if let Some(pid) = b.project_id {
            svc_project::ensure_membership_writable(conn, pid).map_err(|e| match e {
                CoreError::ProjectNotFound => ApiError::new(404, "Project not found"),
                CoreError::ProjectDeleted(m) => ApiError::new(400, m),
                other => other.into(),
            })?;
        }
        let metas = bibtex_import(&text)
            .map_err(|m| ApiError::new(400, format!("BibTeX parse error: {m}")))?;
        let mut saved: Vec<String> = Vec::new();
        for meta in &metas {
            let (sid, _) = svc_paper::save_paper_metadata(conn, meta, None)?;
            saved.push(sid);
        }
        if let Some(pid) = b.project_id {
            if !saved.is_empty() {
                svc_project::link_imported(conn, pid, &saved).map_err(|e| {
                    ApiError::new(
                        400,
                        format!(
                            "{} paper(s) were imported but could not be linked: {e}",
                            saved.len()
                        ),
                    )
                })?;
            }
        }
        Ok(json!({ "saved_count": saved.len(), "source_ids": saved }))
    })
}

/// `POST /api/projects/import/preview` — `api_import_preview`. `preview_import`
/// takes a zip PATH, so spill the decoded bytes to a temp `.lxproj` first; any
/// error (write, open, parse) is a 400. The temp file is removed on every path.
fn import_preview(ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: FileBody = ctx.parse_body()?;
    let content = decode_b64(&b.file_b64)?;
    let tmp = write_temp_lxproj(&content)?;
    let res = export_import::preview_import(&tmp);
    std::fs::remove_file(&tmp).ok();
    let p = res.map_err(|e| ApiError::new(400, e.to_string()))?;
    Ok(json!({
        "project_name": p.project_name,
        "description": p.description,
        "paper_count": p.paper_count,
        "note_count": p.note_count,
        "has_pdfs": p.has_pdfs,
        "format_version": p.format_version,
    }))
}

/// `POST /api/projects/import/commit` — `api_import_commit`. `on_conflict` defaults
/// to "merge"; a `ProjectImportError` is a 422, any other failure a 400.
fn import_commit(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        file_b64: String,
        #[serde(default)]
        on_conflict: Option<String>,
    }
    let b: Body = ctx.parse_body()?;
    let on_conflict = match b.on_conflict.as_deref() {
        None | Some("merge") => OnConflict::Merge,
        Some("overwrite") => OnConflict::Overwrite,
        // app.py's `pattern="^(merge|overwrite)$"` query validator → 422.
        Some(_) => return Err(ApiError::new(422, "on_conflict must be 'merge' or 'overwrite'")),
    };
    let content = decode_b64(&b.file_b64)?;
    let pdf_dir = state.pdf_dir.clone();
    let tmp = write_temp_lxproj(&content)?;
    let res = state.with_conn(|conn| export_import::commit_import(conn, &tmp, on_conflict, &pdf_dir));
    std::fs::remove_file(&tmp).ok();
    let project_fk = res.map_err(|e| match e {
        CoreError::ProjectImport(m) => ApiError::new(422, m),
        other => ApiError::new(400, other.to_string()),
    })?;
    Ok(json!({ "project_id": project_fk }))
}

/// Spill upload bytes to a nanos-suffixed temp `.lxproj` (no tempfile dep; same
/// pattern as `route/pdfs.rs` tests). Callers remove it on every path.
fn write_temp_lxproj(content: &[u8]) -> Result<PathBuf, ApiError> {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("linxiv_import_{n}.lxproj"));
    std::fs::write(&path, content).map_err(|e| ApiError::new(500, e.to_string()))?;
    Ok(path)
}

fn sid_key(source_id: &str) -> Paper {
    Paper { source_id: Some(source_id.to_string()), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::models::PaperMetadata;
    use linxiv_core::storage;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn post(st: &AppState, path: &str, body: Value) -> Result<Value, ApiError> {
        route(st, ApiRequest { method: "POST".into(), path: path.into(), body: Some(body) }).await
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
        // A paper must exist (404 precedes the magic-byte check in app.py order).
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
        assert_eq!(err.detail, "Paper not found");
    }

    #[tokio::test]
    async fn import_bibtex_one_entry_saves_one() {
        let bib = b"@article{k, title={A Title}, author={Ada Lovelace}, year={1843}}";
        let out = post(&state(), "/api/papers/import/bibtex", json!({ "file_b64": b64(bib) }))
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
        assert_eq!(err.detail, "Project not found");
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
