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
        ("GET", ["api", "authors", id, "merge-candidates"]) => Some(merge_candidates(state, id)),
        ("PATCH", ["api", "authors", id]) => Some(update(state, id, ctx)),
        ("POST", ["api", "authors", id, "merge"]) => Some(merge(state, id, ctx)),
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

/// `GET /api/authors/{id}/merge-candidates` — other authors sharing this
/// author's ORCID, for the merge UI's "likely duplicate" suggestion.
fn merge_candidates(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let author_id = path_i64(id)?;
    let candidates = state.with_conn(|conn| -> Result<_, ApiError> {
        if svc_author::get(conn, &author_ref(author_id))?.is_none() {
            return Err(ApiError::new(404, "Author not found"));
        }
        Ok(svc_author::orcid_merge_candidates(conn, author_id)?)
    })?;
    Ok(json!({ "candidates": candidates }))
}

/// `PATCH /api/authors/{id}` — `api_author_update`. Forwards the (all-optional)
/// fields to `update_fields`, then returns the same detail shape as GET.
fn update(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let author_id = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        full_name: Option<String>,
        first_name: Option<String>,
        last_name: Option<String>,
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

/// `POST /api/authors/{id}/merge` — fold `duplicate_ids` into author `{id}`,
/// re-pointing their papers, then returns the canonical author's detail shape.
fn merge(state: &AppState, id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let canonical_id = path_i64(id)?;
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        duplicate_ids: Vec<i64>,
    }
    let b: Body = ctx.parse_body()?;
    let merged_ids = state.with_conn(|conn| -> Result<Vec<i64>, ApiError> {
        if svc_author::get(conn, &author_ref(canonical_id))?.is_none() {
            return Err(ApiError::new(404, "Author not found"));
        }
        Ok(svc_author::merge(conn, canonical_id, &b.duplicate_ids)?)
    })?;
    let mut v = detail_response(state, canonical_id)?;
    if let Value::Object(map) = &mut v {
        map.insert("merged_ids".into(), json!(merged_ids));
    }
    Ok(v)
}

/// `DELETE /api/authors/{id}` — `api_author_delete`. 404 if absent, 409 if still
/// linked to papers; both guards live in `svc_author::delete`.
fn delete(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let author_id = path_i64(id)?;
    state.with_conn(|conn| svc_author::delete(conn, &author_ref(author_id)))?;
    Ok(json!({ "ok": true }))
}

/// Build `{**author.to_dict(), paper_count, papers}` — shared by GET and PATCH.
fn detail_response(state: &AppState, author_id: i64) -> Result<Value, ApiError> {
    state.with_conn(|conn| {
        let author = svc_author::get(conn, &author_ref(author_id))?
            .ok_or_else(|| ApiError::new(404, "Author not found"))?;
        let previews = svc_author::get_paper_previews(conn, author_id)?;
        let mut v = crate::route::to_value(&author)?;
        if let Value::Object(map) = &mut v {
            map.insert("paper_count".into(), json!(previews.len()));
            map.insert("papers".into(), crate::route::to_value(&previews)?);
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
    use linxiv_core::models::AuthorIn;
    use linxiv_core::storage;
    use rusqlite::{params, Connection};

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    async fn req(
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

    // Two paper roots, each with one distinct author linked. Returns
    // (canonical_author, dup_author, canonical_paper_id, dup_paper_id).
    fn seed_two_authors_with_papers(conn: &mut Connection) -> (i64, i64, i64, i64) {
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1')", [])
            .unwrap();
        let fk1 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('arxiv:1', 1, 'T1', ?)",
            params![fk1],
        )
        .unwrap();
        let pid1 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?, '2024-01-01')",
            params![pid1],
        )
        .unwrap();

        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:2')", [])
            .unwrap();
        let fk2 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('arxiv:2', 1, 'T2', ?)",
            params![fk2],
        )
        .unwrap();
        let pid2 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?, '2024-01-02')",
            params![pid2],
        )
        .unwrap();

        let author = |c: &mut Connection| {
            svc_author::create(
                c,
                &AuthorIn {
                    full_name: "Bob Stone".into(),
                    first_name: None,
                    last_name: None,
                    orcid: None,
                },
            )
            .unwrap()
        };
        let canonical = author(conn);
        let dup = author(conn);
        linxiv_core::storage::queries::author::link_author_to_paper(conn, canonical, pid1, Some(0))
            .unwrap();
        linxiv_core::storage::queries::author::link_author_to_paper(conn, dup, pid2, Some(0))
            .unwrap();
        (canonical, dup, pid1, pid2)
    }

    #[tokio::test]
    async fn merge_folds_duplicate_and_returns_merged_ids() {
        let st = state();
        let (canonical, dup, pid1, pid2) = st.with_conn(seed_two_authors_with_papers);

        let resp = req(
            &st,
            "POST",
            &format!("/api/authors/{canonical}/merge"),
            Some(json!({ "duplicate_ids": [dup] })),
        )
        .await
        .unwrap();

        assert_eq!(resp["merged_ids"], json!([dup]));
        assert_eq!(resp["paper_count"], json!(2));
        let paper_ids: Vec<i64> = resp["papers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["paper_id"].as_i64().unwrap())
            .collect();
        assert!(paper_ids.contains(&pid1));
        assert!(paper_ids.contains(&pid2));
    }

    #[tokio::test]
    async fn merge_missing_canonical_is_404() {
        let err = req(
            &state(),
            "POST",
            "/api/authors/999/merge",
            Some(json!({ "duplicate_ids": [] })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[tokio::test]
    async fn merge_bogus_duplicate_id_is_absent_but_still_ok() {
        let st = state();
        let canonical = st.with_conn(|conn| {
            svc_author::create(
                conn,
                &AuthorIn {
                    full_name: "Solo Author".into(),
                    first_name: None,
                    last_name: None,
                    orcid: None,
                },
            )
            .unwrap()
        });

        let resp = req(
            &st,
            "POST",
            &format!("/api/authors/{canonical}/merge"),
            Some(json!({ "duplicate_ids": [999_999] })),
        )
        .await
        .unwrap();

        assert_eq!(resp["merged_ids"], json!([]));
    }

    #[tokio::test]
    async fn merge_candidates_matches_same_orcid_only() {
        let st = state();
        let (a, b, c) = st.with_conn(|conn| {
            let mk = |c: &mut Connection, name: &str, orcid: Option<&str>| {
                svc_author::create(
                    c,
                    &AuthorIn {
                        full_name: name.into(),
                        first_name: None,
                        last_name: None,
                        orcid: orcid.map(String::from),
                    },
                )
                .unwrap()
            };
            let a = mk(conn, "Alice Cole", Some("0000-1"));
            let b = mk(conn, "A. Cole", Some("0000-1"));
            let c = mk(conn, "Carl No-Orcid", None);
            (a, b, c)
        });

        let resp = req(
            &st,
            "GET",
            &format!("/api/authors/{a}/merge-candidates"),
            None,
        )
        .await
        .unwrap();
        let ids: Vec<i64> = resp["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["author_id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![b]);

        // No ORCID -> no candidates, not an error.
        let resp = req(
            &st,
            "GET",
            &format!("/api/authors/{c}/merge-candidates"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(resp["candidates"], json!([]));
    }

    #[tokio::test]
    async fn merge_candidates_missing_author_is_404() {
        let err = req(&state(), "GET", "/api/authors/999/merge-candidates", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[tokio::test]
    async fn list_on_empty_db_wraps_empty_array() {
        assert_eq!(
            req(&state(), "GET", "/api/authors", None).await.unwrap(),
            json!({ "authors": [] })
        );
    }

    #[tokio::test]
    async fn get_missing_author_is_404() {
        let err = req(&state(), "GET", "/api/authors/999", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Author not found");
    }

    #[tokio::test]
    async fn non_integer_id_is_422() {
        let err = req(&state(), "GET", "/api/authors/abc", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn delete_missing_author_is_404() {
        let err = req(&state(), "DELETE", "/api/authors/999", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[tokio::test]
    async fn delete_linked_author_is_409() {
        let st = state();
        let (canonical, ..) = st.with_conn(seed_two_authors_with_papers);
        let err = req(&st, "DELETE", &format!("/api/authors/{canonical}"), None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 409);
    }

    #[tokio::test]
    async fn update_with_no_fields_is_422() {
        let st = state();
        let (canonical, ..) = st.with_conn(seed_two_authors_with_papers);
        let err = req(
            &st,
            "PATCH",
            &format!("/api/authors/{canonical}"),
            Some(json!({})),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
    }
}
