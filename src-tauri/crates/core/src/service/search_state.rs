//! search-state service — the saved Search page working set plus its term history.
//!
//! The history side-effect is the whole reason this seam exists: saving state also
//! records each non-empty clause term, gated on `search_history_enabled` and capped
//! by `search_history_max`. That rule used to live in the route handler, which made
//! it unavailable to any other surface (ADR 0010).

use rusqlite::Connection;
use serde_json::{json, Map, Value};

use crate::config::UserSettings;
use crate::error::Result;
use crate::storage::queries::{search_history, search_state as q};

/// The Search page's working set — one saved row, overwritten on each save.
#[derive(Debug, Default, Clone)]
pub struct SavedSearch {
    pub clauses: Vec<Map<String, Value>>,
    pub source: String,
    pub max_results: i64,
    pub results: Vec<Value>,
    pub saved_ids: Vec<String>,
    pub sort_prefs: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
pub struct SearchHistoryResponse {
    pub suggestions: Vec<String>,
}

/// `GET /api/search/state` envelope — the saved blob (untyped JSON) or `null`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchStateResponse {
    pub state: Option<Value>,
}

/// Past search terms starting with `prefix`, newest first.
pub fn suggestions(conn: &Connection, prefix: &str, limit: i64) -> Result<Vec<String>> {
    search_history::get_suggestions(conn, prefix, limit)
}

/// The saved state, or `None` when nothing has been saved yet.
pub fn load(conn: &Connection) -> Result<Option<Value>> {
    q::load_state(conn)
}

/// Record each non-empty clause term to history (when enabled), then overwrite the
/// saved state. History failures are not swallowed — a full history table is a real
/// fault, not a cosmetic one.
pub fn save(conn: &Connection, s: &SavedSearch) -> Result<()> {
    let settings = UserSettings::load()?;
    let enabled = settings
        .get("search_history_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if enabled {
        let max_history = settings
            .get("search_history_max")
            .and_then(Value::as_i64)
            .unwrap_or(200);
        for clause in &s.clauses {
            if let Some(term) = clause.get("value").and_then(Value::as_str) {
                if !term.trim().is_empty() {
                    search_history::add_term(conn, term, max_history)?;
                }
            }
        }
    }
    q::save_state(
        conn,
        &json!(s.clauses),
        &s.source,
        s.max_results,
        &json!(s.results),
        &json!(s.saved_ids),
        s.sort_prefs.clone().map(Value::Object).as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    fn seeded() -> Connection {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn
    }

    fn clause(v: &str) -> Map<String, Value> {
        json!({ "value": v }).as_object().unwrap().clone()
    }

    #[test]
    fn save_records_clause_terms_and_round_trips_state() {
        let conn = seeded();
        assert!(load(&conn).unwrap().is_none());

        save(
            &conn,
            &SavedSearch {
                clauses: vec![clause("manifold"), clause("   ")],
                source: "arxiv".into(),
                max_results: 25,
                ..Default::default()
            },
        )
        .unwrap();

        let st = load(&conn).unwrap().unwrap();
        assert_eq!(st["source"], json!("arxiv"));
        assert_eq!(st["max_results"], json!(25));
        // the blank clause is not a search term
        assert_eq!(suggestions(&conn, "man", 10).unwrap(), vec!["manifold"]);
        assert!(suggestions(&conn, "zz", 10).unwrap().is_empty());
    }
}
