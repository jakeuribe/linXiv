//! Group `paper` — cmd_paper_* in `linxiv_cli.py`.

use clap::Subcommand;
use serde_json::json;

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

use linxiv_core::config;
use linxiv_core::models::PaperMetadata;
use linxiv_core::service::{paper as svc_paper, project as svc_project};

#[derive(Subcommand)]
pub enum PaperCmd {
    /// Get full details for a paper
    Get { source_id: String },
    /// Soft-delete a paper
    Delete { source_id: String },
    /// List all stored versions of a paper
    Versions { source_id: String },
    /// Overwrite paper metadata in-place
    Repair {
        source_id: String,
        #[arg(long, required = true)]
        title: String,
        #[arg(long, required = true, num_args = 1..)]
        authors: Vec<String>,
        /// Publication date (YYYY-MM-DD)
        #[arg(long, required = true)]
        published: String,
        #[arg(long, default_value = "")]
        summary: String,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        doi: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long, num_args = 0..)]
        tags: Option<Vec<String>>,
    },
    /// Restore a soft-deleted paper
    Restore { source_id: String },
    /// Permanently delete a paper
    HardDelete { source_id: String },
    /// Full-text search within local library
    Search {
        query: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Remove a paper from every project
    RemoveFromAllProjects { source_id: String },
    /// List other paper roots sharing this paper's DOI
    DoiCandidates { source_id: String },
    /// Fetch a paper's arXiv TeX source and index it for full-text search
    FetchSource {
        source_id: String,
        /// Re-fetch even when the source was already indexed
        #[arg(long)]
        force: bool,
    },
    /// Backfill full-text search over stored arXiv papers with no TeX source yet
    IndexSources {
        /// Stop after this many papers — arXiv pacing puts each fetch ~7s apart
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
}

/// Fetch one paper's TeX source and store it — `service::paper`'s two-phase
/// ingest (always the paper's latest version; the source URL is derived from
/// that version's PDF link).
async fn ingest_source(
    ctx: &mut Ctx,
    paper: &linxiv_core::models::PaperDetails,
) -> linxiv_core::error::Result<svc_paper::FullTextReceipt> {
    let fetched = svc_paper::fetch_full_text(paper, &config::data_dir()).await?;
    fetched.commit(&mut ctx.conn)
}

/// `_resolve_paper_or_exit`: load a paper or fail with the not-found error.
pub(super) fn resolve_paper_or_exit(
    ctx: &Ctx,
    source_id: &str,
) -> linxiv_core::models::PaperDetails {
    match svc_paper::get(&ctx.conn, &paper(source_id)) {
        Ok(Some(p)) => p,
        Ok(None) => fail(linxiv_core::error::CoreError::PaperNotFound(
            source_id.to_string(),
        )),
        Err(e) => fail(e),
    }
}

fn paper(source_id: &str) -> svc_paper::Paper {
    svc_paper::Paper {
        source_id: Some(source_id.to_string()),
        ..Default::default()
    }
}

pub async fn run(cmd: PaperCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        // cmd_paper_get: resolve-or-exit, then dump the details dict.
        PaperCmd::Get { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            let details = resolve_paper_or_exit(ctx, &source_id);
            output(&details);
        }

        // cmd_paper_delete: ensure it exists, soft-delete, report the id.
        PaperCmd::Delete { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            resolve_paper_or_exit(ctx, &source_id);
            svc_paper::delete(&mut ctx.conn, &paper(&source_id))?;
            output(&json!({ "deleted": source_id }));
        }

        // cmd_paper_versions: all stored versions, or not-found.
        PaperCmd::Versions { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            match svc_paper::get_all(&ctx.conn, &paper(&source_id))? {
                Some(all) => output(&all),
                None => fail(linxiv_core::error::CoreError::PaperNotFound(
                    source_id.clone(),
                )),
            }
        }

        // cmd_paper_repair: overwrite metadata in-place on the existing root.
        PaperCmd::Repair {
            source_id,
            title,
            authors,
            published,
            summary,
            category,
            doi,
            url,
            tags,
        } => {
            let source_id = as_source_id(&source_id, "arxiv");
            let source_fk =
                svc_paper::resolve_source_fk(&ctx.conn, &source_id).unwrap_or_else(|e| fail(e));
            // `existing.version if existing is not None else 1`.
            let version = svc_paper::get(&ctx.conn, &paper(&source_id))?
                .map(|e| e.version)
                .unwrap_or(1);
            let meta = PaperMetadata {
                source_id: source_id.clone(),
                version,
                title,
                authors,
                published: match svc_paper::parse_published(&published) {
                    Ok(d) => d,
                    Err(e) => fail(e.to_string()),
                },
                updated: None,
                summary,
                category,
                categories: None,
                doi,
                journal_ref: None,
                comment: None,
                url,
                tags,
                source: None,
                author_orcids: None,
            };
            // repair_paper normalizes and validates (blank title, no authors, empty DOI).
            match svc_paper::repair_paper(&mut ctx.conn, source_fk, &meta) {
                Ok(()) => {}
                Err(e @ linxiv_core::error::CoreError::Validation(_)) => fail(e.to_string()),
                Err(e) => return Err(e.into()),
            }
            // Route parity (`PUT /api/papers/sfk/{fk}`): the repaired PaperDetails.
            output(&resolve_paper_or_exit(ctx, &source_id));
        }

        // cmd_paper_restore: only valid from trash; returns pdf path + project links.
        PaperCmd::Restore { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            svc_paper::require_trashed(&ctx.conn, &source_id).unwrap_or_else(|e| fail(e));
            let (pdf_path, project_fks) = svc_paper::restore(&mut ctx.conn, &paper(&source_id))?;
            output(&linxiv_core::service::trash::RestoredPaper {
                ok: true,
                restored: source_id,
                pdf_path,
                project_fks,
            });
        }

        // cmd_paper_hard_delete: permanently remove an existing paper.
        PaperCmd::HardDelete { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            svc_paper::resolve_source_fk(&ctx.conn, &source_id).unwrap_or_else(|e| fail(e));
            svc_paper::hard_delete(&mut ctx.conn, &paper(&source_id))?;
            output(&linxiv_core::service::trash::HardDeletedPaper {
                ok: true,
                hard_deleted: source_id,
            });
        }

        // cmd_paper_search: `svc_paper.search_papers` — the shared FTS + note-content
        // merge, so CLI, route and MCP return the same set.
        PaperCmd::Search { query, limit } => {
            output(&svc_paper::search_library(&ctx.conn, &query, limit)?);
        }

        // cmd_paper_remove_from_all: drop the paper from every project it's in.
        PaperCmd::RemoveFromAllProjects { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            match svc_project::remove_paper_from_all_projects_by_id(&mut ctx.conn, &source_id)? {
                // One envelope across route/CLI/MCP; the caller already knows the id.
                Some(removed) => output(&json!({
                    "ok": true,
                    "removed_from_projects": removed,
                })),
                None => fail(linxiv_core::error::CoreError::PaperNotFound(
                    source_id.clone(),
                )),
            }
        }

        // GET /api/papers/sfk/{fk}/doi-candidates keys off SOURCE_FK; resolve the
        // CLI's source_id to its root first. Empty when the paper has no DOI.
        PaperCmd::DoiCandidates { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            let source_fk =
                svc_paper::resolve_source_fk(&ctx.conn, &source_id).unwrap_or_else(|e| fail(e));
            let candidates = svc_paper::find_doi_version_candidates(&ctx.conn, source_fk)?;
            output(&json!({ "candidates": candidates }));
        }

        // The write half of `paper search`: pull the TeX tarball, extract, index.
        PaperCmd::FetchSource { source_id, force } => {
            let source_id = as_source_id(&source_id, "arxiv");
            let paper = resolve_paper_or_exit(ctx, &source_id);
            output(&fetch_source_result(ctx, &paper, force).await);
        }

        // Backfill for papers saved before source retrieval was wired up. One
        // paper's failure is reported and skipped, never aborting the run.
        PaperCmd::IndexSources { limit } => {
            output(&index_sources_result(ctx, limit).await);
        }
    }
    Ok(())
}

/// Body of `PaperCmd::FetchSource`: skip-or-fetch, returning the shared receipt.
async fn fetch_source_result(
    ctx: &mut Ctx,
    paper: &linxiv_core::models::PaperDetails,
    force: bool,
) -> serde_json::Value {
    let receipt = if paper.downloaded_source && !force {
        svc_paper::FullTextReceipt::already_indexed(paper)
    } else {
        match ingest_source(ctx, paper).await {
            Ok(r) => r,
            Err(e) => fail(e),
        }
    };
    serde_json::to_value(&receipt).unwrap_or_else(|e| fail(e))
}

/// Body of `PaperCmd::IndexSources`: walk the unfetched work list and ingest
/// each, stopping after `limit` papers have actually been attempted.
///
/// The work list is source_ids only and each paper is loaded as its turn comes,
/// so the scan never holds more than one paper's TeX body in memory.
async fn index_sources_result(ctx: &mut Ctx, limit: usize) -> serde_json::Value {
    let work_list = svc_paper::full_text_backfill_candidates(&ctx.conn).unwrap_or_else(|e| fail(e));
    let pending = work_list.len();
    let mut unfetchable = 0usize;
    let mut attempted = 0usize;
    let mut indexed = 0usize;
    let mut skipped = 0usize;
    let mut failed = Vec::new();

    for source_id in &work_list {
        if attempted >= limit {
            break;
        }
        let Ok(Some(paper)) = svc_paper::get(&ctx.conn, &paper(source_id)) else {
            continue; // deleted between building the list and reaching it
        };
        // The work-list query selects only papers this accepts, so `unfetchable`
        // should stay 0; it reports anything the two rules disagree about rather
        // than spending a request on it.
        if svc_paper::source_fetch_url(&paper).is_err() {
            unfetchable += 1;
            continue;
        }
        attempted += 1;
        match ingest_source(ctx, &paper).await {
            Ok(r) if r.indexed => {
                indexed += 1;
                eprintln!(
                    "[full-text] {} — {} chars",
                    paper.source_id,
                    r.chars.unwrap_or(0)
                );
            }
            Ok(_) => skipped += 1,
            Err(e) => failed.push(json!({
                "source_id": paper.source_id,
                "error": e.to_string(),
            })),
        }
    }
    json!({
        "pending": pending,
        "attempted": attempted,
        "indexed": indexed,
        "skipped": skipped,
        "unfetchable": unfetchable,
        "failed": failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use linxiv_core::storage;
    use std::env;

    fn arxiv_meta(source_id: &str, url: &str) -> PaperMetadata {
        serde_json::from_value(json!({
            "source_id": source_id,
            "version": 1,
            "title": "T",
            "authors": ["Alice"],
            "published": "2024-01-01",
            "summary": "s",
            "category": "cs.LG",
            "url": url,
            "source": "arxiv",
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn fetch_source_and_index_sources_never_touch_the_network() {
        let dir = env::temp_dir().join(format!("linxiv-cli-paper-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let conn = storage::open(&dir.join("papers.db")).unwrap();
        storage::init_db(&conn).unwrap();
        let mut ctx = Ctx {
            conn,
            pdf_dir: dir.join("pdfs"),
            settings: config::UserSettings::load().unwrap(),
        };

        // (a) already downloaded, no --force: skip without calling ingest_source.
        svc_paper::save_paper_metadata(
            &mut ctx.conn,
            &arxiv_meta("arxiv:skip1", "http://arxiv.org/pdf/1v1"),
            None,
        )
        .unwrap();
        svc_paper::set_full_text(&mut ctx.conn, "arxiv:skip1", 1, "already have this").unwrap();

        let fetched = resolve_paper_or_exit(&ctx, "arxiv:skip1");
        let v = fetch_source_result(&mut ctx, &fetched, false).await;
        assert_eq!(v["indexed"], false);
        // The reason is single-sourced from core (route wording: force=true).
        assert!(v["reason"].as_str().unwrap().contains("force"));
        let unchanged = svc_paper::get(&ctx.conn, &paper("arxiv:skip1"))
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.full_text.as_deref(), Some("already have this"));

        // (b) an arXiv paper with no /pdf/ URL has no tarball URL to derive.
        svc_paper::save_paper_metadata(
            &mut ctx.conn,
            &arxiv_meta("arxiv:nopdf", "http://arxiv.org/abs/2v1"),
            None,
        )
        .unwrap();

        // skip1 is already fetched and nopdf has nothing to fetch, so both are
        // off the work list: nothing is attempted and no network call happens.
        let v = index_sources_result(&mut ctx, 10).await;
        assert_eq!(v["pending"], 0);
        assert_eq!(v["attempted"], 0);
        assert_eq!(v["indexed"], 0);
        assert_eq!(v["unfetchable"], 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
