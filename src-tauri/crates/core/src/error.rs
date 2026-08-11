//! Core error model — Rust port of the typed exceptions raised across
//! `service/` and `sources/`, plus the HTTP status codes `api/app.py` maps
//! them to. Variants preserve the boundary contract so Tauri commands can
//! return the same `{"error": "<msg>"}` body + status the FastAPI layer did.
//! Plan §5 + D21.

#[derive(thiserror::Error, Debug)]
pub enum CoreError {
    // ── Typed failure modes (named so callers can branch + word them) ──────
    /// service.project.ProjectNotFoundError — membership guard. 404. Carries
    /// the project id so every surface words the message identically.
    #[error("Project {0} not found")]
    ProjectNotFound(i64),
    /// service.project.ProjectDeletedError — project is soft-deleted. 400.
    #[error("{0}")]
    ProjectDeleted(String),
    /// Paper lookup miss. 404.
    #[error("Paper not found")]
    PaperNotFound,
    /// service.paper.PdfImportError — metadata extraction failed. 422.
    #[error("Could not extract PDF metadata: {0}")]
    PdfImport(String),
    /// service.paper.PaperLinkError — paper imported but project link failed. 400.
    #[error("{0}")]
    PaperLink(String),
    /// Upload over the size limit. 413.
    #[error("{0}")]
    PdfTooLarge(String),
    /// sources.arxiv_source.ArxivNotFoundError. 404.
    #[error("{0}")]
    ArxivNotFound(String),
    /// sources.openalex_source.OpenAlexNotFoundError. 404.
    #[error("{0}")]
    OpenAlexNotFound(String),
    /// sources.openalex_source.OpenAlexHTTPError — upstream HTTP failure. 502.
    #[error("{0}")]
    OpenAlexHttp(String),
    /// sources.openalex_source.OpenAlexInputError — bad query/input. 400.
    #[error("{0}")]
    OpenAlexInput(String),
    /// service.export_import.ProjectImportError — import bundle invalid. 422.
    #[error("{0}")]
    ProjectImport(String),

    // ── Generic catch-alls (one per HTTP class used by app.py) ─────────────
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    /// sqlite IntegrityError / managed-storage / author-has-papers. 409.
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Validation(String),
    /// Any upstream/source gateway failure. 502.
    #[error("{0}")]
    Upstream(String),
    #[error("{0}")]
    Internal(String),
}

impl CoreError {
    /// HTTP-equivalent status, so commands preserve the FastAPI contract.
    pub fn http_status(&self) -> u16 {
        use CoreError::*;
        match self {
            ProjectNotFound(_) | PaperNotFound | ArxivNotFound(_) | OpenAlexNotFound(_)
            | NotFound(_) => 404,
            ProjectDeleted(_) | PaperLink(_) | OpenAlexInput(_) | BadRequest(_) => 400,
            Conflict(_) => 409,
            PdfTooLarge(_) => 413,
            PdfImport(_) | ProjectImport(_) | Validation(_) => 422,
            OpenAlexHttp(_) | Upstream(_) => 502,
            Internal(_) => 500,
        }
    }
}

/// rusqlite failures surface as Internal (500) — same as Python's bare sqlite3
/// errors bubbling to the FastAPI 500 handler. The few cases the API maps to 409
/// (IntegrityError) are raised explicitly as CoreError::Conflict at the call site.
impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Internal(e.to_string())
    }
}

/// IO/JSON failures likewise surface as Internal (500), matching the Python
/// layer's unhandled OSError/JSONDecodeError bubbling to the 500 handler.
impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::Internal(e.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::Internal(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_matches_contract() {
        assert_eq!(CoreError::ProjectNotFound(1).http_status(), 404);
        assert_eq!(CoreError::ProjectDeleted("gone".into()).http_status(), 400);
        assert_eq!(CoreError::PdfImport("x".into()).http_status(), 422);
        assert_eq!(CoreError::PaperLink("x".into()).http_status(), 400);
        assert_eq!(CoreError::PdfTooLarge("big".into()).http_status(), 413);
        assert_eq!(CoreError::OpenAlexHttp("502".into()).http_status(), 502);
        assert_eq!(CoreError::Conflict("dup".into()).http_status(), 409);
        assert_eq!(CoreError::Internal("boom".into()).http_status(), 500);
    }
}
