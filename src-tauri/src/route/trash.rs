//! `/api/trash` routes — `api/app.py` 1099–1161. Soft-delete (trash) management.
//!
//! FAITHFULNESS NOTE (verified against fastapi 0.137 / starlette 1.3): app.py
//! declares `/api/trash/{source_id:path}` (1142) and `.../{source_id:path}/restore`
//! (1129) BEFORE the `/api/trash/projects/{id}` routes (1148, 1156). Starlette
//! matches in declaration order and the `:path` converter swallows slashes, so the
//! project routes are SHADOWED and never execute in the shipped Python backend:
//! `DELETE /api/trash/projects/5` lands on the paper hard-delete handler with
//! source_id="projects/5" (a 200 no-op), and the project restore likewise. We
//! reproduce that exactly — a byte-faithful port must, or the frontend's trash
//! project buttons (src/api/trash.ts) would silently change behavior mid-port.
//! That project-trash routes are dead in Python is a latent upstream bug; making
//! them real is a deliberate, separate change (frontend + golden coordination),
//! NOT part of this port.

use serde_json::{json, Value};

use linxiv_core::service::paper::{self as svc_paper, Paper};
use linxiv_core::service::project as svc_project;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "trash"]) => Some(list(state)),
        // `:path` greedy capture: everything after `/api/trash/` (minus a trailing
        // `/restore`) is the source_id — including a literal `projects/5`.
        ("POST", ["api", "trash", rest @ .., "restore"]) if !rest.is_empty() => {
            Some(restore(state, &rest.join("/")))
        }
        ("DELETE", ["api", "trash", rest @ ..]) if !rest.is_empty() => {
            Some(hard_delete(state, &rest.join("/")))
        }
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

/// `POST /api/trash/{source_id:path}/restore` — `api_trash_restore`. Idempotent:
/// an unknown source_id (e.g. the shadowed `projects/{id}`) restores nothing and
/// returns `{ok, pdf_path:null, project_fks:[]}`, exactly as Python does.
fn restore(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    let (pdf_path, project_fks) = state.with_conn(|conn| {
        svc_paper::restore(
            conn,
            &Paper {
                source_id: Some(source_id.to_string()),
                ..Default::default()
            },
        )
    })?;
    Ok(json!({ "ok": true, "pdf_path": pdf_path, "project_fks": project_fks }))
}

/// `DELETE /api/trash/{source_id:path}` — `api_trash_hard_delete`. Idempotent on
/// an unknown source_id (200 no-op), matching Python.
fn hard_delete(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    state.with_conn(|conn| {
        svc_paper::hard_delete(
            conn,
            &Paper {
                source_id: Some(source_id.to_string()),
                ..Default::default()
            },
        )
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
        route(
            st,
            ApiRequest {
                method: method.into(),
                path: path.into(),
                body: None,
            },
        )
        .await
    }

    #[tokio::test]
    async fn list_on_empty_db_wraps_empty_arrays() {
        let v = req(&state(), "GET", "/api/trash").await.unwrap();
        assert_eq!(v, json!({ "papers": [], "projects": [] }));
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"papers":[],"projects":[]}"#
        );
    }

    #[tokio::test]
    async fn hard_delete_unknown_source_is_idempotent_ok() {
        let v = req(&state(), "DELETE", "/api/trash/2204.00001")
            .await
            .unwrap();
        assert_eq!(v, json!({ "ok": true }));
    }

    // The shadowed project paths resolve to the paper handler with a "projects/{id}"
    // source_id (no such paper), reproducing Python's 200 no-op — NOT a 404.
    #[tokio::test]
    async fn shadowed_project_hard_delete_is_paper_noop_200() {
        let v = req(&state(), "DELETE", "/api/trash/projects/999")
            .await
            .unwrap();
        assert_eq!(v, json!({ "ok": true }));
    }

    #[tokio::test]
    async fn shadowed_project_restore_is_paper_noop_200() {
        let v = req(&state(), "POST", "/api/trash/projects/999/restore")
            .await
            .unwrap();
        assert_eq!(
            v,
            json!({ "ok": true, "pdf_path": null, "project_fks": [] })
        );
    }
}
