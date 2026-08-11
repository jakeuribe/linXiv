//! Group `author` — cmd_author_* in `linxiv_cli.py`.

use clap::Subcommand;
use serde_json::json;

use linxiv_core::service::author::{self as svc_author, Author};

use crate::ctx::Ctx;
use crate::output::{fail, output};

#[derive(Subcommand)]
pub enum AuthorCmd {
    /// List all authors with paper counts
    List,
    /// Get author details and paper list
    Get { author_id: i64 },
    /// Update author fields
    Update {
        author_id: i64,
        #[arg(long = "full-name")]
        full_name: Option<String>,
        #[arg(long = "first-name")]
        first_name: Option<String>,
        #[arg(long = "last-name")]
        last_name: Option<String>,
        #[arg(long)]
        orcid: Option<String>,
    },
    /// Delete an author (blocked if linked to papers)
    Delete { author_id: i64 },
    /// Merge duplicate authors into a canonical one
    Merge {
        canonical_id: i64,
        #[arg(required = true, num_args = 1..)]
        duplicate_ids: Vec<i64>,
    },
    /// List authors sharing this author's ORCID (likely duplicates)
    MergeCandidates { author_id: i64 },
}

fn by_id(author_id: i64) -> Author {
    Author {
        author_id: Some(author_id),
        ..Default::default()
    }
}

pub async fn run(cmd: AuthorCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        // cmd_author_list: every author + active-paper count (min_papers=0 default).
        AuthorCmd::List => {
            output(&svc_author::list_with_paper_count(&ctx.conn, 0)?);
        }
        // cmd_author_get: the canonical AuthorWithPapers composite.
        AuthorCmd::Get { author_id } => {
            let Some(detail) = svc_author::get_with_papers(&ctx.conn, author_id)? else {
                fail(format!("Author {author_id} not found"))
            };
            output(&detail);
        }
        // cmd_author_update: update_fields owns the "exists" + "at least one field" guards.
        AuthorCmd::Update {
            author_id,
            full_name,
            first_name,
            last_name,
            orcid,
        } => {
            svc_author::update_fields(
                &ctx.conn,
                author_id,
                full_name.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                orcid.as_deref(),
            )
            .unwrap_or_else(|e| fail(e));
            output(&json!({ "updated_author_id": author_id }));
        }
        // cmd_author_delete: svc_author::delete owns the "exists" + "still linked" guards.
        AuthorCmd::Delete { author_id } => {
            svc_author::delete(&ctx.conn, &by_id(author_id)).unwrap_or_else(|e| fail(e));
            output(&json!({ "deleted_author_id": author_id }));
        }
        // `merge_authors` (MCP) / POST /api/authors/{id}/merge: 404 on the canonical
        // first, then re-point the duplicates' papers. Absent ids merge to nothing.
        AuthorCmd::Merge {
            canonical_id,
            duplicate_ids,
        } => {
            if svc_author::get(&ctx.conn, &by_id(canonical_id))?.is_none() {
                fail(format!("Author {canonical_id} not found"));
            }
            let merged = svc_author::merge(&mut ctx.conn, canonical_id, &duplicate_ids)?;
            output(&json!({ "canonical_id": canonical_id, "merged_ids": merged }));
        }
        // GET /api/authors/{id}/merge-candidates: empty when the author has no ORCID.
        AuthorCmd::MergeCandidates { author_id } => {
            if svc_author::get(&ctx.conn, &by_id(author_id))?.is_none() {
                fail(format!("Author {author_id} not found"));
            }
            let candidates = svc_author::orcid_merge_candidates(&ctx.conn, author_id)?;
            output(&json!({ "candidates": candidates }));
        }
    }
    Ok(())
}
