//! `/api/{arxiv,openalex,doi}` source routes — `api/app.py` 740–850, 1177–1187,
//! 1475–1489. These arms `.await` the source layer (live arXiv/OpenAlex/CrossRef
//! HTTP), then save through `service::paper`. Core binding mirrors
//! `mcp/src/papers.rs` (search/fetch) + `mcp/src/io_authors_misc.rs` (doi).
//!
//! The shared wire shape is `SearchResultOut` (models.rs SERIALIZER 1): it strips
//! the source namespace from `source_id`, blanks `published` on the `date.min`
//! sentinel, renames url→paper_url / category→primary_category, and keeps the full
//! id in `entry_id`. `to_search_result` is the single mapping point for every
//! arxiv/openalex search + fetch arm.

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::config;
use linxiv_core::error::CoreError;
use linxiv_core::models::{strip_namespace, PaperMetadata, SearchResultOut};
use linxiv_core::service::paper as svc_paper;
use linxiv_core::sources::{doi_resolve, fetch as svc_fetch};

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("POST", ["api", "arxiv", "search"]) => Some(arxiv_search(state, ctx).await),
        ("POST", ["api", "arxiv", "fetch"]) => Some(arxiv_fetch(state, ctx).await),
        ("POST", ["api", "openalex", "search"]) => Some(openalex_search(ctx).await),
        ("POST", ["api", "openalex", "save"]) => Some(openalex_save(state, ctx).await),
        ("POST", ["api", "doi", "resolve"]) => Some(doi_resolve_route(ctx).await),
        ("POST", ["api", "doi", "save"]) => Some(doi_save_route(state, ctx).await),
        _ => None,
    }
}

/// `SearchResultOut.from_metadata` — the one mapping shared by every search/fetch
/// arm. Delegates to the core serializer (models.rs SERIALIZER 1).
fn to_search_result(meta: PaperMetadata) -> SearchResultOut {
    SearchResultOut::from(meta)
}

/// OpenAlex polite-pool address, mirroring the CLI/MCP/Python source clients.
fn mailto() -> String {
    std::env::var("OPENALEX_MAILTO").unwrap_or_default()
}

/// `api/app.py`'s `except Exception: 502` for the search/fetch source calls.
fn upstream_502(e: CoreError) -> ApiError {
    ApiError::new(502, e.to_string())
}

/// `meta.model_dump(mode="json")` — the full PaperMetadata serde shape.
fn meta_json(meta: &PaperMetadata) -> Result<Value, ApiError> {
    serde_json::to_value(meta).map_err(|e| ApiError::new(500, e.to_string()))
}

fn default_max_results() -> i64 {
    25
}
fn default_sort() -> String {
    "relevance".to_string()
}
fn default_true() -> bool {
    true
}

/// `POST /api/arxiv/search` (740–758). `502` on any source error; `save` swallows
/// an IntegrityError (returns results with `saved_source_ids=[]`, as Python does).
async fn arxiv_search(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        query: String,
        #[serde(default = "default_max_results")]
        max_results: i64,
        #[serde(default)]
        save: bool,
        #[serde(default = "default_sort")]
        sort: String,
    }
    let b: Body = ctx.parse_body()?;
    if b.query.is_empty() {
        return Err(ApiError::new(422, "query must not be empty"));
    }
    let results = svc_fetch::search(
        "arxiv",
        b.query.trim(),
        b.max_results as u32,
        &b.sort,
        &config::data_dir(),
        &mailto(),
    )
    .await
    .map_err(upstream_502)?;

    let saved = if b.save && !results.is_empty() {
        // ponytail: per-item save loop — core has no atomic `save_papers_metadata`.
        // Ceiling: not one transaction, so an IntegrityError on item N leaves items
        // <N committed while we report `saved=[]` (Python's batch rolls all back).
        // Upgrade path: add a batched save_papers_metadata to core if it bites.
        state.with_conn(|conn| -> Result<Vec<String>, ApiError> {
            let mut out = Vec::new();
            for m in &results {
                match svc_paper::save_paper_metadata(conn, m, None) {
                    Ok((sid, _)) => out.push(strip_namespace(&sid)),
                    Err(CoreError::Conflict(_)) => return Ok(Vec::new()), // swallow, like app.py
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(out)
        })?
    } else {
        Vec::new()
    };

    let results: Vec<SearchResultOut> = results.into_iter().map(to_search_result).collect();
    Ok(json!({ "results": results, "saved_source_ids": saved }))
}

/// `POST /api/arxiv/fetch` (804–823). `404` not-found / `502` other / `409` on a
/// save IntegrityError (CoreError::Conflict → 409 via `?`).
async fn arxiv_fetch(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        source_id: String,
        #[serde(default = "default_true")]
        save: bool,
    }
    let b: Body = ctx.parse_body()?;
    if b.source_id.is_empty() {
        return Err(ApiError::new(422, "source_id must not be empty"));
    }
    let meta = svc_fetch::fetch_by_id("arxiv", b.source_id.trim(), &config::data_dir(), &mailto())
        .await
        .map_err(|e| match e {
            CoreError::ArxivNotFound(m) => ApiError::new(404, m),
            other => ApiError::new(502, other.to_string()),
        })?;
    let mut source_id = strip_namespace(&meta.source_id);
    if b.save {
        let (stored, _) =
            state.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))?;
        source_id = strip_namespace(&stored);
    }
    let paper = to_search_result(meta);
    Ok(json!({ "paper": paper, "saved": b.save, "source_id": source_id }))
}

/// `POST /api/openalex/search` (1177–1187). `502` on any source error.
async fn openalex_search(ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        query: String,
        #[serde(default = "default_max_results")]
        max_results: i64,
        #[serde(default = "default_sort")]
        sort: String,
    }
    let b: Body = ctx.parse_body()?;
    if b.query.is_empty() {
        return Err(ApiError::new(422, "query must not be empty"));
    }
    let results = svc_fetch::search(
        "openalex",
        b.query.trim(),
        b.max_results as u32,
        &b.sort,
        &config::data_dir(),
        &mailto(),
    )
    .await
    .map_err(upstream_502)?;
    let results: Vec<SearchResultOut> = results.into_iter().map(to_search_result).collect();
    Ok(json!({ "results": results }))
}

/// `POST /api/openalex/save` (1475–1489). `404`/`400`/`502` on fetch, `409` on a
/// save IntegrityError. Returns the stripped stored source_id.
async fn openalex_save(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        source_id: String,
    }
    let b: Body = ctx.parse_body()?;
    if b.source_id.is_empty() {
        return Err(ApiError::new(422, "source_id must not be empty"));
    }
    let meta = svc_fetch::fetch_by_id("openalex", b.source_id.trim(), &config::data_dir(), &mailto())
        .await
        .map_err(|e| match e {
            CoreError::OpenAlexNotFound(m) => ApiError::new(404, m),
            CoreError::OpenAlexInput(m) => ApiError::new(400, m),
            other => ApiError::new(502, other.to_string()),
        })?;
    let (stored, _) = state.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))?;
    Ok(json!({ "saved": true, "source_id": strip_namespace(&stored) }))
}

/// `POST /api/doi/resolve` (830–836). `400` on a bad DOI (CoreError::BadRequest →
/// 400 via `?`, matching Python's `ValueError`).
async fn doi_resolve_route(ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        doi: String,
    }
    let b: Body = ctx.parse_body()?;
    if b.doi.is_empty() {
        return Err(ApiError::new(422, "doi must not be empty"));
    }
    let meta = doi_resolve::resolve_doi(b.doi.trim(), &config::data_dir()).await?;
    Ok(json!({ "metadata": meta_json(&meta)? }))
}

/// `POST /api/doi/save` (843–850). Resolve then save; returns the resolved meta.
async fn doi_save_route(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        doi: String,
    }
    let b: Body = ctx.parse_body()?;
    if b.doi.is_empty() {
        return Err(ApiError::new(422, "doi must not be empty"));
    }
    let meta = doi_resolve::resolve_doi(b.doi.trim(), &config::data_dir()).await?;
    state.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))?;
    Ok(json!({ "metadata": meta_json(&meta)?, "saved": true }))
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

    async fn post(st: &AppState, path: &str, body: Value) -> Result<Value, ApiError> {
        route(st, ApiRequest { method: "POST".into(), path: path.into(), body: Some(body) }).await
    }

    /// Build a synthetic PaperMetadata through serde (the app crate has no chrono dep).
    fn meta(published: &str, url: Value, category: Value) -> PaperMetadata {
        serde_json::from_value(json!({
            "source_id": "arxiv:2204.12985",
            "version": 2,
            "title": "T",
            "authors": ["A", "B"],
            "published": published,
            "summary": "S",
            "category": category,
            "url": url,
        }))
        .unwrap()
    }

    #[test]
    fn to_search_result_strips_namespace_and_renames_fields() {
        let v = serde_json::to_value(to_search_result(meta(
            "2024-01-15",
            json!("http://x"),
            json!("cs.LG"),
        )))
        .unwrap();
        // Exact wire shape + key order (SearchResultOut field order is the contract).
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"source_id":"2204.12985","version":2,"title":"T","summary":"S","authors":["A","B"],"published":"2024-01-15","paper_url":"http://x","primary_category":"cs.LG","entry_id":"arxiv:2204.12985"}"#
        );
    }

    #[test]
    fn to_search_result_blanks_date_min_and_defaults_nulls() {
        let v = serde_json::to_value(to_search_result(meta("0001-01-01", Value::Null, Value::Null)))
            .unwrap();
        assert_eq!(v["published"], json!(""));
        assert_eq!(v["paper_url"], json!(""));
        assert_eq!(v["primary_category"], json!(""));
        assert_eq!(v["entry_id"], json!("arxiv:2204.12985"));
    }

    #[tokio::test]
    async fn empty_query_is_422_before_any_network() {
        for path in ["/api/arxiv/search", "/api/openalex/search"] {
            let err = post(&state(), path, json!({ "query": "" })).await.unwrap_err();
            assert_eq!(err.status, 422);
        }
    }

    #[tokio::test]
    async fn empty_source_id_and_doi_are_422_before_any_network() {
        for path in ["/api/arxiv/fetch", "/api/openalex/save"] {
            let err = post(&state(), path, json!({ "source_id": "" })).await.unwrap_err();
            assert_eq!(err.status, 422);
        }
        for path in ["/api/doi/resolve", "/api/doi/save"] {
            let err = post(&state(), path, json!({ "doi": "" })).await.unwrap_err();
            assert_eq!(err.status, 422);
        }
    }
}
