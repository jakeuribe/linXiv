//! `/api/graph` routes — `api/app.py` `api_graph` (464-466) + `api_graph_project_options`
//! (539-541). Built by `linxiv_core::graph`. Reached from the graph iframe over the
//! `linxiv://` scheme (which bridges `/api/*` GETs into this router — see `protocol`).

use serde_json::{json, Value};

use linxiv_core::graph;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "graph"]) => Some(graph_data(state, ctx)),
        ("GET", ["api", "graph", "project-options"]) => Some(project_options(state)),
        _ => None,
    }
}

/// `GET /api/graph?exclude_single_authors=` — `{nodes, edges}`.
fn graph_data(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let exclude = ctx.q_bool("exclude_single_authors");
    Ok(state.with_conn(|conn| graph::augmented_graph_data(conn, exclude))?)
}

/// `GET /api/graph/project-options` — `{projects: [...]}`.
fn project_options(state: &AppState) -> Result<Value, ApiError> {
    let opts = state.with_conn(|conn| graph::project_filter_options(conn))?;
    Ok(json!({ "projects": opts }))
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
    async fn get(s: &AppState, p: &str) -> Result<Value, ApiError> {
        route(
            s,
            ApiRequest {
                method: "GET".into(),
                path: p.into(),
                body: None,
            },
        )
        .await
    }

    #[tokio::test]
    async fn empty_graph_and_options_envelopes() {
        let s = st();
        assert_eq!(
            get(&s, "/api/graph").await.unwrap(),
            json!({ "nodes": [], "edges": [] })
        );
        assert_eq!(
            get(&s, "/api/graph/project-options").await.unwrap(),
            json!({ "projects": [] })
        );
    }
}
