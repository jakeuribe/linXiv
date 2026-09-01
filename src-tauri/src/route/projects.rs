//! `/api/projects` routes — `api/app.py` 512–688. Mirrors `authors.rs`: a `handle`
//! owning the `/api/projects` subtree, path-param extraction, body deserialization,
//! and the exact JSON envelopes / status codes `app.py` returned. Core binding
//! follows `mcp/src/projects_tags.rs`.

use std::path::Path;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::error::CoreError;
use linxiv_core::formats;
use linxiv_core::models::{PaperDetails, ProjectDetails, ProjectIn, ProjectUpdateIn, Status};
use linxiv_core::service::export_import;
use linxiv_core::service::project::{self, Project, Projects};

use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "projects"]) => Some(list(state, ctx)),
        ("POST", ["api", "projects"]) => Some(create(state, ctx)),
        ("GET", ["api", "projects", id]) => Some(get_one(state, id)),
        ("PATCH", ["api", "projects", id]) => Some(patch(state, id, ctx)),
        ("DELETE", ["api", "projects", id]) => Some(delete(state, id)),
        ("POST", ["api", "projects", id, "papers"]) => Some(add_paper(state, id, ctx)),
        ("POST", ["api", "projects", id, "papers", "bulk"]) => {
            Some(add_papers_bulk(state, id, ctx))
        }
        ("DELETE", ["api", "projects", id, "papers", sid]) => Some(remove_paper(state, id, sid)),
        ("POST", ["api", "projects", id, "export"]) => Some(export(state, id, ctx)),
        ("GET", ["api", "projects", id, "export", "bibtex"]) => {
            Some(export_text(state, id, ctx, formats::bibtex_export))
        }
        ("GET", ["api", "projects", id, "export", "obsidian"]) => {
            Some(export_text(state, id, ctx, formats::obsidian_export))
        }
        _ => None,
    }
}

/// `POST /api/projects/{id}/export` — `api_project_export` (dest_path branch only).
/// Writes the `.lxproj` archive to `dest_path` and returns `{ok}`. The streaming
/// (no-dest_path) branch is browser-only; the Tauri frontend always sends a path.
fn export(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let project_fk = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        dest_path: Option<String>,
        #[serde(default)]
        include_pdfs: bool,
    }
    let b: Body = ctx.parse_body()?;
    let Some(dest) = b.dest_path.filter(|s| !s.is_empty()) else {
        return Err(ApiError::new(
            400,
            "dest_path is required for in-process export",
        ));
    };
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| -> Result<(), ApiError> {
        // export_project's own get_required words the miss (typed 404 via `?`).
        export_import::export_project(
            conn,
            project_fk,
            Path::new(&dest),
            b.include_pdfs,
            &pdf_dir,
        )?;
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

/// `GET /api/projects/{id}/export/{bibtex,obsidian}?dest_path=` — the dest_path
/// branch of the text exporters. Writes the formatted project to disk, `{ok}`.
fn export_text(
    state: &AppState,
    id: &str,
    ctx: &ReqCtx<'_>,
    fmt: fn(&[PaperDetails]) -> String,
) -> Result<Value, ApiError> {
    let project_fk = path_i64(id)?;
    let Some(dest) = ctx.q("dest_path").filter(|s| !s.is_empty()) else {
        return Err(ApiError::new(
            400,
            "dest_path is required for in-process export",
        ));
    };
    let content = state.with_conn(|conn| -> Result<String, ApiError> {
        let proj = project::get_required(conn, project_fk)?;
        Ok(fmt(&project::export_papers(conn, &proj.source_fks)?))
    })?;
    std::fs::write(dest, content).map_err(|e| ApiError::new(500, e.to_string()))?;
    Ok(json!({ "ok": true }))
}

/// Canonical project wire shape — `service::project::to_out` (SERIALIZER 3;
/// identical bytes on route, CLI and MCP). Shared with the tag-detail scan.
pub(crate) fn project_out(conn: &Connection, p: ProjectDetails) -> Result<Value, ApiError> {
    serde_json::to_value(project::to_out(conn, p)?).map_err(|e| ApiError::new(500, e.to_string()))
}

/// `GET /api/projects?status=` — `api_projects`. Default "active"; "all" => no filter.
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let status = ctx.q("status").unwrap_or("active").to_string();
    let out = state.with_conn(|conn| -> Result<Vec<Value>, ApiError> {
        let filter = match status.as_str() {
            "all" => Projects::default(),
            // app.py parity: an unparseable filter matches nothing, it is not a 400.
            s => match s.parse::<Status>() {
                Ok(st) => Projects {
                    status: Some(st),
                    ..Default::default()
                },
                Err(_) => return Ok(Vec::new()),
            },
        };
        let mut projects = project::get_many(conn, &filter)?;
        // app.py drops null-id rows (data-integrity guard)
        projects.retain(|p| p.id.is_some());
        project::to_out_many(conn, projects)?
            .into_iter()
            .map(|p| serde_json::to_value(p).map_err(|e| ApiError::new(500, e.to_string())))
            .collect()
    })?;
    Ok(json!({ "projects": out }))
}

/// `POST /api/projects` — `api_project_create`.
fn create(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        name: String,
        #[serde(default)]
        description: String,
        color_hex: Option<String>,
        #[serde(default)]
        project_tags: Vec<String>,
    }
    let b: Body = ctx.parse_body()?;
    let name = b.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::new(400, "name cannot be blank"));
    }
    let color = parse_color(b.color_hex.as_deref())?;
    let pin = ProjectIn {
        name: name.clone(),
        description: b.description.trim().to_string(),
        color,
        tags: b.project_tags,
        source_fks: Vec::new(), // papers are linked via POST /projects/{id}/papers
    };
    let fk = state.with_conn(|conn| project::create(conn, &pin))?;
    Ok(json!({ "project": { "id": fk, "name": name } }))
}

/// `color_from_hex(hex) if hex else None`, 400 "Invalid color_hex" on a bad value.
fn parse_color(hex: Option<&str>) -> Result<Option<i32>, ApiError> {
    match hex.filter(|s| !s.is_empty()) {
        Some(h) => project::color_from_hex(h)
            .map(Some)
            .map_err(|_| ApiError::new(400, "Invalid color_hex")),
        None => Ok(None),
    }
}

/// `GET /api/projects/{id}` — `api_project_get`. Not-found wording comes from
/// `CoreError::ProjectNotFound` (the shared contract), mapped to 404 here.
fn get_one(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    state.with_conn(|conn| {
        let p = project::get_required(conn, pid)?;
        project_out(conn, p)
    })
}

/// `PATCH /api/projects/{id}` — `api_project_patch`. Partial update; color cleared
/// only when the `color_hex` key is present in the body.
fn patch(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        name: Option<String>,
        description: Option<String>,
        color_hex: Option<String>,
        status: Option<String>,
        project_tags: Option<Vec<String>>,
    }
    let b: Body = ctx.parse_body()?;

    // Core owns the parse and the message; the 400 is kept so the frontend's
    // handling of this response is unchanged (Validation would otherwise be 422).
    let status = match b.status.as_deref() {
        Some(s) => Some(
            s.parse::<Status>()
                .map_err(|e| ApiError::new(400, e.to_string()))?,
        ),
        None => None,
    };
    // color: only touched when the key was explicitly sent (app.py model_fields_set).
    // Sent + non-empty => set; sent + null/"" => clear (Some(None)); absent => unchanged.
    let color_sent = ctx
        .body
        .and_then(Value::as_object)
        .is_some_and(|m| m.contains_key("color_hex"));
    let color = if color_sent {
        Some(parse_color(b.color_hex.as_deref())?)
    } else {
        None
    };
    let name = b.name.map(|n| n.trim().to_string());
    if name.as_deref() == Some("") {
        return Err(ApiError::new(400, "name cannot be blank"));
    }
    let upd = ProjectUpdateIn {
        project_fk: pid,
        name,
        description: b.description.map(|d| d.trim().to_string()),
        color,
        project_tags: b.project_tags,
        status,
    };
    state
        .with_conn(|conn| project::update(conn, &upd))
        .map_err(|e| match e {
            // app.py: LookupError -> 404 "Project not found"; ValueError -> 400 str(e).
            CoreError::ProjectDeleted(m) | CoreError::Validation(m) => ApiError::new(400, m),
            other => other.into(),
        })?;
    Ok(json!({ "ok": true }))
}

/// `DELETE /api/projects/{id}` — `api_project_delete`. 404 if absent, then soft-delete.
fn delete(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    let proj = Project {
        project_fk: Some(pid),
    };
    state.with_conn(|conn| -> Result<(), ApiError> {
        project::get_required(conn, pid)?;
        project::delete(conn, &proj)?;
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

/// `POST /api/projects/{id}/papers` — `api_project_add_paper`. Core's shared
/// receipt; `?` keeps app.py's statuses (PaperNotFound → 404 with the paper id,
/// ProjectNotFound → 404, ProjectDeleted → 400).
fn add_paper(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        source_id: String,
    }
    let b: Body = ctx.parse_body()?;
    let receipt = state.with_conn(|conn| project::add_paper(conn, pid, &b.source_id))?;
    crate::route::to_value(&receipt)
}

/// `POST /api/projects/{id}/papers/bulk` — `api_project_add_papers`. Partial success.
fn add_papers_bulk(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        source_ids: Vec<String>,
    }
    let b: Body = ctx.parse_body()?;
    let failed = state.with_conn(|conn| project::add_papers(conn, pid, &b.source_ids))?;
    Ok(json!({ "ok": failed.is_empty(), "failed": failed }))
}

/// `DELETE /api/projects/{id}/papers/{sid}` — `api_project_remove_paper`. `sid`
/// arrives already percent-decoded in `ctx.segs`.
fn remove_paper(state: &AppState, id: &str, sid: &str) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    let receipt = state.with_conn(|conn| project::remove_paper(conn, pid, sid))?;
    crate::route::to_value(&receipt)
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
            req(&state(), "GET", "/api/projects", None).await.unwrap(),
            json!({ "projects": [] })
        );
    }

    #[tokio::test]
    async fn get_missing_project_is_404() {
        let err = req(&state(), "GET", "/api/projects/999", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project 999 not found");
    }

    #[tokio::test]
    async fn non_integer_id_is_422() {
        let err = req(&state(), "GET", "/api/projects/abc", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn create_blank_name_is_400() {
        let err = req(
            &state(),
            "POST",
            "/api/projects",
            Some(json!({ "name": "   " })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
        assert_eq!(err.detail, "name cannot be blank");
    }

    #[tokio::test]
    async fn create_bad_color_is_400() {
        let err = req(
            &state(),
            "POST",
            "/api/projects",
            Some(json!({ "name": "P", "color_hex": "zzz" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
        assert_eq!(err.detail, "Invalid color_hex");
    }

    /// Canonical wire keys of `ProjectOut` (SERIALIZER 3), in order.
    const WIRE_KEYS: [&str; 12] = [
        "id",
        "name",
        "description",
        "color_hex",
        "project_tags",
        "source_ids",
        "paper_count",
        "status",
        "created_at",
        "updated_at",
        "archived_at",
        "share_id",
    ];

    fn assert_wire_shape(v: &Value, pid: i64) {
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, WIRE_KEYS);
        assert_eq!(v["id"], json!(pid));
        assert_eq!(v["name"], json!("RL"));
        assert_eq!(v["description"], json!(""));
        assert_eq!(v["color_hex"], json!("#00ff00"));
        assert_eq!(v["project_tags"], json!([]));
        assert_eq!(v["source_ids"], json!([]));
        assert_eq!(v["paper_count"], json!(0));
        assert_eq!(v["status"], json!("active"));
        assert_eq!(v["archived_at"], Value::Null);
        assert_eq!(v["share_id"], Value::Null);
    }

    #[tokio::test]
    async fn create_then_get_and_list_emit_canonical_wire_shape() {
        let st = state();
        let created = req(
            &st,
            "POST",
            "/api/projects",
            Some(json!({ "name": "RL", "color_hex": "#00ff00" })),
        )
        .await
        .unwrap();
        let pid = created["project"]["id"].as_i64().unwrap();
        assert_eq!(created, json!({ "project": { "id": pid, "name": "RL" } }));

        // get and list emit the same canonical shape (paper_count included on both).
        let got = req(&st, "GET", &format!("/api/projects/{pid}"), None)
            .await
            .unwrap();
        assert_wire_shape(&got, pid);

        let listed = req(&st, "GET", "/api/projects", None).await.unwrap();
        assert_wire_shape(&listed["projects"][0], pid);
    }

    #[tokio::test]
    async fn patch_invalid_status_is_400() {
        let err = req(
            &state(),
            "PATCH",
            "/api/projects/1",
            Some(json!({ "status": "nope" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 400);
        // Single-sourced from `Status: FromStr` — route, CLI and MCP word it alike.
        assert_eq!(
            err.detail,
            "Invalid status 'nope'. Use 'active', 'archived', or 'deleted'."
        );
    }

    #[tokio::test]
    async fn patch_missing_project_is_404() {
        let err = req(
            &state(),
            "PATCH",
            "/api/projects/999",
            Some(json!({ "name": "x" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project 999 not found");
    }

    #[tokio::test]
    async fn delete_missing_project_is_404() {
        let err = req(&state(), "DELETE", "/api/projects/999", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project 999 not found");
    }

    #[tokio::test]
    async fn add_paper_missing_project_is_404() {
        let err = req(
            &state(),
            "POST",
            "/api/projects/999/papers",
            Some(json!({ "source_id": "arxiv:1" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project 999 not found");
    }

    #[tokio::test]
    async fn add_unknown_paper_to_real_project_is_404() {
        let st = state();
        let created = req(&st, "POST", "/api/projects", Some(json!({ "name": "P" })))
            .await
            .unwrap();
        let pid = created["project"]["id"].as_i64().unwrap();
        let err = req(
            &st,
            "POST",
            &format!("/api/projects/{pid}/papers"),
            Some(json!({ "source_id": "ghost" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper ghost not found");
    }

    #[tokio::test]
    async fn bulk_add_reports_failed_verbatim() {
        let st = state();
        let created = req(&st, "POST", "/api/projects", Some(json!({ "name": "P" })))
            .await
            .unwrap();
        let pid = created["project"]["id"].as_i64().unwrap();
        let out = req(
            &st,
            "POST",
            &format!("/api/projects/{pid}/papers/bulk"),
            Some(json!({ "source_ids": ["ghost"] })),
        )
        .await
        .unwrap();
        assert_eq!(out, json!({ "ok": false, "failed": ["ghost"] }));
    }
}
