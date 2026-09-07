//! Group `doi` — cmd_doi_* in `linxiv_cli.py`.

use clap::Subcommand;

use linxiv_core::models::DoiSaveResponse;
use linxiv_core::service::{paper as svc_paper, source as svc_source};

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
    let meta = svc_source::resolve_doi(doi).await.unwrap_or_else(|e| {
        eprintln!("[doi] {e}");
        fail(e)
    });
    match cmd {
        // cmd_doi_resolve: dump metadata.
        DoiCmd::Resolve { .. } => output(&meta),
        // cmd_doi_save: persist, then emit the route's envelope
        // (`POST /api/doi/save`): the resolved metadata + saved flag.
        DoiCmd::Save { .. } => {
            svc_paper::save_paper_metadata(&mut ctx.conn, &meta, None)?;
            output(&DoiSaveResponse {
                metadata: meta,
                saved: true,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the `doi save` envelope: `{metadata: <PaperMetadata>, saved: true}`,
    /// not the old `{source_id, version, title}` triple.
    #[test]
    fn doi_save_emits_route_envelope() {
        use serde_json::json;
        let meta: linxiv_core::models::PaperMetadata = serde_json::from_value(json!({
            "source_id": "doi:10.1000/xyz",
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-01",
            "summary": "S",
        }))
        .unwrap();
        let v = serde_json::to_value(DoiSaveResponse {
            metadata: meta,
            saved: true,
        })
        .unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["metadata", "saved"]);
        assert_eq!(v["saved"], json!(true));
        assert_eq!(v["metadata"]["source_id"], json!("doi:10.1000/xyz"));
    }
}
