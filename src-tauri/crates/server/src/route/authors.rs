//! `/api/authors` routes — `api/app.py` 1011–1045. The reference resource group:
//! a `handle` that owns its path subtree, path-param extraction, 404/409 mapping,
//! body deserialization, and composite serialization. Every other group module
//! copies this shape. Core binding mirrors `mcp/src/io_authors_misc.rs`.

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::service::author::{self as svc_author, Author, Authors};
use linxiv_core::service::paper::{self as svc_paper, PaperRef};

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
        ("POST", ["api", "authors", id, "papers", pid]) => Some(link_paper(state, id, pid)),
        ("DELETE", ["api", "authors", id, "papers", pid]) => Some(unlink_paper(state, id, pid)),
        ("DELETE", ["api", "authors", id]) => Some(delete(state, id)),
        _ => None,
    }
}

/// `GET /api/authors?exclude_single=` — `api_authors_list`.
fn list(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    // Floor of 1, not 0: paperless AUTHOR rows exist by design (trash-linked
    // papers keep their links for restore; ADR-0009 leaves hard-delete orphans)
    // and must stay out of the list. CLI keeps 0 to see them.
    let min_papers = if ctx.q_bool("exclude_single") { 2 } else { 1 };
    let authors = state.with_conn(|conn| svc_author::list_with_paper_count(conn, min_papers))?;
    Ok(json!({ "authors": authors }))
}

/// `GET /api/authors/{id}` — `api_author_get` → `_author_detail_response`.
fn detail(state: &AppState, id: &str) -> Result<Value, ApiError> {
    detail_response(state, path_i64(id)?)
}

/// `GET /api/authors/{id}/merge-candidates` — likely-duplicate suggestions for
/// the merge UI, split by evidence strength: `candidates` shares this author's
/// ORCID (near-certain duplicate), `name_candidates` only its exact full name
/// (NOCASE — weak evidence). An author matching both surfaces once, as ORCID.
fn merge_candidates(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let author_id = path_i64(id)?;
    let (orcid, by_name) = state.with_conn(|conn| -> Result<_, ApiError> {
        let author = svc_author::get(conn, &author_ref(author_id))?
            .ok_or_else(|| ApiError::new(404, "Author not found"))?;
        let orcid = svc_author::orcid_merge_candidates(conn, author_id)?;
        let mut by_name = match author.full_name {
            Some(name) => svc_author::get_many(
                conn,
                &Authors {
                    paper_id: None,
                    name: Some(vec![name]),
                    author_ids: None,
                },
            )?,
            None => Vec::new(),
        };
        let taken: std::collections::HashSet<i64> = orcid
            .iter()
            .map(|c| c.author_id)
            .chain([author_id])
            .collect();
        by_name.retain(|c| !taken.contains(&c.author_id));
        by_name.sort_by_key(|c| c.author_id);
        Ok((orcid, by_name))
    })?;
    Ok(json!({ "candidates": orcid, "name_candidates": by_name }))
}

/// `POST /api/authors/{id}/papers/{paper_id}` — attach one paper to an author
/// without merging whole author records. Idempotent: relinking an existing pair
/// is a no-op (storage INSERT OR IGNORE). 404 if either side is absent — the
/// FK constraint would otherwise surface as a 500.
fn link_paper(state: &AppState, id: &str, pid: &str) -> Result<Value, ApiError> {
    let author_id = path_i64(id)?;
    let paper_id = path_i64(pid)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        if svc_author::get(conn, &author_ref(author_id))?.is_none() {
            return Err(ApiError::new(404, "Author not found"));
        }
        if svc_paper::get(conn, &PaperRef::Id(paper_id))?.is_none() {
            return Err(ApiError::new(404, "Paper not found"));
        }
        Ok(svc_author::link_author_to_paper(
            conn, author_id, paper_id, None,
        )?)
    })?;
    Ok(json!({ "ok": true }))
}

/// `DELETE /api/authors/{id}/papers/{paper_id}` — detach one paper from an
/// author. Unlinks every stored version of the paper's root: link rows are
/// per-version and the author detail page shows latest-version ids, so an
/// author linked only to v1 must still detach when addressed by v2's id.
/// Idempotent on an absent link; unlinking a paper's last author is allowed
/// (author-less papers are a legal state — `delete` requires it).
fn unlink_paper(state: &AppState, id: &str, pid: &str) -> Result<Value, ApiError> {
    let author_id = path_i64(id)?;
    let paper_id = path_i64(pid)?;
    state.with_conn(|conn| -> Result<(), ApiError> {
        if svc_author::get(conn, &author_ref(author_id))?.is_none() {
            return Err(ApiError::new(404, "Author not found"));
        }
        if !svc_author::unlink_author_from_paper(conn, author_id, paper_id)? {
            return Err(ApiError::new(404, "Paper not found"));
        }
        Ok(())
    })?;
    Ok(json!({ "ok": true }))
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

/// The canonical `AuthorWithPapers` composite — shared by GET and PATCH.
fn detail_response(state: &AppState, author_id: i64) -> Result<Value, ApiError> {
    state.with_conn(|conn| {
        let detail = svc_author::get_with_papers(conn, author_id)?
            .ok_or_else(|| ApiError::new(404, "Author not found"))?;
        crate::route::to_value(&detail)
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
    async fn list_hides_authors_with_no_active_papers() {
        let st = state();
        // canonical: one active paper; dup: only paper about to be trashed.
        let (active, trashed, _pid1, _pid2) = st.with_conn(seed_two_authors_with_papers);
        st.with_conn(|conn| {
            conn.execute(
                "UPDATE PAPER_ROOTS SET STATUS = 'deleted' WHERE SOURCE_ID = 'arxiv:2'",
                [],
            )
            .unwrap();
            // A hard-delete orphan (ADR-0009): AUTHOR row with no links at all.
            svc_author::create(
                conn,
                &AuthorIn {
                    full_name: "Ghost Author".into(),
                    first_name: None,
                    last_name: None,
                    orcid: None,
                },
            )
            .unwrap()
        });

        let resp = req(&st, "GET", "/api/authors", None).await.unwrap();
        let ids: Vec<i64> = resp["authors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["author_id"].as_i64().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![active],
            "trash-linked and orphan authors must not list"
        );
        let _ = trashed;
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
    async fn merge_candidates_splits_orcid_from_name_matches() {
        let st = state();
        let (target, orcid_twin, name_twin_a, name_twin_b) = st.with_conn(|conn| {
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
            let target = mk(conn, "Bob Stone", Some("0000-1"));
            // Same ORCID *and* same name -> must surface as ORCID only, not twice.
            let orcid_twin = mk(conn, "Bob Stone", Some("0000-1"));
            let name_twin_b = mk(conn, "bob stone", None); // NOCASE match
            let name_twin_a = mk(conn, "Bob Stone", Some("0000-2"));
            mk(conn, "Someone Else", None);
            (target, orcid_twin, name_twin_a, name_twin_b)
        });

        let resp = req(
            &st,
            "GET",
            &format!("/api/authors/{target}/merge-candidates"),
            None,
        )
        .await
        .unwrap();
        let ids = |key: &str| -> Vec<i64> {
            resp[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["author_id"].as_i64().unwrap())
                .collect()
        };
        assert_eq!(ids("candidates"), vec![orcid_twin]);
        // Name bucket excludes self and the ORCID match; ordered by author_id.
        assert_eq!(
            ids("name_candidates"),
            vec![name_twin_b.min(name_twin_a), name_twin_b.max(name_twin_a)]
        );
    }

    #[tokio::test]
    async fn merge_candidates_nameless_author_has_no_name_bucket() {
        let st = state();
        let target = st.with_conn(|conn| {
            conn.execute("INSERT INTO AUTHOR (AUTHOR_ORCID) VALUES ('0000-9')", [])
                .unwrap();
            conn.last_insert_rowid()
        });
        let resp = req(
            &st,
            "GET",
            &format!("/api/authors/{target}/merge-candidates"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(resp["name_candidates"], json!([]));
    }

    #[tokio::test]
    async fn link_and_unlink_move_a_single_paper() {
        let st = state();
        let (canonical, dup, _pid1, pid2) = st.with_conn(seed_two_authors_with_papers);

        // Reassign pid2 from dup to canonical: link, then unlink.
        req(
            &st,
            "POST",
            &format!("/api/authors/{canonical}/papers/{pid2}"),
            None,
        )
        .await
        .unwrap();
        req(
            &st,
            "DELETE",
            &format!("/api/authors/{dup}/papers/{pid2}"),
            None,
        )
        .await
        .unwrap();

        let canonical_detail = req(&st, "GET", &format!("/api/authors/{canonical}"), None)
            .await
            .unwrap();
        assert_eq!(canonical_detail["paper_count"], json!(2));
        let dup_detail = req(&st, "GET", &format!("/api/authors/{dup}"), None)
            .await
            .unwrap();
        // Unlike merge, the duplicate author survives — only the link moved.
        assert_eq!(dup_detail["paper_count"], json!(0));
    }

    #[tokio::test]
    async fn link_is_idempotent_on_existing_pair() {
        let st = state();
        let (canonical, _dup, pid1, _pid2) = st.with_conn(seed_two_authors_with_papers);
        req(
            &st,
            "POST",
            &format!("/api/authors/{canonical}/papers/{pid1}"),
            None,
        )
        .await
        .unwrap();
        let detail = req(&st, "GET", &format!("/api/authors/{canonical}"), None)
            .await
            .unwrap();
        assert_eq!(detail["paper_count"], json!(1));
    }

    #[tokio::test]
    async fn link_and_unlink_missing_author_or_paper_is_404() {
        let st = state();
        let (canonical, _dup, pid1, _pid2) = st.with_conn(seed_two_authors_with_papers);
        for (method, path) in [
            ("POST", format!("/api/authors/999/papers/{pid1}")),
            ("POST", format!("/api/authors/{canonical}/papers/999")),
            ("DELETE", format!("/api/authors/999/papers/{pid1}")),
            ("DELETE", format!("/api/authors/{canonical}/papers/999")),
        ] {
            let err = req(&st, method, &path, None).await.unwrap_err();
            assert_eq!(err.status, 404, "{method} {path}");
        }
    }

    #[tokio::test]
    async fn unlink_covers_every_version_of_the_root() {
        let st = state();
        let (canonical, _dup, pid1, _pid2) = st.with_conn(seed_two_authors_with_papers);
        // A second version of paper 1: the author stays linked to v1's row only,
        // while the UI addresses the paper by the latest version's id.
        let pid1_v2 = st.with_conn(|conn| {
            let fk: i64 = conn
                .query_row(
                    "SELECT SOURCE_FK FROM PAPER WHERE PAPER_ID = ?",
                    params![pid1],
                    |r| r.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('arxiv:1', 2, 'T1v2', ?)",
                params![fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?, '2024-02-01')",
                params![pid],
            )
            .unwrap();
            pid
        });

        req(
            &st,
            "DELETE",
            &format!("/api/authors/{canonical}/papers/{pid1_v2}"),
            None,
        )
        .await
        .unwrap();
        let detail = req(&st, "GET", &format!("/api/authors/{canonical}"), None)
            .await
            .unwrap();
        assert_eq!(detail["paper_count"], json!(0));
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
