//! Core error model — Rust port of the typed exceptions raised across
//! `service/` and `sources/`, plus the HTTP status codes `api/app.py` maps
//! them to. Variants preserve the boundary contract so Tauri commands can
//! return the same `{"error": "<msg>"}` body + status the FastAPI layer did.
//! Plan §5 + D21.

use serde::ser::{Serialize, SerializeStruct, Serializer};

#[derive(thiserror::Error, Debug)]
pub enum CoreError {
    // ── Typed failure modes (named so callers can branch + word them) ──────
    /// service.project.ProjectNotFoundError — membership guard. 404.
    #[error("Project not found")]
    ProjectNotFound,
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
            ProjectNotFound | PaperNotFound | ArxivNotFound(_) | OpenAlexNotFound(_)
            | NotFound(_) => 404,
            ProjectDeleted(_) | PaperLink(_) | OpenAlexInput(_) | BadRequest(_) => 400,
            Conflict(_) => 409,
            PdfTooLarge(_) => 413,
            PdfImport(_) | ProjectImport(_) | Validation(_) => 422,
            OpenAlexHttp(_) | Upstream(_) => 502,
            Internal(_) => 500,
        }
    }

    /// CLI exit code. 1 for every failure today.
    pub fn exit_code(&self) -> i32 {
        1
    }

    /// `{"error": "<msg>"}` JSON body, matching the FastAPI error shape.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| format!("{{\"error\":{:?}}}", self.to_string()))
    }
}

impl Serialize for CoreError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut st = serializer.serialize_struct("CoreError", 1)?;
        st.serialize_field("error", &self.to_string())?;
        st.end()
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

pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_json_match_contract() {
        assert_eq!(CoreError::ProjectNotFound.http_status(), 404);
        assert_eq!(CoreError::ProjectDeleted("gone".into()).http_status(), 400);
        assert_eq!(CoreError::PdfImport("x".into()).http_status(), 422);
        assert_eq!(CoreError::PaperLink("x".into()).http_status(), 400);
        assert_eq!(CoreError::PdfTooLarge("big".into()).http_status(), 413);
        assert_eq!(CoreError::OpenAlexHttp("502".into()).http_status(), 502);
        assert_eq!(CoreError::Conflict("dup".into()).http_status(), 409);
        assert_eq!(CoreError::Internal("boom".into()).http_status(), 500);
        assert_eq!(CoreError::ProjectNotFound.exit_code(), 1);
        assert_eq!(CoreError::ProjectNotFound.to_json(), r#"{"error":"Project not found"}"#);
    }
}
