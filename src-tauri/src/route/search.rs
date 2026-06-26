//! `/api/search/{history,state}` — `api/app.py` 763–801. Autocomplete suggestions
//! plus the single saved search-page state. Backed by the `search_history` /
//! `search_state` storage modules; the enabled flag + max cap come from settings.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use linxiv_core::config::UserSettings;
use linxiv_core::storage::queries::{search_history, search_state};

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "search", "history"]) => Some(history(state, ctx)),
        ("GET", ["api", "search", "state"]) => Some(get_state(state)),
        ("POST", ["api", "search", "state"]) => Some(save(state, ctx)),
        _ => None,
    }
}

/// `GET /api/search/history?prefix=&limit=` — `api_search_history`. `limit` is
/// `Query(default=10, ge=1, le=50)`: out-of-range or non-integer is a 422.
fn history(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let prefix = ctx.q("prefix").unwrap_or("");
    let limit = match ctx.q("limit") {
        None => 10,
        Some(v) => match v.parse::<i64>() {
            Ok(n) if (1..=50).contains(&n) => n,
            _ => return Err(ApiError::new(422, "limit must be an integer between 1 and 50")),
        },
    };
    let suggestions = state.with_conn(|conn| search_history::get_suggestions(conn, prefix, limit))?;
    Ok(json!({ "suggestions": suggestions }))
}

/// `GET /api/search/state` — `api_search_state_get`. `{state: null|obj}`.
fn get_state(state: &AppState) -> Result<Value, ApiError> {
    let st = state.with_conn(|conn| search_state::load_state(conn))?;
    Ok(json!({ "state": st.unwrap_or(Value::Null) }))
}

/// `SearchStateBody` (app.py 769-775) — typed so a wrong field type 422s like
/// pydantic (rather than being silently coerced to a default).
#[derive(Deserialize)]
struct SearchStateBody {
    #[serde(default)]
    clauses: Vec<Map<String, Value>>,
    #[serde(default = "default_source")]
    source: String,
    #[serde(default = "default_max_results")]
    max_results: i64,
    #[serde(default)]
    results: Vec<Value>,
    #[serde(default)]
    saved_ids: Vec<String>,
    #[serde(default)]
    sort_prefs: Option<Map<String, Value>>,
}
fn default_source() -> String {
    "arxiv".into()
}
fn default_max_results() -> i64 {
    25
}

/// `POST /api/search/state` — `api_search_state_save`. Records each non-empty
/// clause value to history (gated on the enabled flag), then saves the state.
fn save(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let body: SearchStateBody = ctx.parse_body()?;

    let settings = UserSettings::load()?;
    let enabled = settings.get("search_history_enabled").and_then(Value::as_bool).unwrap_or(true);
    let max_history = settings.get("search_history_max").and_then(Value::as_i64).unwrap_or(200);

    let clauses = json!(body.clauses);
    let results = Value::Array(body.results);
    let saved_ids = json!(body.saved_ids);
    let sort_prefs = body.sort_prefs.map(Value::Object);

    state.with_conn(|conn| -> Result<(), ApiError> {
        if enabled {
            for clause in &body.clauses {
                if let Some(term) = clause.get("value").and_then(Value::as_str) {
                    if !term.trim().is_empty() {
                        search_history::add_term(conn, term, max_history)?;
                    }
                }
            }
        }
        search_state::save_state(conn, &clauses, &body.source, body.max_results, &results, &saved_ids, sort_prefs.as_ref())?;
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::storage;

    fn st() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }
    async fn req(s: &AppState, m: &str, p: &str, b: Option<Value>) -> Result<Value, ApiError> {
        route(s, ApiRequest { method: m.into(), path: p.into(), body: b }).await
    }

    #[tokio::test]
    async fn history_empty_prefix_is_empty_list() {
        assert_eq!(req(&st(), "GET", "/api/search/history?prefix=", None).await.unwrap(), json!({ "suggestions": [] }));
    }

    #[tokio::test]
    async fn state_roundtrips_and_records_clause_history() {
        let s = st();
        assert_eq!(req(&s, "GET", "/api/search/state", None).await.unwrap(), json!({ "state": null }));

        let body = json!({
            "clauses": [{ "value": "manifold" }],
            "source": "arxiv", "max_results": 25, "results": [], "saved_ids": []
        });
        assert_eq!(req(&s, "POST", "/api/search/state", Some(body.clone())).await.unwrap(), json!({ "ok": true }));

        let got = req(&s, "GET", "/api/search/state", None).await.unwrap();
        assert_eq!(got["state"]["source"], json!("arxiv"));
        assert_eq!(got["state"]["clauses"], body["clauses"]);
        // the clause value was recorded to history
        let sugg = req(&s, "GET", "/api/search/history?prefix=man", None).await.unwrap();
        assert_eq!(sugg["suggestions"], json!(["manifold"]));
    }
}
