//! PDF annotation tools cluster. Mirrors `notes_pdf_trash.rs`: an annotation is a
//! highlight (opaque ANCHOR JSON) plus an optional written comment, attached to a
//! paper by source id and optionally scoped to a project.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use linxiv_core::error::CoreError;
use linxiv_core::models::{AnnotationIn, AnnotationUpdateIn};
use linxiv_core::service::annotation as svc_ann;
use linxiv_core::storage::queries::paper as store_paper;

use crate::Server;

fn invalid(msg: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(msg.into(), None)
}

fn core_err(e: CoreError) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

fn json_ok<T: Serialize>(v: &T) -> Result<String, ErrorData> {
    serde_json::to_string(v).map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

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
            let root = store_paper::get_paper_root(conn, &p.paper_id).map_err(core_err)?;
            let source_fk = match root {
                Some(r) => r.source_fk,
                None => {
                    return Err(invalid(format!(
                        "Paper {} not found. Run fetch_paper first.",
                        crate::util::pyrepr(&p.paper_id)
                    )))
                }
            };
            let id = svc_ann::create(
                conn,
                &AnnotationIn {
                    source_fk,
                    anchor: p.anchor.clone(),
                    comment: p.comment.clone(),
                    project_fk: p.project_id,
                },
            )
            .map_err(|e| match e {
                CoreError::Validation(m) => invalid(m),
                other => core_err(other),
            })?;
            match svc_ann::get(
                conn,
                &svc_ann::Annotation {
                    annotation_id: Some(id),
                },
            )
            .map_err(core_err)?
            {
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
        self.with_conn(|conn| {
            match svc_ann::get(
                conn,
                &svc_ann::Annotation {
                    annotation_id: Some(p.annotation_id),
                },
            )
            .map_err(core_err)?
            {
                Some(a) => json_ok(&a),
                None => json_ok(&Value::Null),
            }
        })
    }

    #[tool(description = "List PDF annotations, optionally filtered by paper or project.")]
    pub async fn list_annotations(
        &self,
        Parameters(p): Parameters<ListAnnotationsParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let annotations = if p.paper_id.is_none() && p.project_id.is_none() {
                svc_ann::list_all(conn).map_err(core_err)?
            } else {
                let source_fk = match &p.paper_id {
                    Some(pid) => match store_paper::get_paper_root(conn, pid).map_err(core_err)? {
                        Some(r) => Some(r.source_fk),
                        None => {
                            return Err(invalid(format!(
                                "Paper {} not found in database.",
                                crate::util::pyrepr(pid)
                            )))
                        }
                    },
                    None => None,
                };
                svc_ann::get_many(
                    conn,
                    &svc_ann::Annotations {
                        source_fk,
                        project_fk: p.project_id,
                        all_projects: p.paper_id.is_some() && p.project_id.is_none(),
                    },
                )
                .map_err(core_err)?
            };
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
            match svc_ann::get(
                conn,
                &svc_ann::Annotation {
                    annotation_id: Some(p.annotation_id),
                },
            )
            .map_err(core_err)?
            {
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
            if !svc_ann::delete(
                conn,
                &svc_ann::Annotation {
                    annotation_id: Some(p.annotation_id),
                },
            )
            .map_err(core_err)?
            {
                return Err(invalid(format!(
                    "Annotation {} not found.",
                    p.annotation_id
                )));
            }
            json_ok(&json!({ "deleted": p.annotation_id }))
        })
    }
}
