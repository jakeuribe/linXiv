//! `/api/trash` routes — `api/app.py` 1099–1161. Soft-delete (trash) management.
//!
//! Upstream Python had a latent route-ordering bug: `/api/trash/{source_id:path}`
//! (1142) and `.../{source_id:path}/restore` (1129) were declared BEFORE the
//! `/api/trash/projects/{id}` routes (1148, 1156). Starlette matches in declaration
//! order and the `:path` converter swallows slashes, so the project routes were
//! SHADOWED and never executed: `DELETE /api/trash/projects/5` landed on the paper
//! hard-delete handler as source_id="projects/5" (a 200 no-op), leaving the project
//! stuck in Trash. We fix that here: the specific `projects/{id}` arms are matched
//! BEFORE the greedy paper `rest @ ..` arms (Rust matches top-to-bottom), so
//! project hard-delete and restore now route to `svc_project` correctly.

use serde_json::Value;

use linxiv_core::service::paper::{self as svc_paper, Paper};
use linxiv_core::service::project as svc_project;
use linxiv_core::service::trash as svc_trash;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "trash"]) => Some(list(state)),
        // Specific project arms MUST precede the greedy paper arms below.
        ("POST", ["api", "trash", "projects", id, "restore"]) => Some(restore_project(state, id)),
        ("DELETE", ["api", "trash", "projects", id]) => Some(hard_delete_project(state, id)),
        // `:path` greedy capture: everything after `/api/trash/` (minus a trailing
        // `/restore`) is the paper source_id. `projects/{id}` is handled by the
        // specific arms above and never reaches here.
        ("POST", ["api", "trash", rest @ .., "restore"]) if !rest.is_empty() => {
            Some(restore(state, &rest.join("/")))
        }
        ("DELETE", ["api", "trash", rest @ ..]) if !rest.is_empty() => {
            Some(hard_delete(state, &rest.join("/")))
        }
        _ => None,
    }
}

/// `GET /api/trash` — `api_trash_list`. The canonical `TrashListing` envelope
/// (core `service::trash`), shared with `linxiv trash list` and MCP `list_trash`.
fn list(state: &AppState) -> Result<Value, ApiError> {
    state.with_conn(|conn| crate::route::to_value(&linxiv_core::service::trash::list_trash(conn)?))
}

/// `POST /api/trash/{source_id:path}/restore` — `api_trash_restore`. 404 unless the
/// paper is actually in the trash (`svc_paper::require_trashed`).
fn restore(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    let (pdf_path, project_fks) =
        state.with_conn(|conn| -> Result<(Option<String>, Vec<i64>), ApiError> {
            svc_paper::require_trashed(conn, source_id)?;
            Ok(svc_paper::restore(
                conn,
                &Paper {
                    source_id: Some(source_id.to_string()),
                    ..Default::default()
                },
            )?)
        })?;
    crate::route::to_value(&svc_trash::RestoredPaper {
        ok: true,
        restored: source_id.to_string(),
        pdf_path,
        project_fks,
    })
}

/// `DELETE /api/trash/{source_id:path}` — `api_trash_hard_delete`. Permanent, so it
/// 404s unless the paper is in the trash; use `DELETE /api/papers/{id}` to trash one.
fn hard_delete(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    state.with_conn(|conn| -> Result<(), ApiError> {
        svc_paper::require_trashed(conn, source_id)?;
        svc_paper::hard_delete(
            conn,
            &Paper {
                source_id: Some(source_id.to_string()),
                ..Default::default()
            },
        )?;
        Ok(())
    })?;
    crate::route::to_value(&svc_trash::HardDeletedPaper {
        ok: true,
        hard_deleted: source_id.to_string(),
    })
}

/// `POST /api/trash/projects/{id}/restore` — un-trash a project. 422 on a non-integer
/// id, 404 if absent, 400 if the project is active or archived rather than trashed.
fn restore_project(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let project_fk = crate::route::path_i64(id)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        svc_project::require_trashed(conn, project_fk)?;
        svc_project::restore(
            conn,
            &svc_project::Project {
                project_fk: Some(project_fk),
            },
        )?;
        Ok(())
    })?;
    crate::route::to_value(&svc_trash::RestoredProject {
        ok: true,
        restored_project_id: project_fk,
    })
}

/// `DELETE /api/trash/projects/{id}` — permanently delete a trashed project. 422 on a
/// non-integer id, 404 if absent, 400 if it is not in the trash.
fn hard_delete_project(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let project_fk = crate::route::path_i64(id)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        svc_project::require_trashed(conn, project_fk)?;
        svc_project::hard_delete(
            conn,
            &svc_project::Project {
                project_fk: Some(project_fk),
            },
        )?;
        Ok(())
    })?;
    crate::route::to_value(&svc_trash::HardDeletedProject {
        ok: true,
        hard_deleted_project_id: project_fk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::testutil::state;
    use serde_json::json;

    // Trash routes never take a body.
    async fn req(st: &AppState, method: &str, path: &str) -> Result<Value, ApiError> {
        crate::route::testutil::req(st, method, path, None).await
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
    async fn hard_delete_unknown_source_is_404() {
        for method in ["DELETE", "POST"] {
            let path = match method {
                "POST" => "/api/trash/2204.00001/restore",
                _ => "/api/trash/2204.00001",
            };
            let err = req(&state(), method, path).await.unwrap_err();
            assert_eq!(err.status, 404);
        }
    }

    use linxiv_core::models::{ProjectIn, Status};

    /// Create a project then soft-delete it (trash), returning its id.
    fn trashed_project(st: &AppState) -> i64 {
        st.with_conn(|conn| {
            let id = svc_project::create(
                conn,
                &ProjectIn {
                    name: "Trashed".into(),
                    description: String::new(),
                    color: None,
                    tags: vec![],
                    source_fks: vec![],
                },
            )
            .unwrap();
            svc_project::delete(
                conn,
                &svc_project::Project {
                    project_fk: Some(id),
                },
            )
            .unwrap();
            id
        })
    }

    fn get_status(st: &AppState, id: i64) -> Option<Status> {
        st.with_conn(|conn| {
            svc_project::get(
                conn,
                &svc_project::Project {
                    project_fk: Some(id),
                },
            )
            .unwrap()
        })
        .map(|d| d.status)
    }

    #[tokio::test]
    async fn project_hard_delete_removes_it() {
        let st = state();
        let id = trashed_project(&st);
        let v = req(&st, "DELETE", &format!("/api/trash/projects/{id}"))
            .await
            .unwrap();
        assert_eq!(v, json!({ "ok": true, "hard_deleted_project_id": id }));
        // Gone from the DB entirely.
        assert_eq!(get_status(&st, id), None);
    }

    #[tokio::test]
    async fn project_restore_reactivates_it() {
        let st = state();
        let id = trashed_project(&st);
        assert_eq!(get_status(&st, id), Some(Status::Deleted));
        let v = req(&st, "POST", &format!("/api/trash/projects/{id}/restore"))
            .await
            .unwrap();
        assert_eq!(v, json!({ "ok": true, "restored_project_id": id }));
        assert_eq!(get_status(&st, id), Some(Status::Active));
    }

    /// The permanent-loss guard: an active (never-trashed) project must survive
    /// both trash endpoints.
    #[tokio::test]
    async fn active_project_is_rejected_by_both_trash_endpoints() {
        let st = state();
        let id = st.with_conn(|conn| {
            svc_project::create(
                conn,
                &ProjectIn {
                    name: "Active".into(),
                    description: String::new(),
                    color: None,
                    tags: vec![],
                    source_fks: vec![],
                },
            )
            .unwrap()
        });

        for (method, path) in [
            ("DELETE", format!("/api/trash/projects/{id}")),
            ("POST", format!("/api/trash/projects/{id}/restore")),
        ] {
            let err = req(&st, method, &path).await.unwrap_err();
            assert_eq!(err.status, 400);
        }
        assert_eq!(get_status(&st, id), Some(Status::Active));
    }

    #[tokio::test]
    async fn missing_project_is_404() {
        let err = req(&state(), "DELETE", "/api/trash/projects/999")
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[tokio::test]
    async fn project_non_integer_id_is_422() {
        for (method, path) in [
            ("DELETE", "/api/trash/projects/abc"),
            ("POST", "/api/trash/projects/abc/restore"),
        ] {
            let err = req(&state(), method, path).await.unwrap_err();
            assert_eq!(err.status, 422);
        }
    }
}
