//! `/api/versions` routes — arXiv new-version monitoring (no Python ancestor).
//! `check` runs one on-demand poll pass: pick the stalest N saved arXiv papers,
//! ask arXiv for their latest versions in ONE rate-limited request, and capture
//! anything newer through the existing save path. `new`/`ack` surface and clear
//! the "new version found" flags.
//!
//! ponytail: manual/on-demand pass only — scheduled background polling when a
//! scheduler exists; then this route body becomes the tick.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::config;
use linxiv_core::service::version_monitor as svc;
use linxiv_core::sources::arxiv;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

// ponytail: single global flag; upgrade to per-account locks if
// throughput needs multiple concurrent checks.
static CHECK_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct CheckGuard;
impl Drop for CheckGuard {
    fn drop(&mut self) {
        CHECK_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("POST", ["api", "versions", "check"]) => Some(check(state, ctx).await),
        ("GET", ["api", "versions", "new"]) => Some(list_new(state)),
        ("POST", ["api", "versions", "ack"]) => Some(ack(state, ctx)),
        _ => None,
    }
}

fn default_limit() -> i64 {
    20
}

/// `POST /api/versions/check` — one poll pass over the stalest `limit` papers.
/// Candidates are read, the single batched arXiv call awaits with no DB lock
/// held, then results are applied. `502` on an arXiv failure (nothing recorded,
/// so the same papers stay at the front of the staleness queue).
async fn check(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        #[serde(default = "default_limit")]
        limit: i64,
    }
    // Body is optional: no body → defaults.
    let limit = match ctx.body {
        Some(_) => ctx.parse_body::<Body>()?.limit,
        None => default_limit(),
    };
    if !(1..=100).contains(&limit) {
        return Err(ApiError::new(422, "limit must be between 1 and 100"));
    }

    if CHECK_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return Err(ApiError::new(409, "version check already in progress"));
    }
    let _guard = CheckGuard;

    let candidates = state.with_conn(|conn| svc::stale_candidates(conn, limit))?;
    if candidates.is_empty() {
        return Ok(json!({ "checked": 0, "new_versions": [] }));
    }
    let ids: Vec<String> = candidates.iter().map(|c| c.source_id.clone()).collect();
    let fetched = arxiv::fetch_by_ids(&ids, &config::data_dir()).await?;
    let found = state.with_conn(|conn| svc::apply_results(conn, &candidates, &fetched))?;
    Ok(json!({ "checked": candidates.len(), "new_versions": found }))
}

/// `GET /api/versions/new` — papers with an un-acknowledged new version.
fn list_new(state: &AppState) -> Result<Value, ApiError> {
    let list = state.with_conn(|conn| svc::list_new_versions(conn))?;
    Ok(json!({ "new_versions": list }))
}

/// `POST /api/versions/ack` — clear the flag for one paper. 404 when unset.
fn ack(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        source_fk: i64,
    }
    let b: Body = ctx.parse_body()?;
    let cleared = state.with_conn(|conn| svc::ack(conn, b.source_fk))?;
    if !cleared {
        return Err(ApiError::new(404, "no new version flagged for this paper"));
    }
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::storage;
    use std::sync::Mutex;

    // Serialize access to CHECK_IN_PROGRESS across all tests to prevent race
    // conditions (flag must reset on Drop even if test panics).
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

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

    #[tokio::test]
    async fn check_on_empty_library_returns_zero_without_network() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let v = req(&state(), "POST", "/api/versions/check", None)
            .await
            .unwrap();
        assert_eq!(v, json!({ "checked": 0, "new_versions": [] }));
    }

    #[tokio::test]
    async fn check_rejects_out_of_range_limit() {
        let _guard = TEST_MUTEX.lock().unwrap();
        for limit in [0, 101, -3] {
            let err = req(
                &state(),
                "POST",
                "/api/versions/check",
                Some(json!({ "limit": limit })),
            )
            .await
            .unwrap_err();
            assert_eq!(err.status, 422, "limit={limit}");
        }
    }

    #[tokio::test]
    async fn check_in_progress_returns_409() {
        let _guard = TEST_MUTEX.lock().unwrap();
        CHECK_IN_PROGRESS.store(true, Ordering::SeqCst);

        let err = req(&state(), "POST", "/api/versions/check", None)
            .await
            .unwrap_err();

        assert_eq!(err.status, 409);

        CHECK_IN_PROGRESS.store(false, Ordering::SeqCst);
    }

    #[tokio::test]
    async fn new_list_empty_and_ack_unset_is_404() {
        let st = state();
        let v = req(&st, "GET", "/api/versions/new", None).await.unwrap();
        assert_eq!(v, json!({ "new_versions": [] }));
        let err = req(
            &st,
            "POST",
            "/api/versions/ack",
            Some(json!({ "source_fk": 1 })),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, 404);
    }
}
