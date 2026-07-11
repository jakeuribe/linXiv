//! Group `pdf` — cmd_pdf_* in `linxiv_cli.py`.

use std::path::PathBuf;

use clap::Subcommand;
use serde::Serialize;

use linxiv_core::config;
use linxiv_core::service::files as svc_files;
use linxiv_core::service::paper_import;

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

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

/// `cmd_pdf_path` / `cmd_pdf_download` output dict.
#[derive(Serialize)]
struct PdfLocation {
    source_id: String,
    version: i64,
    path: Option<PathBuf>,
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

pub async fn run(cmd: PdfCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        PdfCmd::Path { source_id, version } => {
            let source_id = as_source_id(&source_id, "arxiv");
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
            let source_id = as_source_id(&source_id, "arxiv");
            let paper = super::paper::resolve_paper_or_exit(ctx, &source_id);
            let version = version.filter(|&v| v != 0).unwrap_or(paper.version);
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
            output(&PdfLocation {
                source_id: paper.source_id,
                version,
                path: Some(path),
            });
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
