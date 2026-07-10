//! Group `tag` — cmd_tag_* in `linxiv_cli.py`.

use clap::Subcommand;
use serde_json::json;

use linxiv_core::error::CoreError;
use linxiv_core::models::TagIn;
use linxiv_core::service::paper::{self as svc_paper, Paper};
use linxiv_core::service::project::{self as svc_project, Project};
use linxiv_core::service::tag::{self as svc_tag, Tag};
use linxiv_core::storage::queries::paper as paperq;
use linxiv_core::storage::queries::tag as tagq;

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

#[derive(Subcommand)]
pub enum TagCmd {
    /// Add tags to a paper
    Add {
        source_id: String,
        #[arg(required = true, num_args = 1..)]
        tags: Vec<String>,
    },
    /// Remove tags from a paper
    Remove {
        source_id: String,
        #[arg(required = true, num_args = 1..)]
        tags: Vec<String>,
    },
    /// List tags on a paper
    List { source_id: String },
    /// List all tags in the database
    ListAll,
    /// Create a tag
    Create { label: String },
    /// Delete a tag by ID
    Delete { tag_id: i64 },
    /// Add tags to a project
    AddProject {
        project_id: i64,
        #[arg(required = true, num_args = 1..)]
        tags: Vec<String>,
    },
    /// Remove tags from a project
    RemoveProject {
        project_id: i64,
        #[arg(required = true, num_args = 1..)]
        tags: Vec<String>,
    },
    /// List tags on a project
    ListProject { project_id: i64 },
}

pub async fn run(cmd: TagCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        // cmd_tag_add: prefix the id, UNION tags onto the paper; KeyError -> not found.
        TagCmd::Add { source_id, tags } => {
            let source_id = as_source_id(&source_id, "arxiv");
            match paperq::add_paper_tags(&mut ctx.conn, &source_id, &tags) {
                Ok(updated) => output(&json!({ "source_id": source_id, "tags": updated })),
                Err(CoreError::NotFound(_)) => fail(format!("Paper {source_id} not found in DB")),
                Err(e) => return Err(e.into()),
            }
        }
        // cmd_tag_remove
        TagCmd::Remove { source_id, tags } => {
            let source_id = as_source_id(&source_id, "arxiv");
            match paperq::remove_paper_tags(&mut ctx.conn, &source_id, &tags) {
                Ok(updated) => output(&json!({ "source_id": source_id, "tags": updated })),
                Err(CoreError::NotFound(_)) => fail(format!("Paper {source_id} not found in DB")),
                Err(e) => return Err(e.into()),
            }
        }
        // cmd_tag_list: missing paper -> empty list (no error), matching get_paper_tags.
        TagCmd::List { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            let tags = svc_paper::get(
                &ctx.conn,
                &Paper {
                    source_id: Some(source_id.clone()),
                    ..Default::default()
                },
            )?
            .map(|d| d.tags)
            .unwrap_or_default();
            output(&json!({ "source_id": source_id, "tags": tags }));
        }
        // cmd_tag_list_all
        TagCmd::ListAll => {
            output(&svc_tag::list_all_tags(&ctx.conn)?);
        }
        // cmd_tag_create
        TagCmd::Create { label } => {
            let tag_id = svc_tag::upsert(
                &mut ctx.conn,
                &TagIn {
                    label: label.clone(),
                },
            )?;
            output(&json!({ "tag_id": tag_id, "label": label }));
        }
        // cmd_tag_delete
        TagCmd::Delete { tag_id } => {
            svc_tag::delete(
                &ctx.conn,
                &Tag {
                    tag_id: Some(tag_id),
                    label: None,
                },
            )?;
            output(&json!({ "deleted_tag_id": tag_id }));
        }
        // cmd_tag_add_project: resolve-or-exit, then add.
        TagCmd::AddProject { project_id, tags } => {
            resolve_project_or_exit(ctx, project_id)?;
            let updated = tagq::add_project_tags(&mut ctx.conn, project_id, &tags)?;
            output(&json!({ "project_id": project_id, "tags": updated }));
        }
        // cmd_tag_remove_project
        TagCmd::RemoveProject { project_id, tags } => {
            resolve_project_or_exit(ctx, project_id)?;
            let updated = tagq::remove_project_tags(&mut ctx.conn, project_id, &tags)?;
            output(&json!({ "project_id": project_id, "tags": updated }));
        }
        // cmd_tag_list_project: tags come off the resolved project's details.
        TagCmd::ListProject { project_id } => {
            let details = resolve_project_or_exit(ctx, project_id)?;
            output(&json!({ "project_id": project_id, "tags": details.project_tags }));
        }
    }
    Ok(())
}

/// `_resolve_project_or_exit`: None -> `{"error": "Project {id} not found"}` + exit(1).
fn resolve_project_or_exit(
    ctx: &Ctx,
    project_id: i64,
) -> anyhow::Result<linxiv_core::models::ProjectDetails> {
    match svc_project::get(
        &ctx.conn,
        &Project {
            project_fk: Some(project_id),
        },
    )? {
        Some(d) => Ok(d),
        None => fail(format!("Project {project_id} not found")),
    }
}
