//! Notes + PDF + trash + import_pdf tools cluster. Owned by the
//! `notes_pdf_trash` Fill agent.
//!
//! Bodies use `self.with_conn(|conn| ...)`; PDF tools also read `self.pdf_dir`.
//! Call `linxiv_core::service::{note, files, paper, project}`. Replicate the
//! Python dict shapes EXACTLY, e.g. get_pdf_path returns
//! `{"paper_id", "version", "path"}`, get_pdf_storage returns
//! `{"storage_mb", "pdf_dir"}`, list_trash returns `{"papers", "projects"}`.
//! Map Python `ValueError` to `Err(ErrorData::invalid_params(msg, None))` with
//! the exact message (mind `{paper_id!r}` -> `{paper_id:?}` quoting).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use linxiv_core::error::CoreError;
use linxiv_core::models::{NoteIn, NoteUpdateIn, Status};
use linxiv_core::service::{
    files as svc_files, note as svc_note, paper as svc_paper, paper_import, project as svc_project,
};
use linxiv_core::sources::pdf_metadata::resolve_pdf_metadata;
use linxiv_core::storage::queries::paper as store_paper;
use linxiv_core::config;

use crate::Server;

/// `ValueError` → MCP invalid-params, preserving the Python message verbatim.
fn invalid(msg: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(msg.into(), None)
}

/// Unexpected core failure (not one of the explicit `ValueError` paths).
fn core_err(e: CoreError) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

/// Serialize a core value to the tool's text result (compact JSON string).
fn json_ok<T: Serialize>(v: &T) -> Result<String, ErrorData> {
    serde_json::to_string(v).map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateNoteParams {
    /// Paper id the note is attached to.
    pub paper_id: String,
    /// Body text of the note.
    pub content: String,
    /// Optional note title.
    #[serde(default)]
    pub title: String,
    /// Associate the note with a specific project.
    #[serde(default)]
    pub project_id: Option<i64>,
}

/// Tools that take only a numeric note id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NoteIdParams {
    /// Numeric note id.
    pub note_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListNotesParams {
    /// Filter by paper source id (e.g. "arxiv:2204.12985").
    #[serde(default)]
    pub paper_id: Option<String>,
    /// Filter by project id.
    #[serde(default)]
    pub project_id: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateNoteParams {
    /// Numeric note id.
    pub note_id: i64,
    /// New title (omit to leave unchanged).
    #[serde(default)]
    pub title: Option<String>,
    /// New content (omit to leave unchanged).
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetNotesForPaperParams {
    /// Paper id to look up notes for.
    pub paper_id: String,
    /// Scope to a specific project (None returns all notes for the paper).
    #[serde(default)]
    pub project_id: Option<i64>,
}

/// Tools that take only a numeric project id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectIdParams {
    /// Numeric project id.
    pub project_id: i64,
}

/// Tools that take only a paper source id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaperIdParams {
    /// The paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPdfPathParams {
    /// The paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
    /// Specific version number (defaults to latest).
    #[serde(default)]
    pub version: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DownloadPdfParams {
    /// The paper source id (e.g. "arxiv:2204.12985").
    pub paper_id: String,
    /// Direct URL to the PDF file.
    pub url: String,
    /// Specific version number (defaults to latest).
    #[serde(default)]
    pub version: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportPdfParams {
    /// Path to the PDF file on disk.
    pub file: String,
    /// Optionally link the imported paper to this project.
    #[serde(default)]
    pub project_id: Option<i64>,
}

#[tool_router(router = tools_notes_pdf_trash, vis = "pub(crate)")]
impl Server {
    #[tool(description = "Create a note attached to a paper, optionally scoped to a project.")]
    pub async fn create_note(
        &self,
        Parameters(p): Parameters<CreateNoteParams>,
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
            let note_id = svc_note::create(
                conn,
                &NoteIn {
                    source_fk,
                    title: p.title.clone(),
                    content: p.content.clone(),
                    paper_id: None,
                    project_fk: p.project_id,
                },
            )
            .map_err(core_err)?;
            match svc_note::get(conn, &svc_note::Note { note_id: Some(note_id) }).map_err(core_err)? {
                Some(n) => json_ok(&n),
                None => json_ok(&json!({
                    "id": note_id,
                    "source_fk": source_fk,
                    "project_id": p.project_id,
                    "title": p.title,
                })),
            }
        })
    }

    #[tool(description = "Get a single note by its id.")]
    pub async fn get_note(
        &self,
        Parameters(p): Parameters<NoteIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            match svc_note::get(conn, &svc_note::Note { note_id: Some(p.note_id) }).map_err(core_err)? {
                Some(n) => json_ok(&n),
                None => json_ok(&Value::Null),
            }
        })
    }

    #[tool(description = "List notes, optionally filtered by paper or project.")]
    pub async fn list_notes(
        &self,
        Parameters(p): Parameters<ListNotesParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let notes = if p.paper_id.is_none() && p.project_id.is_none() {
                svc_note::list_all(conn).map_err(core_err)?
            } else {
                let source_fk = match &p.paper_id {
                    Some(pid) => {
                        let root = store_paper::get_paper_root(conn, pid).map_err(core_err)?;
                        match root {
                            Some(r) => Some(r.source_fk),
                            None => {
                                return Err(invalid(format!(
                                    "Paper {} not found in database.",
                                    crate::util::pyrepr(pid)
                                )))
                            }
                        }
                    }
                    None => None,
                };
                svc_note::get_many(
                    conn,
                    &svc_note::Notes {
                        source_fk,
                        project_fk: p.project_id,
                        all_projects: p.paper_id.is_some() && p.project_id.is_none(),
                        ..Default::default()
                    },
                )
                .map_err(core_err)?
            };
            json_ok(&notes)
        })
    }

    #[tool(description = "Update a note's title and/or content.")]
    pub async fn update_note(
        &self,
        Parameters(p): Parameters<UpdateNoteParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let ok = svc_note::update(
                conn,
                &NoteUpdateIn {
                    note_id: p.note_id,
                    title: p.title.clone(),
                    content: p.content.clone(),
                },
            )
            .map_err(core_err)?;
            if !ok {
                return Err(invalid(format!("Note {} not found.", p.note_id)));
            }
            match svc_note::get(conn, &svc_note::Note { note_id: Some(p.note_id) }).map_err(core_err)? {
                Some(n) => json_ok(&n),
                None => json_ok(&json!({})),
            }
        })
    }

    #[tool(description = "Delete a note by its id.")]
    pub async fn delete_note(
        &self,
        Parameters(p): Parameters<NoteIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            if svc_note::get(conn, &svc_note::Note { note_id: Some(p.note_id) })
                .map_err(core_err)?
                .is_none()
            {
                return Err(invalid(format!("Note {} not found.", p.note_id)));
            }
            svc_note::delete(conn, &svc_note::Note { note_id: Some(p.note_id) }).map_err(core_err)?;
            json_ok(&json!({ "deleted": p.note_id }))
        })
    }

    #[tool(description = "Retrieve notes attached to a paper.")]
    pub async fn get_notes_for_paper(
        &self,
        Parameters(p): Parameters<GetNotesForPaperParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let root = store_paper::get_paper_root(conn, &p.paper_id).map_err(core_err)?;
            let source_fk = match root {
                Some(r) => r.source_fk,
                None => {
                    return Err(invalid(format!(
                        "Paper {} not found in database.",
                        crate::util::pyrepr(&p.paper_id)
                    )))
                }
            };
            let notes = svc_note::get_many(
                conn,
                &svc_note::Notes {
                    source_fk: Some(source_fk),
                    project_fk: p.project_id,
                    all_projects: p.project_id.is_none(),
                    ..Default::default()
                },
            )
            .map_err(core_err)?;
            json_ok(&notes)
        })
    }

    #[tool(description = "Retrieve all notes scoped to a project, across all its papers.")]
    pub async fn get_notes_for_project(
        &self,
        Parameters(p): Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let notes = svc_note::get_many(
                conn,
                &svc_note::Notes {
                    project_fk: Some(p.project_id),
                    ..Default::default()
                },
            )
            .map_err(core_err)?;
            json_ok(&notes)
        })
    }

    #[tool(description = "Get the local filesystem path for a paper's PDF, if downloaded.")]
    pub async fn get_pdf_path(
        &self,
        Parameters(p): Parameters<GetPdfPathParams>,
    ) -> Result<String, ErrorData> {
        let pdf_dir = self.pdf_dir.clone();
        self.with_conn(|conn| {
            let paper = svc_paper::get(
                conn,
                &svc_paper::Paper {
                    source_id: Some(p.paper_id.clone()),
                    version: p.version,
                    ..Default::default()
                },
            )
            .map_err(core_err)?;
            let paper = match paper {
                Some(paper) => paper,
                None => {
                    return Err(invalid(format!(
                        "Paper {} not found in database.",
                        crate::util::pyrepr(&p.paper_id)
                    )))
                }
            };
            let ver = paper.version;
            let path = svc_files::pdf_path(&pdf_dir, &paper.source_id, ver, paper.pdf_path.as_deref());
            json_ok(&json!({
                "paper_id": p.paper_id,
                "version": ver,
                "path": path.map(|p| p.to_string_lossy().into_owned()),
            }))
        })
    }

    #[tool(description = "Download a PDF for a paper and save it to the managed PDF directory.")]
    pub async fn download_pdf(
        &self,
        Parameters(p): Parameters<DownloadPdfParams>,
    ) -> Result<String, ErrorData> {
        let pdf_dir = self.pdf_dir.clone();
        // Resolve the paper (and its concrete version) under the lock, then drop
        // it before the network download — the mutex must not span the await.
        let (source_id, ver) = self.with_conn(|conn| {
            let paper = svc_paper::get(
                conn,
                &svc_paper::Paper {
                    source_id: Some(p.paper_id.clone()),
                    version: p.version,
                    ..Default::default()
                },
            )
            .map_err(core_err)?;
            match paper {
                Some(paper) => Ok((paper.source_id, paper.version)),
                None => Err(invalid(format!(
                    "Paper {} not found in database.",
                    crate::util::pyrepr(&p.paper_id)
                ))),
            }
        })?;
        let path = svc_files::download_pdf(&pdf_dir, &source_id, ver, &p.url)
            .await
            .map_err(core_err)?;
        let path_str = path.to_string_lossy().into_owned();
        self.with_conn(|conn| {
            svc_paper::mark_pdf_saved(conn, &source_id, &path_str, ver).map_err(core_err)
        })?;
        json_ok(&json!({
            "paper_id": p.paper_id,
            "version": ver,
            "path": path_str,
        }))
    }

    #[tool(description = "Report total PDF storage usage for all managed PDFs.")]
    pub async fn get_pdf_storage(&self) -> Result<String, ErrorData> {
        let mb = svc_files::pdf_storage_mb(&self.pdf_dir);
        let storage_mb = (mb * 1000.0).round() / 1000.0;
        json_ok(&json!({
            "storage_mb": storage_mb,
            "pdf_dir": self.pdf_dir.to_string_lossy(),
        }))
    }

    #[tool(description = "List all soft-deleted papers and projects currently in the trash.")]
    pub async fn list_trash(&self) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let papers = svc_paper::list_deleted(conn).map_err(core_err)?;
            let projects = svc_project::list_deleted(conn).map_err(core_err)?;
            json_ok(&json!({
                "papers": papers,
                "projects": projects,
            }))
        })
    }

    #[tool(description = "Permanently delete a trashed paper. Only works if the paper is in the trash.")]
    pub async fn trash_hard_delete_paper(
        &self,
        Parameters(p): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            if !svc_paper::is_paper_deleted(conn, &p.paper_id).map_err(core_err)? {
                return Err(invalid(format!("Paper {} not found in trash.", crate::util::pyrepr(&p.paper_id))));
            }
            if store_paper::get_paper_root(conn, &p.paper_id)
                .map_err(core_err)?
                .is_none()
            {
                return Err(invalid(format!("Paper {} not found.", crate::util::pyrepr(&p.paper_id))));
            }
            svc_paper::hard_delete(
                conn,
                &svc_paper::Paper {
                    source_id: Some(p.paper_id.clone()),
                    ..Default::default()
                },
            )
            .map_err(core_err)?;
            json_ok(&json!({ "hard_deleted": p.paper_id }))
        })
    }

    #[tool(description = "Restore a project from the trash. Only works if the project is soft-deleted.")]
    pub async fn restore_project_from_trash(
        &self,
        Parameters(p): Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let details = svc_project::get(conn, &svc_project::Project { project_fk: Some(p.project_id) })
                .map_err(core_err)?;
            let details = match details {
                Some(d) => d,
                None => return Err(invalid(format!("Project {} not found.", p.project_id))),
            };
            if details.status != Status::Deleted {
                return Err(invalid(format!("Project {} is not in trash.", p.project_id)));
            }
            svc_project::restore(conn, &svc_project::Project { project_fk: Some(p.project_id) })
                .map_err(core_err)?;
            json_ok(&json!({ "restored_project_id": p.project_id }))
        })
    }

    #[tool(description = "Permanently delete a trashed project. Only works if the project is soft-deleted.")]
    pub async fn hard_delete_project_from_trash(
        &self,
        Parameters(p): Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let details = svc_project::get(conn, &svc_project::Project { project_fk: Some(p.project_id) })
                .map_err(core_err)?;
            let details = match details {
                Some(d) => d,
                None => return Err(invalid(format!("Project {} not found.", p.project_id))),
            };
            if details.status != Status::Deleted {
                return Err(invalid(format!("Project {} is not in trash.", p.project_id)));
            }
            svc_project::hard_delete(conn, &svc_project::Project { project_fk: Some(p.project_id) })
                .map_err(core_err)?;
            json_ok(&json!({ "hard_deleted_project_id": p.project_id }))
        })
    }

    #[tool(description = "Import a local PDF file, extracting paper metadata from its contents.")]
    pub async fn import_pdf(
        &self,
        Parameters(p): Parameters<ImportPdfParams>,
    ) -> Result<String, ErrorData> {
        let pdf_dir = self.pdf_dir.clone();
        let data_dir = config::data_dir();
        // Pre-read guard: convert a missing project to the MCP ValueError before
        // touching the file (matches the Python ordering).
        if let Some(pid) = p.project_id {
            self.with_conn(|conn| match svc_project::ensure_membership_writable(conn, pid) {
                Err(CoreError::ProjectNotFound) => Err(invalid(format!("Project {pid} not found."))),
                Err(e) => Err(core_err(e)),
                Ok(()) => Ok(()),
            })?;
        }
        let content = std::fs::read(&p.file)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        // Resolve metadata (network) OUTSIDE the lock, then do the sync DB+FS
        // import under it — mirrors `import_pdf_default` without holding the
        // mutex across the await.
        let resolved = resolve_pdf_metadata(&content, &data_dir)
            .await
            .map_err(core_err)?;
        self.with_conn(|conn| {
            match paper_import::import_pdf(conn, &pdf_dir, &content, p.project_id, |_| {
                Ok(resolved.clone())
            }) {
                Ok(result) => json_ok(&result),
                Err(CoreError::ProjectNotFound) => {
                    Err(invalid(format!("Project {} not found.", p.project_id.unwrap_or_default())))
                }
                Err(e @ CoreError::PaperLink(_)) => Err(invalid(e.to_string())),
                Err(e) => Err(core_err(e)),
            }
        })
    }
}
