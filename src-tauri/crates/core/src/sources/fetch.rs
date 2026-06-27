//! fetch — source dispatcher. Port of `linxiv_mcp.py::{_resolve_source, fetch_paper,
//! search_papers}`: route a `source` string to the right client's `fetch_by_id` / `search`.
//!
//! This is pure routing glue — every wire parser lives in its per-source module
//! (`arxiv`/`openalex`/`crossref`), already fixture-tested there. The only logic that lives
//! here is source-name resolution (unknown → error, matching Python's `_resolve_source`
//! `ValueError`) and crossref's `doi:` namespace strip. Plan §5.4.
//!
//! DI: `data_dir` feeds arxiv's `.arxiv_ratelimit` file; `mailto` selects OpenAlex's polite
//! pool. Both are params, never read from config here. The async wrappers reach the network
//! through the per-source modules (themselves on the `sources::http` seam), so the success
//! paths are integration-tested with the source modules; the routing/error logic is unit-tested
//! below without touching the network.

use std::path::Path;

use crate::error::{CoreError, Result};
use crate::models::PaperMetadata;
use crate::sources::{arxiv, crossref, openalex};

/// `_resolve_source`'s unknown-source `ValueError`. Plain string → single-quoted, matching
/// Python's `{source!r}`.
fn unknown_source(source: &str) -> CoreError {
    CoreError::Validation(format!(
        "Unknown source '{source}'. Use 'arxiv', 'crossref', or 'openalex'."
    ))
}

/// CrossRef ids carry a `doi:` namespace; strip it before the DOI lookup
/// (port of `CrossRefSource.fetch_by_id`'s `source_id.removeprefix("doi:")`).
fn strip_doi_prefix(source_id: &str) -> &str {
    source_id.strip_prefix("doi:").unwrap_or(source_id)
}

/// Fetch full metadata for one paper by id from `source`, normalized to `PaperMetadata`.
/// Port of `fetch_paper` → `_resolve_source(source)().fetch_by_id(paper_id)`.
pub async fn fetch_by_id(
    source: &str,
    paper_id: &str,
    data_dir: &Path,
    mailto: &str,
) -> Result<PaperMetadata> {
    match source {
        "arxiv" => arxiv::fetch_by_id(paper_id, data_dir).await,
        "openalex" => openalex::fetch_by_id(paper_id, mailto).await,
        // CrossRef returns Option (None on any non-200/parse error); the Python
        // `fetch_by_id` raises ValueError on None — mirror that with Validation.
        "crossref" => crossref::fetch_by_doi(strip_doi_prefix(paper_id))
            .await
            .ok_or_else(|| {
                CoreError::Validation(format!("CrossRef: no record found for DOI '{paper_id}'"))
            }),
        other => Err(unknown_source(other)),
    }
}

/// Search a `source` for papers by keyword. Port of `search_papers` →
/// `_resolve_source(source)().search(query, max_results=...)`.
pub async fn search(
    source: &str,
    query: &str,
    max_results: u32,
    sort: &str,
    data_dir: &Path,
    mailto: &str,
) -> Result<Vec<PaperMetadata>> {
    match source {
        "arxiv" => arxiv::search(query, max_results, sort, data_dir).await,
        "openalex" => openalex::search(query, max_results, sort, mailto).await,
        // CrossRef title search ignores `sort` (relevance order only) and never errors.
        "crossref" => Ok(crossref::search_by_title(query, max_results).await),
        other => Err(unknown_source(other)),
    }
}

// ---------------------------------------------------------------------------
// Tests — routing/error logic only. No network: the unknown-source arm and the
// doi-prefix strip are reached before any client call, and the dispatch tests
// pass a bogus source so the match short-circuits to the error arm.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_doi_prefix_removes_namespace_once() {
        assert_eq!(strip_doi_prefix("doi:10.1000/xyz"), "10.1000/xyz");
        // No prefix → unchanged.
        assert_eq!(strip_doi_prefix("10.1000/xyz"), "10.1000/xyz");
        // Only a leading `doi:` is stripped (removeprefix semantics).
        assert_eq!(strip_doi_prefix("doi:doi:1"), "doi:1");
    }

    #[test]
    fn unknown_source_message_matches_python() {
        let e = unknown_source("bogus");
        assert!(matches!(e, CoreError::Validation(_)));
        assert_eq!(
            e.to_string(),
            "Unknown source 'bogus'. Use 'arxiv', 'crossref', or 'openalex'."
        );
    }

    #[tokio::test]
    async fn fetch_by_id_rejects_unknown_source_without_network() {
        let dir = std::path::Path::new("/nonexistent");
        let err = fetch_by_id("scopus", "123", dir, "").await.unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
        assert!(err.to_string().contains("Unknown source 'scopus'"));
    }

    #[tokio::test]
    async fn search_rejects_unknown_source_without_network() {
        let dir = std::path::Path::new("/nonexistent");
        let err = search("scopus", "q", 5, "relevance", dir, "")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
    }
}
