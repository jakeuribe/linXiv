//! `storage/search_state.py` — the single-row (ID = 1) saved search-page state.
//! The JSON columns are stored verbatim from the request body and parsed back on
//! load; a malformed stored column degrades the whole row to `None` (Python's
//! `except (JSONDecodeError, ValueError): return None`).

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::error::{CoreError, Result};

fn to_json_string(v: &Value) -> Result<String> {
    serde_json::to_string(v).map_err(|e| CoreError::Internal(e.to_string()))
}

/// `save_state` — upsert the ID = 1 row. `clauses`/`results`/`saved_ids` are the
/// request-body JSON arrays (stored verbatim); `sort_prefs` → JSON text or NULL.
pub fn save_state(
    conn: &Connection,
    clauses: &Value,
    source: &str,
    max_results: i64,
    results: &Value,
    saved_ids: &Value,
    sort_prefs: Option<&Value>,
) -> Result<()> {
    let sort = sort_prefs.map(to_json_string).transpose()?;
    conn.execute(
        "INSERT INTO SEARCH_STATE \
            (ID, CLAUSES_JSON, SOURCE, MAX_RESULTS, RESULTS_JSON, SAVED_IDS_JSON, SORT_JSON, UPDATED_AT) \
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, datetime('now')) \
         ON CONFLICT(ID) DO UPDATE SET \
            CLAUSES_JSON   = excluded.CLAUSES_JSON, \
            SOURCE         = excluded.SOURCE, \
            MAX_RESULTS    = excluded.MAX_RESULTS, \
            RESULTS_JSON   = excluded.RESULTS_JSON, \
            SAVED_IDS_JSON = excluded.SAVED_IDS_JSON, \
            SORT_JSON      = excluded.SORT_JSON, \
            UPDATED_AT     = datetime('now')",
        params![to_json_string(clauses)?, source, max_results, to_json_string(results)?, to_json_string(saved_ids)?, sort],
    )?;
    Ok(())
}

/// `load_state` — the ID = 1 row as a JSON object (key order matches app.py), or
/// `None` if unsaved or any stored JSON column fails to parse.
pub fn load_state(conn: &Connection) -> Result<Option<Value>> {
    let row = conn
        .query_row(
            "SELECT CLAUSES_JSON, SOURCE, MAX_RESULTS, RESULTS_JSON, SAVED_IDS_JSON, SORT_JSON, UPDATED_AT \
             FROM SEARCH_STATE WHERE ID = 1",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((clauses, source, max_results, results, saved_ids, sort, updated_at)) = row else {
        return Ok(None);
    };
    let built = (|| -> std::result::Result<Value, serde_json::Error> {
        Ok(serde_json::json!({
            "clauses":     serde_json::from_str::<Value>(&clauses)?,
            "source":      source,
            "max_results": max_results,
            "results":     serde_json::from_str::<Value>(&results)?,
            "saved_ids":   serde_json::from_str::<Value>(&saved_ids)?,
            "sort_prefs":  match &sort { Some(s) => serde_json::from_str::<Value>(s)?, None => Value::Null },
            "updated_at":  updated_at,
        }))
    })();
    Ok(built.ok()) // malformed stored JSON → None, like Python
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};
    use serde_json::json;

    #[test]
    fn save_then_load_roundtrips_and_upserts_single_row() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        assert_eq!(load_state(&conn).unwrap(), None);

        let clauses = json!([{ "field": "all", "value": "manifold" }]);
        save_state(
            &conn,
            &clauses,
            "arxiv",
            25,
            &json!([{ "source_id": "x" }]),
            &json!(["x"]),
            None,
        )
        .unwrap();

        let st = load_state(&conn).unwrap().unwrap();
        assert_eq!(st["clauses"], clauses);
        assert_eq!(st["source"], json!("arxiv"));
        assert_eq!(st["max_results"], json!(25));
        assert_eq!(st["saved_ids"], json!(["x"]));
        assert_eq!(st["sort_prefs"], Value::Null);
        // key order is the wire contract.
        assert!(st.as_object().unwrap().keys().eq([
            "clauses",
            "source",
            "max_results",
            "results",
            "saved_ids",
            "sort_prefs",
            "updated_at"
        ]));

        // upsert: a second save overwrites the single row, not a second insert.
        save_state(
            &conn,
            &json!([]),
            "openalex",
            50,
            &json!([]),
            &json!([]),
            Some(&json!({ "by": "date" })),
        )
        .unwrap();
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM SEARCH_STATE", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 1);
        let st2 = load_state(&conn).unwrap().unwrap();
        assert_eq!(st2["source"], json!("openalex"));
        assert_eq!(st2["sort_prefs"], json!({ "by": "date" }));
    }
}
