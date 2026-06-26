//! `/api/trash` routes — `api/app.py` 1099–1161. Soft-delete (trash) management
//! for papers and projects. Mirrors `mcp/src/notes_pdf_trash.rs`
//! (list_trash/restore/hard_delete) but returns the API envelopes verbatim.

use serde_json::{json, Value};

use linxiv_core::service::paper::{self as svc_paper, Paper};
use linxiv_core::service::project::{self as svc_project, Project};
use linxiv_core::storage::queries::project as store_project;

use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "trash"]) => Some(list(state)),
        // Project arms first: their `"projects"` literal + segment count make them
        // unambiguous against the `{source_id}` arms below.
        ("POST", ["api", "trash", "projects", id, "restore"]) => Some(project_restore(state, id)),
        ("DELETE", ["api", "trash", "projects", id]) => Some(project_hard_delete(state, id)),
        ("POST", ["api", "trash", sid, "restore"]) => Some(restore(state, sid)),
        ("DELETE", ["api", "trash", sid]) => Some(hard_delete(state, sid)),
        _ => None,
    }
}

/// `GET /api/trash` — `api_trash_list`. Two arrays with hand-picked keys (the
/// `DeletedPaperDetails`/`ProjectDetails` structs carry more than the API exposes).
fn list(state: &AppState) -> Result<Value, ApiError> {
    state.with_conn(|conn| {
        let papers = svc_paper::list_deleted(conn)?;
        let projects = svc_project::list_deleted(conn)?;
        let papers: Vec<Value> = papers
            .iter()
            .map(|d| {
                json!({
                    "source_fk": d.source_fk,
                    "source_id": d.source_id,
                    "title": d.title,
                    "authors": d.authors,
                    "published": d.published,
                    "deleted_at": d.deleted_at,
                    "had_pdf": d.had_pdf,
                })
            })
            .collect();
        let projects: Vec<Value> = projects
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    // archived_at is overwritten by delete(), so it holds the deletion timestamp
                    "deleted_at": p.archived_at,
                    "paper_count": p.source_fks.len(),
                })
            })
            .collect();
        Ok(json!({ "papers": papers, "projects": projects }))
    })
}

/// `POST /api/trash/{source_id}/restore` — `api_trash_restore`.
fn restore(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    let (pdf_path, project_fks) = state.with_conn(|conn| {
        svc_paper::restore(conn, &Paper { source_id: Some(source_id.to_string()), ..Default::default() })
    })?;
    Ok(json!({ "ok": true, "pdf_path": pdf_path, "project_fks": project_fks }))
}

/// `DELETE /api/trash/{source_id}` — `api_trash_hard_delete`.
fn hard_delete(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    state.with_conn(|conn| {
        svc_paper::hard_delete(conn, &Paper { source_id: Some(source_id.to_string()), ..Default::default() })
    })?;
    Ok(json!({ "ok": true }))
}

/// `POST /api/trash/projects/{project_id}/restore` — `api_trash_project_restore`.
fn project_restore(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let project_id = path_i64(id)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        if store_project::get_project(conn, project_id, false)?.is_none() {
            return Err(ApiError::new(404, "Project not found"));
        }
        svc_project::restore(conn, &Project { project_fk: Some(project_id) })?;
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

/// `DELETE /api/trash/projects/{project_id}` — `api_trash_project_hard_delete`.
fn project_hard_delete(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let project_id = path_i64(id)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        if store_project::get_project(conn, project_id, false)?.is_none() {
            return Err(ApiError::new(404, "Project not found"));
        }
        svc_project::hard_delete(conn, &Project { project_fk: Some(project_id) })?;
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
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

    async fn req(st: &AppState, method: &str, path: &str) -> Result<Value, ApiError> {
        route(st, ApiRequest { method: method.into(), path: path.into(), body: None }).await
    }

    #[tokio::test]
    async fn list_on_empty_db_wraps_empty_arrays() {
        let v = req(&state(), "GET", "/api/trash").await.unwrap();
        assert_eq!(v, json!({ "papers": [], "projects": [] }));
        // key order is part of the contract (preserve_order).
        assert_eq!(serde_json::to_string(&v).unwrap(), r#"{"papers":[],"projects":[]}"#);
    }

    #[tokio::test]
    async fn restore_missing_project_is_404() {
        let err = req(&state(), "POST", "/api/trash/projects/999/restore").await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project not found");
    }

    #[tokio::test]
    async fn hard_delete_missing_project_is_404() {
        let err = req(&state(), "DELETE", "/api/trash/projects/999").await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project not found");
    }

    #[tokio::test]
    async fn project_route_non_integer_id_is_422() {
        let err = req(&state(), "DELETE", "/api/trash/projects/abc").await.unwrap_err();
        assert_eq!(err.status, 422);
    }
}
