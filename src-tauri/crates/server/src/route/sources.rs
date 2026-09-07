//! `/api/{arxiv,openalex,doi}` source routes — `api/app.py` 740–850, 1177–1187,
//! 1475–1489. These arms `.await` the source layer (live arXiv/OpenAlex/CrossRef
//! HTTP), then save through `service::paper`. Core binding mirrors
//! `mcp/src/papers.rs` (search/fetch) + `mcp/src/io_authors_misc.rs` (doi).
//!
//! The shared wire shape is `SearchResultOut` (models.rs SERIALIZER 1): it strips
//! the source namespace from `source_id`, blanks `published` on the `date.min`
//! sentinel, renames url→paper_url / category→primary_category, and keeps the full
//! id in `entry_id`. `SearchResultOut::from` is the single mapping point for
//! every arxiv/openalex search + fetch arm.

use serde::Deserialize;
use serde_json::Value;

use linxiv_core::error::CoreError;
use linxiv_core::models::{
    strip_namespace, ArxivFetchResponse, ArxivSearchResponse, CrossrefSearchResponse,
    DoiResolveResponse, DoiSaveResponse, OpenAlexSaveResponse, OpenAlexSearchResponse,
    PaperMetadata, SearchResultOut,
};
use linxiv_core::service::{paper as svc_paper, source as svc_source};

use crate::route::{to_value, ApiError, ReqCtx};
use crate::state::AppState;

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("POST", ["api", "arxiv", "search"]) => Some(arxiv_search(state, ctx).await),
        ("POST", ["api", "arxiv", "fetch"]) => Some(arxiv_fetch(state, ctx).await),
        ("POST", ["api", "openalex", "search"]) => Some(openalex_search(state, ctx).await),
        ("POST", ["api", "openalex", "save"]) => Some(openalex_save(state, ctx).await),
        ("POST", ["api", "crossref", "search"]) => Some(crossref_search(ctx).await),
        ("POST", ["api", "doi", "resolve"]) => Some(doi_resolve_route(ctx).await),
        ("POST", ["api", "doi", "save"]) => Some(doi_save_route(state, ctx).await),
        _ => None,
    }
}

/// `api/app.py`'s `except Exception: 502` for the search/fetch source calls.
fn upstream_502(e: CoreError) -> ApiError {
    ApiError::new(502, e.to_string())
}

fn default_max_results() -> i64 {
    25
}

/// FastAPI `Field(ge=1, le=100)` on `max_results`: out-of-range is a 422 (status
/// matches; detail is our plain string). Critical — the raw value is cast to u32
/// and sent upstream, so a negative would wrap to a huge unbounded page request.
fn check_max_results(n: i64) -> Result<u32, ApiError> {
    if (1..=100).contains(&n) {
        Ok(n as u32)
    } else {
        Err(ApiError::new(422, "max_results must be between 1 and 100"))
    }
}

/// FastAPI per-source `Literal` on `sort`: an out-of-set value is a 422 (Python
/// app.py:737 arxiv / 1174 openalex), not a 502 from the source layer.
fn check_sort(source: &str, sort: &str) -> Result<(), ApiError> {
    let allowed: &[&str] = match source {
        "arxiv" => &["relevance", "newest", "oldest", "lastUpdated"],
        "openalex" => &["relevance", "newest", "oldest", "citations"],
        _ => return Ok(()),
    };
    if allowed.contains(&sort) {
        Ok(())
    } else {
        Err(ApiError::new(422, format!("Invalid sort value: {sort:?}")))
    }
}
fn default_sort() -> String {
    "relevance".to_string()
}
fn default_true() -> bool {
    true
}

/// Optionally save every result, then report which of them the library already
/// holds — so the GUI can check off results that are in the library rather than
/// offering to save them again. Stripped ids, matching the wire `source_id`.
fn saved_ids(
    state: &AppState,
    results: &[PaperMetadata],
    save: bool,
) -> Result<Vec<String>, ApiError> {
    if results.is_empty() {
        return Ok(Vec::new());
    }
    state.with_conn(|conn| -> Result<Vec<String>, ApiError> {
        if save {
            svc_paper::save_papers_metadata(conn, results)?;
        }
        let ids: Vec<String> = results.iter().map(|m| m.source_id.clone()).collect();
        Ok(svc_paper::existing_source_ids(conn, &ids)?
            .iter()
            .map(|s| strip_namespace(s))
            .collect())
    })
}

/// `POST /api/arxiv/search` request body.
#[derive(Deserialize, ts_rs::TS)]
pub struct ArxivSearchBody {
    pub query: String,
    #[serde(default = "default_max_results")]
    #[ts(as = "Option<i64>", optional)]
    pub max_results: i64,
    #[serde(default)]
    #[ts(as = "Option<bool>", optional)]
    pub save: bool,
    #[serde(default = "default_sort")]
    #[ts(as = "Option<String>", optional)]
    pub sort: String,
}

/// `POST /api/arxiv/search` (740–758). `502` on any source error. `save` bulk-saves
/// every result; `saved_source_ids` reports library membership either way.
async fn arxiv_search(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: ArxivSearchBody = ctx.parse_body()?;
    if b.query.is_empty() {
        return Err(ApiError::new(422, "query must not be empty"));
    }
    let max_results = check_max_results(b.max_results)?;
    check_sort("arxiv", &b.sort)?;
    let results = svc_source::search("arxiv", b.query.trim(), max_results, &b.sort)
        .await
        .map_err(upstream_502)?;

    let saved = saved_ids(state, &results, b.save)?;

    let results: Vec<SearchResultOut> = results.into_iter().map(SearchResultOut::from).collect();
    to_value(&ArxivSearchResponse {
        results,
        saved_source_ids: saved,
    })
}

/// `POST /api/arxiv/fetch` request body.
#[derive(Deserialize, ts_rs::TS)]
pub struct ArxivFetchBody {
    pub source_id: String,
    #[serde(default = "default_true")]
    #[ts(as = "Option<bool>", optional)]
    pub save: bool,
}

/// `POST /api/arxiv/fetch` (804–823). `404` not-found / `502` other. The save is
/// idempotent (INSERT OR IGNORE), so re-fetching a stored paper cannot conflict.
async fn arxiv_fetch(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: ArxivFetchBody = ctx.parse_body()?;
    if b.source_id.is_empty() {
        return Err(ApiError::new(422, "source_id must not be empty"));
    }
    let meta = svc_source::fetch_by_id("arxiv", b.source_id.trim())
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
    let paper = SearchResultOut::from(meta);
    to_value(&ArxivFetchResponse {
        paper,
        saved: b.save,
        source_id,
    })
}

/// `POST /api/openalex/search` request body.
#[derive(Deserialize, ts_rs::TS)]
pub struct OpenAlexSearchBody {
    pub query: String,
    #[serde(default = "default_max_results")]
    #[ts(as = "Option<i64>", optional)]
    pub max_results: i64,
    #[serde(default = "default_sort")]
    #[ts(as = "Option<String>", optional)]
    pub sort: String,
}

/// `POST /api/openalex/search` (1177–1187). `502` on any source error. Never
/// saves; `saved_source_ids` reports what the library already holds.
async fn openalex_search(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: OpenAlexSearchBody = ctx.parse_body()?;
    if b.query.is_empty() {
        return Err(ApiError::new(422, "query must not be empty"));
    }
    let max_results = check_max_results(b.max_results)?;
    check_sort("openalex", &b.sort)?;
    let results = svc_source::search("openalex", b.query.trim(), max_results, &b.sort)
        .await
        .map_err(upstream_502)?;
    let saved = saved_ids(state, &results, false)?;
    let results: Vec<SearchResultOut> = results.into_iter().map(SearchResultOut::from).collect();
    to_value(&OpenAlexSearchResponse {
        results,
        saved_source_ids: saved,
    })
}

/// `POST /api/openalex/save` request body.
#[derive(Deserialize, ts_rs::TS)]
pub struct OpenAlexSaveBody {
    pub source_id: String,
}

/// `POST /api/openalex/save` (1475–1489). `404`/`400`/`502` on fetch. The save is
/// idempotent (INSERT OR IGNORE). Returns the stripped stored source_id.
async fn openalex_save(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: OpenAlexSaveBody = ctx.parse_body()?;
    if b.source_id.is_empty() {
        return Err(ApiError::new(422, "source_id must not be empty"));
    }
    let meta = svc_source::fetch_by_id("openalex", b.source_id.trim())
        .await
        .map_err(|e| match e {
            CoreError::OpenAlexNotFound(m) => ApiError::new(404, m),
            CoreError::OpenAlexInput(m) => ApiError::new(400, m),
            other => ApiError::new(502, other.to_string()),
        })?;
    let (stored, _) = state.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))?;
    to_value(&OpenAlexSaveResponse {
        saved: true,
        source_id: strip_namespace(&stored),
    })
}

/// `POST /api/crossref/search` request body.
#[derive(Deserialize, ts_rs::TS)]
pub struct CrossrefSearchBody {
    pub query: String,
    #[serde(default = "default_max_results")]
    #[ts(as = "Option<i64>", optional)]
    pub max_results: i64,
}

/// `POST /api/crossref/search` — same envelope as the openalex arm. The wire body
/// carries no `sort`, so this arm pins relevance; a transport failure is a 502
/// rather than an empty result list.
async fn crossref_search(ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: CrossrefSearchBody = ctx.parse_body()?;
    if b.query.is_empty() {
        return Err(ApiError::new(422, "query must not be empty"));
    }
    let max_results = check_max_results(b.max_results)?;
    let results = svc_source::search("crossref", b.query.trim(), max_results, "relevance")
        .await
        .map_err(upstream_502)?;
    let results: Vec<SearchResultOut> = results.into_iter().map(SearchResultOut::from).collect();
    to_value(&CrossrefSearchResponse { results })
}

/// `POST /api/doi/resolve` request body.
#[derive(Deserialize, ts_rs::TS)]
pub struct DoiResolveBody {
    pub doi: String,
}

/// `POST /api/doi/resolve` (830–836). `400` on a bad DOI (CoreError::BadRequest →
/// 400 via `?`, matching Python's `ValueError`).
async fn doi_resolve_route(ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: DoiResolveBody = ctx.parse_body()?;
    if b.doi.is_empty() {
        return Err(ApiError::new(422, "doi must not be empty"));
    }
    let meta = svc_source::resolve_doi(b.doi.trim()).await?;
    to_value(&DoiResolveResponse { metadata: meta })
}

/// `POST /api/doi/save` request body.
#[derive(Deserialize, ts_rs::TS)]
pub struct DoiSaveBody {
    pub doi: String,
}

/// `POST /api/doi/save` (843–850). Resolve then save; returns the resolved meta.
async fn doi_save_route(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let b: DoiSaveBody = ctx.parse_body()?;
    if b.doi.is_empty() {
        return Err(ApiError::new(422, "doi must not be empty"));
    }
    let meta = svc_source::resolve_doi(b.doi.trim()).await?;
    state.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))?;
    to_value(&DoiSaveResponse {
        metadata: meta,
        saved: true,
    })
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
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn post(st: &AppState, path: &str, body: Value) -> Result<Value, ApiError> {
        route(
            st,
            ApiRequest {
                method: "POST".into(),
                path: path.into(),
                body: Some(body),
            },
        )
        .await
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
        let v = serde_json::to_value(SearchResultOut::from(meta(
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
        let v = serde_json::to_value(SearchResultOut::from(meta(
            "0001-01-01",
            Value::Null,
            Value::Null,
        )))
        .unwrap();
        assert_eq!(v["published"], json!(""));
        assert_eq!(v["paper_url"], json!(""));
        assert_eq!(v["primary_category"], json!(""));
        assert_eq!(v["entry_id"], json!("arxiv:2204.12985"));
    }

    #[tokio::test]
    async fn empty_query_is_422_before_any_network() {
        for path in [
            "/api/arxiv/search",
            "/api/openalex/search",
            "/api/crossref/search",
        ] {
            let err = post(&state(), path, json!({ "query": "" }))
                .await
                .unwrap_err();
            assert_eq!(err.status, 422);
        }
    }

    #[tokio::test]
    async fn out_of_range_max_results_is_422_before_network() {
        // 0, >100, and a negative (which would wrap to a huge u32) all 422.
        for mr in [0i64, 200, -1] {
            let err = post(
                &state(),
                "/api/arxiv/search",
                json!({ "query": "abc", "max_results": mr }),
            )
            .await
            .unwrap_err();
            assert_eq!(err.status, 422, "max_results={mr}");
        }
    }

    #[tokio::test]
    async fn crossref_search_rejects_out_of_range_max_results_before_network() {
        let err = post(
            &state(),
            "/api/crossref/search",
            json!({ "query": "abc", "max_results": 0 }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn out_of_set_sort_is_422_before_network() {
        // `citations` is openalex-only; invalid for arxiv.
        let err = post(
            &state(),
            "/api/arxiv/search",
            json!({ "query": "abc", "sort": "citations" }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[test]
    fn saved_ids_reports_library_hits_as_stripped_ids() {
        let st = state();
        let results = vec![meta("2024-01-15", json!("http://x"), json!("cs.LG"))];

        // Nothing stored yet -> nothing checked off.
        assert!(saved_ids(&st, &results, false).unwrap().is_empty());

        // save=true stores it; the same call reports it back.
        assert_eq!(
            saved_ids(&st, &results, true).unwrap(),
            vec!["2204.12985".to_string()]
        );
        // Already in the library -> reported without saving again.
        assert_eq!(
            saved_ids(&st, &results, false).unwrap(),
            vec!["2204.12985".to_string()]
        );
        assert!(saved_ids(&st, &[], false).unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_source_id_and_doi_are_422_before_any_network() {
        for path in ["/api/arxiv/fetch", "/api/openalex/save"] {
            let err = post(&state(), path, json!({ "source_id": "" }))
                .await
                .unwrap_err();
            assert_eq!(err.status, 422);
        }
        for path in ["/api/doi/resolve", "/api/doi/save"] {
            let err = post(&state(), path, json!({ "doi": "" }))
                .await
                .unwrap_err();
            assert_eq!(err.status, 422);
        }
    }
}
