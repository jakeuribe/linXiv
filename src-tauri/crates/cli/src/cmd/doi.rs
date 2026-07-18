//! Group `doi` — cmd_doi_* in `linxiv_cli.py`.

use clap::Subcommand;
use serde_json::json;

use linxiv_core::config;
use linxiv_core::service::paper as svc_paper;
use linxiv_core::sources::doi_resolve::resolve_doi;

use crate::ctx::Ctx;
use crate::output::{fail, output};

#[derive(Subcommand)]
pub enum DoiCmd {
    /// Resolve DOI to metadata (no save)
    Resolve { doi: String },
    /// Resolve DOI and save paper to library
    Save { doi: String },
}

pub async fn run(cmd: DoiCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    let (DoiCmd::Resolve { doi } | DoiCmd::Save { doi }) = &cmd;
    // The `[doi] {e}` prefix line + error JSON mirror Python's two-line stderr on failure.
    let meta = resolve_doi(doi, &config::data_dir())
        .await
        .unwrap_or_else(|e| {
            eprintln!("[doi] {e}");
            fail(e)
        });
    match cmd {
        // cmd_doi_resolve: dump metadata.
        DoiCmd::Resolve { .. } => output(&meta),
        // cmd_doi_save: persist, then emit {source_id, version, title}.
        DoiCmd::Save { .. } => {
            let (source_id, version) = svc_paper::save_paper_metadata(&mut ctx.conn, &meta, None)?;
            output(&json!({
                "source_id": source_id,
                "version": version,
                "title": meta.title,
            }));
        }
    }
    Ok(())
}
