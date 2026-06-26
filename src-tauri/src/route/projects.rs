//! `/api/projects` routes — api/app.py 512-688 (list/create/get/patch/delete/add/bulk/remove).
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
