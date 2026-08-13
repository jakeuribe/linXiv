//! Projects + tags tools cluster. Owned by the `projects_tags` Fill agent.
//!
//! Bodies use `self.with_conn(|conn| ...)` and call `linxiv_core::service::project`
//! and `::tag`. Replicate the Python dict shapes EXACTLY (field names + order),
//! e.g. delete returns `{"deleted": project_id}`; add/remove_paper_to_project
//! emit core's `PaperMembershipReceipt`. Map Python `ValueError` paths to
//! `Err(ErrorData::invalid_params(msg, None))` with the exact message string
//! Misses word themselves via the typed `CoreError` variants; status uses `{status:?}`.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use linxiv_core::error::CoreError;
use linxiv_core::models::{ProjectDetails, ProjectIn, ProjectUpdateIn, Status, TagIn};
use linxiv_core::service::paper::{self, Paper};
use linxiv_core::service::project::{self, Project, Projects};
use linxiv_core::service::tag::{self, Tag};

use crate::util::{core_err, guard_err};
use crate::Server;

/// Python `_SvcStatus(status)` — core owns the three-string parse and its message.
fn parse_status(s: &str) -> Result<Status, ErrorData> {
    s.parse::<Status>().map_err(guard_err)
}

/// Serialize a value into the tool's text response (compact JSON string).
fn jval(v: impl serde::Serialize) -> Result<String, ErrorData> {
    crate::util::json_ok(&v)
}

fn proj(id: i64) -> Project {
    Project {
        project_fk: Some(id),
    }
}

fn get_project(conn: &rusqlite::Connection, id: i64) -> Result<Option<ProjectDetails>, ErrorData> {
    project::get(conn, &proj(id)).map_err(core_err)
}

/// Existence guard — core's `get_required` owns the not-found wording.
fn ensure_project(conn: &rusqlite::Connection, id: i64) -> Result<(), ErrorData> {
    project::get_required(conn, id)
        .map(|_| ())
        .map_err(guard_err)
}

/// Shared body of add_paper_to_project / remove_paper_from_project — core's
/// receipt; misses word themselves via the typed variants' Display.
fn paper_membership(
    conn: &rusqlite::Connection,
    project_id: i64,
    paper_id: String,
    op: fn(&rusqlite::Connection, i64, &str) -> Result<project::PaperMembershipReceipt, CoreError>,
) -> Result<String, ErrorData> {
    match op(conn, project_id, &paper_id) {
        Ok(receipt) => jval(receipt),
        Err(e @ CoreError::PaperNotFound(_)) => Err(crate::util::guard_err(e)),
        Err(e @ CoreError::ProjectNotFound(_)) => Err(crate::util::guard_err(e)),
        Err(e) => Err(core_err(e)),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListProjectsParams {
    /// Filter by status — "active", "archived", or "deleted". Default: all non-deleted.
    #[serde(default)]
    pub status: Option<String>,
}

/// Tools that take only a numeric project id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectIdParams {
    /// Numeric project id.
    pub project_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateProjectParams {
    /// Project name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateProjectParams {
    /// Numeric project id.
    pub project_id: i64,
    /// New name (omit to leave unchanged).
    #[serde(default)]
    pub name: Option<String>,
    /// New description (omit to leave unchanged).
    #[serde(default)]
    pub description: Option<String>,
    /// New hex color, e.g. "#4f86f7" (omit to leave unchanged).
    #[serde(default)]
    pub color: Option<String>,
    /// Replacement project tag list (omit to leave unchanged; [] clears all).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// New lifecycle status — "active", "archived", or "deleted".
    #[serde(default)]
    pub status: Option<String>,
}

/// Tools that take a project id plus a paper source id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectPaperParams {
    /// Numeric project id.
    pub project_id: i64,
    /// Paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
}

/// Tools that take a project id plus a list of tag labels.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectTagsParams {
    /// Numeric project id.
    pub project_id: i64,
    /// List of tag labels.
    pub tags: Vec<String>,
}

/// Tools that take only a paper source id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaperIdParams {
    /// The paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
}

/// Tools that take a paper source id plus a list of tag labels.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaperTagsParams {
    /// The paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
    /// List of tag labels.
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectPapersParams {
    /// Numeric project id.
    pub project_id: i64,
    /// Paper source ids to add (e.g. ["arxiv:2204.12985"]).
    pub paper_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateTagParams {
    /// Tag label text.
    pub label: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTagParams {
    /// Numeric tag id (from create_tag or list_all_tags).
    pub tag_id: i64,
}

#[tool_router(router = tools_projects_tags, vis = "pub(crate)")]
impl Server {
    #[tool(description = "List research projects.")]
    pub async fn list_projects(
        &self,
        _params: Parameters<ListProjectsParams>,
    ) -> Result<String, ErrorData> {
        let status = _params.0.status;
        self.with_conn(|conn| {
            let projects = match status {
                Some(s) => {
                    let st = parse_status(&s)?;
                    project::get_many(
                        conn,
                        &Projects {
                            status: Some(st),
                            ..Default::default()
                        },
                    )
                    .map_err(core_err)?
                }
                None => project::get_many(conn, &Projects::default())
                    .map_err(core_err)?
                    .into_iter()
                    .filter(|p| p.status != Status::Deleted)
                    .collect(),
            };
            let out = projects
                .into_iter()
                .map(|p| project::to_out(conn, p))
                .collect::<Result<Vec<_>, _>>()
                .map_err(core_err)?;
            jval(out)
        })
    }

    #[tool(description = "Get full details for a project.")]
    pub async fn get_project(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        // Not-found is a tool ERROR (not JSON null), worded by CoreError like
        // the route's 404 and the CLI's exit-1 body.
        self.with_conn(|conn| {
            let d = project::get_required(conn, id).map_err(crate::util::guard_err)?;
            jval(project::to_out(conn, d).map_err(core_err)?)
        })
    }

    #[tool(description = "Create a new research project.")]
    pub async fn create_project(
        &self,
        _params: Parameters<CreateProjectParams>,
    ) -> Result<String, ErrorData> {
        let CreateProjectParams { name, description } = _params.0;
        self.with_conn(|conn| {
            let pin = ProjectIn {
                name: name.clone(),
                description,
                color: None,
                tags: Vec::new(),
                source_fks: Vec::new(),
            };
            let fk = project::create(conn, &pin).map_err(core_err)?;
            match get_project(conn, fk)? {
                Some(d) => jval(project::to_out(conn, d).map_err(core_err)?),
                None => jval(json!({ "id": fk, "name": name })),
            }
        })
    }

    #[tool(description = "Update a project's name, description, color, tags, or lifecycle status.")]
    pub async fn update_project(
        &self,
        _params: Parameters<UpdateProjectParams>,
    ) -> Result<String, ErrorData> {
        let UpdateProjectParams {
            project_id,
            name,
            description,
            color,
            tags,
            status,
        } = _params.0;
        self.with_conn(|conn| {
            ensure_project(conn, project_id)?;
            // color: outer None = unchanged (Python UNSET); Some(hex) = set the parsed value.
            let color = color
                .map(|hex| project::color_from_hex(&hex).map(Some))
                .transpose()
                .map_err(core_err)?;
            let status = status.as_deref().map(parse_status).transpose()?;
            let upd = ProjectUpdateIn {
                project_fk: project_id,
                name,
                description,
                color,
                project_tags: tags,
                status,
            };
            project::update(conn, &upd).map_err(core_err)?;
            match get_project(conn, project_id)? {
                Some(d) => jval(project::to_out(conn, d).map_err(core_err)?),
                None => jval(json!({})),
            }
        })
    }

    #[tool(description = "Soft-delete a project (moves it to trash).")]
    pub async fn delete_project(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| {
            ensure_project(conn, id)?;
            project::delete(conn, &proj(id)).map_err(core_err)?;
            jval(json!({ "deleted": id }))
        })
    }

    #[tool(description = "Add a paper to an existing project.")]
    pub async fn add_paper_to_project(
        &self,
        _params: Parameters<ProjectPaperParams>,
    ) -> Result<String, ErrorData> {
        let ProjectPaperParams {
            project_id,
            paper_id,
        } = _params.0;
        self.with_conn(|conn| paper_membership(conn, project_id, paper_id, project::add_paper))
    }

    #[tool(
        description = "Add several papers to a project in one call. Ids that are not in the local \
                       database come back in `failed` instead of failing the whole call."
    )]
    pub async fn add_papers_to_project(
        &self,
        _params: Parameters<ProjectPapersParams>,
    ) -> Result<String, ErrorData> {
        let ProjectPapersParams {
            project_id,
            paper_ids,
        } = _params.0;
        // `failed` comes back deduped, so `added` is derived from a deduped list too —
        // otherwise a repeated id is reported added twice and won't match paper_count.
        let mut seen = std::collections::HashSet::new();
        let paper_ids: Vec<String> = paper_ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty() && seen.insert(id.clone()))
            .collect();
        self.with_conn(|conn| {
            let failed = match project::add_papers(conn, project_id, &paper_ids) {
                Ok(f) => f,
                Err(e @ CoreError::ProjectNotFound(_)) => return Err(crate::util::guard_err(e)),
                Err(e @ CoreError::ProjectDeleted(_)) => {
                    return Err(ErrorData::invalid_params(e.to_string(), None))
                }
                Err(e) => return Err(core_err(e)),
            };
            let added: Vec<&String> = paper_ids.iter().filter(|id| !failed.contains(id)).collect();
            let count = get_project(conn, project_id)?
                .ok_or_else(|| crate::util::guard_err(CoreError::ProjectNotFound(project_id)))?
                .source_fks
                .len();
            jval(json!({
                "project_id": project_id,
                "ok": failed.is_empty(),
                "added": added,
                "failed": failed,
                "paper_count": count,
            }))
        })
    }

    #[tool(description = "Remove a paper from a project.")]
    pub async fn remove_paper_from_project(
        &self,
        _params: Parameters<ProjectPaperParams>,
    ) -> Result<String, ErrorData> {
        let ProjectPaperParams {
            project_id,
            paper_id,
        } = _params.0;
        self.with_conn(|conn| paper_membership(conn, project_id, paper_id, project::remove_paper))
    }

    #[tool(description = "Archive a project (read-only, still visible).")]
    pub async fn archive_project(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| {
            ensure_project(conn, id)?;
            project::archive(conn, &proj(id)).map_err(core_err)?;
            jval(json!({ "archived_project_id": id }))
        })
    }

    #[tool(description = "Restore an archived or soft-deleted project back to active status.")]
    pub async fn restore_project(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| {
            ensure_project(conn, id)?;
            project::restore(conn, &proj(id)).map_err(core_err)?;
            jval(linxiv_core::service::trash::RestoredProject {
                ok: true,
                restored_project_id: id,
            })
        })
    }

    #[tool(description = "Permanently delete a project. Irreversible. Papers themselves are kept.")]
    pub async fn hard_delete_project(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| {
            ensure_project(conn, id)?;
            project::hard_delete(conn, &proj(id)).map_err(core_err)?;
            jval(linxiv_core::service::trash::HardDeletedProject {
                ok: true,
                hard_deleted_project_id: id,
            })
        })
    }

    #[tool(description = "Add one or more tags to a project.")]
    pub async fn add_tags_to_project(
        &self,
        _params: Parameters<ProjectTagsParams>,
    ) -> Result<String, ErrorData> {
        let ProjectTagsParams { project_id, tags } = _params.0;
        self.with_conn(|conn| {
            let updated = project::add_project_tags(conn, project_id, &tags).map_err(guard_err)?;
            jval(json!({ "project_id": project_id, "tags": updated }))
        })
    }

    #[tool(description = "Remove one or more tags from a project.")]
    pub async fn remove_tags_from_project(
        &self,
        _params: Parameters<ProjectTagsParams>,
    ) -> Result<String, ErrorData> {
        let ProjectTagsParams { project_id, tags } = _params.0;
        self.with_conn(|conn| {
            let updated =
                project::remove_project_tags(conn, project_id, &tags).map_err(guard_err)?;
            jval(json!({ "project_id": project_id, "tags": updated }))
        })
    }

    #[tool(description = "Get all tags applied to a project.")]
    pub async fn get_project_tags(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| match get_project(conn, id)? {
            Some(d) => jval(json!({ "project_id": id, "tags": d.project_tags })),
            None => Err(crate::util::guard_err(CoreError::ProjectNotFound(id))),
        })
    }

    #[tool(description = "List all tags in the database.")]
    pub async fn list_all_tags(&self) -> Result<String, ErrorData> {
        self.with_conn(|conn| jval(tag::list_all_tags(conn).map_err(core_err)?))
    }

    #[tool(description = "Get all tags applied to a specific paper.")]
    pub async fn get_paper_tags(
        &self,
        _params: Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        let paper_id = _params.0.paper_id;
        self.with_conn(|conn| {
            // Python `get_paper_tags` returns [] for an absent paper (no error).
            let tags = paper::get(
                conn,
                &Paper {
                    source_id: Some(paper_id.clone()),
                    ..Default::default()
                },
            )
            .map_err(core_err)?
            .map(|p| p.tags)
            .unwrap_or_default();
            jval(json!({ "paper_id": paper_id, "tags": tags }))
        })
    }

    #[tool(description = "Add one or more tags to a paper.")]
    pub async fn add_tags_to_paper(
        &self,
        _params: Parameters<PaperTagsParams>,
    ) -> Result<String, ErrorData> {
        let PaperTagsParams { paper_id, tags } = _params.0;
        self.with_conn(|conn| {
            let updated = paper::add_paper_tags(conn, &paper_id, &tags).map_err(guard_err)?;
            jval(json!({ "paper_id": paper_id, "tags": updated }))
        })
    }

    #[tool(description = "Remove one or more tags from a paper.")]
    pub async fn remove_tags_from_paper(
        &self,
        _params: Parameters<PaperTagsParams>,
    ) -> Result<String, ErrorData> {
        let PaperTagsParams { paper_id, tags } = _params.0;
        self.with_conn(|conn| {
            let updated = paper::remove_paper_tags(conn, &paper_id, &tags).map_err(guard_err)?;
            jval(json!({ "paper_id": paper_id, "tags": updated }))
        })
    }

    #[tool(description = "Create a new tag (or return its id if it already exists).")]
    pub async fn create_tag(
        &self,
        _params: Parameters<CreateTagParams>,
    ) -> Result<String, ErrorData> {
        let label = _params.0.label;
        self.with_conn(|conn| {
            let tag_id = tag::upsert(
                conn,
                &TagIn {
                    label: label.clone(),
                },
            )
            .map_err(core_err)?;
            if tag_id < 0 {
                return Err(ErrorData::internal_error(
                    format!("Failed to create or locate tag {label:?}."),
                    None,
                ));
            }
            jval(json!({ "tag_id": tag_id, "label": label }))
        })
    }

    #[tool(description = "Delete a tag by its id.")]
    pub async fn delete_tag(
        &self,
        _params: Parameters<DeleteTagParams>,
    ) -> Result<String, ErrorData> {
        let tag_id = _params.0.tag_id;
        self.with_conn(|conn| {
            if tag::get(
                conn,
                &Tag {
                    tag_id: Some(tag_id),
                    ..Default::default()
                },
            )
            .map_err(core_err)?
            .is_none()
            {
                return Err(ErrorData::invalid_params(
                    format!("Tag {tag_id} not found."),
                    None,
                ));
            }
            tag::delete(
                conn,
                &Tag {
                    tag_id: Some(tag_id),
                    ..Default::default()
                },
            )
            .map_err(core_err)?;
            jval(json!({ "deleted": tag_id }))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use linxiv_core::models::PaperMetadata;
    use linxiv_core::storage;

    use super::*;

    /// Mirrors `papers.rs`'s test `server()`: an in-memory DB, tool methods
    /// called directly rather than dispatched through `tool_router`.
    fn server() -> Server {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        Server {
            conn: Arc::new(Mutex::new(conn)),
            pdf_dir: std::env::temp_dir(),
            tool_router: Server::tools_projects_tags(),
        }
    }

    /// One known id is linked while an unknown one is reported in `failed`,
    /// instead of the whole call failing.
    #[tokio::test]
    async fn bulk_add_reports_partial_success() {
        let srv = server();
        let meta: PaperMetadata = serde_json::from_value(json!({
            "source_id": "arxiv:1",
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-01",
            "summary": "S",
        }))
        .unwrap();
        srv.with_conn(|conn| paper::save_paper_metadata(conn, &meta, None))
            .unwrap();
        let project_id = srv
            .with_conn(|conn| {
                project::create(
                    conn,
                    &ProjectIn {
                        name: "P".to_string(),
                        description: String::new(),
                        color: None,
                        tags: Vec::new(),
                        source_fks: Vec::new(),
                    },
                )
            })
            .unwrap();

        let out = srv
            .add_papers_to_project(Parameters(ProjectPapersParams {
                project_id,
                paper_ids: vec!["arxiv:1".to_string(), "arxiv:nope".to_string()],
            }))
            .await
            .unwrap();
        let out: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(out["ok"], json!(false));
        assert_eq!(out["added"], json!(["arxiv:1"]));
        assert_eq!(out["failed"], json!(["arxiv:nope"]));
        assert_eq!(out["paper_count"], json!(1));

        let err = srv
            .add_papers_to_project(Parameters(ProjectPapersParams {
                project_id: 999,
                paper_ids: vec!["arxiv:1".to_string()],
            }))
            .await
            .unwrap_err();
        assert_eq!(err.message.as_ref(), "Project 999 not found");
    }
}
