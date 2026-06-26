//! `/api/projects` routes — `api/app.py` 512–688. Mirrors `authors.rs`: a `handle`
//! owning the `/api/projects` subtree, path-param extraction, body deserialization,
//! and the exact JSON envelopes / status codes `app.py` returned. Core binding
//! follows `mcp/src/projects_tags.rs`.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::error::CoreError;
use linxiv_core::formats;
use linxiv_core::models::{ProjectDetails, ProjectIn, ProjectUpdateIn, Status};
use linxiv_core::service::paper as svc_paper;
use linxiv_core::service::project::{self, Project, Projects};
use linxiv_core::service::export_import;

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
        ("POST", ["api", "projects", id, "papers", "bulk"]) => Some(add_papers_bulk(state, id, ctx)),
        ("DELETE", ["api", "projects", id, "papers", sid]) => Some(remove_paper(state, id, sid)),
        ("POST", ["api", "projects", id, "export"]) => Some(export(state, id, ctx)),
        ("GET", ["api", "projects", id, "export", "bibtex"]) => Some(export_text(state, id, ctx, Fmt::Bibtex)),
        ("GET", ["api", "projects", id, "export", "obsidian"]) => Some(export_text(state, id, ctx, Fmt::Obsidian)),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Fmt {
    Bibtex,
    Obsidian,
}

/// `POST /api/projects/{id}/export` — `api_project_export` (dest_path branch only).
/// Writes the `.lxproj` archive to `dest_path` and returns `{ok}`. The streaming
/// (no-dest_path) branch is browser-only; the Tauri frontend always sends a path.
fn export(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let project_fk = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        dest_path: Option<String>,
        #[serde(default)]
        include_pdfs: bool,
    }
    let b: Body = ctx.parse_body()?;
    let Some(dest) = b.dest_path.filter(|s| !s.is_empty()) else {
        return Err(ApiError::new(400, "dest_path is required for in-process export"));
    };
    let pdf_dir = state.pdf_dir.clone();
    state.with_conn(|conn| -> Result<(), ApiError> {
        // app.py maps export_project's ValueError -> 404 with `str(e)` =
        // "Project {fk} not found"; pre-check to produce the same status + message.
        if project::get(conn, &Project { project_fk: Some(project_fk) })?.is_none() {
            return Err(ApiError::new(404, format!("Project {project_fk} not found")));
        }
        export_import::export_project(conn, project_fk, Path::new(&dest), b.include_pdfs, &pdf_dir)?;
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

/// `GET /api/projects/{id}/export/{bibtex,obsidian}?dest_path=` — the dest_path
/// branch of the text exporters. Writes the formatted project to disk, `{ok}`.
fn export_text(state: &AppState, id: &str, ctx: &ReqCtx<'_>, fmt: Fmt) -> Result<Value, ApiError> {
    let project_fk = path_i64(id)?;
    let Some(dest) = ctx.q("dest_path").filter(|s| !s.is_empty()) else {
        return Err(ApiError::new(400, "dest_path is required for in-process export"));
    };
    let content = state.with_conn(|conn| -> Result<String, ApiError> {
        let proj = project::get(conn, &Project { project_fk: Some(project_fk) })?
            .ok_or_else(|| ApiError::new(404, "Project not found"))?;
        let ids: HashSet<String> =
            svc_paper::sfks_to_source_ids(conn, &proj.source_fks)?.into_iter().collect();
        // Iterate the latest-papers view in its order, keeping the project's papers
        // (matches app.py's `[p for p in list_paper_details(latest) if id in ids]`).
        let papers: Vec<_> = svc_paper::list_papers(conn, true, None, 0, None)?
            .into_iter()
            .filter(|p| ids.contains(&p.source_id))
            .collect();
        Ok(match fmt {
            Fmt::Bibtex => formats::bibtex_export(&papers),
            Fmt::Obsidian => formats::obsidian_export(&papers),
        })
    })?;
    std::fs::write(dest, content).map_err(|e| ApiError::new(500, e.to_string()))?;
    Ok(json!({ "ok": true }))
}

/// `Status(s)` — the three lifecycle strings, else None (caller decides the error).
fn status_from_str(s: &str) -> Option<Status> {
    match s {
        "active" => Some(Status::Active),
        "archived" => Some(Status::Archived),
        "deleted" => Some(Status::Deleted),
        _ => None,
    }
}

/// `app.py` builds each project dict the same way for list + get. Consumes the row
/// (callers own it). Emits the 7 shared keys in order; the list arm appends
/// `paper_count` after `status`, the get arm stops here.
fn project_to_dict(conn: &Connection, p: ProjectDetails) -> Result<Value, ApiError> {
    let source_ids = svc_paper::sfks_to_source_ids(conn, &p.source_fks)?;
    let color_hex = p.color.map(project::color_to_hex);
    Ok(json!({
        "id": p.id,
        "name": p.name,
        "description": p.description,
        "color_hex": color_hex,
        "project_tags": p.project_tags,
        "source_ids": source_ids,
        "status": p.status,
    }))
}

/// `GET /api/projects?status=` — `api_projects`. Default "active"; "all" => no filter.
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let status = ctx.q("status").unwrap_or("active").to_string();
    let out = state.with_conn(|conn| -> Result<Vec<Value>, ApiError> {
        let filter = match status.as_str() {
            "all" => Projects::default(),
            s => match status_from_str(s) {
                Some(st) => Projects { status: Some(st), ..Default::default() },
                None => return Ok(Vec::new()),
            },
        };
        let mut out = Vec::new();
        for p in project::get_many(conn, &filter)? {
            if p.id.is_none() {
                continue; // app.py drops null-id rows (data-integrity guard)
            }
            let mut obj = project_to_dict(conn, p)?;
            if let Value::Object(m) = &mut obj {
                let count = m.get("source_ids").and_then(Value::as_array).map_or(0, Vec::len);
                m.insert("paper_count".into(), json!(count));
            }
            out.push(obj);
        }
        Ok(out)
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
        #[serde(default)]
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

/// `GET /api/projects/{id}` — `api_project_get`.
fn get_one(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    state.with_conn(|conn| {
        let p = project::get(conn, &Project { project_fk: Some(pid) })?
            .ok_or_else(|| ApiError::new(404, "Project not found"))?;
        project_to_dict(conn, p)
    })
}

/// `PATCH /api/projects/{id}` — `api_project_patch`. Partial update; color cleared
/// only when the `color_hex` key is present in the body.
fn patch(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        color_hex: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        project_tags: Option<Vec<String>>,
    }
    let b: Body = ctx.parse_body()?;

    let status = match b.status {
        Some(s) => Some(status_from_str(&s).ok_or_else(|| ApiError::new(400, "Invalid status"))?),
        None => None,
    };
    // color: only touched when the key was explicitly sent (app.py model_fields_set).
    // Sent + non-empty => set; sent + null/"" => clear (Some(None)); absent => unchanged.
    let color_sent =
        ctx.body.and_then(Value::as_object).is_some_and(|m| m.contains_key("color_hex"));
    let color = if color_sent { Some(parse_color(b.color_hex.as_deref())?) } else { None };
    let name = match b.name {
        Some(n) => {
            let n = n.trim().to_string();
            if n.is_empty() {
                return Err(ApiError::new(400, "name cannot be blank"));
            }
            Some(n)
        }
        None => None,
    };
    let upd = ProjectUpdateIn {
        project_fk: pid,
        name,
        description: b.description.map(|d| d.trim().to_string()),
        color,
        project_tags: b.project_tags,
        status,
    };
    state.with_conn(|conn| project::update(conn, &upd)).map_err(|e| match e {
        // app.py: LookupError -> 404 "Project not found"; ValueError -> 400 str(e).
        CoreError::ProjectNotFound => ApiError::new(404, "Project not found"),
        CoreError::ProjectDeleted(m) | CoreError::Validation(m) => ApiError::new(400, m),
        other => other.into(),
    })?;
    Ok(json!({ "ok": true }))
}

/// `DELETE /api/projects/{id}` — `api_project_delete`. 404 if absent, then soft-delete.
fn delete(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        if project::get(conn, &Project { project_fk: Some(pid) })?.is_none() {
            return Err(ApiError::new(404, "Project not found"));
        }
        project::delete(conn, &Project { project_fk: Some(pid) })?;
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

/// Map the `add_papers`/`remove_papers` membership guards to app.py's status codes.
fn map_membership(r: linxiv_core::error::Result<Vec<String>>) -> Result<Vec<String>, ApiError> {
    r.map_err(|e| match e {
        CoreError::ProjectNotFound => ApiError::new(404, "Project not found"),
        CoreError::ProjectDeleted(m) => ApiError::new(400, m),
        other => other.into(),
    })
}

/// `POST /api/projects/{id}/papers` — `api_project_add_paper`.
fn add_paper(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        source_id: String,
    }
    let b: Body = ctx.parse_body()?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        let failed = map_membership(project::add_papers(conn, pid, &[b.source_id]))?;
        if !failed.is_empty() {
            return Err(ApiError::new(404, "Paper not found"));
        }
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

/// `POST /api/projects/{id}/papers/bulk` — `api_project_add_papers`. Partial success.
fn add_papers_bulk(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        source_ids: Vec<String>,
    }
    let b: Body = ctx.parse_body()?;
    let failed =
        state.with_conn(|conn| map_membership(project::add_papers(conn, pid, &b.source_ids)))?;
    Ok(json!({ "ok": failed.is_empty(), "failed": failed }))
}

/// `DELETE /api/projects/{id}/papers/{sid}` — `api_project_remove_paper`. `sid`
/// arrives already percent-decoded in `ctx.segs`.
fn remove_paper(state: &AppState, id: &str, sid: &str) -> Result<Value, ApiError> {
    let pid = path_i64(id)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        let failed = map_membership(project::remove_papers(conn, pid, &[sid.to_string()]))?;
        if !failed.is_empty() {
            return Err(ApiError::new(404, "Paper not found"));
        }
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

    async fn req(
        st: &AppState,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        route(st, ApiRequest { method: method.into(), path: path.into(), body }).await
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
        let err = req(&state(), "GET", "/api/projects/999", None).await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project not found");
    }

    #[tokio::test]
    async fn non_integer_id_is_422() {
        let err = req(&state(), "GET", "/api/projects/abc", None).await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn create_blank_name_is_400() {
        let err = req(&state(), "POST", "/api/projects", Some(json!({ "name": "   " })))
            .await
            .unwrap_err();
        assert_eq!(err.status, 400);
        assert_eq!(err.detail, "name cannot be blank");
    }

    #[tokio::test]
    async fn create_bad_color_is_400() {
        let err =
            req(&state(), "POST", "/api/projects", Some(json!({ "name": "P", "color_hex": "zzz" })))
                .await
                .unwrap_err();
        assert_eq!(err.status, 400);
        assert_eq!(err.detail, "Invalid color_hex");
    }

    #[tokio::test]
    async fn create_then_get_and_list_match_envelopes() {
        let st = state();
        let created =
            req(&st, "POST", "/api/projects", Some(json!({ "name": "RL", "color_hex": "#00ff00" })))
                .await
                .unwrap();
        let pid = created["project"]["id"].as_i64().unwrap();
        assert_eq!(created, json!({ "project": { "id": pid, "name": "RL" } }));

        let got = req(&st, "GET", &format!("/api/projects/{pid}"), None).await.unwrap();
        assert_eq!(
            serde_json::to_string(&got).unwrap(),
            format!(
                r##"{{"id":{pid},"name":"RL","description":"","color_hex":"#00ff00","project_tags":[],"source_ids":[],"status":"active"}}"##
            )
        );

        // list appends paper_count after status, in order.
        let listed = req(&st, "GET", "/api/projects", None).await.unwrap();
        assert_eq!(
            serde_json::to_string(&listed).unwrap(),
            format!(
                r##"{{"projects":[{{"id":{pid},"name":"RL","description":"","color_hex":"#00ff00","project_tags":[],"source_ids":[],"status":"active","paper_count":0}}]}}"##
            )
        );
    }

    #[tokio::test]
    async fn patch_invalid_status_is_400() {
        let err = req(&state(), "PATCH", "/api/projects/1", Some(json!({ "status": "nope" })))
            .await
            .unwrap_err();
        assert_eq!(err.status, 400);
        assert_eq!(err.detail, "Invalid status");
    }

    #[tokio::test]
    async fn patch_missing_project_is_404() {
        let err = req(&state(), "PATCH", "/api/projects/999", Some(json!({ "name": "x" })))
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project not found");
    }

    #[tokio::test]
    async fn delete_missing_project_is_404() {
        let err = req(&state(), "DELETE", "/api/projects/999", None).await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project not found");
    }

    #[tokio::test]
    async fn add_paper_missing_project_is_404() {
        let err =
            req(&state(), "POST", "/api/projects/999/papers", Some(json!({ "source_id": "arxiv:1" })))
                .await
                .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Project not found");
    }

    #[tokio::test]
    async fn add_unknown_paper_to_real_project_is_404() {
        let st = state();
        let created = req(&st, "POST", "/api/projects", Some(json!({ "name": "P" }))).await.unwrap();
        let pid = created["project"]["id"].as_i64().unwrap();
        let err =
            req(&st, "POST", &format!("/api/projects/{pid}/papers"), Some(json!({ "source_id": "ghost" })))
                .await
                .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper not found");
    }

    #[tokio::test]
    async fn bulk_add_reports_failed_verbatim() {
        let st = state();
        let created = req(&st, "POST", "/api/projects", Some(json!({ "name": "P" }))).await.unwrap();
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
