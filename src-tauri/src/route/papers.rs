//! `/api/papers` routes — api/app.py 204-461 (list/versions/by-sfk/search/get/delete/repair).
//! Stub: filled in Phase 5b. Copy the shape of `route/authors.rs`.

use serde_json::Value;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(
    _state: &AppState,
    _ctx: &ReqCtx<'_>,
) -> Option<Result<Value, ApiError>> {
    None
}
