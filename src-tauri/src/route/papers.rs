//! `/api/papers` routes — `api/app.py` 204–261, 365–379, 433–461, 1135–1139.
//! Mirrors the `papers` MCP cluster (`mcp/src/papers.rs`) over the same core
//! service (`service::paper`). Shape copied from `route/authors.rs`.
//!
//! The generic `{source_id}` arms match EXACTLY 3 segments; the `/pdf` and
//! `/pdf-path` subtrees belong to the `pdfs` group (tried first in `mod.rs`).

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::models::PaperMetadata;
use linxiv_core::service::paper::{self as svc_paper, Paper};
use linxiv_core::service::project as svc_project;
use linxiv_core::storage::queries::{note as store_note, search as store_search};

use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "papers"]) => Some(list(state, ctx)),
        ("GET", ["api", "papers", "sfk", fk, "versions"]) => Some(versions(state, fk)),
        ("GET", ["api", "papers", "sfk", fk]) => Some(by_sfk(state, fk, ctx)),
        ("PUT", ["api", "papers", "sfk", fk]) => Some(repair(state, fk, ctx)),
        ("DELETE", ["api", "papers", "sfk", fk, "projects"]) => Some(remove_from_projects(state, fk)),
        // `search` must precede the generic `{source_id}` arm (both 3 segments).
        ("GET", ["api", "papers", "search"]) => Some(search(state, ctx)),
        ("GET", ["api", "papers", id]) => Some(get_one(state, id)),
        ("DELETE", ["api", "papers", id]) => Some(delete(state, id)),
        _ => None,
    }
}

/// `GET /api/papers?limit=&offset=` — `api_list_papers`.
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    // ponytail: FastAPI's Query(ge/le) 422s out-of-range; we clamp instead (the
    // contract task's call) — the frontend never sends out-of-range values.
    let limit = ctx.q_i64("limit").unwrap_or(200).clamp(1, 5000);
    let offset = ctx.q_i64("offset").unwrap_or(0).max(0);
    let papers =
        state.with_conn(|conn| svc_paper::list_papers(conn, true, Some(limit), offset, None))?;
    Ok(json!({ "papers": papers }))
}

/// `GET /api/papers/sfk/{fk}/versions` — `api_get_paper_versions`.
fn versions(state: &AppState, fk: &str) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    let all = state.with_conn(|conn| svc_paper::get_all(conn, &sfk_key(source_fk)))?;
    let all = all.ok_or_else(|| ApiError::new(404, "Paper not found"))?;
    let versions: Vec<Value> = all
        .versions
        .iter()
        .map(|v| {
            json!({
                "version": v.version,
                "published": v.published, // Option<NaiveDate> -> ISO string or null
                "updated": v.updated,
                "has_pdf": v.has_pdf,
            })
        })
        .collect();
    Ok(json!({
        "source_id": all.source_id,
        "latest_version": all.latest_version,
        "versions": versions,
    }))
}

/// `GET /api/papers/sfk/{fk}?version=` — `api_get_paper_by_sfk`. Bare `to_dict()`.
fn by_sfk(state: &AppState, fk: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    state.with_conn(|conn| -> Result<Value, ApiError> {
        let paper = if let Some(version) = ctx.q_i64("version") {
            // version branch: resolve source_id first, then the pinned version.
            let source_id = svc_paper::get_source_id(conn, source_fk)?
                .ok_or_else(|| ApiError::new(404, "Paper not found"))?;
            let key =
                Paper { source_id: Some(source_id), version: Some(version), ..Default::default() };
            svc_paper::get(conn, &key)?
                .ok_or_else(|| ApiError::new(404, format!("Version {version} not stored")))?
        } else {
            svc_paper::get(conn, &sfk_key(source_fk))?
                .ok_or_else(|| ApiError::new(404, "Paper not found"))?
        };
        to_value(&paper)
    })
}

/// `GET /api/papers/search?q=&limit=` — `api_search_papers`.
fn search(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let q = ctx.q("q").unwrap_or("").trim().to_string();
    if q.chars().count() < 3 {
        return Err(ApiError::new(
            422,
            "Query must contain at least 3 non-whitespace characters",
        ));
    }
    let limit = ctx.q_i64("limit").unwrap_or(50).clamp(1, 100);
    let papers = state.with_conn(|conn| -> Result<Vec<_>, ApiError> {
        // ponytail: core has no service::paper::search_papers; recompose Python's
        // FTS+notes merge here (FTS first, then note-linked papers, dedup, cap).
        // FTS syntax errors fall back to [] like Python's OperationalError catch.
        let mut papers = store_search::search_full_text(conn, &q, limit).unwrap_or_default();
        let mut seen: HashSet<String> = papers.iter().map(|p| p.source_id.clone()).collect();
        for sfk in store_note::search_notes_source_fks(conn, &q, limit)? {
            if let Some(p) = svc_paper::get(conn, &sfk_key(sfk))? {
                if seen.insert(p.source_id.clone()) {
                    papers.push(p);
                }
            }
        }
        papers.truncate(limit as usize);
        Ok(papers)
    })?;
    Ok(json!({ "papers": papers }))
}

/// `GET /api/papers/{source_id}` — `api_get_paper`. Bare `to_dict()`.
fn get_one(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    let paper = state.with_conn(|conn| svc_paper::get(conn, &sid_key(source_id)))?;
    let paper = paper.ok_or_else(|| ApiError::new(404, "Paper not found"))?;
    to_value(&paper)
}

/// `DELETE /api/papers/{source_id}` — `api_delete_paper`.
fn delete(state: &AppState, source_id: &str) -> Result<Value, ApiError> {
    state.with_conn(|conn| -> Result<(), ApiError> {
        if svc_paper::get(conn, &sid_key(source_id))?.is_none() {
            return Err(ApiError::new(404, "Paper not found"));
        }
        svc_paper::delete(conn, &sid_key(source_id))?;
        Ok(())
    })?;
    Ok(json!({ "deleted": source_id }))
}

/// `PUT /api/papers/sfk/{fk}` — `api_repair_paper`. Rebuilds metadata from the
/// existing paper's identity (source_id/version/source) + the PUT body.
fn repair(state: &AppState, fk: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    let b: RepairBody = ctx.parse_body()?;
    state.with_conn(|conn| -> Result<Value, ApiError> {
        let paper = svc_paper::get(conn, &sfk_key(source_fk))?
            .ok_or_else(|| ApiError::new(404, "Paper not found"))?;
        let meta = PaperMetadata {
            source_id: paper.source_id, // identity key; not changeable here (ADR-0008)
            version: paper.version,
            title: b.title,
            authors: b.authors,
            published: b.published,
            updated: None,
            summary: b.summary,
            category: b.category,
            categories: None,
            doi: b.doi,
            journal_ref: None,
            comment: None,
            url: b.url,
            tags: b.tags,
            source: paper.source,
        };
        // ponytail: Python maps sqlite3.IntegrityError -> 409, but this endpoint
        // never renames source_id so no UNIQUE conflict can arise; a stray rusqlite
        // error surfaces as CoreError::Internal (500). Reachable paths stay faithful.
        svc_paper::repair_paper(conn, source_fk, &meta)?;
        let updated = svc_paper::get(conn, &sfk_key(source_fk))?
            .ok_or_else(|| ApiError::new(500, "Repair failed"))?;
        to_value(&updated)
    })
}

/// `DELETE /api/papers/sfk/{fk}/projects` — `api_remove_paper_from_all_projects`.
fn remove_from_projects(state: &AppState, fk: &str) -> Result<Value, ApiError> {
    let source_fk = path_i64(fk)?;
    let removed =
        state.with_conn(|conn| svc_project::remove_paper_from_all_projects(conn, source_fk))?;
    Ok(json!({ "ok": true, "removed_from": removed }))
}

/// `PaperRepairBody` (`src/api/papers.ts`). `published` binds as a `NaiveDate`
/// (FastAPI `datetime.date`); a malformed date 422s via `parse_body`.
#[derive(Deserialize)]
struct RepairBody {
    title: String,
    authors: Vec<String>,
    published: chrono::NaiveDate,
    summary: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    doi: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

fn sfk_key(source_fk: i64) -> Paper {
    Paper { source_fk: Some(source_fk), ..Default::default() }
}

fn sid_key(source_id: &str) -> Paper {
    Paper { source_id: Some(source_id.to_string()), ..Default::default() }
}

/// Serialize a domain struct == Python `to_dict()`; an encode failure is a 500.
fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError::new(500, e.to_string()))
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
        assert_eq!(req(&state(), "GET", "/api/papers", None).await.unwrap(), json!({ "papers": [] }));
    }

    #[tokio::test]
    async fn get_missing_paper_is_404() {
        let err = req(&state(), "GET", "/api/papers/arxiv:nope", None).await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper not found");
    }

    #[tokio::test]
    async fn delete_missing_paper_is_404() {
        let err = req(&state(), "DELETE", "/api/papers/arxiv:nope", None).await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper not found");
    }

    #[tokio::test]
    async fn versions_missing_is_404() {
        let err = req(&state(), "GET", "/api/papers/sfk/999/versions", None).await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper not found");
    }

    #[tokio::test]
    async fn by_sfk_missing_is_404_both_branches() {
        let st = state();
        assert_eq!(
            req(&st, "GET", "/api/papers/sfk/999", None).await.unwrap_err().detail,
            "Paper not found"
        );
        // version branch: unknown sfk -> "Paper not found" (source_id resolves to None).
        assert_eq!(
            req(&st, "GET", "/api/papers/sfk/999?version=2", None).await.unwrap_err().detail,
            "Paper not found"
        );
    }

    #[tokio::test]
    async fn non_integer_sfk_is_422() {
        let err = req(&state(), "GET", "/api/papers/sfk/abc/versions", None).await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn search_short_query_is_422() {
        let err = req(&state(), "GET", "/api/papers/search?q=ab", None).await.unwrap_err();
        assert_eq!(err.status, 422);
        assert_eq!(err.detail, "Query must contain at least 3 non-whitespace characters");
    }

    #[tokio::test]
    async fn search_whitespace_only_query_is_422() {
        // q is trimmed before the length check (matches app.py `q.strip()`).
        let err = req(&state(), "GET", "/api/papers/search?q=%20%20a%20%20", None).await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn search_empty_db_wraps_empty_array() {
        assert_eq!(
            req(&state(), "GET", "/api/papers/search?q=manifold", None).await.unwrap(),
            json!({ "papers": [] })
        );
    }

    #[tokio::test]
    async fn repair_missing_paper_is_404() {
        let body = json!({"title":"T","authors":["A"],"published":"2024-01-01","summary":"s"});
        let err = req(&state(), "PUT", "/api/papers/sfk/999", Some(body)).await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper not found");
    }

    #[tokio::test]
    async fn repair_bad_date_is_422() {
        let body = json!({"title":"T","authors":["A"],"published":"not-a-date","summary":"s"});
        let err = req(&state(), "PUT", "/api/papers/sfk/1", Some(body)).await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn remove_from_projects_empty_is_ok() {
        assert_eq!(
            req(&state(), "DELETE", "/api/papers/sfk/999/projects", None).await.unwrap(),
            json!({ "ok": true, "removed_from": [] })
        );
    }
}
