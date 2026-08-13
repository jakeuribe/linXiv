//! Paper tools cluster. Owned by the `papers` Fill agent — do not edit other
//! cluster files.
//!
//! Every body reaches the DB via `self.with_conn(|conn| ...)` and calls into
//! `linxiv_core::service::paper` (and `::tag` for full-text/categories as the
//! Python port does). Return `Ok(Json(value))` on success; on the error paths
//! the Python code raises `ValueError`, so map those to
//! `Err(ErrorData::invalid_params(msg, None))` with the EXACT message string
//! Misses word themselves via `CoreError::PaperNotFound`'s Display — no
//! per-surface message building.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use linxiv_core::error::CoreError;
use linxiv_core::models::{PaperMetadata, SearchResultOut};
use linxiv_core::service::paper::{self as svc_paper, PaperSort};
use linxiv_core::sources::fetch as svc_fetch;
use linxiv_core::{config, service::project as svc_project};

use crate::Server;

use linxiv_core::config::openalex_mailto as mailto;

use crate::util::{core_err, invalid, json_ok};

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
    /// Sort metric: publication date (default), when the paper was added
    /// locally, or title.
    #[serde(default)]
    pub sort: Option<SortKey>,
    /// Descending order. Defaults per metric: newest first for dates, A–Z for
    /// titles.
    #[serde(default)]
    pub desc: Option<bool>,
}

/// The `PaperSort` metrics, as a schema enum so the tool advertises the valid
/// values instead of silently coercing a typo to the default.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    Published,
    Added,
    Title,
}

impl From<SortKey> for PaperSort {
    fn from(k: SortKey) -> Self {
        match k {
            SortKey::Published => PaperSort::Published,
            SortKey::Added => PaperSort::Added,
            SortKey::Title => PaperSort::Title,
        }
    }
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
pub struct FetchFullTextParams {
    /// The paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
    /// Re-fetch even when the source was already indexed.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FullTextPendingParams {
    /// Maximum number of candidate ids to return (default: all).
    #[serde(default)]
    pub limit: Option<i64>,
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
        // The canonical search wire shape all three surfaces emit (ADR-0011).
        let results: Vec<SearchResultOut> =
            results.into_iter().map(SearchResultOut::from).collect();
        json_ok(&results)
    }

    #[tool(
        description = "Fetch full metadata for a paper by id and save it to the local database."
    )]
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

    #[tool(
        description = "List papers stored in the local database, optionally sorted by publication \
                       date, date added, or title."
    )]
    pub async fn list_papers(
        &self,
        Parameters(ListPapersParams {
            limit,
            offset,
            category,
            sort,
            desc,
        }): Parameters<ListPapersParams>,
    ) -> Result<String, ErrorData> {
        let sort: PaperSort = sort.map(Into::into).unwrap_or_default();
        let desc = desc.unwrap_or_else(|| sort.default_desc());
        // `list_paper_details` defaults latest_only=True.
        let papers = self
            .with_conn(|conn| {
                svc_paper::list_papers_sorted(
                    conn,
                    true,
                    limit,
                    offset,
                    category.as_deref(),
                    sort,
                    desc,
                )
            })
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
        json_ok(&paper)
    }

    #[tool(description = "Soft-delete a paper from the local database (moves it to trash).")]
    pub async fn delete_paper(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            if svc_paper::get(conn, &paper_key(&paper_id))?.is_none() {
                return Ok(Err(crate::util::guard_err(CoreError::PaperNotFound(
                    paper_id.clone(),
                ))));
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
        json_ok(&all_ver)
    }

    #[tool(description = "Full-text search over downloaded TeX source and note content.")]
    pub async fn search_full_text(
        &self,
        Parameters(SearchFullTextParams { query, limit }): Parameters<SearchFullTextParams>,
    ) -> Result<String, ErrorData> {
        // Python swallows FTS errors and returns []. It logs the error to STDOUT —
        // a bug that corrupts JSON-RPC here; route the diagnostic to STDERR instead.
        match self.with_conn(|conn| svc_paper::search_library(conn, &query, limit)) {
            Ok(results) => json_ok(&results),
            Err(exc) => {
                tracing::warn!(%query, error = %exc, "search_full_text failed");
                json_ok(&Value::Array(Vec::new()))
            }
        }
    }

    #[tool(
        description = "Download a paper's arXiv TeX source and index it so search_full_text can \
                       find it. arXiv only; an already-indexed paper is skipped unless force=true."
    )]
    pub async fn fetch_full_text(
        &self,
        Parameters(FetchFullTextParams { paper_id, force }): Parameters<FetchFullTextParams>,
    ) -> Result<String, ErrorData> {
        // Resolve under the lock and drop it before the fetch (same pattern as download_pdf).
        let paper = self
            .with_conn(|conn| svc_paper::get(conn, &paper_key(&paper_id)))
            .map_err(core_err)?
            .ok_or_else(|| crate::util::guard_err(CoreError::PaperNotFound(paper_id.clone())))?;
        if paper.downloaded_source && !force {
            return json_ok(&svc_paper::FullTextReceipt::already_indexed(&paper));
        }
        // Mirrors io_authors_misc.rs's map_core: BadRequest/Validation/PaperNotFound
        // are user-facing refusals, not server faults.
        let map_fetch_err = |e: CoreError| match e {
            CoreError::BadRequest(m) | CoreError::Validation(m) => invalid(m),
            e @ CoreError::PaperNotFound(_) => invalid(e.to_string()),
            other => core_err(other),
        };
        // Two-phase ingest from core: fetch outside the lock, commit under it.
        let fetched = svc_paper::fetch_full_text(&paper, &config::data_dir())
            .await
            .map_err(map_fetch_err)?;
        let receipt = self
            .with_conn(|conn| fetched.commit(conn))
            .map_err(map_fetch_err)?;
        json_ok(&receipt)
    }

    #[tool(
        description = "List stored arXiv papers whose TeX source has not been indexed yet — the \
                       backlog fetch_full_text still has to work through. Read-only."
    )]
    pub async fn full_text_pending(
        &self,
        Parameters(FullTextPendingParams { limit }): Parameters<FullTextPendingParams>,
    ) -> Result<String, ErrorData> {
        let (pending, mut candidates) = self
            .with_conn(|conn| {
                Ok::<_, CoreError>((
                    svc_paper::full_text_backfill_count(conn)?,
                    svc_paper::full_text_backfill_candidates(conn)?,
                ))
            })
            .map_err(core_err)?;
        // `pending` stays the whole backlog; `limit` only trims the returned ids.
        if let Some(n) = limit {
            candidates.truncate(n.max(0) as usize);
        }
        json_ok(&serde_json::json!({
            "pending": pending,
            "candidates": candidates,
        }))
    }

    #[tool(
        description = "Find other papers sharing this paper's DOI — likely the same work resolved \
                       from a different source."
    )]
    pub async fn find_doi_candidates(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        // Keyed by the stable paper root, as the sfk route is.
        let candidates = self.with_conn(|conn| {
            let source_fk =
                svc_paper::resolve_source_fk(conn, &paper_id).map_err(crate::util::guard_err)?;
            svc_paper::find_doi_version_candidates(conn, source_fk).map_err(core_err)
        })?;
        json_ok(&serde_json::json!({
            "paper_id": paper_id,
            "candidates": candidates,
        }))
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
        let updated = self
            .with_conn(|conn| {
                let source_fk = match svc_paper::resolve_source_fk(conn, &paper_id) {
                    Ok(fk) => fk,
                    Err(e) => return Ok(Err(crate::util::guard_err(e))),
                };
                // Date validated after the existence check, matching Python ordering.
                let published_date = match svc_paper::parse_published(&published) {
                    Ok(d) => d,
                    Err(e) => return Ok(Err(ErrorData::invalid_params(e.to_string(), None))),
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
                    author_orcids: None,
                };
                // Validation lives in the service so every front door refuses the same input.
                if let Err(e) = svc_paper::repair_paper(conn, source_fk, &meta) {
                    return match e {
                        CoreError::Validation(m) => Ok(Err(invalid(m))),
                        other => Err(other),
                    };
                }
                // Route parity: return the repaired paper's full `PaperDetails`.
                Ok(Ok(svc_paper::get(conn, &paper_key(&paper_id))?))
            })
            .map_err(core_err)??;
        let updated = updated.ok_or_else(|| ErrorData::internal_error("Repair failed", None))?;
        json_ok(&updated)
    }

    #[tool(description = "Restore a soft-deleted (trashed) paper back into the library.")]
    pub async fn restore_paper(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        let (pdf_path, project_fks) = self
            .with_conn(|conn| {
                if let Err(e) = svc_paper::require_trashed(conn, &paper_id) {
                    return Ok(Err(crate::util::guard_err(e)));
                }
                Ok(Ok(svc_paper::restore(conn, &paper_key(&paper_id))?))
            })
            .map_err(core_err)??;
        json_ok(&linxiv_core::service::trash::RestoredPaper {
            ok: true,
            restored: paper_id,
            pdf_path,
            project_fks,
        })
    }

    #[tool(description = "Permanently delete a paper and all its data. Irreversible.")]
    pub async fn hard_delete_paper(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            if let Err(e) = svc_paper::resolve_source_fk(conn, &paper_id) {
                return Ok(Err(crate::util::guard_err(e)));
            }
            svc_paper::hard_delete(conn, &paper_key(&paper_id))?;
            Ok(Ok(()))
        })
        .map_err(core_err)??;
        json_ok(&linxiv_core::service::trash::HardDeletedPaper {
            ok: true,
            hard_deleted: paper_id,
        })
    }

    #[tool(description = "Remove a paper from every project it currently belongs to.")]
    pub async fn remove_paper_from_all_projects(
        &self,
        Parameters(PaperIdParams { paper_id }): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        let removed = self
            .with_conn(|conn| svc_project::remove_paper_from_all_projects_by_id(conn, &paper_id))
            .map_err(core_err)?
            .ok_or_else(|| crate::util::guard_err(CoreError::PaperNotFound(paper_id.clone())))?;
        // One envelope across route/CLI/MCP; the caller already knows the id.
        json_ok(&serde_json::json!({
            "ok": true,
            "removed_from_projects": removed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use linxiv_core::storage;

    use super::*;

    /// Mirrors `route/papers.rs`'s `state()`: an in-memory DB, no router merge
    /// since these tests call tool methods directly rather than dispatching
    /// through `tool_router`.
    fn server() -> Server {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        Server {
            conn: Arc::new(Mutex::new(conn)),
            pdf_dir: std::env::temp_dir(),
            tool_router: Server::tools_papers(),
        }
    }

    fn meta(source_id: &str, source: Option<&str>, url: Option<&str>) -> PaperMetadata {
        let mut m: PaperMetadata = serde_json::from_value(serde_json::json!({
            "source_id": source_id,
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-01",
            "summary": "S",
        }))
        .unwrap();
        m.source = source.map(String::from);
        m.url = url.map(String::from);
        m
    }

    async fn fetch(srv: &Server, paper_id: &str, force: bool) -> Result<Value, ErrorData> {
        let out = srv
            .fetch_full_text(Parameters(FetchFullTextParams {
                paper_id: paper_id.to_string(),
                force,
            }))
            .await?;
        Ok(serde_json::from_str(&out).unwrap())
    }

    /// The guards that run before any network call: unknown paper, and a
    /// source with no TeX to fetch (non-arXiv, or arXiv with no `/pdf/` URL).
    #[tokio::test]
    async fn fetch_full_text_rejects_before_reaching_the_network() {
        let srv = server();
        let err = fetch(&srv, "arxiv:nope", false).await.unwrap_err();
        assert_eq!(err.message.as_ref(), "Paper arxiv:nope not found");

        srv.with_conn(|conn| {
            svc_paper::save_paper_metadata(conn, &meta("doi:10.1/z", Some("crossref"), None), None)
        })
        .unwrap();
        let err = fetch(&srv, "doi:10.1/z", false).await.unwrap_err();
        assert!(
            err.message.contains("is not an arXiv paper"),
            "unexpected message: {}",
            err.message
        );

        srv.with_conn(|conn| {
            svc_paper::save_paper_metadata(conn, &meta("arxiv:2", Some("arxiv"), None), None)
        })
        .unwrap();
        let err = fetch(&srv, "arxiv:2", false).await.unwrap_err();
        assert!(
            err.message.contains("no arXiv PDF URL"),
            "unexpected message: {}",
            err.message
        );
    }

    /// The backlog is every stored arXiv paper with a `/pdf/` url and no TeX
    /// yet; `limit` trims the returned ids without changing `pending`.
    #[tokio::test]
    async fn full_text_pending_reports_the_backlog() {
        let srv = server();
        for sid in ["arxiv:p1", "arxiv:p2"] {
            let url = format!("http://arxiv.org/pdf/{sid}v1");
            srv.with_conn(|conn| {
                svc_paper::save_paper_metadata(conn, &meta(sid, Some("arxiv"), Some(&url)), None)
            })
            .unwrap();
        }
        let all = srv
            .full_text_pending(Parameters(FullTextPendingParams { limit: None }))
            .await
            .unwrap();
        let all: Value = serde_json::from_str(&all).unwrap();
        assert_eq!(all["pending"], serde_json::json!(2));
        assert_eq!(all["candidates"].as_array().unwrap().len(), 2);

        let one = srv
            .full_text_pending(Parameters(FullTextPendingParams { limit: Some(1) }))
            .await
            .unwrap();
        let one: Value = serde_json::from_str(&one).unwrap();
        assert_eq!(one["pending"], serde_json::json!(2));
        assert_eq!(one["candidates"].as_array().unwrap().len(), 1);
    }

    /// An unknown id is refused with the service's typed not-found (no root
    /// conjured); a paper with no DOI twin lists no candidates.
    #[tokio::test]
    async fn doi_candidates_rejects_unknown_papers() {
        let srv = server();
        let err = srv
            .find_doi_candidates(Parameters(PaperIdParams {
                paper_id: "arxiv:nope".to_string(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.message.as_ref(), "Paper arxiv:nope not found");
        assert!(srv
            .with_conn(|conn| svc_paper::get_paper_root(conn, "arxiv:nope"))
            .unwrap()
            .is_none());

        srv.with_conn(|conn| {
            svc_paper::save_paper_metadata(conn, &meta("arxiv:9", Some("arxiv"), None), None)
        })
        .unwrap();
        let out = srv
            .find_doi_candidates(Parameters(PaperIdParams {
                paper_id: "arxiv:9".to_string(),
            }))
            .await
            .unwrap();
        let out: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(out["candidates"], serde_json::json!([]));
    }

    /// An already-indexed paper short-circuits without `force`; `force=true`
    /// re-opens it (and here falls through to the same unfetchable-URL guard).
    #[tokio::test]
    async fn fetch_full_text_skips_an_already_indexed_paper() {
        let srv = server();
        srv.with_conn(|conn| {
            svc_paper::save_paper_metadata(conn, &meta("arxiv:3", Some("arxiv"), None), None)
        })
        .unwrap();
        srv.with_conn(|conn| svc_paper::set_full_text(conn, "arxiv:3", 1, "already here"))
            .unwrap();

        let out = fetch(&srv, "arxiv:3", false).await.unwrap();
        assert_eq!(out["indexed"], serde_json::json!(false));
        // Route-parity envelope: keyed `source_id`, not the old `paper_id`.
        assert_eq!(out["source_id"], serde_json::json!("arxiv:3"));
        assert!(out.get("paper_id").is_none(), "stale paper_id key: {out}");

        let err = fetch(&srv, "arxiv:3", true).await.unwrap_err();
        assert!(err.message.contains("no arXiv PDF URL"));
    }

    /// `search_papers` emits `SearchResultOut` (ADR-0011) — pin the exact wire
    /// shape so this surface can't drift back to raw `PaperMetadata`.
    #[test]
    fn search_results_pin_the_canonical_wire_shape() {
        let mut m = meta("arxiv:2204.12985", Some("arxiv"), Some("http://x"));
        m.category = Some("cs.LG".into());
        let v = serde_json::to_value(SearchResultOut::from(m)).unwrap();
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"source_id":"2204.12985","version":1,"title":"T","summary":"S","authors":["A"],"published":"2024-01-01","paper_url":"http://x","primary_category":"cs.LG","entry_id":"arxiv:2204.12985"}"#
        );
    }

    /// `repair_paper` returns the repaired paper's full `PaperDetails` (route
    /// parity), not the old `{"repaired": id}` receipt — and never `full_text`.
    #[tokio::test]
    async fn repair_returns_the_updated_paper_details() {
        let srv = server();
        srv.with_conn(|conn| {
            svc_paper::save_paper_metadata(conn, &meta("arxiv:7", Some("arxiv"), None), None)
        })
        .unwrap();
        let out = srv
            .repair_paper(Parameters(RepairPaperParams {
                paper_id: "arxiv:7".to_string(),
                title: "Fixed".to_string(),
                authors: vec!["B".to_string()],
                published: "2024-02-02".to_string(),
                summary: "S2".to_string(),
                category: None,
                doi: None,
                url: None,
                tags: None,
            }))
            .await
            .unwrap();
        let out: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(out["source_id"], serde_json::json!("arxiv:7"));
        assert_eq!(out["title"], serde_json::json!("Fixed"));
        assert!(out["paper_id"].is_i64(), "missing PaperDetails keys: {out}");
        assert!(out.get("full_text").is_none(), "leaked full_text: {out}");
        assert!(out.get("repaired").is_none(), "stale receipt shape: {out}");
    }
}
