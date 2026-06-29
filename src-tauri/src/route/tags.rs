//! `/api/tags` routes — `api/app.py` 474–509. Copies the `authors.rs` shape:
//! a `handle` that owns the path subtree, returning `Some(result)` for routes it
//! owns and `None` to pass. Core binding mirrors `mcp/src/projects_tags.rs`.

use serde_json::{json, Value};

use linxiv_core::models::Status;
use linxiv_core::service::paper as svc_paper;
use linxiv_core::service::project::{self as svc_project, Projects};
use linxiv_core::service::tag::{self as svc_tag, Tag};

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "tags"]) => Some(list(state)),
        ("GET", ["api", "tags", label]) => Some(detail(state, label)),
        _ => None,
    }
}

/// `GET /api/tags` — `api_tags`. Each tag carries its active-paper count so the
/// index can render a table sortable by name or by count.
fn list(state: &AppState) -> Result<Value, ApiError> {
    let tags = state.with_conn(|conn| svc_tag::list_tags_with_count(conn))?;
    Ok(json!({ "tags": tags }))
}

/// `GET /api/tags/{label}` — `api_tag_detail`. Canonical label via `tag::get`
/// (raw label if the tag is unknown); papers via `get_papers_by_tag`; projects by
/// a case-insensitive scan of all projects (no `list_projects_by_tag` in core).
fn detail(state: &AppState, label: &str) -> Result<Value, ApiError> {
    state.with_conn(|conn| -> Result<Value, ApiError> {
        let canonical = svc_tag::get(
            conn,
            &Tag {
                label: Some(label.to_string()),
                ..Default::default()
            },
        )?
        .and_then(|t| t.label)
        .unwrap_or_else(|| label.to_string());

        let papers = svc_paper::get_papers_by_tag(conn, label)?;
        let papers: Vec<Value> = papers
            .iter()
            .map(|p| serde_json::to_value(p).map_err(|e| ApiError::new(500, e.to_string())))
            .collect::<Result<_, _>>()?;

        // Status::Active filter matches Python's `_LIST_PROJECTS_BY_TAG_SQL`
        // (`AND pr.STATUS = 'active'`): PROJECT_TO_TAG rows survive soft-delete, so
        // an unfiltered scan would leak archived/deleted projects the API excludes.
        let active = Projects {
            project_fks: None,
            status: Some(Status::Active),
        };
        let mut projects = Vec::new();
        for p in svc_project::get_many(conn, &active)? {
            if !p.project_tags.iter().any(|t| t.eq_ignore_ascii_case(label)) {
                continue;
            }
            let source_ids = svc_paper::sfks_to_source_ids(conn, &p.source_fks)?;
            projects.push(json!({
                "id": p.id,
                "name": p.name,
                "description": p.description,
                "color_hex": p.color.map(svc_project::color_to_hex),
                "project_tags": p.project_tags,
                "source_ids": source_ids,
                "status": p.status,
                "paper_count": source_ids.len(),
            }));
        }

        Ok(json!({ "label": canonical, "papers": papers, "projects": projects }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::storage;

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
    async fn detail_unknown_label_falls_back_to_raw_label() {
        // No tag, no papers, no projects — canonical label is the raw segment.
        let v = get(&state(), "/api/tags/Neural%20Nets").await.unwrap();
        assert_eq!(
            v,
            json!({ "label": "Neural Nets", "papers": [], "projects": [] })
        );
    }
}
