//! source — the Provider front door for consumers (ADR-0010). Route/CLI/MCP name
//! a Provider; this dispatches to the Provider's module and hands it its own
//! configuration, so no consumer passes `data_dir` or `mailto` (or reaches into
//! `sources::` at all).
//!
//! The config read lives here rather than in `sources::`, which stays pure DI.
//! `config::{openalex,crossref}_mailto()` prefer the env var and fall back to
//! user settings, so the CLI and MCP processes keep the polite pools.
//!
//! Every call here is network I/O: `.await` it with no DB lock held, then commit
//! through `service::paper::save_paper_metadata`.

use crate::config;
use crate::error::{CoreError, Result};
use crate::models::{strip_provider_prefix, PaperMetadata, DOI_ID_PREFIX};
use crate::sources::{arxiv, crossref, doi_resolve, openalex};

/// `_resolve_source`'s unknown-source `ValueError`. Plain string → single-quoted,
/// matching Python's `{source!r}`.
fn unknown_source(source: &str) -> CoreError {
    CoreError::Validation(format!(
        "Unknown source '{source}'. Use 'arxiv', 'crossref', or 'openalex'."
    ))
}

/// Fetch full metadata for one paper by id from `source`. Ids may be namespaced
/// (`arxiv:…`, `openalex:…`, `doi:…`) or bare; a miss is an `Err`, never `None`.
pub async fn fetch_by_id(source: &str, paper_id: &str) -> Result<PaperMetadata> {
    match source {
        "arxiv" => arxiv::fetch_by_id(paper_id, &config::data_dir()).await,
        "openalex" => openalex::fetch_by_id(paper_id, &config::openalex_mailto()).await,
        // CrossRef ids are DOIs under the `doi:` namespace. The underlying lookup
        // separates "no such work" from a transport failure; both become `Err`.
        "crossref" => {
            let doi = strip_provider_prefix(paper_id, DOI_ID_PREFIX);
            crossref::fetch_by_doi_checked(doi, &config::crossref_mailto())
                .await?
                .ok_or_else(|| {
                    CoreError::Validation(format!("CrossRef: no record found for DOI '{paper_id}'"))
                })
        }
        other => Err(unknown_source(other)),
    }
}

/// Search `source` by keyword. `sort` is validated by the Provider module, which
/// refuses a key it cannot honour (no silent drops).
pub async fn search(
    source: &str,
    query: &str,
    max_results: u32,
    sort: &str,
) -> Result<Vec<PaperMetadata>> {
    match source {
        "arxiv" => arxiv::search(query, max_results, sort, &config::data_dir()).await,
        "openalex" => openalex::search(query, max_results, sort, &config::openalex_mailto()).await,
        "crossref" => {
            crossref::search_by_title_checked(query, max_results, sort, &config::crossref_mailto())
                .await
        }
        other => Err(unknown_source(other)),
    }
}

/// DOI → metadata via the three-strategy ladder. Outside the per-Provider
/// dispatch on purpose: one operation spanning several Providers, not a
/// Provider of its own.
pub async fn resolve_doi(doi: &str) -> Result<PaperMetadata> {
    doi_resolve::resolve_doi(doi, &config::data_dir(), &config::crossref_mailto()).await
}

/// Every record CrossRef and OpenAlex hold for one DOI, plus whether either
/// lookup *failed* (as opposed to cleanly finding nothing) — the per-DOI step of
/// an ORCID backfill pass. Order is CrossRef first: `service::orcid_backfill`
/// takes the first ORCID it finds.
pub async fn orcid_records_for_doi(doi: &str) -> (Vec<PaperMetadata>, bool) {
    fold_doi_results(
        crossref::fetch_by_doi_checked(doi, &config::crossref_mailto()).await,
        openalex::fetch_by_doi(doi, &config::openalex_mailto()).await,
    )
}

/// Combine one DOI's two source results into records-to-try + whether either
/// source failed (vs. a clean not-found).
fn fold_doi_results(
    crossref_result: Result<Option<PaperMetadata>>,
    openalex_result: Result<PaperMetadata>,
) -> (Vec<PaperMetadata>, bool) {
    let mut records = Vec::new();
    let mut errored = false;
    match crossref_result {
        Ok(Some(m)) => records.push(m),
        Ok(None) => {}
        Err(_) => errored = true,
    }
    match openalex_result {
        Ok(m) => records.push(m),
        Err(CoreError::OpenAlexNotFound(_)) => {}
        Err(_) => errored = true,
    }
    (records, errored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(source_id: &str) -> PaperMetadata {
        serde_json::from_value(serde_json::json!({
            "source_id": source_id,
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-01",
            "summary": "S",
        }))
        .unwrap()
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

    #[test]
    fn fold_both_match_keeps_crossref_then_openalex_order() {
        let (records, errored) = fold_doi_results(Ok(Some(meta("a"))), Ok(meta("b")));
        assert_eq!(
            records
                .iter()
                .map(|m| m.source_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(!errored);
    }

    #[test]
    fn fold_not_found_on_both_is_not_errored() {
        let (records, errored) =
            fold_doi_results(Ok(None), Err(CoreError::OpenAlexNotFound("x".into())));
        assert!(records.is_empty());
        assert!(!errored);
    }

    #[test]
    fn fold_crossref_failure_counts_errored_but_keeps_openalex_match() {
        let (records, errored) =
            fold_doi_results(Err(CoreError::Upstream("x".into())), Ok(meta("b")));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source_id, "b");
        assert!(errored);
    }

    #[test]
    fn fold_both_failing_is_errored_once_with_no_records() {
        let (records, errored) = fold_doi_results(
            Err(CoreError::Upstream("x".into())),
            Err(CoreError::OpenAlexHttp("y".into())),
        );
        assert!(records.is_empty());
        assert!(errored);
    }
}
