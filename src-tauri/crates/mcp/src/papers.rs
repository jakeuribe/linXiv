//! Paper tools cluster. Owned by the `papers` Fill agent — do not edit other
//! cluster files. Each `#[tool]` method below has a `todo!()` body to replace.
//!
//! Every body reaches the DB via `self.with_conn(|conn| ...)` and calls into
//! `linxiv_core::service::paper` (and `::tag` for full-text/categories as the
//! Python port does). Return `Ok(Json(value))` on success; on the error paths
//! the Python code raises `ValueError`, so map those to
//! `Err(ErrorData::invalid_params(msg, None))` with the EXACT message string
//! (mind Python `{x!r}` quoting, e.g. `format!("Paper {paper_id:?} not found in database.")`).

use chrono::NaiveDate;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use linxiv_core::error::CoreError;
use linxiv_core::models::PaperMetadata;
use linxiv_core::service::paper as svc_paper;
use linxiv_core::sources::fetch as svc_fetch;
use linxiv_core::storage::queries::paper as store_paper;
use linxiv_core::storage::queries::search::search_full_text;
use linxiv_core::{config, service::project as svc_project};

use crate::Server;

/// OpenAlex polite-pool address (`OPENALEX_MAILTO`), mirroring the CLI/Python
/// source clients. CR/LF are stripped downstream in `openalex::user_agent`.
fn mailto() -> String {
    std::env::var("OPENALEX_MAILTO").unwrap_or_default()
}

/// Serialize a core value to the tool's text result (compact JSON string).
fn json_ok<T: serde::Serialize>(v: &T) -> Result<String, ErrorData> {
    serde_json::to_string(v).map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

/// Map an unexpected core error (DB/FS) to an MCP internal error.
fn core_err(e: CoreError) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

/// `Paper(source_id=paper_id)` lookup key, as the Python tools build it.
fn paper_key(paper_id: &str) -> svc_paper::Paper {
    svc_paper::Paper {
        source_id: Some(paper_id.to_string()),
        ..Default::default()
    }
}

fn default_source() -> String {
    "arxiv".to_string()
}
fn default_max_results() -> i64 {
    10
}
fn default_full_text_limit() -> i64 {
    20
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchPapersParams {
    /// Search query string (e.g. "transformer attention mechanism").
    pub query: String,
    /// Data source — "arxiv", "crossref", or "openalex".
    #[serde(default = "default_source")]
    pub source: String,
    /// Maximum number of results to return (default 10).
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FetchPaperParams {
    /// arXiv-style id, CrossRef DOI, or OpenAlex id.
    pub paper_id: String,
    /// Data source — "arxiv", "crossref", or "openalex".
    #[serde(default = "default_source")]
    pub source: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPapersParams {
    /// Maximum number of papers to return (default: all).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Number of papers to skip for pagination.
    #[serde(default)]
    pub offset: i64,
    /// Filter by arXiv primary category (e.g. "cs.LG").
    #[serde(default)]
    pub category: Option<String>,
}

/// Tools that take only a paper source id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaperIdParams {
    /// The paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchFullTextParams {
    /// SQLite FTS5 query string.
    pub query: String,
    /// Maximum number of results (default 20).
    #[serde(default = "default_full_text_limit")]
    pub limit: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RepairPaperParams {
    /// The paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
    /// Corrected title.
    pub title: String,
    /// Corrected list of author names.
    pub authors: Vec<String>,
    /// Publication date in YYYY-MM-DD format.
    pub published: String,
    /// Abstract / summary text.
    #[serde(default)]
    pub summary: String,
    /// Primary category (e.g. "cs.LG").
    #[serde(default)]
    pub category: Option<String>,
    /// DOI string.
    #[serde(default)]
    pub doi: Option<String>,
    /// Canonical URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Replacement tag list.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[tool_router(router = tools_papers, vis = "pub(crate)")]
impl Server {
    #[tool(description = "Search for academic papers by keyword.")]
    pub async fn search_papers(
        &self,
        Parameters(SearchPapersParams {
            query,
            source,
            max_results,
        }): Parameters<SearchPapersParams>,
    ) -> Result<String, ErrorData> {
        // Python `source.search(query, max_results)` defaults sort="relevance".
        let results = svc_fetch::search(
            &source,
            &query,
            max_results as u32,
            "relevance",
            &config::data_dir(),
            &mailto(),
        )
        .await
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        json_ok(&results)
    }

    #[tool(description = "Fetch full metadata for a paper by id and save it to the local database.")]
    pub async fn fetch_paper(
        &self,
        Parameters(FetchPaperParams { paper_id, source }): Parameters<FetchPaperParams>,
    ) -> Result<String, ErrorData> {
        let meta = svc_fetch::fetch_by_id(&source, &paper_id, &config::data_dir(), &mailto())
            .await
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        self.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))
            .map_err(core_err)?;
        json_ok(&meta)
    }

    #[tool(description = "List papers stored in the local database.")]
    pub async fn list_papers(
        &self,
        Parameters(ListPapersParams {
            limit,
            offset,
            category,
        }): Parameters<ListPapersParams>,
    ) -> Result<String, ErrorData> {
        // `list_paper_details` defaults latest_only=True.
        let papers = self
            .with_conn(|conn| svc_paper::list_papers(conn, true, limit, offset, category.as_deref()))
            .map_err(core_err)?;
        json_ok(&papers)
    }

    #[tool(description = "Get full metadata for a single paper from the local database.")]
    pub async fn get_paper(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        let paper = self
            .with_conn(|conn| svc_paper::get(conn, &paper_key(&paper_id)))
            .map_err(core_err)?;
        match paper {
            Some(p) => json_ok(&p),
            None => json_ok(&Value::Null),
        }
    }

    #[tool(description = "Soft-delete a paper from the local database (moves it to trash).")]
    pub async fn delete_paper(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            if svc_paper::get(conn, &paper_key(&paper_id))?.is_none() {
                return Ok(Err(ErrorData::invalid_params(
                    format!("Paper {} not found in database.", crate::util::pyrepr(&paper_id)),
                    None,
                )));
            }
            svc_paper::delete(conn, &paper_key(&paper_id))?;
            Ok(Ok(()))
        })
        .map_err(core_err)??;
        json_ok(&serde_json::json!({ "deleted": paper_id }))
    }

    #[tool(description = "Get all stored versions of a paper.")]
    pub async fn get_paper_versions(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        let all_ver = self
            .with_conn(|conn| svc_paper::get_all(conn, &paper_key(&paper_id)))
            .map_err(core_err)?;
        match all_ver {
            Some(v) => json_ok(&v),
            None => json_ok(&Value::Null),
        }
    }

    #[tool(description = "Full-text search over downloaded TeX source content.")]
    pub async fn search_full_text(
        &self,
        Parameters(SearchFullTextParams { query, limit }): Parameters<SearchFullTextParams>,
    ) -> Result<String, ErrorData> {
        // Python swallows FTS errors and returns []. It logs the error to STDOUT —
        // a bug that corrupts JSON-RPC here; route the diagnostic to STDERR instead.
        match self.with_conn(|conn| search_full_text(conn, &query, limit)) {
            Ok(results) => json_ok(&results),
            Err(exc) => {
                tracing::warn!(%query, error = %exc, "search_full_text failed");
                json_ok(&Value::Array(Vec::new()))
            }
        }
    }

    #[tool(description = "Overwrite a paper's metadata in-place to fix a bad import.")]
    pub async fn repair_paper(
        &self,
        Parameters(RepairPaperParams {
            paper_id,
            title,
            authors,
            published,
            summary,
            category,
            doi,
            url,
            tags,
        }): Parameters<RepairPaperParams>,
    ) -> Result<String, ErrorData> {
        // Keyed by the stable paper root so the fix survives a source_id rename.
        self.with_conn(|conn| {
            let Some(root) = store_paper::get_paper_root(conn, &paper_id)? else {
                return Ok(Err(ErrorData::invalid_params(
                    format!("Paper {} not found in database.", crate::util::pyrepr(&paper_id)),
                    None,
                )));
            };
            // Date validated after the existence check, matching Python ordering.
            let published_date = match NaiveDate::parse_from_str(&published, "%Y-%m-%d") {
                Ok(d) => d,
                Err(_) => {
                    return Ok(Err(ErrorData::invalid_params(
                        format!("Invalid date {}; use YYYY-MM-DD.", crate::util::pyrepr(&published)),
                        None,
                    )))
                }
            };
            // Python `existing.version if existing else 1`.
            let version = svc_paper::get(conn, &paper_key(&paper_id))?
                .map(|p| p.version)
                .unwrap_or(1);
            let meta = PaperMetadata {
                source_id: paper_id.clone(),
                version,
                title: title.clone(),
                authors: authors.clone(),
                published: published_date,
                updated: None,
                summary: summary.clone(),
                category: category.clone(),
                categories: None,
                doi: doi.clone(),
                journal_ref: None,
                comment: None,
                url: url.clone(),
                // Python `tags or None`: an empty list becomes None.
                tags: tags.clone().filter(|t| !t.is_empty()),
                source: None,
            };
            svc_paper::repair_paper(conn, root.source_fk, &meta)?;
            Ok(Ok(()))
        })
        .map_err(core_err)??;
        json_ok(&serde_json::json!({ "repaired": paper_id }))
    }

    #[tool(description = "Restore a soft-deleted (trashed) paper back into the library.")]
    pub async fn restore_paper(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        let (pdf_path, project_fks) = self
            .with_conn(|conn| {
                if !svc_paper::is_paper_deleted(conn, &paper_id)? {
                    return Ok(Err(ErrorData::invalid_params(
                        format!("Paper {} not found in trash.", crate::util::pyrepr(&paper_id)),
                        None,
                    )));
                }
                Ok(Ok(svc_paper::restore(conn, &paper_key(&paper_id))?))
            })
            .map_err(core_err)??;
        json_ok(&serde_json::json!({
            "restored": paper_id,
            "pdf_path": pdf_path,
            "project_fks": project_fks,
        }))
    }

    #[tool(description = "Permanently delete a paper and all its data. Irreversible.")]
    pub async fn hard_delete_paper(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            if store_paper::get_paper_root(conn, &paper_id)?.is_none() {
                return Ok(Err(ErrorData::invalid_params(
                    format!("Paper {} not found.", crate::util::pyrepr(&paper_id)),
                    None,
                )));
            }
            svc_paper::hard_delete(conn, &paper_key(&paper_id))?;
            Ok(Ok(()))
        })
        .map_err(core_err)??;
        json_ok(&serde_json::json!({ "hard_deleted": paper_id }))
    }

    #[tool(description = "Remove a paper from every project it currently belongs to.")]
    pub async fn remove_paper_from_all_projects(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        let removed = self
            .with_conn(|conn| svc_project::remove_paper_from_all_projects_by_id(conn, &paper_id))
            .map_err(core_err)?
            .ok_or_else(|| {
                ErrorData::invalid_params(format!("Paper {} not found.", crate::util::pyrepr(&paper_id)), None)
            })?;
        json_ok(&serde_json::json!({
            "paper_id": paper_id,
            "removed_from_projects": removed,
        }))
    }
}
