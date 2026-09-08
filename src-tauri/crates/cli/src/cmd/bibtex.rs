//! Group `bibtex`. The whole operation (guard, parse, save, link) lives in
//! `service::paper_import::import_bibtex`; this is a thin call site.

use clap::Subcommand;

use linxiv_core::service::paper_import;

use crate::ctx::Ctx;
use crate::output::{fail, output};

#[derive(Subcommand)]
pub enum BibtexCmd {
    /// Import papers from a .bib file
    Import {
        /// Path to .bib file
        file: String,
        /// Link imported papers to a project
        #[arg(long = "project-id")]
        project_id: Option<i64>,
    },
}

pub async fn run(cmd: BibtexCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        BibtexCmd::Import { file, project_id } => {
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) => {
                    // Two-line stderr on failure: `[bibtex-import] {e}` then the error JSON.
                    eprintln!("[bibtex-import] {e}");
                    fail(e);
                }
            };
            match paper_import::import_bibtex(&mut ctx.conn, &text, project_id) {
                Ok(receipt) => output(&receipt),
                Err(e) => fail(e),
            }
        }
    }
    Ok(())
}
