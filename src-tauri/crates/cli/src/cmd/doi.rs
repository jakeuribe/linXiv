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
    let data_dir = config::data_dir();
    match cmd {
        // cmd_doi_resolve: resolve, dump metadata. The `[doi] {e}` prefix line +
        // error JSON mirror Python's two-line stderr on failure.
        DoiCmd::Resolve { doi } => {
            let meta = match resolve_doi(&doi, &data_dir).await {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[doi] {e}");
                    fail(e);
                }
            };
            output(&meta);
        }
        // cmd_doi_save: resolve, persist, then emit {source_id, version, title}.
        DoiCmd::Save { doi } => {
            let meta = match resolve_doi(&doi, &data_dir).await {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[doi] {e}");
                    fail(e);
                }
            };
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
