//! Notes + PDF + trash + import_pdf tools cluster. Owned by the
//! `notes_pdf_trash` Fill agent.
//!
//! Bodies use `self.with_conn(|conn| ...)`; PDF tools also read `self.pdf_dir`.
//! Call `linxiv_core::service::{note, files, paper, project}`. Wire shapes are
//! the canonical core serializers shared with the route/CLI surfaces:
//! `NoteDetails` (create/get/update), `DeletedNote`, `PdfLocation`
//! (get_pdf_path/download_pdf), `TrashListing` (list_trash); get_pdf_storage
//! returns `{"storage_mb", "pdf_dir"}`.
//! Map Python `ValueError` to `Err(ErrorData::invalid_params(msg, None))` with
//! the exact message (mind `{paper_id!r}` -> `{paper_id:?}` quoting).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::config;
use linxiv_core::error::CoreError;
use linxiv_core::models::{NoteIn, NoteUpdateIn};
use linxiv_core::service::{
    editor_project as svc_editor, files as svc_files, note as svc_note, paper as svc_paper,
    paper_import, project as svc_project,
};
use linxiv_core::sources::pdf_metadata::resolve_pdf_metadata;

use crate::util::{core_err, guard_err, invalid, json_ok};
use crate::Server;

/// Cap on `list_pdfs` rows, matching `GET /api/pdfs`.
const SAVED_PDF_LIST_CAP: usize = 200;

/// Shared trash-tool guard: the project must exist and be soft-deleted.
fn require_trashed_project(conn: &rusqlite::Connection, id: i64) -> Result<(), ErrorData> {
    svc_project::require_trashed(conn, id).map_err(guard_err)
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
            let source_fk =
                svc_paper::resolve_source_fk(conn, &p.paper_id).map_err(|e| match e {
                    CoreError::NotFound(m) => invalid(format!("{m}. Run fetch_paper first.")),
                    other => core_err(other),
                })?;
            let note_id = svc_note::create(
                conn,
                &NoteIn {
                    source_fk,
                    title: p.title.clone(),
                    content: p.content.clone(),
                    paper_id: None,
                    project_fk: p.project_id,
                    uuid: None,
                },
            )
            .map_err(core_err)?;
            // Canonical create envelope: the full NoteDetails serialization.
            json_ok(&svc_note::get_required(conn, note_id).map_err(core_err)?)
        })
    }

    #[tool(description = "Get a single note by its id.")]
    pub async fn get_note(
        &self,
        Parameters(p): Parameters<NoteIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            // A missing note is an error (shared contract), never a JSON null.
            json_ok(&svc_note::get_required(conn, p.note_id).map_err(guard_err)?)
        })
    }

    #[tool(description = "List notes, optionally filtered by paper or project.")]
    pub async fn list_notes(
        &self,
        Parameters(p): Parameters<ListNotesParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let source_fk = match &p.paper_id {
                Some(pid) => Some(svc_paper::resolve_source_fk(conn, pid).map_err(guard_err)?),
                None => None,
            };
            let notes = svc_note::list_filtered(conn, source_fk, p.project_id).map_err(core_err)?;
            json_ok(&notes)
        })
    }

    #[tool(description = "Update a note's title and/or content.")]
    pub async fn update_note(
        &self,
        Parameters(p): Parameters<UpdateNoteParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            svc_note::update(
                conn,
                &NoteUpdateIn {
                    note_id: p.note_id,
                    title: p.title.clone(),
                    content: p.content.clone(),
                },
            )
            .map_err(guard_err)?;
            // No row matched -> get_required raises the shared not-found; else
            // the canonical update envelope is the full NoteDetails serialization.
            json_ok(&svc_note::get_required(conn, p.note_id).map_err(guard_err)?)
        })
    }

    #[tool(description = "Delete a note by its id.")]
    pub async fn delete_note(
        &self,
        Parameters(p): Parameters<NoteIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            if !svc_editor::delete_note(conn, &config::vault_dir(), p.note_id).map_err(core_err)? {
                return Err(guard_err(svc_note::not_found(p.note_id)));
            }
            json_ok(&svc_note::DeletedNote {
                deleted_note_id: p.note_id,
            })
        })
    }

    #[tool(description = "Retrieve notes attached to a paper.")]
    pub async fn get_notes_for_paper(
        &self,
        Parameters(p): Parameters<GetNotesForPaperParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            let source_fk = svc_paper::resolve_source_fk(conn, &p.paper_id).map_err(guard_err)?;
            let notes =
                svc_note::list_filtered(conn, Some(source_fk), p.project_id).map_err(core_err)?;
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
            .map_err(core_err)?
            .ok_or_else(|| {
                invalid(format!(
                    "Paper {} not found in database.",
                    crate::util::pyrepr(&p.paper_id)
                ))
            })?;
            let ver = paper.version;
            let path =
                svc_files::pdf_path(&pdf_dir, &paper.source_id, ver, paper.pdf_path.as_deref());
            // Canonical location envelope, shared with `pdf path` and the route.
            json_ok(&svc_files::PdfLocation {
                source_id: paper.source_id,
                version: ver,
                path,
            })
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
            svc_paper::get(
                conn,
                &svc_paper::Paper {
                    source_id: Some(p.paper_id.clone()),
                    version: p.version,
                    ..Default::default()
                },
            )
            .map_err(core_err)?
            .ok_or_else(|| {
                invalid(format!(
                    "Paper {} not found in database.",
                    crate::util::pyrepr(&p.paper_id)
                ))
            })
            .map(|paper| (paper.source_id, paper.version))
        })?;
        let max_pdf_bytes = config::UserSettings::load()
            .map_err(core_err)?
            .pdf_save_limit_bytes();
        let path = svc_files::download_pdf(&pdf_dir, &source_id, ver, &p.url, max_pdf_bytes)
            .await
            .map_err(core_err)?;
        let path_str = path.to_string_lossy().into_owned();
        self.with_conn(|conn| {
            svc_paper::mark_pdf_saved(conn, &source_id, &path_str, ver).map_err(core_err)
        })?;
        json_ok(&svc_files::PdfLocation {
            source_id,
            version: ver,
            path: Some(path),
        })
    }

    #[tool(
        description = "List every paper whose PDF is stored on disk, with file sizes, largest \
                       first (capped at 200)."
    )]
    pub async fn list_pdfs(&self) -> Result<String, ErrorData> {
        let pdf_dir = self.pdf_dir.clone();
        // Pull the rows under the lock; the files are stat'd after it is released.
        let papers = self
            .with_conn(|conn| svc_paper::list_papers(conn, true, None, 0, None))
            .map_err(core_err)?;
        let mut pdfs: Vec<Value> = Vec::new();
        for paper in papers.into_iter().filter(|p| p.has_pdf) {
            let Some(path) = svc_files::pdf_path(
                &pdf_dir,
                &paper.source_id,
                paper.version,
                paper.pdf_path.as_deref(),
            ) else {
                continue;
            };
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            pdfs.push(json!({
                "source_id": paper.source_id,
                "source_fk": paper.source_fk,
                "title": paper.title,
                "version": paper.version,
                "size_bytes": meta.len(),
            }));
        }
        pdfs.sort_by(|a, b| {
            b["size_bytes"]
                .as_u64()
                .cmp(&a["size_bytes"].as_u64())
                .then_with(|| a["source_id"].as_str().cmp(&b["source_id"].as_str()))
        });
        pdfs.truncate(SAVED_PDF_LIST_CAP);
        json_ok(&json!({ "pdfs": pdfs }))
    }

    #[tool(
        description = "Delete a paper's stored PDF files from disk — every version — keeping the \
                       paper record itself."
    )]
    pub async fn delete_pdf(
        &self,
        Parameters(p): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        let pdf_dir = self.pdf_dir.clone();
        self.with_conn(|conn| {
            let all = svc_paper::get_all(
                conn,
                &svc_paper::Paper {
                    source_id: Some(p.paper_id.clone()),
                    ..Default::default()
                },
            )
            .map_err(core_err)?
            .ok_or_else(|| {
                invalid(format!(
                    "Paper {} not found in database.",
                    crate::util::pyrepr(&p.paper_id)
                ))
            })?;
            for ver in &all.versions {
                let path = svc_files::pdf_path(
                    &pdf_dir,
                    &p.paper_id,
                    ver.version,
                    ver.pdf_path.as_deref(),
                );
                if let Some(path) = &path {
                    if !svc_files::delete_pdf(&pdf_dir, &path.to_string_lossy()) {
                        return Err(invalid("PDF is outside managed storage"));
                    }
                }
                // Clear the flag/path before a later version may refuse.
                svc_paper::set_has_pdf(conn, &p.paper_id, ver.version, false).map_err(core_err)?;
                if path.is_some() {
                    svc_paper::set_pdf_path(conn, &p.paper_id, "", Some(ver.version))
                        .map_err(core_err)?;
                }
            }
            json_ok(&json!({ "deleted": true, "paper_id": p.paper_id }))
        })
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
            // Canonical TrashListing envelope (core service::trash).
            json_ok(&linxiv_core::service::trash::list_trash(conn).map_err(core_err)?)
        })
    }

    #[tool(
        description = "Permanently delete a trashed paper. Only works if the paper is in the trash."
    )]
    pub async fn trash_hard_delete_paper(
        &self,
        Parameters(p): Parameters<PaperIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            // require_trashed passing implies the root exists (STATUS='deleted' row),
            // so no separate existence check — same reasoning as cli/cmd/trash.rs.
            svc_paper::require_trashed(conn, &p.paper_id).map_err(guard_err)?;
            svc_paper::hard_delete(
                conn,
                &svc_paper::Paper {
                    source_id: Some(p.paper_id.clone()),
                    ..Default::default()
                },
            )
            .map_err(core_err)?;
            json_ok(&linxiv_core::service::trash::HardDeletedPaper {
                ok: true,
                hard_deleted: p.paper_id.clone(),
            })
        })
    }

    #[tool(
        description = "Restore a project from the trash. Only works if the project is soft-deleted."
    )]
    pub async fn restore_project_from_trash(
        &self,
        Parameters(p): Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            require_trashed_project(conn, p.project_id)?;
            svc_project::restore(
                conn,
                &svc_project::Project {
                    project_fk: Some(p.project_id),
                },
            )
            .map_err(core_err)?;
            json_ok(&linxiv_core::service::trash::RestoredProject {
                ok: true,
                restored_project_id: p.project_id,
            })
        })
    }

    #[tool(
        description = "Permanently delete a trashed project. Only works if the project is soft-deleted."
    )]
    pub async fn hard_delete_project_from_trash(
        &self,
        Parameters(p): Parameters<ProjectIdParams>,
    ) -> Result<String, ErrorData> {
        self.with_conn(|conn| {
            require_trashed_project(conn, p.project_id)?;
            svc_project::hard_delete(
                conn,
                &svc_project::Project {
                    project_fk: Some(p.project_id),
                },
            )
            .map_err(core_err)?;
            json_ok(&linxiv_core::service::trash::HardDeletedProject {
                ok: true,
                hard_deleted_project_id: p.project_id,
            })
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
            self.with_conn(
                |conn| match svc_project::ensure_membership_writable(conn, pid) {
                    Err(CoreError::ProjectNotFound) => {
                        Err(invalid(format!("Project {pid} not found.")))
                    }
                    Err(e) => Err(core_err(e)),
                    Ok(()) => Ok(()),
                },
            )?;
        }
        let content =
            std::fs::read(&p.file).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        // `pdf_save_limit_mb` total-storage quota BEFORE the (expensive) pdfium
        // metadata resolve — no point fully parsing a PDF that's about to be
        // rejected; core's `import_pdf` re-checks it before any FS/DB write.
        let max_pdf_bytes = config::UserSettings::load()
            .map_err(core_err)?
            .pdf_save_limit_bytes();
        paper_import::check_pdf_storage_quota(&pdf_dir, content.len(), max_pdf_bytes)
            .map_err(core_err)?;
        // Resolve metadata (network) OUTSIDE the lock, then do the sync DB+FS
        // import under it — mirrors `import_pdf_default` without holding the
        // mutex across the await.
        let resolved = resolve_pdf_metadata(&content, &data_dir)
            .await
            .map_err(core_err)?;
        self.with_conn(|conn| {
            match paper_import::import_pdf(
                conn,
                &pdf_dir,
                &content,
                p.project_id,
                max_pdf_bytes,
                |_| Ok(resolved.clone()),
            ) {
                Ok(result) => json_ok(&result),
                Err(CoreError::ProjectNotFound) => Err(invalid(format!(
                    "Project {} not found.",
                    p.project_id.unwrap_or_default()
                ))),
                Err(e @ CoreError::PaperLink(_)) => Err(invalid(e.to_string())),
                Err(e) => Err(core_err(e)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use linxiv_core::models::PaperMetadata;
    use linxiv_core::storage;

    use super::*;

    /// Mirrors `papers.rs`'s test `server()`, with a scratch PDF dir: these
    /// tests call tool methods directly instead of dispatching through the router.
    fn server(pdf_dir: std::path::PathBuf) -> Server {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        Server {
            conn: Arc::new(Mutex::new(conn)),
            pdf_dir,
            tool_router: Server::tools_notes_pdf_trash(),
        }
    }

    /// Unique scratch dir (no tempfile dep): nanos-suffixed under the system temp.
    fn scratch() -> std::path::PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("linxiv_mcp_pdfs_{n}"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A managed PDF is listed with its size, then deleted off disk while the
    /// paper row survives; an unknown id is refused.
    #[tokio::test]
    async fn list_then_delete_a_managed_pdf() {
        let dir = scratch();
        let srv = server(dir.clone());
        let meta: PaperMetadata = serde_json::from_value(json!({
            "source_id": "arxiv:1",
            "version": 1,
            "title": "T",
            "authors": ["A"],
            "published": "2024-01-01",
            "summary": "S",
        }))
        .unwrap();
        srv.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))
            .unwrap();
        let path = dir.join(svc_paper::pdf_on_disk_name("arxiv:1", 1));
        std::fs::write(&path, b"%PDF").unwrap();
        srv.with_conn(|conn| {
            svc_paper::mark_pdf_saved(conn, "arxiv:1", &path.to_string_lossy(), 1)
        })
        .unwrap();

        let listed: Value = serde_json::from_str(&srv.list_pdfs().await.unwrap()).unwrap();
        assert_eq!(listed["pdfs"].as_array().unwrap().len(), 1);
        assert_eq!(listed["pdfs"][0]["source_id"], json!("arxiv:1"));
        assert_eq!(listed["pdfs"][0]["size_bytes"], json!(4));

        srv.delete_pdf(Parameters(PaperIdParams {
            paper_id: "arxiv:1".to_string(),
        }))
        .await
        .unwrap();
        assert!(!path.exists());
        let listed: Value = serde_json::from_str(&srv.list_pdfs().await.unwrap()).unwrap();
        assert_eq!(listed["pdfs"], json!([]));

        let err = srv
            .delete_pdf(Parameters(PaperIdParams {
                paper_id: "arxiv:nope".to_string(),
            }))
            .await
            .unwrap_err();
        assert_eq!(
            err.message.as_ref(),
            "Paper 'arxiv:nope' not found in database."
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
