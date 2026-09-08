//! `/api/search/{history,state}` — autocomplete suggestions
//! plus the single saved search-page state. `service::search_state` owns the
//! history side-effect and the settings gate; this module is query/body parsing.

use serde::Deserialize;
use serde_json::{Map, Value};

use linxiv_core::models::OkReceipt;
use linxiv_core::service::search_state::{
    self as svc_search, SavedSearch, SearchHistoryResponse, SearchStateResponse,
};

use crate::route::{to_value, ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "search", "history"]) => Some(history(state, ctx)),
        ("GET", ["api", "search", "state"]) => Some(get_state(state)),
        ("POST", ["api", "search", "state"]) => Some(save(state, ctx)),
        _ => None,
    }
}

/// `GET /api/search/history?prefix=&limit=` — `limit` defaults to 10, range
/// 1–50: out-of-range or non-integer is a 422.
fn history(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let prefix = ctx.q("prefix").unwrap_or("");
    let limit = match ctx.q("limit") {
        None => 10,
        Some(v) => match v.parse::<i64>() {
            Ok(n) if (1..=50).contains(&n) => n,
            _ => {
                return Err(ApiError::new(
                    422,
                    "limit must be an integer between 1 and 50",
                ))
            }
        },
    };
    let suggestions = state.with_conn(|conn| svc_search::suggestions(conn, prefix, limit))?;
    to_value(&SearchHistoryResponse { suggestions })
}

/// `GET /api/search/state` — `{state: null|obj}`.
fn get_state(state: &AppState) -> Result<Value, ApiError> {
    let st = state.with_conn(|conn| svc_search::load(conn))?;
    to_value(&SearchStateResponse { state: st })
}

/// `POST /api/search/state` request body; `clauses`/`results`/`sort_prefs` stay untyped JSON, so no ts-rs derive.
#[derive(Deserialize)]
pub struct SearchStateBody {
    #[serde(default)]
    pub clauses: Vec<Map<String, Value>>,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
    #[serde(default)]
    pub results: Vec<Value>,
    #[serde(default)]
    pub saved_ids: Vec<String>,
    #[serde(default)]
    pub sort_prefs: Option<Map<String, Value>>,
}
fn default_source() -> String {
    "arxiv".into()
}
fn default_max_results() -> i64 {
    25
}

/// `POST /api/search/state` — the history recording and its settings gate live
/// in `service::search_state::save`.
fn save(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let body: SearchStateBody = ctx.parse_body()?;
    let saved = SavedSearch {
        clauses: body.clauses,
        source: body.source,
        max_results: body.max_results,
        results: body.results,
        saved_ids: body.saved_ids,
        sort_prefs: body.sort_prefs,
    };
    state.with_conn(|conn| svc_search::save(conn, &saved))?;
    to_value(&OkReceipt { ok: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::storage;
    use serde_json::json;

    fn st() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }
    async fn req(s: &AppState, m: &str, p: &str, b: Option<Value>) -> Result<Value, ApiError> {
        route(
            s,
            ApiRequest {
                method: m.into(),
                path: p.into(),
                body: b,
            },
        )
        .await
    }

    #[tokio::test]
    async fn history_empty_prefix_is_empty_list() {
        assert_eq!(
            req(&st(), "GET", "/api/search/history?prefix=", None)
                .await
                .unwrap(),
            json!({ "suggestions": [] })
        );
    }

    #[tokio::test]
    async fn state_roundtrips_and_records_clause_history() {
        let s = st();
        assert_eq!(
            req(&s, "GET", "/api/search/state", None).await.unwrap(),
            json!({ "state": null })
        );

        let body = json!({
            "clauses": [{ "value": "manifold" }],
            "source": "arxiv", "max_results": 25, "results": [], "saved_ids": []
        });
        assert_eq!(
            req(&s, "POST", "/api/search/state", Some(body.clone()))
                .await
                .unwrap(),
            json!({ "ok": true })
        );

        let got = req(&s, "GET", "/api/search/state", None).await.unwrap();
        assert_eq!(got["state"]["source"], json!("arxiv"));
        assert_eq!(got["state"]["clauses"], body["clauses"]);
        // the clause value was recorded to history
        let sugg = req(&s, "GET", "/api/search/history?prefix=man", None)
            .await
            .unwrap();
        assert_eq!(sugg["suggestions"], json!(["manifold"]));
    }
}
