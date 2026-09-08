//! `GET /api/graph` — the Knowledge Graph payload, built by `linxiv_core::graph`.
//! One request answers the whole page.

use serde_json::Value;

use linxiv_core::graph;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "graph"]) => Some(graph_view(state, ctx)),
        _ => None,
    }
}

/// `GET /api/graph?exclude_single_authors=` — the whole `GraphView`.
fn graph_view(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let exclude = ctx.q_bool("exclude_single_authors");
    let view = state.with_conn(|conn| graph::graph_view(conn, exclude))?;
    serde_json::to_value(view)
        .map_err(|e| ApiError::new(500, format!("serializing graph view: {e}")))
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
    async fn empty_graph_answers_every_list_empty() {
        let s = st();
        assert_eq!(
            get(&s, "/api/graph").await.unwrap(),
            json!({
                "papers": [], "authors": [], "tags": [], "edges": [],
                "categories": [], "projects": [],
            })
        );
    }

    /// `/api/graph/project-options` rode into `/api/graph`; the old path must 404,
    /// not answer something stale.
    #[tokio::test]
    async fn project_options_endpoint_is_retired() {
        let s = st();
        let err = get(&s, "/api/graph/project-options").await.unwrap_err();
        assert_eq!(err.status, 404);
    }
}
