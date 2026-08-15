//! PDF annotation tools cluster. Mirrors `notes_pdf_trash.rs`: an annotation is a
//! highlight (opaque ANCHOR JSON) plus an optional written comment, attached to a
//! paper by source id and optionally scoped to a project.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use linxiv_core::error::CoreError;
use linxiv_core::models::{AnnotationIn, AnnotationUpdateIn};
use linxiv_core::service::annotation as svc_ann;
use linxiv_core::service::paper as svc_paper;

use crate::util::{core_err, guard_err, invalid, json_ok};
use crate::Server;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateAnnotationParams {
    /// Paper source id the annotation is attached to (e.g. "arxiv:2204.12985").
    pub paper_id: String,
    /// Opaque highlight anchor JSON ({v,version,page,color,quote,rects}).
    pub anchor: String,
    /// Optional written comment ("" = highlight only).
    #[serde(default)]
    pub comment: String,
    /// Associate the annotation with a specific project.
    #[serde(default)]
    pub project_id: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnnotationIdParams {
    /// Numeric annotation id.
    pub annotation_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAnnotationsParams {
    /// Filter by paper source id (e.g. "arxiv:2204.12985").
    #[serde(default)]
    pub paper_id: Option<String>,
    /// Filter by project id.
    #[serde(default)]
    pub project_id: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateAnnotationParams {
    /// Numeric annotation id.
    pub annotation_id: i64,
    /// New comment text.
    pub comment: String,
}

#[tool_router(router = tools_annotations, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "Create a PDF highlight annotation on a paper, optionally scoped to a project."
    )]
    pub async fn create_annotation(
        &self,
        Parameters(p): Parameters<CreateAnnotationParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let source_fk =
                svc_paper::resolve_source_fk(conn, &p.paper_id).map_err(|e| match e {
                    e @ CoreError::PaperNotFound(_) => {
                        invalid(format!("{e}. Run fetch_paper first."))
                    }
                    other => core_err(other),
                })?;
            let id = svc_ann::create(
                conn,
                &AnnotationIn {
                    source_fk,
                    anchor: p.anchor.clone(),
                    comment: p.comment.clone(),
                    project_fk: p.project_id,
                    uuid: None,
                },
            )
            .map_err(|e| match e {
                CoreError::Validation(m) => invalid(m),
                other => core_err(other),
            })?;
            match svc_ann::get(conn, id).map_err(core_err)? {
                Some(a) => json_ok(&a),
                None => json_ok(&json!({ "id": id })),
            }
        })
    }

    #[tool(description = "Get a single annotation by its id.")]
    pub async fn get_annotation(
        &self,
        Parameters(p): Parameters<AnnotationIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| json_ok(&svc_ann::get(conn, p.annotation_id).map_err(core_err)?))
    }

    #[tool(description = "List PDF annotations, optionally filtered by paper or project.")]
    pub async fn list_annotations(
        &self,
        Parameters(p): Parameters<ListAnnotationsParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let source_fk = match &p.paper_id {
                Some(pid) => Some(svc_paper::resolve_source_fk(conn, pid).map_err(guard_err)?),
                None => None,
            };
            let annotations =
                svc_ann::list_filtered(conn, source_fk, p.project_id).map_err(core_err)?;
            json_ok(&annotations)
        })
    }

    #[tool(description = "Update a PDF annotation's written comment.")]
    pub async fn update_annotation(
        &self,
        Parameters(p): Parameters<UpdateAnnotationParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let ok = svc_ann::update(
                conn,
                &AnnotationUpdateIn {
                    annotation_id: p.annotation_id,
                    comment: p.comment.clone(),
                },
            )
            .map_err(core_err)?;
            if !ok {
                return Err(invalid(format!(
                    "Annotation {} not found.",
                    p.annotation_id
                )));
            }
            match svc_ann::get(conn, p.annotation_id).map_err(core_err)? {
                Some(a) => json_ok(&a),
                None => json_ok(&json!({})),
            }
        })
    }

    #[tool(description = "Delete a PDF annotation by its id.")]
    pub async fn delete_annotation(
        &self,
        Parameters(p): Parameters<AnnotationIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            if !svc_ann::delete(conn, p.annotation_id).map_err(core_err)? {
                return Err(invalid(format!(
                    "Annotation {} not found.",
                    p.annotation_id
                )));
            }
            json_ok(&json!({ "deleted": p.annotation_id }))
        })
    }
}
