//! Projects + tags tools cluster. Owned by the `projects_tags` Fill agent.
//!
//! Bodies use `self.with_conn(|conn| ...)` and call `linxiv_core::service::project`
//! and `::tag`. Replicate the Python dict shapes EXACTLY (field names + order),
//! e.g. delete returns `{"deleted": project_id}`, add_paper_to_project returns
//! `{"project_id", "paper_id", "paper_count"}`. Map Python `ValueError` paths to
//! `Err(ErrorData::invalid_params(msg, None))` with the exact message string
//! (e.g. `format!("Project {project_id} not found.")`, status uses `{status:?}`).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::error::CoreError;
use linxiv_core::models::{ProjectIn, ProjectUpdateIn, Status, TagIn};
use linxiv_core::service::paper::{self, Paper};
use linxiv_core::service::project::{self, Project, Projects};
use linxiv_core::service::tag::{self, Tag};
use linxiv_core::storage::queries::{paper as paperq, tag as tagq};

use crate::Server;

/// Any `CoreError` that isn't an expected validation/not-found path becomes a
/// JSON-RPC internal error (its `Display` is the message).
fn core_err(e: CoreError) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

/// Python `f"Project {id} not found."` — a bare int, no quoting.
fn project_not_found(id: i64) -> ErrorData {
    ErrorData::invalid_params(format!("Project {id} not found."), None)
}

/// Python `_SvcStatus(status)` — the three lifecycle strings, else a `ValueError`.
fn parse_status(s: &str) -> Result<Status, ErrorData> {
    match s {
        "active" => Ok(Status::Active),
        "archived" => Ok(Status::Archived),
        "deleted" => Ok(Status::Deleted),
        _ => Err(ErrorData::invalid_params(
            format!("Invalid status {s:?}. Use 'active', 'archived', or 'deleted'."),
            None,
        )),
    }
}

/// Serialize a value into the tool's text response (compact JSON string).
fn jval(v: impl serde::Serialize) -> Result<String, ErrorData> {
    serde_json::to_string(&v).map_err(|e| ErrorData::internal_error(e.to_string(), None))
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
                    project::get_many(conn, &Projects { status: Some(st), ..Default::default() })
                        .map_err(core_err)?
                }
                None => project::get_many(conn, &Projects::default())
                    .map_err(core_err)?
                    .into_iter()
                    .filter(|p| p.status != Status::Deleted)
                    .collect(),
            };
            jval(projects)
        })
    }

    #[tool(description = "Get full details for a project.")]
    pub async fn get_project(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| {
            match project::get(conn, &Project { project_fk: Some(id) }).map_err(core_err)? {
                Some(d) => jval(d),
                None => jval(Value::Null),
            }
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
            match project::get(conn, &Project { project_fk: Some(fk) }).map_err(core_err)? {
                Some(d) => jval(d),
                None => jval(json!({ "id": fk, "name": name })),
            }
        })
    }

    #[tool(description = "Update a project's name, description, color, tags, or lifecycle status.")]
    pub async fn update_project(
        &self,
        _params: Parameters<UpdateProjectParams>,
    ) -> Result<String, ErrorData> {
        let UpdateProjectParams { project_id, name, description, color, tags, status } = _params.0;
        self.with_conn(|conn| {
            if project::get(conn, &Project { project_fk: Some(project_id) })
                .map_err(core_err)?
                .is_none()
            {
                return Err(project_not_found(project_id));
            }
            // color: outer None = unchanged (Python UNSET); Some(hex) = set the parsed value.
            let color = match color {
                Some(hex) => Some(Some(project::color_from_hex(&hex).map_err(core_err)?)),
                None => None,
            };
            let status = match status {
                Some(s) => Some(parse_status(&s)?),
                None => None,
            };
            let upd = ProjectUpdateIn {
                project_fk: project_id,
                name,
                description,
                color,
                project_tags: tags,
                status,
            };
            project::update(conn, &upd).map_err(core_err)?;
            match project::get(conn, &Project { project_fk: Some(project_id) }).map_err(core_err)? {
                Some(d) => jval(d),
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
            if project::get(conn, &Project { project_fk: Some(id) }).map_err(core_err)?.is_none() {
                return Err(project_not_found(id));
            }
            project::delete(conn, &Project { project_fk: Some(id) }).map_err(core_err)?;
            jval(json!({ "deleted": id }))
        })
    }

    #[tool(description = "Add a paper to an existing project.")]
    pub async fn add_paper_to_project(
        &self,
        _params: Parameters<ProjectPaperParams>,
    ) -> Result<String, ErrorData> {
        let ProjectPaperParams { project_id, paper_id } = _params.0;
        self.with_conn(|conn| {
            let failed = match project::add_papers(conn, project_id, &[paper_id.clone()]) {
                Ok(f) => f,
                Err(CoreError::ProjectNotFound) => return Err(project_not_found(project_id)),
                Err(e) => return Err(core_err(e)),
            };
            if !failed.is_empty() {
                return Err(ErrorData::invalid_params(
                    format!("Paper {paper_id:?} not found in database."),
                    None,
                ));
            }
            let count = project::get(conn, &Project { project_fk: Some(project_id) })
                .map_err(core_err)?
                .ok_or_else(|| project_not_found(project_id))?
                .source_fks
                .len();
            jval(json!({ "project_id": project_id, "paper_id": paper_id, "paper_count": count }))
        })
    }

    #[tool(description = "Remove a paper from a project.")]
    pub async fn remove_paper_from_project(
        &self,
        _params: Parameters<ProjectPaperParams>,
    ) -> Result<String, ErrorData> {
        let ProjectPaperParams { project_id, paper_id } = _params.0;
        self.with_conn(|conn| {
            let failed = match project::remove_papers(conn, project_id, &[paper_id.clone()]) {
                Ok(f) => f,
                Err(CoreError::ProjectNotFound) => return Err(project_not_found(project_id)),
                Err(e) => return Err(core_err(e)),
            };
            if !failed.is_empty() {
                return Err(ErrorData::invalid_params(
                    format!("Paper {paper_id:?} not found in database."),
                    None,
                ));
            }
            let count = project::get(conn, &Project { project_fk: Some(project_id) })
                .map_err(core_err)?
                .ok_or_else(|| project_not_found(project_id))?
                .source_fks
                .len();
            jval(json!({ "project_id": project_id, "paper_id": paper_id, "paper_count": count }))
        })
    }

    #[tool(description = "Archive a project (read-only, still visible).")]
    pub async fn archive_project(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| {
            if project::get(conn, &Project { project_fk: Some(id) }).map_err(core_err)?.is_none() {
                return Err(project_not_found(id));
            }
            project::archive(conn, &Project { project_fk: Some(id) }).map_err(core_err)?;
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
            if project::get(conn, &Project { project_fk: Some(id) }).map_err(core_err)?.is_none() {
                return Err(project_not_found(id));
            }
            project::restore(conn, &Project { project_fk: Some(id) }).map_err(core_err)?;
            jval(json!({ "restored_project_id": id }))
        })
    }

    #[tool(description = "Permanently delete a project. Irreversible. Papers themselves are kept.")]
    pub async fn hard_delete_project(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| {
            if project::get(conn, &Project { project_fk: Some(id) }).map_err(core_err)?.is_none() {
                return Err(project_not_found(id));
            }
            project::hard_delete(conn, &Project { project_fk: Some(id) }).map_err(core_err)?;
            jval(json!({ "hard_deleted_project_id": id }))
        })
    }

    #[tool(description = "Add one or more tags to a project.")]
    pub async fn add_tags_to_project(
        &self,
        _params: Parameters<ProjectTagsParams>,
    ) -> Result<String, ErrorData> {
        let ProjectTagsParams { project_id, tags } = _params.0;
        self.with_conn(|conn| {
            if project::get(conn, &Project { project_fk: Some(project_id) })
                .map_err(core_err)?
                .is_none()
            {
                return Err(project_not_found(project_id));
            }
            let updated = tagq::add_project_tags(conn, project_id, &tags).map_err(core_err)?;
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
            if project::get(conn, &Project { project_fk: Some(project_id) })
                .map_err(core_err)?
                .is_none()
            {
                return Err(project_not_found(project_id));
            }
            let updated = tagq::remove_project_tags(conn, project_id, &tags).map_err(core_err)?;
            jval(json!({ "project_id": project_id, "tags": updated }))
        })
    }

    #[tool(description = "Get all tags applied to a project.")]
    pub async fn get_project_tags(
        &self,
        _params: Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        let id = _params.0.project_id;
        self.with_conn(|conn| {
            match project::get(conn, &Project { project_fk: Some(id) }).map_err(core_err)? {
                Some(d) => jval(json!({ "project_id": id, "tags": d.project_tags })),
                None => Err(project_not_found(id)),
            }
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
            let tags = paper::get(conn, &Paper { source_id: Some(paper_id.clone()), ..Default::default() })
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
            if paper::get(conn, &Paper { source_id: Some(paper_id.clone()), ..Default::default() })
                .map_err(core_err)?
                .is_none()
            {
                return Err(ErrorData::invalid_params(
                    format!("Paper {paper_id:?} not found in database."),
                    None,
                ));
            }
            let updated = paperq::add_paper_tags(conn, &paper_id, &tags).map_err(core_err)?;
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
            if paper::get(conn, &Paper { source_id: Some(paper_id.clone()), ..Default::default() })
                .map_err(core_err)?
                .is_none()
            {
                return Err(ErrorData::invalid_params(
                    format!("Paper {paper_id:?} not found in database."),
                    None,
                ));
            }
            let updated = paperq::remove_paper_tags(conn, &paper_id, &tags).map_err(core_err)?;
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
            let tag_id = tag::upsert(conn, &TagIn { label: label.clone() }).map_err(core_err)?;
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
            if tag::get(conn, &Tag { tag_id: Some(tag_id), ..Default::default() })
                .map_err(core_err)?
                .is_none()
            {
                return Err(ErrorData::invalid_params(format!("Tag {tag_id} not found."), None));
            }
            tag::delete(conn, &Tag { tag_id: Some(tag_id), ..Default::default() }).map_err(core_err)?;
            jval(json!({ "deleted": tag_id }))
        })
    }
}
