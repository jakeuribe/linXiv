//! `/api/papers/{id}/pdf-path` — `api/app.py` 281–289 (`api_paper_pdf_path`).
//! Resolves the local on-disk PDF path for a paper version. The `/api/pdfs`
//! subtree (GET/DELETE) is deferred to the sidecar — this group owns ONLY the
//! `pdf-path` leaf. Shape mirrors `mcp/src/notes_pdf_trash.rs::get_pdf_path`.

use std::path::Path;

use serde_json::{json, Value};

use linxiv_core::service::paper::{self as svc_paper, pdf_on_disk_name, Paper};

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

/// Returns `Some(result)` only for `GET /api/papers/{id}/pdf-path`; `None` passes.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "papers", id, "pdf-path"]) => Some(pdf_path(state, id, ctx)),
        _ => None,
    }
}

/// `GET /api/papers/{source_id:path}/pdf-path?version=` — `api_paper_pdf_path`.
fn pdf_path(state: &AppState, source_id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    // Query(default=None, ge=1): absent → latest; present must be a positive int.
    let version = match ctx.q("version") {
        None => None,
        Some(v) => match v.parse::<i64>() {
            Ok(n) if n >= 1 => Some(n),
            _ => return Err(ApiError::new(422, "version must be an integer >= 1")),
        },
    };
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| {
        let paper = svc_paper::get(
            conn,
            &Paper { source_id: Some(source_id.to_string()), version, ..Default::default() },
        )?
        .ok_or_else(|| ApiError::new(404, "Paper not found"))?;
        let ver = version.unwrap_or(paper.version);
        let path = resolve_local_pdf(&pdf_dir, paper.pdf_path.as_deref(), &paper.source_id, ver)
            .ok_or_else(|| ApiError::new(404, "PDF file not found on disk"))?;
        Ok(json!({ "path": path }))
    })
}

/// Port of `_resolve_local_pdf` (app.py 124–135): the stored `pdf_path` if it
/// exists on disk, else the standard managed location `pdf_dir/<on-disk-name>`
/// (skipped for `ver <= 0`, which the download pipeline never writes).
fn resolve_local_pdf(
    pdf_dir: &Path,
    pdf_path: Option<&str>,
    source_id: &str,
    ver: i64,
) -> Option<String> {
    if let Some(p) = pdf_path {
        if Path::new(p).is_file() {
            return Some(p.to_string());
        }
    }
    if ver <= 0 {
        return None;
    }
    let std = pdf_dir.join(pdf_on_disk_name(source_id, ver));
    std.is_file().then(|| std.to_string_lossy().into_owned())
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
        route(st, ApiRequest { method: "GET".into(), path: path.into(), body: None }).await
    }

    /// Unique scratch dir (no tempfile dep): nanos-suffixed under the system temp.
    fn scratch() -> std::path::PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("linxiv_pdfpath_{n}"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn missing_paper_is_404() {
        let err = get(&state(), "/api/papers/2204.12985/pdf-path").await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper not found");
    }

    #[tokio::test]
    async fn bad_version_is_422() {
        let err = get(&state(), "/api/papers/2204.12985/pdf-path?version=0").await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[test]
    fn resolves_stored_path_then_managed_then_none() {
        let dir = scratch();
        let pdf_dir = dir.as_path();

        // 1. stored pdf_path wins when it exists on disk.
        let stored = pdf_dir.join("custom.pdf");
        std::fs::write(&stored, b"%PDF").unwrap();
        let stored_s = stored.to_string_lossy().into_owned();
        assert_eq!(
            resolve_local_pdf(pdf_dir, Some(&stored_s), "2204.12985", 1),
            Some(stored_s.clone())
        );

        // 2. stored absent → fall back to the managed on-disk name.
        let managed = pdf_dir.join(pdf_on_disk_name("2204.12985", 2));
        std::fs::write(&managed, b"%PDF").unwrap();
        assert_eq!(
            resolve_local_pdf(pdf_dir, None, "2204.12985", 2),
            Some(managed.to_string_lossy().into_owned())
        );

        // 3. ver<=0 never probes the managed location.
        assert_eq!(resolve_local_pdf(pdf_dir, None, "2204.12985", 0), None);

        // 4. nothing on disk → None.
        assert_eq!(resolve_local_pdf(pdf_dir, None, "nope", 1), None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
