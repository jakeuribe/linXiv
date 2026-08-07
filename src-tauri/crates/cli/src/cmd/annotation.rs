//! Group `annotation` — PDF highlight CRUD, mirroring `cmd::note`.

use clap::Subcommand;
use serde_json::json;

use linxiv_core::models::{AnnotationIn, AnnotationUpdateIn};
use linxiv_core::service::annotation as svc_ann;
use linxiv_core::service::paper as svc_paper;

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

#[derive(Subcommand)]
pub enum AnnotationCmd {
    /// Create a PDF highlight annotation on a paper
    Create {
        source_id: String,
        /// Opaque highlight anchor JSON ({v,version,page,color,quote,rects})
        anchor: String,
        /// Written comment ("" = highlight only)
        #[arg(long, default_value = "")]
        comment: String,
        /// Associate the annotation with a project
        #[arg(long = "project-id")]
        project_id: Option<i64>,
    },
    /// Get an annotation by ID
    Get { annotation_id: i64 },
    /// List annotations
    List {
        /// Filter by paper source ID
        #[arg(long = "paper-id")]
        source_id: Option<String>,
        /// Filter by project ID
        #[arg(long = "project-id")]
        project_id: Option<i64>,
    },
    /// Update an annotation's comment
    Update {
        annotation_id: i64,
        #[arg(long)]
        comment: String,
    },
    /// Delete an annotation by ID
    Delete { annotation_id: i64 },
}

pub async fn run(cmd: AnnotationCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    let conn = &ctx.conn;
    match cmd {
        AnnotationCmd::Create {
            source_id,
            anchor,
            comment,
            project_id,
        } => {
            let source_id = as_source_id(&source_id, "arxiv");
            let source_fk = svc_paper::resolve_source_fk(conn, &source_id)?;
            let id = svc_ann::create(
                conn,
                &AnnotationIn {
                    source_fk,
                    anchor,
                    comment,
                    project_fk: project_id,
                    uuid: None,
                },
            )?;
            output(&json!({ "id": id, "source_fk": source_fk, "project_id": project_id }));
        }
        AnnotationCmd::Get { annotation_id } => match svc_ann::get(conn, annotation_id)? {
            Some(details) => output(&details),
            None => fail(format!("Annotation {annotation_id} not found")),
        },
        AnnotationCmd::List {
            source_id,
            project_id,
        } => {
            let source_fk = crate::output::resolve_source_fk(conn, source_id)?;
            output(&svc_ann::list_filtered(conn, source_fk, project_id)?);
        }
        AnnotationCmd::Update {
            annotation_id,
            comment,
        } => {
            if !svc_ann::update(
                conn,
                &AnnotationUpdateIn {
                    annotation_id,
                    comment,
                },
            )? {
                fail(format!("Annotation {annotation_id} not found"));
            }
            output(&json!({ "id": annotation_id, "updated": true }));
        }
        AnnotationCmd::Delete { annotation_id } => {
            if !svc_ann::delete(conn, annotation_id)? {
                fail(format!("Annotation {annotation_id} not found"));
            }
            output(&json!({ "deleted_annotation_id": annotation_id }));
        }
    }
    Ok(())
}
