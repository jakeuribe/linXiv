//! `/api/tags` routes — api/app.py 474-509 (list / detail).
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
