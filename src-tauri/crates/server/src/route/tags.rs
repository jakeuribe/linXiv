//! `/api/tags` routes (plus the paper-tag mutations under `/api/papers/{id}/tags`,
//! which are 4-segment paths no other group claims) — `api/app.py` 474–509.
//! Copies the `authors.rs` shape:
//! a `handle` that owns the path subtree, returning `Some(result)` for routes it
//! owns and `None` to pass. Core binding mirrors `mcp/src/projects_tags.rs`.

use serde::Deserialize;
use serde_json::Value;

use linxiv_core::models::TagIn;
use linxiv_core::service::paper as svc_paper;
use linxiv_core::service::tag::{self as svc_tag, CreatedTag, DeletedTag, PaperTags, Tag};

use crate::route::{path_i64, ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "tags"]) => Some(list(state)),
        ("POST", ["api", "tags"]) => Some(create(state, ctx)),
        // 3-segment `/api/tags/{…}`: GET takes a label, DELETE a numeric id.
        ("GET", ["api", "tags", label]) => Some(detail(state, label)),
        ("DELETE", ["api", "tags", id]) => Some(delete(state, id)),
        ("POST", ["api", "papers", source_id, "tags"]) => Some(add_tags(state, source_id, ctx)),
        ("DELETE", ["api", "papers", source_id, "tags"]) => {
            Some(remove_tags(state, source_id, ctx))
        }
        _ => None,
    }
}

/// `GET /api/tags` — `api_tags`. Each tag carries its active-paper count so the
/// index can render a table sortable by name or by count.
fn list(state: &AppState) -> Result<Value, ApiError> {
    let tags = state.with_conn(|conn| svc_tag::list_tags_with_count(conn))?;
    crate::route::to_value(&svc_tag::TagsResponse { tags })
}

/// `GET /api/tags/{label}` — `api_tag_detail`. The composite lives in core
/// (`svc_tag::detail` → `TagDetail`): canonical label, papers, active projects.
fn detail(state: &AppState, label: &str) -> Result<Value, ApiError> {
    let d = state.with_conn(|conn| svc_tag::detail(conn, label))?;
    crate::route::to_value(&d)
}

/// `POST /api/tags` — `svc_tag::upsert`, same envelope as the CLI `tag create`.
/// Upsert semantics: an existing label returns its id instead of erroring.
fn create(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        label: String,
    }
    let b: Body = ctx.parse_body()?;
    let label = b.label.trim().to_string();
    if label.is_empty() {
        return Err(ApiError::new(422, "label must not be empty"));
    }
    let tag_id = state.with_conn(|conn| {
        svc_tag::upsert(
            conn,
            &TagIn {
                label: label.clone(),
            },
        )
    })?;
    crate::route::to_value(&CreatedTag { tag_id, label })
}

/// `DELETE /api/tags/{id}` — `svc_tag::delete` by numeric id. Core's delete is a
/// silent no-op on an unknown id, so existence is checked first for the 404.
fn delete(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let tag_id = path_i64(id)?;
    let key = Tag {
        tag_id: Some(tag_id),
        label: None,
    };
    state.with_conn(|conn| -> Result<(), ApiError> {
        if svc_tag::get(conn, &key)?.is_none() {
            return Err(ApiError::new(404, format!("Tag {tag_id} not found")));
        }
        svc_tag::delete(conn, &key)?;
        Ok(())
    })?;
    crate::route::to_value(&DeletedTag {
        deleted_tag_id: tag_id,
    })
}

/// The `{"tags": [...]}` body both paper-tag arms take. Trimmed, blanks dropped,
/// case-deduped — the normalization `create` and `add_project_tags` already
/// apply, so a label of `"  "` can't reach the TAG table. Nothing left is a 422.
fn body_tags(ctx: &ReqCtx<'_>) -> Result<Vec<String>, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        tags: Vec<String>,
    }
    let b: Body = ctx.parse_body()?;
    let mut seen = std::collections::HashSet::new();
    let tags: Vec<String> = b
        .tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty() && seen.insert(t.to_lowercase()))
        .map(str::to_string)
        .collect();
    if tags.is_empty() {
        return Err(ApiError::new(422, "tags must not be empty"));
    }
    Ok(tags)
}

/// `POST /api/papers/{source_id}/tags` — `svc_paper::add_paper_tags`; returns the
/// paper's full tag list after the union. Unknown paper → `PaperNotFound` → 404.
fn add_tags(state: &AppState, source_id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let tags = body_tags(ctx)?;
    let updated = state.with_conn(|conn| svc_paper::add_paper_tags(conn, source_id, &tags))?;
    crate::route::to_value(&PaperTags {
        source_id: source_id.to_string(),
        tags: updated,
    })
}

/// `DELETE /api/papers/{source_id}/tags` — `svc_paper::remove_paper_tags`; returns
/// the remaining tags. Unknown paper → `PaperNotFound` → 404.
fn remove_tags(state: &AppState, source_id: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    let tags = body_tags(ctx)?;
    let updated = state.with_conn(|conn| svc_paper::remove_paper_tags(conn, source_id, &tags))?;
    crate::route::to_value(&PaperTags {
        source_id: source_id.to_string(),
        tags: updated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::models::PaperMetadata;
    use linxiv_core::storage;
    use serde_json::json;

    fn state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

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

    async fn send(
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

    /// One active paper `p1` written through the real writer, so the PAPER_META row
    /// `latest_papers` inner-joins is present. Built via serde (no chrono dep here).
    fn seed_paper(st: &AppState) {
        let meta: PaperMetadata = serde_json::from_value(json!({
            "source_id": "p1",
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-15",
            "summary": "S",
        }))
        .unwrap();
        st.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))
            .unwrap();
    }

    #[tokio::test]
    async fn list_on_empty_db_wraps_empty_array() {
        assert_eq!(
            get(&state(), "/api/tags").await.unwrap(),
            json!({ "tags": [] })
        );
    }

    #[tokio::test]
    async fn list_returns_label_and_paper_count_keys() {
        let st = state();
        st.with_conn(|conn| {
            conn.execute("INSERT INTO TAG (TAG) VALUES ('Used')", []).unwrap();
            conn.execute(
                "INSERT INTO PAPER_ROOTS (SOURCE_ID, STATUS) VALUES ('p1', 'active')",
                [],
            )
            .unwrap();
            let fk = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('p1', 1, 'T', ?)",
                [fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_TO_TAG (PAPER_ID, TAG_FK) SELECT ?, TAG_FK FROM TAG WHERE TAG = 'Used'",
                [pid],
            )
            .unwrap();
        });

        let v = get(&st, "/api/tags").await.unwrap();
        let tag = &v["tags"][0];
        assert_eq!(tag["label"], json!("Used"));
        assert_eq!(tag["paper_count"], json!(1));
    }

    #[tokio::test]
    async fn create_upserts_and_shows_up_in_list() {
        let st = state();
        let v = send(&st, "POST", "/api/tags", Some(json!({ "label": "Nets" })))
            .await
            .unwrap();
        let id = v["tag_id"].as_i64().unwrap();
        assert_eq!(v["label"], json!("Nets"));
        // Second create of the same label returns the same id, not a duplicate.
        let again = send(&st, "POST", "/api/tags", Some(json!({ "label": "Nets" })))
            .await
            .unwrap();
        assert_eq!(again["tag_id"].as_i64().unwrap(), id);
        assert_eq!(
            get(&st, "/api/tags").await.unwrap()["tags"][0]["label"],
            json!("Nets")
        );
    }

    #[tokio::test]
    async fn create_rejects_blank_label() {
        let err = send(
            &state(),
            "POST",
            "/api/tags",
            Some(json!({ "label": "  " })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn delete_removes_the_tag_and_404s_when_unknown() {
        let st = state();
        let id = send(&st, "POST", "/api/tags", Some(json!({ "label": "Gone" })))
            .await
            .unwrap()["tag_id"]
            .as_i64()
            .unwrap();

        let v = send(&st, "DELETE", &format!("/api/tags/{id}"), None)
            .await
            .unwrap();
        assert_eq!(v, json!({ "deleted_tag_id": id }));
        assert_eq!(get(&st, "/api/tags").await.unwrap(), json!({ "tags": [] }));

        let err = send(&st, "DELETE", &format!("/api/tags/{id}"), None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[tokio::test]
    async fn delete_non_numeric_id_is_422() {
        let err = send(&state(), "DELETE", "/api/tags/nope", None)
            .await
            .unwrap_err();
        assert_eq!(err.status, 422);
    }

    #[tokio::test]
    async fn add_then_remove_paper_tags_round_trips() {
        let st = state();
        seed_paper(&st);

        let v = send(
            &st,
            "POST",
            "/api/papers/p1/tags",
            Some(json!({ "tags": ["a", "b"] })),
        )
        .await
        .unwrap();
        assert_eq!(v["source_id"], json!("p1"));
        assert_eq!(v["tags"], json!(["a", "b"]));

        let v = send(
            &st,
            "DELETE",
            "/api/papers/p1/tags",
            Some(json!({ "tags": ["a"] })),
        )
        .await
        .unwrap();
        assert_eq!(v["tags"], json!(["b"]));
    }

    #[tokio::test]
    async fn paper_tags_on_unknown_paper_is_404() {
        let err = send(
            &state(),
            "POST",
            "/api/papers/missing/tags",
            Some(json!({ "tags": ["a"] })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
    }

    #[tokio::test]
    async fn paper_tags_reject_an_empty_tag_list() {
        let st = state();
        seed_paper(&st);
        for method in ["POST", "DELETE"] {
            let err = send(
                &st,
                method,
                "/api/papers/p1/tags",
                Some(json!({ "tags": [] })),
            )
            .await
            .unwrap_err();
            assert_eq!(err.status, 422, "method={method}");
        }
    }

    #[tokio::test]
    async fn detail_lists_only_active_projects_with_the_label_nocase() {
        use linxiv_core::models::ProjectIn;
        use linxiv_core::service::project::{self as svc_project, Project};

        let st = state();
        fn mk(conn: &mut rusqlite::Connection, name: &str, tags: Vec<String>) -> i64 {
            svc_project::create(
                conn,
                &ProjectIn {
                    name: name.into(),
                    description: String::new(),
                    color: None,
                    tags,
                    source_fks: vec![],
                },
            )
            .unwrap()
        }
        let (tagged, trashed) = st.with_conn(|conn| {
            let tagged = mk(conn, "kept", vec!["Neural".into()]);
            let trashed = mk(conn, "trashed", vec!["Neural".into()]);
            mk(conn, "untagged", vec![]);
            svc_project::delete(
                conn,
                &Project {
                    project_fk: Some(trashed),
                },
            )
            .unwrap();
            (tagged, trashed)
        });

        let v = get(&st, "/api/tags/nEuRaL").await.unwrap();
        assert_eq!(v["label"], json!("Neural"), "canonical stored casing");
        let ids: Vec<i64> = v["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_i64().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![tagged],
            "trashed ({trashed}) and untagged excluded"
        );
    }

    #[tokio::test]
    async fn detail_unknown_label_falls_back_to_raw_label() {
        // No tag, no papers, no projects — canonical label is the raw segment.
        let v = get(&state(), "/api/tags/Neural%20Nets").await.unwrap();
        assert_eq!(
            v,
            json!({ "label": "Neural Nets", "papers": [], "projects": [] })
        );
    }
}
