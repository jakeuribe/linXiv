//! `/api/notes` routes — `api/app.py` 872–918. Notes are keyed to a paper's
//! SOURCE_FK (resolved from the `source_id`), optionally scoped to a project.
//! Core binding mirrors `mcp/src/notes_pdf_trash.rs`. Shape copies `authors.rs`.

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::models::{NoteIn, NoteUpdateIn};
use linxiv_core::service::editor_project as svc_editor;
use linxiv_core::service::note::{self as svc_note, Note, Notes};
use linxiv_core::service::paper as svc_paper;
use linxiv_core::service::vault as svc_vault;
use linxiv_core::storage::queries::paper as store_paper;

use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "notes"]) => Some(list(state, ctx)),
        ("POST", ["api", "notes"]) => Some(create(state, ctx)),
        ("PATCH", ["api", "notes", id]) => Some(update(state, id, ctx)),
        ("DELETE", ["api", "notes", id]) => Some(delete(state, id)),
        _ => None,
    }
}

/// `GET /api/notes?source_id=&project_id=&all_projects=` — `api_notes`. Unknown
/// paper → `{"notes": []}` (not 404).
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let source_id = ctx
        .q("source_id")
        .ok_or_else(|| ApiError::new(422, "Missing required query parameter: source_id"))?;
    let project_fk = ctx.q_i64("project_id");
    let all_projects = ctx.q_bool("all_projects");
    state.with_conn(|conn| {
        let root = match store_paper::get_paper_root(conn, source_id)? {
            Some(r) => r,
            None => return Ok(json!({ "notes": [] })),
        };
        let notes = svc_note::get_many(
            conn,
            &Notes { source_fk: Some(root.source_fk), project_fk, all_projects, ..Default::default() },
        )?;
        Ok(json!({ "notes": notes }))
    })
}

/// `POST /api/notes` — `api_note_create`. Ensures the paper root, inserts the note.
fn create(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        source_id: String,
        #[serde(default)]
        project_id: Option<i64>,
        #[serde(default)]
        paper_id: Option<i64>,
        #[serde(default)]
        title: String,
        #[serde(default)]
        content: String,
    }
    let b: Body = ctx.parse_body()?;
    state.with_conn(|conn| {
        let source_fk = svc_paper::ensure_paper_root(conn, b.source_id.trim())?;
        let note_id = svc_note::create(
            conn,
            &NoteIn {
                source_fk,
                project_fk: b.project_id,
                paper_id: b.paper_id,
                title: b.title,
                content: b.content,
            },
        )?;
        Ok(json!({ "id": note_id }))
    })
}

/// `PATCH /api/notes/{id}` — `api_note_update`. 404 if no row matched.
fn update(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let note_id = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        content: Option<String>,
    }
    let b: Body = ctx.parse_body()?;
    state.with_conn(|conn| {
        if !svc_note::update(conn, &NoteUpdateIn { note_id, title: b.title, content: b.content })? {
            return Err(ApiError::new(404, "Note not found"));
        }
        Ok(json!({ "ok": true }))
    })
}

/// `DELETE /api/notes/{id}` — `api_note_delete`. Resolve the editor-project flag
/// BEFORE the delete (the frontmatter must still be readable), then drop the vault.
fn delete(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let note_id = path_i64(id)?;
    let is_editor_project = state.with_conn(|conn| -> Result<bool, ApiError> {
        let is_editor = svc_editor::get_meta(conn, note_id)?.is_some();
        if !svc_note::delete(conn, &Note { note_id: Some(note_id) })? {
            return Err(ApiError::new(404, "Note not found"));
        }
        Ok(is_editor)
    })?;
    if is_editor_project {
        // Vault dir layout mirrors core's private `vault_root`: `<vault>/note_<id>`.
        svc_vault::delete_vault(&state.vault_root.join(format!("note_{note_id}")));
    }
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

    async fn req(st: &AppState, method: &str, path: &str, body: Option<Value>) -> Result<Value, ApiError> {
        route(st, ApiRequest { method: method.into(), path: path.into(), body }).await
    }

    #[tokio::test]
    async fn list_unknown_paper_returns_empty_not_404() {
        let v = req(&state(), "GET", "/api/notes?source_id=arxiv:404", None).await.unwrap();
        assert_eq!(v, json!({ "notes": [] }));
    }

    #[tokio::test]
    async fn list_missing_source_id_is_422() {
        let err = req(&state(), "GET", "/api/notes", None).await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn create_then_list_roundtrip() {
        let st = state();
        let created = req(
            &st,
            "POST",
            "/api/notes",
            Some(json!({ "source_id": "arxiv:1", "title": "t", "content": "c" })),
        )
        .await
        .unwrap();
        assert_eq!(created, json!({ "id": 1 }));
        let listed = req(&st, "GET", "/api/notes?source_id=arxiv:1", None).await.unwrap();
        assert_eq!(listed["notes"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn update_missing_note_is_404() {
        let err = req(&state(), "PATCH", "/api/notes/999", Some(json!({ "title": "x" })))
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Note not found");
    }

    #[tokio::test]
    async fn delete_missing_note_is_404() {
        let err = req(&state(), "DELETE", "/api/notes/999", None).await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Note not found");
    }

    #[tokio::test]
    async fn non_integer_id_is_422() {
        let err = req(&state(), "PATCH", "/api/notes/abc", Some(json!({ "title": "x" })))
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }
}
