//! `/api/papers/{id}/pdf-path` plus the `/api/pdfs` subtree (list saved PDFs,
//! delete all versions' PDFs). Path resolution goes through
//! `service::files::pdf_path` (stored custom path first, then the managed
//! location). Shape mirrors `mcp/src/notes_pdf_trash.rs::get_pdf_path`.

use serde_json::{json, Value};

use linxiv_core::error::CoreError;
use linxiv_core::service::files;
use linxiv_core::service::paper::{self as svc_paper, PaperRef};

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

const SAVED_PDF_LIST_CAP: usize = 200;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "papers", id, "pdf-path"]) => Some(pdf_path(state, id, ctx)),
        ("GET", ["api", "pdfs"]) => Some(list_saved(state)),
        ("DELETE", ["api", "pdfs", id]) => Some(delete_saved(state, id)),
        _ => None,
    }
}

/// `GET /api/pdfs` — `api_list_saved_pdfs`. Latest-version papers whose PDF is on
/// disk, with file sizes, largest first, capped at 200.
fn list_saved(state: &AppState) -> Result<Value, ApiError> {
    let pdf_dir = state.pdf_dir.clone();
    // Pull the rows under the lock; stat the files outside it.
    let rows = state.with_conn(|conn| {
        svc_paper::list_pdf_papers(conn).map(|ps| {
            ps.into_iter()
                .map(|p| (p.source_id, p.source_fk, p.title, p.version, p.pdf_path))
                .collect::<Vec<_>>()
        })
    })?;
    let mut out: Vec<Value> = Vec::new();
    for (source_id, source_fk, title, version, pdf_path) in rows {
        let Some(path) = files::pdf_path(&pdf_dir, &source_id, version, pdf_path.as_deref()) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        out.push(json!({
            "source_id": source_id,
            "source_fk": source_fk,
            "title": title,
            "version": version,
            "size_bytes": meta.len(),
        }));
    }
    // size desc, then source_id asc — matches app.py (_LIST_PDFS_SQL orders by
    // source_id, then a stable sort by size_bytes desc keeps that as the tiebreak).
    out.sort_by(|a, b| {
        b["size_bytes"]
            .as_u64()
            .cmp(&a["size_bytes"].as_u64())
            .then_with(|| a["source_id"].as_str().cmp(&b["source_id"].as_str()))
    });
    out.truncate(SAVED_PDF_LIST_CAP);
    Ok(json!({ "pdfs": out }))
}

/// `DELETE /api/pdfs/{source_id}` — `api_delete_saved_pdf`. Drops every version's
/// local PDF (409 if a file is outside the managed dir), keeping the paper record.
fn delete_saved(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| -> Result<(), ApiError> {
        let all = svc_paper::get_all(conn, &PaperRef::source(source_id.to_string()))?
            .ok_or_else(|| CoreError::PaperNotFound(source_id.to_string()))?;
        for ver in &all.versions {
            let path = files::pdf_path(&pdf_dir, source_id, ver.version, ver.pdf_path.as_deref());
            if let Some(p) = &path {
                if !files::delete_pdf(&pdf_dir, &p.to_string_lossy()) {
                    return Err(ApiError::new(409, "PDF is outside managed storage"));
                }
            }
            // Clear the flag/path before the next iteration may raise 409.
            svc_paper::set_has_pdf(conn, source_id, ver.version, false)?;
            if path.is_some() {
                svc_paper::set_pdf_path(conn, source_id, "", Some(ver.version))?;
            }
        }
        Ok(())
    })?;
    Ok(json!({ "deleted": true }))
}

/// `GET /api/papers/{source_id:path}/pdf-path?version=` — `api_paper_pdf_path`.
fn pdf_path(state: &AppState, source_id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    // Query(default=None, ge=1): absent → latest; present must be a positive int.
    let version = crate::route::q_version(ctx)?;
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| {
        let paper = svc_paper::get(
            conn,
            &PaperRef::Source {
                source_id: source_id.to_string(),
                version,
            },
        )?
        .ok_or_else(|| CoreError::PaperNotFound(source_id.to_string()))?;
        let ver = version.unwrap_or(paper.version);
        let path = files::pdf_path(&pdf_dir, &paper.source_id, ver, paper.pdf_path.as_deref())
            .ok_or_else(|| ApiError::new(404, "PDF file not found on disk"))?;
        // Canonical location envelope (path is always Some here — missing is 404).
        crate::route::to_value(&files::PdfLocation {
            source_id: paper.source_id,
            version: ver,
            path: Some(path),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::storage;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn get(st: &AppState, path: &str) -> Result<Value, ApiError> {
        route(
            st,
            ApiRequest {
                method: "GET".into(),
                path: path.into(),
                body: None,
            },
        )
        .await
    }

    #[tokio::test]
    async fn missing_paper_is_404() {
        let err = get(&state(), "/api/papers/2204.12985/pdf-path")
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper 2204.12985 not found");
    }

    #[tokio::test]
    async fn bad_version_is_422() {
        let err = get(&state(), "/api/papers/2204.12985/pdf-path?version=0")
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }
}
