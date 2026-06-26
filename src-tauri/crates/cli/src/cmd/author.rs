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
}

pub async fn run(cmd: AuthorCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        // cmd_author_list: every author + active-paper count (min_papers=0 default).
        AuthorCmd::List => {
            output(&svc_author::list_with_paper_count(&ctx.conn, 0)?);
        }
        // cmd_author_get: author dict with an inlined "papers" preview list.
        AuthorCmd::Get { author_id } => {
            let author = match svc_author::get(
                &ctx.conn,
                &Author { author_id: Some(author_id), ..Default::default() },
            )? {
                Some(a) => a,
                None => fail(format!("Author {author_id} not found")),
            };
            let previews = svc_author::get_paper_previews(&ctx.conn, author_id)?;
            let mut result = serde_json::to_value(&author)?;
            result["papers"] = serde_json::to_value(&previews)?;
            output(&result);
        }
        // cmd_author_update: 404 first, then require at least one field, then partial update.
        AuthorCmd::Update { author_id, full_name, first_name, last_name, orcid } => {
            if svc_author::get(
                &ctx.conn,
                &Author { author_id: Some(author_id), ..Default::default() },
            )?
            .is_none()
            {
                fail(format!("Author {author_id} not found"));
            }
            if full_name.is_none() && first_name.is_none() && last_name.is_none() && orcid.is_none() {
                fail("at least one of --full-name, --first-name, --last-name, or --orcid must be provided");
            }
            svc_author::update_fields(
                &ctx.conn,
                author_id,
                full_name.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                orcid.as_deref(),
            )?;
            output(&json!({ "updated_author_id": author_id }));
        }
        // cmd_author_delete: blocked while linked to any paper.
        AuthorCmd::Delete { author_id } => {
            let link_count = svc_author::count_paper_links(&ctx.conn, author_id)?;
            if link_count > 0 {
                fail(format!(
                    "Author {author_id} is linked to {link_count} paper(s); unlink first"
                ));
            }
            svc_author::delete(&ctx.conn, &Author { author_id: Some(author_id), ..Default::default() })?;
            output(&json!({ "deleted_author_id": author_id }));
        }
    }
    Ok(())
}
