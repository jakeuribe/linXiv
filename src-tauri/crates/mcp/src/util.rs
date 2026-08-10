//! Small output-parity helpers shared across the tool clusters.

use linxiv_core::error::CoreError;
use rmcp::ErrorData;

pub use linxiv_core::formats::pyrepr;

/// `ValueError` → MCP invalid-params, preserving the Python message verbatim.
pub(crate) fn invalid(msg: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(msg.into(), None)
}

/// Unexpected core failure (not one of the explicit `ValueError` paths).
pub(crate) fn core_err(e: CoreError) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

/// Service-layer guard failures (absent row, still-linked conflict, empty patch)
/// are the Python `ValueError`s → invalid-params; DB/FS failures stay internal.
pub(crate) fn guard_err(e: CoreError) -> ErrorData {
    match e {
        CoreError::Internal(m) => ErrorData::internal_error(m, None),
        other => invalid(other.to_string()),
    }
}

/// Serialize a core value to the tool's text result (compact JSON string).
pub(crate) fn json_ok<T: serde::Serialize>(v: &T) -> Result<String, ErrorData> {
    serde_json::to_string(v).map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

/// Run blocking DB/file work off the async runtime. A panic inside `f` poisons the
/// shared connection mutex, so the join error is surfaced rather than unwrapped.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T, ErrorData>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ErrorData::internal_error(format!("background task failed: {e}"), None))
}
