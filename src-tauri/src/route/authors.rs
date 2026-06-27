//! `/api/authors` routes — `api/app.py` 1011–1045. The reference resource group:
//! a `handle` that owns its path subtree, path-param extraction, 404/409 mapping,
//! body deserialization, and composite serialization. Every other group module
//! copies this shape. Core binding mirrors `mcp/src/io_authors_misc.rs`.

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::service::author::{self as svc_author, Author};

use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

/// Returns `Some(result)` if this group owns `(method, path)`, else `None` so the
/// dispatcher tries the next group.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "authors"]) => Some(list(state, ctx)),
        ("GET", ["api", "authors", id]) => Some(detail(state, id)),
        ("PATCH", ["api", "authors", id]) => Some(update(state, id, ctx)),
        ("DELETE", ["api", "authors", id]) => Some(delete(state, id)),
        _ => None,
    }
}

/// `GET /api/authors?exclude_single=` — `api_authors_list`.
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let min_papers = if ctx.q_bool("exclude_single") { 2 } else { 0 };
    let authors = state.with_conn(|conn| svc_author::list_with_paper_count(conn, min_papers))?;
    Ok(json!({ "authors": authors }))
}

/// `GET /api/authors/{id}` — `api_author_get` → `_author_detail_response`.
fn detail(state: &AppState, id: &str) -> Result<Value, ApiError> {
    detail_response(state, path_i64(id)?)
}

/// `PATCH /api/authors/{id}` — `api_author_update`. Forwards the (all-optional)
/// fields to `update_fields`, then returns the same detail shape as GET.
fn update(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let author_id = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        full_name: Option<String>,
        #[serde(default)]
        first_name: Option<String>,
        #[serde(default)]
        last_name: Option<String>,
        #[serde(default)]
        orcid: Option<String>,
    }
    let b: Body = ctx.parse_body()?;
    state.with_conn(|conn| {
        svc_author::update_fields(
            conn,
            author_id,
            b.full_name.as_deref(),
            b.first_name.as_deref(),
            b.last_name.as_deref(),
            b.orcid.as_deref(),
        )
    })?;
    detail_response(state, author_id)
}

/// `DELETE /api/authors/{id}` — `api_author_delete`. 404 if absent, 409 if still
/// linked to papers (message byte-matches app.py).
fn delete(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let author_id = path_i64(id)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        if svc_author::get(conn, &author_ref(author_id))?.is_none() {
            return Err(ApiError::new(404, "Author not found"));
        }
        let links = svc_author::count_paper_links(conn, author_id)?;
        if links > 0 {
            return Err(ApiError::new(
                409,
                format!("Author is linked to {links} paper(s); unlink before deleting."),
            ));
        }
        svc_author::delete(conn, &author_ref(author_id))?;
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
}

/// Build `{**author.to_dict(), paper_count, papers}` — shared by GET and PATCH.
fn detail_response(state: &AppState, author_id: i64) -> Result<Value, ApiError> {
    state.with_conn(|conn| {
        let author = svc_author::get(conn, &author_ref(author_id))?
            .ok_or_else(|| ApiError::new(404, "Author not found"))?;
        let previews = svc_author::get_paper_previews(conn, author_id)?;
        let mut v = serde_json::to_value(&author).map_err(|e| ApiError::new(500, e.to_string()))?;
        if let Value::Object(map) = &mut v {
            map.insert("paper_count".into(), json!(previews.len()));
            map.insert(
                "papers".into(),
                serde_json::to_value(&previews).map_err(|e| ApiError::new(500, e.to_string()))?,
            );
        }
        Ok(v)
    })
}

fn author_ref(author_id: i64) -> Author {
    Author {
        author_id: Some(author_id),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::route;
    use crate::route::ApiRequest;
    use linxiv_core::storage;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn req(st: &AppState, method: &str, path: &str) -> Result<Value, ApiError> {
        route(
            st,
            ApiRequest {
                method: method.into(),
                path: path.into(),
                body: None,
            },
        )
        .await
    }

    #[tokio::test]
    async fn list_on_empty_db_wraps_empty_array() {
        assert_eq!(
            req(&state(), "GET", "/api/authors").await.unwrap(),
            json!({ "authors": [] })
        );
    }

    #[tokio::test]
    async fn get_missing_author_is_404() {
        let err = req(&state(), "GET", "/api/authors/999").await.unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Author not found");
    }

    #[tokio::test]
    async fn non_integer_id_is_422() {
        let err = req(&state(), "GET", "/api/authors/abc").await.unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn delete_missing_author_is_404() {
        let err = req(&state(), "DELETE", "/api/authors/999")
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
    }
}
