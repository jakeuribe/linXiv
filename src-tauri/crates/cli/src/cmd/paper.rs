//! Group `paper` — cmd_paper_* in `linxiv_cli.py`.

use clap::Subcommand;
use serde_json::json;

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

use linxiv_core::models::PaperMetadata;
use linxiv_core::service::{paper as svc_paper, project as svc_project};
use linxiv_core::storage;

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
}

/// `_resolve_paper_or_exit`: load a paper or fail with the not-found error.
pub(super) fn resolve_paper_or_exit(ctx: &Ctx, source_id: &str) -> linxiv_core::models::PaperDetails {
    match svc_paper::get(&ctx.conn, &paper(source_id)) {
        Ok(Some(p)) => p,
        Ok(None) => fail(format!(
            "Paper {} not found in DB",
            crate::output::pyrepr(source_id)
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
                None => fail(format!(
                    "Paper {} not found in DB",
                    crate::output::pyrepr(&source_id)
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
            let root = match storage::queries::paper::get_paper_root(&ctx.conn, &source_id)? {
                Some(r) => r,
                None => fail(format!(
                    "Paper {} not found",
                    crate::output::pyrepr(&source_id)
                )),
            };
            // `existing.version if existing is not None else 1`.
            let version = svc_paper::get(&ctx.conn, &paper(&source_id))?
                .map(|e| e.version)
                .unwrap_or(1);
            // Build via serde so the `published` string is parsed/validated as a
            // NaiveDate without naming chrono (not a cli dep). All other fields are
            // well-typed, so the only deserialize failure is a bad date string —
            // matching Python's `datetime.date.fromisoformat` ValueError guard.
            // `args.summary or ""` is already the clap default; `args.tags or None`
            // collapses an empty list to None.
            let meta: PaperMetadata = match serde_json::from_value(json!({
                "source_id": &source_id,
                "version": version,
                "title": title,
                "authors": authors,
                "published": &published,
                "summary": summary,
                "category": category,
                "doi": doi,
                "url": url,
                "tags": tags.filter(|t| !t.is_empty()),
            })) {
                Ok(m) => m,
                Err(_) => fail(format!(
                    "Invalid date {}; use YYYY-MM-DD",
                    crate::output::pyrepr(&published)
                )),
            };
            svc_paper::repair_paper(&mut ctx.conn, root.source_fk, &meta)?;
            output(&json!({ "repaired": source_id }));
        }

        // cmd_paper_restore: only valid from trash; returns pdf path + project links.
        PaperCmd::Restore { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            if !svc_paper::is_paper_deleted(&ctx.conn, &source_id)? {
                fail(format!(
                    "Paper {} not found in trash",
                    crate::output::pyrepr(&source_id)
                ));
            }
            let (pdf_path, project_fks) = svc_paper::restore(&mut ctx.conn, &paper(&source_id))?;
            output(&json!({
                "restored": source_id,
                "pdf_path": pdf_path,
                "project_fks": project_fks,
            }));
        }

        // cmd_paper_hard_delete: permanently remove an existing paper.
        PaperCmd::HardDelete { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            if storage::queries::paper::get_paper_root(&ctx.conn, &source_id)?.is_none() {
                fail(format!(
                    "Paper {} not found",
                    crate::output::pyrepr(&source_id)
                ));
            }
            svc_paper::hard_delete(&mut ctx.conn, &paper(&source_id))?;
            output(&json!({ "hard_deleted": source_id }));
        }

        // cmd_paper_search: `svc_paper.search_papers` — FTS over TeX source merged
        // with note-content (LIKE) hits. Python swallows FTS5 syntax errors and
        // returns [] for the FTS path so note hits still populate. FTS rows first;
        // then the latest active paper per note SOURCE_FK (in note-recency order),
        // deduped by source_id, capped at limit.
        PaperCmd::Search { query, limit } => {
            let mut results =
                storage::search_full_text(&ctx.conn, &query, limit).unwrap_or_default();
            let mut seen: std::collections::HashSet<String> =
                results.iter().map(|r| r.source_id.clone()).collect();

            let notes_sfks =
                storage::queries::note::search_notes_source_fks(&ctx.conn, &query, limit)?;
            if !notes_sfks.is_empty() {
                // Latest active paper per SOURCE_FK (== Python db.get_papers_by_source_fks);
                // re-ordered by notes_sfks rank since get_many returns published DESC.
                let note_papers = svc_paper::get_many(
                    &ctx.conn,
                    &svc_paper::Papers {
                        source_fks: Some(notes_sfks.clone()),
                        ..Default::default()
                    },
                )?;
                for sfk in &notes_sfks {
                    if let Some(p) = note_papers.iter().find(|p| p.source_fk == Some(*sfk)) {
                        if seen.insert(p.source_id.clone()) {
                            results.push(p.clone());
                        }
                    }
                }
            }
            results.truncate(limit as usize);
            output(&results);
        }

        // cmd_paper_remove_from_all: drop the paper from every project it's in.
        PaperCmd::RemoveFromAllProjects { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            match svc_project::remove_paper_from_all_projects_by_id(&mut ctx.conn, &source_id)? {
                Some(removed) => output(&json!({
                    "source_id": source_id,
                    "removed_from_projects": removed,
                })),
                None => fail(format!(
                    "Paper {} not found",
                    crate::output::pyrepr(&source_id)
                )),
            }
        }
    }
    Ok(())
}
