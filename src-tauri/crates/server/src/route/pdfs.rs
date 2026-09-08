//! `/api/papers/{id}/pdf-path` plus the `/api/pdfs` subtree (list saved PDFs,
//! delete all versions' PDFs). Resolution: stored custom path first, then managed.

use serde_json::Value;

use linxiv_core::error::CoreError;
use linxiv_core::service::files;
use linxiv_core::service::paper as svc_paper;

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

/// `GET /api/pdfs` — latest-version papers whose PDF is on disk, with file
/// sizes, largest first, capped at 200.
fn list_saved(state: &AppState) -> Result<Value, ApiError> {
    // Pull the rows under the lock; stat the files outside it.
    let papers = state.with_conn(|conn| svc_paper::list_pdf_papers(conn))?;
    let mut pdfs = files::saved_pdf_sizes(&state.pdf_dir, papers);
    pdfs.truncate(SAVED_PDF_LIST_CAP);
    crate::route::to_value(&files::SavedPdfListing { pdfs })
}

/// `DELETE /api/pdfs/{source_id}` — drops every version's local PDF (409 if a
/// file is outside the managed dir), keeping the paper record.
fn delete_saved(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| -> Result<(), ApiError> {
        if !svc_paper::delete_saved_pdfs(conn, &pdf_dir, source_id)? {
            return Err(ApiError::new(409, "PDF is outside managed storage"));
        }
        Ok(())
    })?;
    crate::route::to_value(&files::DeletedPdf { deleted: true })
}

/// `GET /api/papers/{source_id:path}/pdf-path?version=`.
fn pdf_path(state: &AppState, source_id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    // version: absent → latest; present must be a positive int (else 422).
    let version = crate::route::q_version(ctx)?;
    let (sid, ver, path) = resolve_pdf(state, source_id, version)?;
    // Canonical location envelope (path is always Some here — missing is 404).
    crate::route::to_value(&files::PdfLocation {
        source_id: sid,
        version: ver,
        path: Some(path),
    })
}

/// Resolve a paper's on-disk PDF to `(canonical source_id, version, path)`.
/// Shared with `remote_query`'s byte lane, so both surfaces resolve — and 404 — identically.
pub(crate) fn resolve_pdf(
    state: &AppState,
    source_id: &str,
    version: Option<i64>,
) -> Result<(String, i64, std::path::PathBuf), ApiError> {
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| {
        let (sid, ver, custom) = svc_paper::pdf_ref(conn, source_id, version)?
            .ok_or_else(|| CoreError::PaperNotFound(source_id.to_string()))?;
        let path = files::pdf_path(&pdf_dir, &sid, ver, custom.as_deref())
            .ok_or_else(|| ApiError::new(404, "PDF file not found on disk"))?;
        Ok((sid, ver, path))
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
