//! provider — the `PaperSource` seam. One interface over the three metadata
//! Providers (CONTEXT.md § Provider), replacing `sources/fetch.rs`'s
//! union-parameter match.
//!
//! Each adapter carries its own configuration — arXiv's `data_dir` (the
//! `.arxiv_ratelimit` pacing file `sources::http` keeps there), OpenAlex's and
//! CrossRef's polite-pool `mailto` — so a caller no longer passes every Provider's
//! parameters regardless of which one it picked. Construction stays DI: nothing
//! here reads `config`, that happens once at the service seam
//! (`service::source`).
//!
//! Outside this trait on purpose: `arxiv::fetch_by_ids` (batched, arXiv-only),
//! `doi_resolve::resolve_doi` and `pdf_metadata::resolve_pdf_metadata` — those
//! are different operations, not different Providers.

use std::future::Future;
use std::path::PathBuf;

use crate::error::{CoreError, Result};
use crate::models::{strip_provider_prefix, PaperMetadata, DOI_ID_PREFIX};
use crate::sources::{arxiv, crossref, openalex};

/// One external metadata Provider.
///
/// Contract:
/// - `fetch_by_id` accepts a namespaced (`arxiv:…`, `openalex:…`, `doi:…`) or a
///   bare id, and reports a miss as `Err` — no `Option` at this seam.
/// - `search`'s `sort` is `relevance` / `newest` / `oldest` plus the Provider's
///   own extras (`lastUpdated` on arXiv, `citations` on OpenAlex and CrossRef).
///   An unsupported key is an error; no adapter may silently ignore it.
/// - Provider configuration is held by the adapter, never passed per call.
pub trait PaperSource {
    /// The `PAPER_META.PROVIDER` value records from this adapter carry.
    fn name(&self) -> &'static str;

    /// Full metadata for one paper, normalized to `PaperMetadata`.
    fn fetch_by_id(&self, paper_id: &str) -> impl Future<Output = Result<PaperMetadata>> + Send;

    /// Keyword search, newest/most-relevant first per `sort`.
    fn search(
        &self,
        query: &str,
        max_results: u32,
        sort: &str,
    ) -> impl Future<Output = Result<Vec<PaperMetadata>>> + Send;
}

/// arXiv. Holds `data_dir`: drop it and the 7s request spacing goes with it.
pub struct Arxiv {
    data_dir: PathBuf,
}

impl Arxiv {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }
}

impl PaperSource for Arxiv {
    fn name(&self) -> &'static str {
        "arxiv"
    }

    async fn fetch_by_id(&self, paper_id: &str) -> Result<PaperMetadata> {
        arxiv::fetch_by_id(paper_id, &self.data_dir).await
    }

    async fn search(
        &self,
        query: &str,
        max_results: u32,
        sort: &str,
    ) -> Result<Vec<PaperMetadata>> {
        arxiv::search(query, max_results, sort, &self.data_dir).await
    }
}

/// OpenAlex. Holds the polite-pool address; empty means the anonymous pool.
pub struct OpenAlex {
    mailto: String,
}

impl OpenAlex {
    pub fn new(mailto: impl Into<String>) -> Self {
        Self {
            mailto: mailto.into(),
        }
    }
}

impl PaperSource for OpenAlex {
    fn name(&self) -> &'static str {
        "openalex"
    }

    async fn fetch_by_id(&self, paper_id: &str) -> Result<PaperMetadata> {
        openalex::fetch_by_id(paper_id, &self.mailto).await
    }

    async fn search(
        &self,
        query: &str,
        max_results: u32,
        sort: &str,
    ) -> Result<Vec<PaperMetadata>> {
        openalex::search(query, max_results, sort, &self.mailto).await
    }
}

/// CrossRef. Holds the polite-pool address; empty means the anonymous pool.
pub struct CrossRef {
    mailto: String,
}

impl CrossRef {
    pub fn new(mailto: impl Into<String>) -> Self {
        Self {
            mailto: mailto.into(),
        }
    }
}

impl PaperSource for CrossRef {
    fn name(&self) -> &'static str {
        "crossref"
    }

    /// CrossRef ids are DOIs under the `doi:` namespace. The underlying lookup
    /// separates "no such work" from a transport failure; both become `Err` here,
    /// the not-found one keeping the message `fetch.rs` used to build inline.
    async fn fetch_by_id(&self, paper_id: &str) -> Result<PaperMetadata> {
        let doi = strip_provider_prefix(paper_id, DOI_ID_PREFIX);
        crossref::fetch_by_doi_checked(doi, &self.mailto)
            .await?
            .ok_or_else(|| {
                CoreError::Validation(format!("CrossRef: no record found for DOI '{paper_id}'"))
            })
    }

    async fn search(
        &self,
        query: &str,
        max_results: u32,
        sort: &str,
    ) -> Result<Vec<PaperMetadata>> {
        crossref::search_by_title_checked(query, max_results, sort, &self.mailto).await
    }
}
