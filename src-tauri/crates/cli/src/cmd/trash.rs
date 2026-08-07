//! Group `trash` — cmd_trash_* in `linxiv_cli.py`.

use clap::Subcommand;
use serde_json::json;

use linxiv_core::service::paper::{self as svc_paper, Paper};
use linxiv_core::service::project::{self as svc_project, Project};

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

#[derive(Subcommand)]
pub enum TrashCmd {
    /// List soft-deleted papers and projects
    List,
    /// Restore a soft-deleted paper
    Restore { source_id: String },
    /// Permanently delete a paper
    HardDelete { source_id: String },
    /// Restore a soft-deleted project
    RestoreProject { project_id: i64 },
    /// Permanently delete a project
    HardDeleteProject { project_id: i64 },
}

pub async fn run(cmd: TrashCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        // cmd_trash_list
        TrashCmd::List => {
            let papers = svc_paper::list_deleted(&ctx.conn)?;
            let projects = svc_project::list_deleted(&ctx.conn)?;
            output(&json!({ "papers": papers, "projects": projects }));
        }
        // cmd_trash_restore -> _do_paper_restore
        TrashCmd::Restore { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            svc_paper::require_trashed(&ctx.conn, &source_id).unwrap_or_else(|e| fail(e));
            let (pdf_path, project_fks) = svc_paper::restore(
                &mut ctx.conn,
                &Paper {
                    source_id: Some(source_id.clone()),
                    ..Default::default()
                },
            )?;
            output(&json!({
                "restored": source_id,
                "pdf_path": pdf_path,
                "project_fks": project_fks,
            }));
        }
        // cmd_trash_hard_delete -> require_trashed guard then _do_paper_hard_delete
        TrashCmd::HardDelete { source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            svc_paper::require_trashed(&ctx.conn, &source_id).unwrap_or_else(|e| fail(e));
            // _do_paper_hard_delete: get_paper_root None -> not found; here unreachable
            // after the guard, but mirror the message off hard_delete's None return.
            if svc_paper::hard_delete(
                &mut ctx.conn,
                &Paper {
                    source_id: Some(source_id.clone()),
                    ..Default::default()
                },
            )?
            .is_none()
            {
                fail(format!(
                    "Paper {} not found",
                    crate::output::pyrepr(&source_id)
                ));
            }
            output(&json!({ "hard_deleted": source_id }));
        }
        // cmd_trash_restore_project
        TrashCmd::RestoreProject { project_id } => {
            svc_project::require_trashed(&ctx.conn, project_id).unwrap_or_else(|e| fail(e));
            svc_project::restore(
                &ctx.conn,
                &Project {
                    project_fk: Some(project_id),
                },
            )?;
            output(&json!({ "restored_project_id": project_id }));
        }
        // cmd_trash_hard_delete_project
        TrashCmd::HardDeleteProject { project_id } => {
            svc_project::require_trashed(&ctx.conn, project_id).unwrap_or_else(|e| fail(e));
            svc_project::hard_delete(
                &mut ctx.conn,
                &Project {
                    project_fk: Some(project_id),
                },
            )?;
            output(&json!({ "hard_deleted_project_id": project_id }));
        }
    }
    Ok(())
}

/// `_resolve_project_or_exit`: None -> `{"error": "Project {id} not found"}` + exit(1).
pub(crate) fn resolve_project_or_exit(
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
