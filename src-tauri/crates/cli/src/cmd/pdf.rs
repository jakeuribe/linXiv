//! Group `pdf` — cmd_pdf_* in `linxiv_cli.py`.

use std::path::PathBuf;

use clap::Subcommand;
use serde::Serialize;
use serde_json::json;

use linxiv_core::config;
use linxiv_core::service::files::{self as svc_files, PdfLocation};
use linxiv_core::service::paper as svc_paper;
use linxiv_core::service::paper_import;

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output, pyrepr};

#[derive(Subcommand)]
pub enum PdfCmd {
    /// Show local PDF path for a paper
    Path {
        source_id: String,
        /// Paper version (defaults to latest)
        #[arg(long)]
        version: Option<i64>,
    },
    /// Download PDF for a paper
    Download {
        source_id: String,
        /// PDF download URL
        url: String,
        /// Paper version (defaults to latest)
        #[arg(long)]
        version: Option<i64>,
    },
    /// List papers with a PDF saved on disk
    List,
    /// Delete every saved version's PDF for a paper
    Delete { source_id: String },
    /// Report total PDF storage usage
    Storage,
    /// Import a local PDF (extract metadata)
    Import {
        /// Path to PDF file
        file: String,
        /// Link imported paper to a project
        #[arg(long = "project-id")]
        project_id: Option<i64>,
    },
}

/// `cmd_pdf_storage` output dict.
#[derive(Serialize)]
struct StorageInfo {
    storage_mb: f64,
    pdf_dir: PathBuf,
}

/// `cmd_pdf_import` output dict.
#[derive(Serialize)]
struct ImportedPdf {
    source_id: String,
    title: String,
}

/// Resolves an explicit `--version` for `pdf download`, validating it against
/// stored versions (mark_pdf_saved below would update no rows otherwise,
/// orphaning the downloaded file), or falls back to the paper's current version.
fn compute_version(
    ctx: &Ctx,
    paper: &linxiv_core::models::PaperDetails,
    source_id: &str,
    version: Option<i64>,
) -> i64 {
    match version.filter(|&v| v != 0) {
        Some(v) if !stored_versions(ctx, source_id).contains(&v) => fail(format!(
            "Paper {} has no version {v} in DB",
            pyrepr(source_id)
        )),
        Some(v) => v,
        None => paper.version,
    }
}

pub async fn run(cmd: PdfCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        PdfCmd::Path { source_id, version } => {
            let source_id = as_source_id(&ctx.conn, &source_id);
            let paper = super::paper::resolve_paper_or_exit(ctx, &source_id);
            // Python `args.version if args.version else paper.version` — 0/None fall back.
            let version = version.filter(|&v| v != 0).unwrap_or(paper.version);
            let path = svc_files::pdf_path(
                &ctx.pdf_dir,
                &paper.source_id,
                version,
                paper.pdf_path.as_deref(),
            );
            output(&PdfLocation {
                source_id: paper.source_id,
                version,
                path,
            });
        }
        PdfCmd::Download {
            source_id,
            url,
            version,
        } => {
            let source_id = as_source_id(&ctx.conn, &source_id);
            let paper = super::paper::resolve_paper_or_exit(ctx, &source_id);
            let version = compute_version(ctx, &paper, &source_id, version);
            let max_pdf_bytes = ctx.settings.pdf_save_limit_bytes();
            let path = svc_files::download_pdf(
                &ctx.pdf_dir,
                &paper.source_id,
                version,
                &url,
                max_pdf_bytes,
            )
            .await
            .unwrap_or_else(|e| {
                eprintln!("[pdf] {e}");
                fail(e)
            });
            // Record the file like the MCP `download_pdf` tool does; without this the
            // paper stays has_pdf=0 and the PDF is invisible to `pdf list` and GET /api/pdfs.
            svc_paper::mark_pdf_saved(
                &mut ctx.conn,
                &paper.source_id,
                &path.to_string_lossy(),
                version,
            )?;
            output(&PdfLocation {
                source_id: paper.source_id,
                version,
                path: Some(path),
            });
        }
        // GET /api/pdfs: latest-version papers whose PDF is actually on disk,
        // largest first. Uncapped — the route's 200-row cap is for the UI list.
        PdfCmd::List => {
            let papers = svc_paper::list_pdf_papers(&ctx.conn)?;
            output(&json!({ "pdfs": svc_files::saved_pdf_sizes(&ctx.pdf_dir, papers) }));
        }

        // DELETE /api/pdfs/{source_id}: drop every version's local file, keeping
        // the paper row. `delete_pdf` refuses paths outside the managed dir.
        PdfCmd::Delete { source_id } => {
            let source_id = as_source_id(&ctx.conn, &source_id);
            if !svc_paper::delete_saved_pdfs(&ctx.conn, &ctx.pdf_dir, &source_id)? {
                fail("PDF is outside managed storage");
            }
            output(&json!({ "source_id": source_id, "deleted": true }));
        }

        PdfCmd::Storage => {
            let mb = svc_files::pdf_storage_mb(&ctx.pdf_dir);
            output(&StorageInfo {
                // Python `round(mb, 3)` is banker's rounding (half-to-even); this is
                // half-away-from-zero. Diverges only on exact 4th-decimal .5 ties.
                storage_mb: (mb * 1000.0).round() / 1000.0,
                pdf_dir: ctx.pdf_dir.clone(),
            });
        }
        PdfCmd::Import { file, project_id } => {
            // import_pdf applies the membership guards itself; any failure (read,
            // extract, guard) becomes the JSON error exit.
            let content = std::fs::read(&file).unwrap_or_else(|e| {
                eprintln!("[pdf-import] {e}");
                fail(e)
            });
            let data_dir = config::data_dir();
            let max_pdf_bytes = ctx.settings.pdf_save_limit_bytes();
            let result = paper_import::import_pdf_default(
                &mut ctx.conn,
                &ctx.pdf_dir,
                &content,
                project_id,
                max_pdf_bytes,
                &data_dir,
            )
            .await
            .unwrap_or_else(|e| {
                eprintln!("[pdf-import] {e}");
                fail(e)
            });
            output(&ImportedPdf {
                source_id: result.source_id,
                title: result.title,
            });
        }
    }
    Ok(())
}

/// Stored VERSIONs for a paper, empty when the root is absent. Used to reject an
/// explicit `--version` before anything is written to disk.
fn stored_versions(ctx: &Ctx, source_id: &str) -> Vec<i64> {
    svc_paper::get_all(
        &ctx.conn,
        &svc_paper::PaperRef::source(source_id.to_string()),
    )
    .ok()
    .flatten()
    .map(|all| all.versions.iter().map(|v| v.version).collect())
    .unwrap_or_default()
}
