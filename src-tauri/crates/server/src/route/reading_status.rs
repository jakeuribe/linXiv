//! `/api/reading-status` routes — the wire surface for the PAPER_TO_READING
//! table. The frontend keys reading status globally per paper (one pill per
//! paper, identical in every list), so this group speaks that shape and
//! `service::reading_list` maps it onto the per-(project, paper) rows — see the
//! keying note there.
//!
//! No Python ancestor: this group is new (the feature previously persisted to
//! localStorage only), so the envelopes below are the contract.

use serde::Deserialize;
use serde_json::Value;

use linxiv_core::service::reading_list;

use crate::route::{to_value, ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("GET", ["api", "reading-status"]) => Some(list(state)),
        ("PUT", ["api", "reading-status", sid]) => Some(put(state, sid, ctx)),
        _ => None,
    }
}

/// `GET /api/reading-status` — `{"statuses": {source_id: "reading"|"read"}}`.
/// Sparse: unread papers are absent.
fn list(state: &AppState) -> Result<Value, ApiError> {
    let statuses = state.with_conn(|conn| reading_list::statuses_response(conn))?;
    to_value(&statuses)
}

/// `PUT /api/reading-status/{source_id}` — set the paper's status in every
/// reading list it belongs to; `"unread"` clears it. `applied` is the number of
/// lists written (0 when the paper is on no reading list — a no-op, not an
/// error, so the client's localStorage migration can push blindly).
fn put(state: &AppState, sid: &str, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        status: String,
    }
    let b: Body = ctx.parse_body()?;
    // CoreError::Validation → 422, matching FastAPI enum-body coercion.
    let status = b.status.parse::<reading_list::ReadingStatus>()?;
    let applied = state.with_conn(|conn| reading_list::set_for_paper(conn, sid, status))?;
    to_value(&reading_list::ReadingStatusReceipt { ok: true, applied })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::testutil::{req, state};
    use serde_json::json;

    /// Seed a paper root plus a tagged reading-list project through the public
    /// routes where possible (paper roots have no route, so that row is direct).
    async fn seed_list_with_paper(st: &AppState) -> i64 {
        st.with_conn(|conn| {
            conn.execute(
                "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1')",
                [],
            )
            .unwrap();
        });
        let created = req(
            st,
            "POST",
            "/api/projects",
            Some(json!({ "name": "RL", "project_tags": ["reading-list"] })),
        )
        .await
        .unwrap();
        let pid = created["project"]["id"].as_i64().unwrap();
        req(
            st,
            "POST",
            &format!("/api/projects/{pid}/papers"),
            Some(json!({ "source_id": "arxiv:1" })),
        )
        .await
        .unwrap();
        pid
    }

    #[tokio::test]
    async fn list_on_empty_db_wraps_empty_object() {
        assert_eq!(
            req(&state(), "GET", "/api/reading-status", None)
                .await
                .unwrap(),
            json!({ "statuses": {} })
        );
    }

    #[tokio::test]
    async fn put_then_get_round_trips_and_unread_clears() {
        let st = state();
        seed_list_with_paper(&st).await;

        let out = req(
            &st,
            "PUT",
            "/api/reading-status/arxiv:1",
            Some(json!({ "status": "reading" })),
        )
        .await
        .unwrap();
        assert_eq!(out, json!({ "ok": true, "applied": 1 }));
        assert_eq!(
            req(&st, "GET", "/api/reading-status", None).await.unwrap(),
            json!({ "statuses": { "arxiv:1": "reading" } })
        );

        req(
            &st,
            "PUT",
            "/api/reading-status/arxiv:1",
            Some(json!({ "status": "unread" })),
        )
        .await
        .unwrap();
        assert_eq!(
            req(&st, "GET", "/api/reading-status", None).await.unwrap(),
            json!({ "statuses": {} })
        );
    }

    #[tokio::test]
    async fn put_unknown_paper_is_404() {
        let err = req(
            &state(),
            "PUT",
            "/api/reading-status/ghost",
            Some(json!({ "status": "read" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
        assert_eq!(err.detail, "Paper ghost not found");
    }

    #[tokio::test]
    async fn put_invalid_status_is_422() {
        let st = state();
        seed_list_with_paper(&st).await;
        let err = req(
            &st,
            "PUT",
            "/api/reading-status/arxiv:1",
            Some(json!({ "status": "skimmed" })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 422);
        assert_eq!(
            err.detail,
            "Invalid reading status \"skimmed\". Use 'unread', 'reading', or 'read'."
        );
    }

    #[tokio::test]
    async fn put_on_paper_outside_any_reading_list_applies_zero() {
        let st = state();
        st.with_conn(|conn| {
            conn.execute(
                "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1')",
                [],
            )
            .unwrap();
        });
        let out = req(
            &st,
            "PUT",
            "/api/reading-status/arxiv:1",
            Some(json!({ "status": "read" })),
        )
        .await
        .unwrap();
        assert_eq!(out, json!({ "ok": true, "applied": 0 }));
    }
}
