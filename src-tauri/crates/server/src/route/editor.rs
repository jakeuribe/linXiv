//! `/api/editor` routes — the embedded TeXbrain editor: editor-project notes plus
//! the per-vault FS bridge. The vault root is `state.vault_root/note_<id>`.

use serde::Deserialize;
use serde_json::Value;

use linxiv_core::error::CoreError;
use linxiv_core::service::editor_project::{self, EditorProjectsResponse};
use linxiv_core::service::vault::{self, FsOp};

use crate::route::{path_i64, to_value, ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "editor", "projects"]) => Some(list(state, ctx)),
        ("POST", ["api", "editor", "projects"]) => Some(create(state, ctx)),
        ("GET", ["api", "editor", "projects", id, "doc"]) => Some(doc(state, id)),
        ("POST", ["api", "editor", "vault", id, "fs"]) => Some(vault_fs(state, id, ctx)),
        _ => None,
    }
}

/// `GET /api/editor/projects?project_id=`.
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let project_id = ctx.q_i64("project_id");
    let projects = state.with_conn(|conn| editor_project::list_projects(conn, project_id))?;
    to_value(&EditorProjectsResponse { projects })
}

#[derive(Deserialize, ts_rs::TS)]
#[ts(optional_fields = nullable)]
pub struct CreateEditorProjectBody {
    pub project_name: String,
    #[serde(default = "default_main_file")]
    #[ts(as = "Option<String>", optional)]
    pub main_file: String,
    pub source_id: Option<String>,
    pub project_id: Option<i64>,
}

/// `POST /api/editor/projects` — `BadRequest` → 400; returns core's created payload.
fn create(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: CreateEditorProjectBody = ctx.parse_body()?;
    // An empty name is a 422; whitespace passes and core sanitizes it to "Untitled".
    if b.project_name.is_empty() {
        return Err(ApiError::new(422, "project_name must not be empty"));
    }
    let created = state.with_conn(|conn| {
        editor_project::create_project(
            conn,
            &state.vault_root,
            &b.project_name,
            &b.main_file,
            b.source_id.as_deref(),
            b.project_id,
        )
    })?;
    serde_json::to_value(&created).map_err(|e| ApiError::new(500, e.to_string()))
}

fn default_main_file() -> String {
    "main.tex".to_string()
}

/// `GET /api/editor/projects/{note_id}/doc`.
fn doc(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let note_id = path_i64(id)?;
    let doc = state
        .with_conn(|conn| editor_project::get_doc(conn, &state.vault_root, note_id))?
        .ok_or_else(|| ApiError::new(404, "Editor project not found"))?;
    serde_json::to_value(&doc).map_err(|e| ApiError::new(500, e.to_string()))
}

/// `POST /api/editor/vault/{note_id}/fs` — 404 if the note is not an editor
/// project; then forward one FsOp. NotFound → 404, other FS-op errors → 400.
fn vault_fs(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let note_id = path_i64(id)?;
    let op: FsOp = ctx.parse_body()?;
    if !state.with_conn(|conn| editor_project::is_editor_project_note(conn, note_id))? {
        return Err(ApiError::new(404, "Editor project not found"));
    }
    let vault_root = state.vault_root.join(format!("note_{note_id}"));
    // run_fs_op only yields NotFound / BadRequest / Internal here, so the catch-all is 400.
    let result = vault::run_fs_op(&vault_root, &op).map_err(|e| match e {
        CoreError::NotFound(s) => ApiError::new(404, format!("Not found: {s}")),
        other => ApiError::new(400, other.to_string()),
    })?;
    serde_json::to_value(&result).map_err(|e| ApiError::new(500, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::storage;
    use serde_json::json;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        // keep() persists the temp vault past the guard (tests never clean up, matching prior behavior).
        AppState::from_parts(
            conn,
            std::env::temp_dir(),
            tempfile::tempdir().unwrap().keep(),
        )
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
    async fn list_on_empty_db_wraps_empty_array() {
        assert_eq!(
            req(&state(), "GET", "/api/editor/projects", None)
                .await
                .unwrap(),
            json!({ "projects": [] })
        );
    }

    #[tokio::test]
    async fn create_then_doc_roundtrips_over_temp_vault() {
        let st = state();
        let created = req(
            &st,
            "POST",
            "/api/editor/projects",
            Some(json!({ "project_name": "My Paper" })),
        )
        .await
        .unwrap();
        assert_eq!(created["projectName"], "My Paper");
        assert_eq!(created["mainFile"], "main.tex");
        let note_id = created["noteId"].as_i64().unwrap();

        let doc = req(
            &st,
            "GET",
            &format!("/api/editor/projects/{note_id}/doc"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(doc["mainFile"], "main.tex");
        assert_eq!(doc["projectName"], "My Paper");
    }

    #[tokio::test]
    async fn create_empty_name_is_422() {
        let err = req(
            &state(),
            "POST",
            "/api/editor/projects",
            Some(json!({ "project_name": "" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn doc_missing_project_is_404() {
        let err = req(&state(), "GET", "/api/editor/projects/999/doc", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Editor project not found");
    }

    #[tokio::test]
    async fn doc_non_integer_id_is_422() {
        let err = req(&state(), "GET", "/api/editor/projects/abc/doc", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn vault_fs_on_non_project_is_404() {
        let err = req(
            &state(),
            "POST",
            "/api/editor/vault/999/fs",
            Some(json!({ "kind": "list", "path": "" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Editor project not found");
    }

    #[tokio::test]
    async fn vault_fs_list_and_mkdir_on_real_project() {
        let st = state();
        let created = req(
            &st,
            "POST",
            "/api/editor/projects",
            Some(json!({ "project_name": "P" })),
        )
        .await
        .unwrap();
        let note_id = created["noteId"].as_i64().unwrap();

        // Fresh vault holds only the scaffolded main.tex.
        let listed = req(
            &st,
            "POST",
            &format!("/api/editor/vault/{note_id}/fs"),
            Some(json!({ "kind": "list", "path": "" })),
        )
        .await
        .unwrap();
        assert_eq!(listed["kind"], "list");
        assert_eq!(listed["entries"][0]["name"], "main.tex");

        // mkdir returns the bare ok result.
        let made = req(
            &st,
            "POST",
            &format!("/api/editor/vault/{note_id}/fs"),
            Some(json!({ "kind": "mkdir", "path": "chapters" })),
        )
        .await
        .unwrap();
        assert_eq!(made, json!({ "kind": "ok" }));
    }

    #[tokio::test]
    async fn vault_fs_traversal_is_400() {
        let st = state();
        let created = req(
            &st,
            "POST",
            "/api/editor/projects",
            Some(json!({ "project_name": "P" })),
        )
        .await
        .unwrap();
        let note_id = created["noteId"].as_i64().unwrap();
        let err = req(
            &st,
            "POST",
            &format!("/api/editor/vault/{note_id}/fs"),
            Some(json!({ "kind": "readFile", "path": "../escape.tex" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
    }
}
