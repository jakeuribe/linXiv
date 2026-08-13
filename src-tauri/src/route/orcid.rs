//! `/api/orcid` routes — no Python ancestor. Mirrors `route::versions`'
//! on-demand-pass shape (candidates → single-flight guard → network → apply).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::models::PaperMetadata;
use linxiv_core::service::{orcid_backfill as svc, source as svc_source};

/// Pause between distinct DOIs so a backfill pass never bursts CrossRef/
/// OpenAlex — neither is rate-limited by `sources::http` the way arXiv is.
const INTER_DOI_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

static BACKFILL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct BackfillGuard;
impl Drop for BackfillGuard {
    fn drop(&mut self) {
        BACKFILL_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// Returns `Some(result)` if this group owns `(method, path)`, else `None`.
pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("POST", ["api", "orcid", "backfill"]) => Some(backfill(state, ctx).await),
        _ => None,
    }
}

fn default_limit() -> i64 {
    20
}

/// `POST /api/orcid/backfill` — one pass over `limit` random ORCID-less,
/// DOI-linked authors, via CrossRef then OpenAlex per DOI (paced, one write).
async fn backfill(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        #[serde(default = "default_limit")]
        limit: i64,
    }
    let limit = match ctx.body {
        Some(_) => ctx.parse_body::<Body>()?.limit,
        None => default_limit(),
    };
    if !(1..=100).contains(&limit) {
        return Err(ApiError::new(422, "limit must be between 1 and 100"));
    }

    if BACKFILL_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return Err(ApiError::new(
            409,
            "an ORCID backfill is already in progress",
        ));
    }
    let _guard = BackfillGuard;

    let candidates = state.with_conn(|conn| svc::orcid_backfill_candidates(conn, limit))?;
    if candidates.is_empty() {
        return Ok(json!({ "checked": 0, "updated": [], "errored": 0 }));
    }

    let mut dois: Vec<&str> = candidates.iter().map(|c| c.doi.as_str()).collect();
    dois.sort_unstable();
    dois.dedup();

    let mut fetched: HashMap<String, Vec<PaperMetadata>> = HashMap::new();
    let mut errored = 0i64;
    let mut dois = dois.into_iter().peekable();
    while let Some(doi) = dois.next() {
        let (records, doi_errored) = svc_source::orcid_records_for_doi(doi).await;
        if doi_errored {
            errored += 1;
        }
        fetched.insert(doi.to_string(), records);
        if dois.peek().is_some() {
            tokio::time::sleep(INTER_DOI_DELAY).await;
        }
    }

    let updated = state.with_conn(|conn| svc::apply_results(conn, &candidates, &fetched))?;
    Ok(json!({ "checked": candidates.len(), "updated": updated, "errored": errored }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::testutil::{req, state};
    use std::sync::Mutex;

    // Serialize access to BACKFILL_IN_PROGRESS across all tests to prevent
    // races (flag must reset on Drop even if a test panics).
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn backfill_on_empty_library_returns_zero_without_network() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let v = req(&state(), "POST", "/api/orcid/backfill", None)
            .await
            .unwrap();
        assert_eq!(v, json!({ "checked": 0, "updated": [], "errored": 0 }));
    }

    #[tokio::test]
    async fn backfill_rejects_out_of_range_limit() {
        let _guard = TEST_MUTEX.lock().unwrap();
        for limit in [0, 101, -3] {
            let err = req(
                &state(),
                "POST",
                "/api/orcid/backfill",
                Some(json!({ "limit": limit })),
            )
            .await
            .unwrap_err();
            assert_eq!(err.status, 422, "limit={limit}");
        }
    }

    #[tokio::test]
    async fn backfill_in_progress_returns_409() {
        let _guard = TEST_MUTEX.lock().unwrap();
        BACKFILL_IN_PROGRESS.store(true, Ordering::SeqCst);
        // Resets the flag on drop, including on assertion panic, so a failure
        // here can't poison the other tests sharing TEST_MUTEX.
        let _reset = BackfillGuard;

        let err = req(&state(), "POST", "/api/orcid/backfill", None)
            .await
            .unwrap_err();

        assert_eq!(err.status, 409);
    }
}
