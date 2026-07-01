//! `/api/annotations` routes — PDF highlight CRUD, mirroring `notes.rs`.
//! An annotation is keyed to a paper's SOURCE_FK (resolved from `source_id`),
//! optionally scoped to a project. The ANCHOR is opaque JSON validated by the
//! frontend; the backend stores and returns it verbatim.

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::models::{AnnotationIn, AnnotationUpdateIn};
use linxiv_core::service::annotation::{self as svc_ann, Annotation, Annotations};
use linxiv_core::service::paper as svc_paper;
use linxiv_core::storage::queries::paper as store_paper;

use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "annotations"]) => Some(list(state, ctx)),
        ("POST", ["api", "annotations"]) => Some(create(state, ctx)),
        ("PATCH", ["api", "annotations", id]) => Some(update(state, id, ctx)),
        ("DELETE", ["api", "annotations", id]) => Some(delete(state, id)),
        _ => None,
    }
}

/// `GET /api/annotations?source_id=&project_id=&all_projects=`. Unknown paper →
/// `{"annotations": []}` (not 404).
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let source_id = ctx
        .q("source_id")
        .ok_or_else(|| ApiError::new(422, "Missing required query parameter: source_id"))?;
    let project_fk = ctx.q_i64("project_id");
    let all_projects = ctx.q_bool("all_projects");
    state.with_conn(|conn| {
        let root = match store_paper::get_paper_root(conn, source_id)? {
            Some(r) => r,
            None => return Ok(json!({ "annotations": [] })),
        };
        let annotations = svc_ann::get_many(
            conn,
            &Annotations {
                source_fk: Some(root.source_fk),
                project_fk,
                all_projects,
            },
        )?;
        Ok(json!({ "annotations": annotations }))
    })
}

/// `POST /api/annotations`. Ensures the paper root, inserts the annotation.
fn create(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        source_id: String,
        #[serde(default)]
        project_id: Option<i64>,
        anchor: String,
        #[serde(default)]
        comment: String,
    }
    let b: Body = ctx.parse_body()?;
    if b.source_id.trim().is_empty() {
        return Err(ApiError::new(422, "source_id must not be empty"));
    }
    state.with_conn(|conn| {
        let source_fk = svc_paper::ensure_paper_root(conn, b.source_id.trim())?;
        let id = svc_ann::create(
            conn,
            &AnnotationIn {
                source_fk,
                project_fk: b.project_id,
                anchor: b.anchor,
                comment: b.comment,
            },
        )?;
        Ok(json!({ "id": id }))
    })
}

/// `PATCH /api/annotations/{id}` — edit the written comment. 404 if no row matched.
fn update(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let annotation_id = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        comment: String,
    }
    let b: Body = ctx.parse_body()?;
    state.with_conn(|conn| {
        if !svc_ann::update(
            conn,
            &AnnotationUpdateIn {
                annotation_id,
                comment: b.comment,
            },
        )? {
            return Err(ApiError::new(404, "Annotation not found"));
        }
        Ok(json!({ "ok": true }))
    })
}

/// `DELETE /api/annotations/{id}`. 404 if no row matched.
fn delete(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let annotation_id = path_i64(id)?;
    state.with_conn(|conn| {
        if !svc_ann::delete(
            conn,
            &Annotation {
                annotation_id: Some(annotation_id),
            },
        )? {
            return Err(ApiError::new(404, "Annotation not found"));
        }
        Ok(json!({ "ok": true }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::storage;

    const ANCHOR: &str = r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn req(
        st: &AppState,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        route(
            st,
            ApiRequest {
                method: method.into(),
                path: path.into(),
                body,
            },
        )
        .await
    }

    #[tokio::test]
    async fn list_unknown_paper_returns_empty_not_404() {
        let v = req(&state(), "GET", "/api/annotations?source_id=arxiv:404", None)
            .await
            .unwrap();
        assert_eq!(v, json!({ "annotations": [] }));
    }

    #[tokio::test]
    async fn missing_source_id_is_422() {
        let err = req(&state(), "GET", "/api/annotations", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn create_list_update_delete_roundtrip() {
        let st = state();
        let created = req(
            &st,
            "POST",
            "/api/annotations",
            Some(json!({ "source_id": "arxiv:1", "anchor": ANCHOR })),
        )
        .await
        .unwrap();
        assert_eq!(created, json!({ "id": 1 }));

        let listed = req(&st, "GET", "/api/annotations?source_id=arxiv:1", None)
            .await
            .unwrap();
        assert_eq!(listed["annotations"].as_array().unwrap().len(), 1);
        assert_eq!(listed["annotations"][0]["comment"], "");
        assert_eq!(listed["annotations"][0]["anchor"], ANCHOR);

        req(
            &st,
            "PATCH",
            "/api/annotations/1",
            Some(json!({ "comment": "note" })),
        )
        .await
        .unwrap();
        let listed = req(&st, "GET", "/api/annotations?source_id=arxiv:1", None)
            .await
            .unwrap();
        assert_eq!(listed["annotations"][0]["comment"], "note");

        req(&st, "DELETE", "/api/annotations/1", None).await.unwrap();
        let listed = req(&st, "GET", "/api/annotations?source_id=arxiv:1", None)
            .await
            .unwrap();
        assert!(listed["annotations"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_missing_anchor_is_422() {
        let err = req(
            &state(),
            "POST",
            "/api/annotations",
            Some(json!({ "source_id": "arxiv:1" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn update_and_delete_missing_is_404() {
        let st = state();
        let err = req(
            &st,
            "PATCH",
            "/api/annotations/999",
            Some(json!({ "comment": "x" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
        let err = req(&st, "DELETE", "/api/annotations/999", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[tokio::test]
    async fn update_missing_comment_is_422() {
        let err = req(&state(), "PATCH", "/api/annotations/1", Some(json!({})))
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }
}
