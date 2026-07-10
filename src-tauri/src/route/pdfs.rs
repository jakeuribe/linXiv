//! `/api/papers/{id}/pdf-path` — `api/app.py` 281–289 (`api_paper_pdf_path`).
//! Resolves the local on-disk PDF path for a paper version. The `/api/pdfs`
//! subtree (GET/DELETE) is deferred to the sidecar — this group owns ONLY the
//! `pdf-path` leaf. Shape mirrors `mcp/src/notes_pdf_trash.rs::get_pdf_path`.

use std::path::Path;

use serde_json::{json, Value};

use linxiv_core::service::files;
use linxiv_core::service::paper::{self as svc_paper, pdf_on_disk_name, Paper};

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
        svc_paper::list_papers(conn, true, None, 0, None).map(|ps| {
            ps.into_iter()
                .filter(|p| p.has_pdf)
                .map(|p| (p.source_id, p.source_fk, p.title, p.version, p.pdf_path))
                .collect::<Vec<_>>()
        })
    })?;
    let mut out: Vec<Value> = Vec::new();
    for (source_id, source_fk, title, version, pdf_path) in rows {
        let Some(path) = resolve_local_pdf(&pdf_dir, pdf_path.as_deref(), &source_id, version)
        else {
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
        let all = svc_paper::get_all(
            conn,
            &Paper {
                source_id: Some(source_id.to_string()),
                ..Default::default()
            },
        )?
        .ok_or_else(|| ApiError::new(404, "Paper not found"))?;
        for ver in &all.versions {
            let path = resolve_local_pdf(&pdf_dir, ver.pdf_path.as_deref(), source_id, ver.version);
            if let Some(p) = &path {
                if !files::delete_pdf(&pdf_dir, p) {
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
            &Paper {
                source_id: Some(source_id.to_string()),
                version,
                ..Default::default()
            },
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
/// (skipped for `ver <= 0`, which the download pipeline never writes). Shared with
/// the `linxiv://` PDF protocol handler (`crate::protocol`).
pub(crate) fn resolve_local_pdf(
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
        let err = get(&state(), "/api/papers/2204.12985/pdf-path")
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper not found");
    }

    #[tokio::test]
    async fn bad_version_is_422() {
        let err = get(&state(), "/api/papers/2204.12985/pdf-path?version=0")
            .await
            .unwrap_err();
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
