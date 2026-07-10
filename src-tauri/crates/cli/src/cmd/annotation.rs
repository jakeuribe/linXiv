//! Group `annotation` — PDF highlight CRUD, mirroring `cmd::note`.

use clap::Subcommand;
use serde::Serialize;

use linxiv_core::models::{AnnotationIn, AnnotationUpdateIn};
use linxiv_core::service::annotation as svc_ann;
use linxiv_core::service::annotation::{Annotation, Annotations};
use linxiv_core::storage::queries::paper as paper_q;

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

#[derive(Serialize)]
struct CreatedAnnotation {
    id: i64,
    source_fk: i64,
    project_id: Option<i64>,
}

#[derive(Serialize)]
struct UpdatedAnnotation {
    id: i64,
    updated: bool,
}

#[derive(Serialize)]
struct DeletedAnnotation {
    deleted_annotation_id: i64,
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
            let source_fk = match paper_q::get_paper_root(conn, &source_id)? {
                Some(root) => root.source_fk,
                None => fail(format!("Paper {source_id} not found in DB")),
            };
            let id = svc_ann::create(
                conn,
                &AnnotationIn {
                    source_fk,
                    anchor,
                    comment,
                    project_fk: project_id,
                },
            )?;
            output(&CreatedAnnotation {
                id,
                source_fk,
                project_id,
            });
        }
        AnnotationCmd::Get { annotation_id } => {
            match svc_ann::get(
                conn,
                &Annotation {
                    annotation_id: Some(annotation_id),
                },
            )? {
                Some(details) => output(&details),
                None => fail(format!("Annotation {annotation_id} not found")),
            }
        }
        AnnotationCmd::List {
            source_id,
            project_id,
        } => {
            let mut source_fk: Option<i64> = None;
            if let Some(raw) = source_id {
                let sid = as_source_id(&raw, "arxiv");
                match paper_q::get_paper_root(conn, &sid)? {
                    Some(root) => source_fk = Some(root.source_fk),
                    None => fail(format!(
                        "Paper {} not found in DB",
                        crate::output::pyrepr(&sid)
                    )),
                }
            }
            let annotations = if source_fk.is_none() && project_id.is_none() {
                svc_ann::list_all(conn)?
            } else {
                svc_ann::get_many(
                    conn,
                    &Annotations {
                        source_fk,
                        project_fk: project_id,
                        all_projects: source_fk.is_some() && project_id.is_none(),
                    },
                )?
            };
            output(&annotations);
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
            output(&UpdatedAnnotation {
                id: annotation_id,
                updated: true,
            });
        }
        AnnotationCmd::Delete { annotation_id } => {
            if !svc_ann::delete(
                conn,
                &Annotation {
                    annotation_id: Some(annotation_id),
                },
            )? {
                fail(format!("Annotation {annotation_id} not found"));
            }
            output(&DeletedAnnotation {
                deleted_annotation_id: annotation_id,
            });
        }
    }
    Ok(())
}
