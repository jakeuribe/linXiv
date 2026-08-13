//! Group `project` — cmd_project_* (incl. export/import) in `linxiv_cli.py`.

use std::path::Path;

use clap::{Subcommand, ValueEnum};
use serde_json::json;

use linxiv_core::error::Result as CoreResult;
use linxiv_core::formats::with_default_ext;
use linxiv_core::models::{ProjectIn, ProjectUpdateIn, Status};
use linxiv_core::service::{export_import, project};

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ProjectStatus {
    Active,
    Archived,
    Deleted,
}

impl ProjectStatus {
    fn to_status(self) -> Status {
        match self {
            ProjectStatus::Active => Status::Active,
            ProjectStatus::Archived => Status::Archived,
            ProjectStatus::Deleted => Status::Deleted,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OnConflict {
    Merge,
    Overwrite,
}

impl OnConflict {
    fn to_core(self) -> export_import::OnConflict {
        match self {
            OnConflict::Merge => export_import::OnConflict::Merge,
            OnConflict::Overwrite => export_import::OnConflict::Overwrite,
        }
    }
}

#[derive(Subcommand)]
pub enum ProjectCmd {
    /// List projects
    List {
        #[arg(long, value_enum)]
        status: Option<ProjectStatus>,
    },
    /// Get project details
    Get { project_id: i64 },
    /// Create a project
    Create {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Hex color (e.g. #4f86f7)
        #[arg(long)]
        color: Option<String>,
        #[arg(long, num_args = 0..)]
        tags: Option<Vec<String>>,
    },
    /// Update project fields
    Update {
        project_id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Hex color (e.g. #4f86f7)
        #[arg(long)]
        color: Option<String>,
        /// Project tags (replaces existing; pass no values to clear)
        #[arg(long, num_args = 0..)]
        tags: Option<Vec<String>>,
        #[arg(long, value_enum)]
        status: Option<ProjectStatus>,
    },
    /// Soft-delete a project
    Delete { project_id: i64 },
    /// Archive an active project
    Archive { project_id: i64 },
    /// Restore an archived or deleted project
    Restore { project_id: i64 },
    /// Permanently delete a project
    HardDelete { project_id: i64 },
    /// Add a paper to a project
    AddPaper { project_id: i64, source_id: String },
    /// Add several papers to a project in one call
    AddPapers {
        project_id: i64,
        #[arg(required = true, num_args = 1..)]
        source_ids: Vec<String>,
    },
    /// Remove a paper from a project
    RemovePaper { project_id: i64, source_id: String },
    /// Export a project to a .lxproj archive
    Export {
        project_id: i64,
        /// Destination path (.lxproj extension added automatically)
        dest: String,
        /// Include bundled PDFs in the archive
        #[arg(long)]
        pdfs: bool,
    },
    /// Import a project from a .lxproj archive
    Import {
        zip_path: String,
        /// Show archive summary without modifying the database
        #[arg(long)]
        preview: bool,
        /// How to handle papers that already exist (default: merge)
        #[arg(long, value_enum, default_value_t = OnConflict::Merge)]
        on_conflict: OnConflict,
    },
    /// Export project papers as BibTeX
    ExportBibtex {
        project_id: i64,
        /// Output file path (.bib added if no extension)
        dest: String,
    },
    /// Export project papers as Obsidian markdown
    ExportObsidian {
        project_id: i64,
        /// Output file path (.md added if no extension)
        dest: String,
    },
}

/// `_resolve_project_or_exit`: fetch by id or exit 1. The not-found wording is
/// `CoreError::ProjectNotFound` — the same message the route and MCP emit.
fn resolve_or_exit(ctx: &Ctx, project_id: i64) -> linxiv_core::models::ProjectDetails {
    match project::get_required(&ctx.conn, project_id) {
        Ok(p) => p,
        Err(e) => fail(e),
    }
}

/// Unwrap a single-paper membership op: user-facing refusals exit 1 with the
/// shared wording, anything else propagates as a hard error.
fn membership_or_exit(
    r: CoreResult<project::PaperMembershipReceipt>,
) -> anyhow::Result<project::PaperMembershipReceipt> {
    use linxiv_core::error::CoreError;
    match r {
        Ok(receipt) => Ok(receipt),
        Err(
            e @ (CoreError::PaperNotFound(_)
            | CoreError::ProjectNotFound(_)
            | CoreError::ProjectDeleted(_)),
        ) => fail(e),
        Err(e) => Err(e.into()),
    }
}

pub async fn run(cmd: ProjectCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        ProjectCmd::List { status } => {
            let core_status = status.map(|s| s.to_status());
            let mut projects = project::get_many(
                &ctx.conn,
                &project::Projects {
                    status: core_status,
                    ..Default::default()
                },
            )?;
            if core_status.is_none() {
                projects.retain(|p| p.status != Status::Deleted);
            }
            let rows = projects
                .into_iter()
                .map(|p| project::to_out(&ctx.conn, p))
                .collect::<CoreResult<Vec<_>>>()?;
            output(&rows);
        }

        ProjectCmd::Get { project_id } => {
            let details = resolve_or_exit(ctx, project_id);
            output(&project::to_out(&ctx.conn, details)?);
        }

        ProjectCmd::Create {
            name,
            description,
            color,
            tags,
        } => {
            // Python `color_from_hex(args.color) if args.color else None`: empty string -> no color.
            let color = match &color {
                Some(hex) if !hex.is_empty() => Some(project::color_from_hex(hex)?),
                _ => None,
            };
            let id = project::create(
                &mut ctx.conn,
                &ProjectIn {
                    name: name.clone(),
                    description,
                    color,
                    tags: tags.unwrap_or_default(),
                    source_fks: Vec::new(),
                },
            )?;
            output(&json!({ "id": id, "name": name, "status": "active" }));
        }

        ProjectCmd::Update {
            project_id,
            name,
            description,
            color,
            tags,
            status,
        } => {
            // Mirror `_resolve_project_or_exit` before mutating.
            resolve_or_exit(ctx, project_id);
            let color = color
                .map(|hex| project::color_from_hex(&hex).map(Some))
                .transpose()
                .unwrap_or_else(|e| fail(e));
            if let Err(e) = project::update(
                &mut ctx.conn,
                &ProjectUpdateIn {
                    project_fk: project_id,
                    name,
                    description,
                    color,
                    project_tags: tags,
                    status: status.map(|s| s.to_status()),
                },
            ) {
                fail(e);
            }
            let updated = resolve_or_exit(ctx, project_id);
            output(&project::to_out(&ctx.conn, updated)?);
        }

        ProjectCmd::Delete { project_id } => {
            resolve_or_exit(ctx, project_id);
            project::delete(
                &ctx.conn,
                &project::Project {
                    project_fk: Some(project_id),
                },
            )?;
            output(&json!({ "deleted_project_id": project_id }));
        }

        ProjectCmd::Archive { project_id } => {
            resolve_or_exit(ctx, project_id);
            project::archive(
                &ctx.conn,
                &project::Project {
                    project_fk: Some(project_id),
                },
            )?;
            output(&json!({ "archived_project_id": project_id }));
        }

        ProjectCmd::Restore { project_id } => {
            resolve_or_exit(ctx, project_id);
            project::restore(
                &ctx.conn,
                &project::Project {
                    project_fk: Some(project_id),
                },
            )?;
            output(&linxiv_core::service::trash::RestoredProject {
                ok: true,
                restored_project_id: project_id,
            });
        }

        ProjectCmd::HardDelete { project_id } => {
            resolve_or_exit(ctx, project_id);
            project::hard_delete(
                &mut ctx.conn,
                &project::Project {
                    project_fk: Some(project_id),
                },
            )?;
            output(&linxiv_core::service::trash::HardDeletedProject {
                ok: true,
                hard_deleted_project_id: project_id,
            });
        }

        ProjectCmd::AddPaper {
            project_id,
            source_id,
        } => {
            let source_id = as_source_id(&source_id, "arxiv");
            output(&membership_or_exit(project::add_paper(
                &ctx.conn, project_id, &source_id,
            ))?);
        }

        // POST /api/projects/{id}/papers/bulk: partial success — `failed` holds the
        // ids that resolved to no paper root, the rest are linked and reported added.
        ProjectCmd::AddPapers {
            project_id,
            source_ids,
        } => {
            // `failed` comes back deduped, so dedup here too — otherwise a repeated
            // id is reported added twice and won't reconcile against paper_count.
            let mut seen = std::collections::HashSet::new();
            let source_ids: Vec<String> = source_ids
                .iter()
                .map(|s| as_source_id(s, "arxiv"))
                .filter(|s| seen.insert(s.clone()))
                .collect();
            let failed = match project::add_papers(&ctx.conn, project_id, &source_ids) {
                Ok(failed) => failed,
                Err(e @ linxiv_core::error::CoreError::ProjectNotFound(_)) => fail(e),
                Err(e @ linxiv_core::error::CoreError::ProjectDeleted(_)) => fail(e),
                Err(e) => return Err(e.into()),
            };
            let added: Vec<&String> = source_ids.iter().filter(|s| !failed.contains(s)).collect();
            output(&json!({
                "project_id": project_id,
                "ok": failed.is_empty(),
                "added": added,
                "failed": failed,
            }));
        }

        ProjectCmd::RemovePaper {
            project_id,
            source_id,
        } => {
            let source_id = as_source_id(&source_id, "arxiv");
            output(&membership_or_exit(project::remove_paper(
                &ctx.conn, project_id, &source_id,
            ))?);
        }

        ProjectCmd::Export {
            project_id,
            dest,
            pdfs,
        } => {
            let out = match export_import::export_project(
                &ctx.conn,
                project_id,
                Path::new(&dest),
                pdfs,
                &ctx.pdf_dir,
            ) {
                Ok(out) => out,
                Err(e) => fail(e),
            };
            output(&json!({ "path": out.display().to_string(), "project_id": project_id }));
        }

        ProjectCmd::Import {
            zip_path,
            preview,
            on_conflict,
        } => {
            let zip = Path::new(&zip_path);
            if preview {
                let prev = match export_import::preview_import(zip) {
                    Ok(p) => p,
                    Err(e) => fail(e),
                };
                output(&prev);
            } else {
                let fk = match export_import::commit_import(
                    &mut ctx.conn,
                    zip,
                    on_conflict.to_core(),
                    &ctx.pdf_dir,
                ) {
                    Ok(fk) => fk,
                    Err(e) => fail(e),
                };
                output(&json!({ "project_id": fk }));
            }
        }

        ProjectCmd::ExportBibtex { project_id, dest } => {
            let details = resolve_or_exit(ctx, project_id);
            let papers = project::export_papers(&ctx.conn, &details.source_fks)?;
            let bibtex = linxiv_core::formats::bibtex_export(&papers);
            let dest = with_default_ext(&dest, "bib");
            std::fs::write(&dest, bibtex)?;
            output(&json!({ "path": dest.display().to_string(), "project_id": project_id }));
        }

        ProjectCmd::ExportObsidian { project_id, dest } => {
            let details = resolve_or_exit(ctx, project_id);
            let papers = project::export_papers(&ctx.conn, &details.source_fks)?;
            let md = linxiv_core::formats::obsidian_export(&papers);
            let dest = with_default_ext(&dest, "md");
            std::fs::write(&dest, md)?;
            output(&json!({ "path": dest.display().to_string(), "project_id": project_id }));
        }
    }
    Ok(())
}
