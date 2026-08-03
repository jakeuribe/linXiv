//! In-process HTTP-shaped router: the Rust collapse of `api/app.py`. The webview's
//! `apiFetch` calls the single `api` Tauri command with `{method, path, body}`;
//! `route` matches `(method, decoded path segments)` and calls `linxiv-core`,
//! returning the same JSON body `api/app.py` did. One router, two front doors:
//! `invoke("api", …)` in the packaged app, and (Phase 6, D32) a dev-only HTTP shim
//! over the same `route` fn for the Vite browser loop.
//!
//! ## Pattern for adding a resource group
//! Add `mod <group>;`, then route the group's first path segment to a `<group>::`
//! fn from the `match` below. Each arm:
//!   - reads the DB via `state.with_conn(|conn| …)`; `?` lifts `CoreError`→`ApiError`,
//!   - serializes domain structs with `serde_json::to_value` (== Python `to_dict`),
//!   - matches `api/app.py`'s response envelope exactly (key names, key order — the
//!     `preserve_order` feature keeps struct/`json!` order), and the HTTP status its
//!     `HTTPException`s used (already encoded in `CoreError::http_status`).
//!
//! `source_id` segments arrive percent-encoded (the webview `encodeURIComponent`s
//! them), so split first, then `pct_decode` — an old-style id like `math-ph/0309136`
//! stays one segment.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use linxiv_core::error::CoreError;
use linxiv_core::service::{paper as svc_paper, tag as svc_tag};

use crate::state::AppState;

mod annotations;
mod authors;
mod editor;
mod feed;
mod graph;
mod notes;
mod orcid;
pub(crate) mod papers; // ingest_full_text reused by the background full-text worker
pub(crate) mod pdfs; // resolve_local_pdf reused by the linxiv:// protocol handler
mod projects;
mod search;
mod settings;
pub mod share; // ShareState + share_api command, managed beside AppState in main.rs
mod sources;
mod storage;
mod tags;
mod trash;
mod uploads;
mod versions;

/// One webview→backend call. `body` is the parsed JSON request body (None for
/// GET/DELETE without a body); file uploads do not come through here (they stay
/// on the HTTP path until Phase 5c).
#[derive(Deserialize)]
pub struct ApiRequest {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body: Option<Value>,
}

/// Error surfaced to the webview. `invoke` rejects with this; `client.ts` turns it
/// back into an `ApiError(status, detail)`, the same shape the `fetch` path threw.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub status: u16,
    pub detail: String,
}

impl ApiError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
        }
    }
    /// Returned when no route arm matches. (During the staged port this was a 501
    /// sentinel telling `client.ts` to fall back to the Python sidecar; the sidecar
    /// and that fallback are gone, so it's now a plain 404.)
    pub(crate) fn not_routed() -> Self {
        Self::new(404, "route not found")
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        ApiError::new(e.http_status(), e.to_string())
    }
}

/// One parsed request, handed to each resource group's `handle`. `segs` are the
/// percent-decoded path segments (`["api","authors","42"]`); `body` is the parsed
/// JSON request body. Group modules match on `(ctx.method, ctx.segs)`.
pub(crate) struct ReqCtx<'a> {
    pub method: &'a str,
    pub segs: &'a [&'a str],
    pub query: &'a HashMap<String, String>,
    pub body: Option<&'a Value>,
}

impl ReqCtx<'_> {
    pub fn q(&self, key: &str) -> Option<&str> {
        self.query.get(key).map(String::as_str)
    }
    pub fn q_i64(&self, key: &str) -> Option<i64> {
        self.q(key).and_then(|v| v.parse().ok())
    }
    pub fn q_bool(&self, key: &str) -> bool {
        matches!(self.q(key), Some("true") | Some("1"))
    }
    /// Deserialize the JSON body into `T`, matching FastAPI's pydantic binding
    /// (422 on a malformed/missing body).
    pub fn parse_body<T: serde::de::DeserializeOwned>(&self) -> Result<T, ApiError> {
        let v = self.body.cloned().unwrap_or(Value::Null);
        serde_json::from_value(v).map_err(|e| ApiError::new(422, e.to_string()))
    }
}

/// Parse an integer path segment (`{author_id}` etc.); 422 on a non-integer, the
/// status FastAPI's path-param coercion returns.
pub(crate) fn path_i64(seg: &str) -> Result<i64, ApiError> {
    seg.parse()
        .map_err(|_| ApiError::new(422, format!("Invalid path parameter: {seg:?}")))
}

/// The Tauri command the webview invokes. Thin wrapper over `route`.
#[tauri::command]
pub async fn api(state: tauri::State<'_, AppState>, req: ApiRequest) -> Result<Value, ApiError> {
    route(state.inner(), req).await
}

/// Match a request to a `linxiv-core` call. The whole router is `async` so the
/// source-backed arms (arxiv/openalex/doi) can `.await`; DB-only arms run their
/// `with_conn` closure to completion (no lock held across an await).
pub async fn route(state: &AppState, req: ApiRequest) -> Result<Value, ApiError> {
    let (raw_path, raw_query) = req.path.split_once('?').unwrap_or((req.path.as_str(), ""));
    let segs = split_segments(raw_path);
    let query = parse_query(raw_query);
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    let ctx = ReqCtx {
        method: req.method.as_str(),
        segs: &s,
        query: &query,
        body: req.body.as_ref(),
    };

    // Flat top-level arms (no resource subtree).
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "health"]) => {
            return Ok(json!({
                "ok": true,
                "service": "linxiv-api",
                "token": std::env::var("LINXIV_HEALTH_TOKEN").unwrap_or_default(),
            }))
        }
        ("GET", ["api", "stats"]) => return stats(state),
        ("GET", ["api", "categories"]) => return categories(state),
        _ => {}
    }

    // Resource groups: each owns its path subtree and returns None to pass. Arms
    // match on exact (method, segment-count, literals), so order is independent —
    // pdfs is listed before papers only for readability (it claims the more
    // specific `/api/papers/{id}/pdf-path`).
    macro_rules! try_groups {
        ($($group:ident),+ $(,)?) => {$(
            if let Some(r) = $group::handle(state, &ctx).await {
                return r;
            }
        )+};
    }
    try_groups!(
        uploads,
        pdfs,
        papers,
        projects,
        notes,
        annotations,
        tags,
        authors,
        sources,
        search,
        settings,
        storage,
        trash,
        editor,
        feed,
        graph,
        versions,
        orcid
    );

    Err(ApiError::not_routed())
}

// ── reference arms (the worked example every group arm copies) ───────────────

/// `GET /api/stats` — `api/app.py::stats`. Note the envelope differs from the CLI
/// `stats` command: it adds `recent_papers` (the 10 newest, full `to_dict`).
fn stats(state: &AppState) -> Result<Value, ApiError> {
    state.with_conn(|conn| {
        let papers = svc_paper::list_papers(conn, true, None, 0, None)?;
        let tags = svc_tag::list_all_tags(conn)?;
        let categories = svc_paper::get_categories(conn)?;
        let pdf_count = papers.iter().filter(|p| p.has_pdf).count();
        let recent: Vec<Value> = papers
            .iter()
            .take(10)
            .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
            .collect();
        Ok(json!({
            "paper_count": papers.len(),
            "tag_count": tags.len(),
            "category_count": categories.len(),
            "pdf_count": pdf_count,
            "recent_papers": recent,
        }))
    })
}

/// `GET /api/categories` — `api/app.py::api_categories`. Note the API wraps the
/// list in `{"categories": …}` (the CLI `categories` command emits a bare array —
/// the envelope is the divergence the router must honor).
fn categories(state: &AppState) -> Result<Value, ApiError> {
    let cats = state.with_conn(|conn| svc_paper::get_categories(conn))?;
    Ok(json!({ "categories": cats }))
}

// ── path/query helpers ──────────────────────────────────────────────────────

/// Split a raw request path into percent-decoded, non-empty segments. Shared with
/// the `share_api` command so both front doors parse paths identically.
pub(crate) fn split_segments(raw_path: &str) -> Vec<String> {
    raw_path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(pct_decode)
        .collect()
}

/// Parse a raw `k=v&k2=v2` query string, percent-decoding keys and values.
/// Serialize a domain struct == Python `to_dict()`; an encode failure is a 500.
pub(crate) fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError::new(500, e.to_string()))
}

/// FastAPI `Query(default=None, ge=1)` semantics for `?version=`: absent → None;
/// present must be an int >= 1, else a 422 (not a silent fall-through to latest).
pub(crate) fn q_version(ctx: &ReqCtx<'_>) -> Result<Option<i64>, ApiError> {
    ctx.q("version")
        .map(|v| {
            v.parse::<i64>()
                .ok()
                .filter(|&n| n >= 1)
                .ok_or_else(|| ApiError::new(422, "version must be an integer >= 1"))
        })
        .transpose()
}

pub(crate) fn parse_query(raw: &str) -> HashMap<String, String> {
    raw.split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (pct_decode(k), pct_decode(v))
        })
        .collect()
}

/// Decode `%XX` escapes (and nothing else — `encodeURIComponent` never emits `+`
/// for space).
/// Shared with the `linxiv://` protocol handler.
pub(crate) fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Shared test helpers for the per-group route test modules (each previously
/// carried its own copy of `state`/`req`).
#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use linxiv_core::storage;

    /// DI: an in-memory DB, never the real data dir. pdf/vault roots are unused
    /// by most arms but must be present.
    pub(crate) fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    pub(crate) async fn req(
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
}

#[cfg(test)]
mod tests {
    use super::testutil::state;
    use super::*;

    async fn get(st: &AppState, path: &str) -> Result<Value, ApiError> {
        route(
            st,
            ApiRequest {
                method: "GET".into(),
                path: path.into(),
                body: None,
            },
        )
        .await
    }

    #[tokio::test]
    async fn stats_on_empty_db_matches_app_py_envelope() {
        let v = get(&state(), "/api/stats").await.unwrap();
        assert_eq!(
            v,
            json!({
                "paper_count": 0,
                "tag_count": 0,
                "category_count": 0,
                "pdf_count": 0,
                "recent_papers": [],
            })
        );
        // key order is part of the contract (preserve_order); pin it explicitly.
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"paper_count":0,"tag_count":0,"category_count":0,"pdf_count":0,"recent_papers":[]}"#
        );
    }

    #[tokio::test]
    async fn categories_on_empty_db_wraps_empty_array() {
        assert_eq!(
            get(&state(), "/api/categories").await.unwrap(),
            json!({ "categories": [] })
        );
    }

    #[tokio::test]
    async fn unrouted_path_returns_404() {
        let err = get(&state(), "/api/nope").await.unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[test]
    fn pct_decode_handles_encoded_source_ids() {
        assert_eq!(pct_decode("math-ph%2F0309136"), "math-ph/0309136");
        assert_eq!(pct_decode("2204.12985"), "2204.12985");
        assert_eq!(pct_decode("a%20b"), "a b");
        assert_eq!(pct_decode("trailing%"), "trailing%"); // malformed: left as-is
    }
}
