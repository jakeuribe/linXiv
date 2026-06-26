//! Export/import + DOI + authors + bibtex import + system tools cluster.
//! Owned by the `io_authors_misc` Fill agent.
//!
//! Bodies use `self.with_conn(|conn| ...)`; export tools also use
//! `self.vault_root`. Call `linxiv_core::service::{export_import, author, paper,
//! tag}`, `::sources` for DOI resolution, and `linxiv_core::config::UserSettings`
//! for the settings tools. `import_bibtex` parses with the `biblatex` crate.
//! Replicate the Python dict shapes EXACTLY, e.g. export returns
//! `{"path", "project_id"}`, get_stats returns
//! `{"paper_count", "tag_count", "category_count", "pdf_count"}`, save_doi
//! returns `{"source_id", "version", "title"}`. Map `ValueError` to
//! `Err(ErrorData::invalid_params(msg, None))` with the exact message.

use std::path::Path;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::config::{self, UserSettings};
use linxiv_core::error::CoreError;
use linxiv_core::service::author::{self as svc_author, Author};
use linxiv_core::service::export_import::{self as svc_ei, OnConflict};
use linxiv_core::service::project::{self as svc_project, Project};
use linxiv_core::service::{paper as svc_paper, tag as svc_tag};
use linxiv_core::sources::doi_resolve;

use crate::Server;

// formats.rs (BibTeX + Obsidian transforms) ships in the scaffold but is not
// wired into main.rs; this cluster is its only consumer, so own the module here.
#[path = "formats.rs"]
mod formats;

/// Map a core error to the MCP error code Python's `ValueError` would surface:
/// user-facing validation (`BadRequest`/`Validation`) → `invalid_params`,
/// everything else (DB/FS) → `internal_error`.
fn map_core(e: CoreError) -> ErrorData {
    match e {
        CoreError::BadRequest(m) | CoreError::Validation(m) => {
            ErrorData::invalid_params(m, None)
        }
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

/// Serialize a core value to the tool's text result (compact JSON string).
fn json_ok<T: serde::Serialize>(v: &T) -> Result<String, ErrorData> {
    serde_json::to_string(v).map_err(|e| ErrorData::internal_error(e.to_string(), None))
}

/// Resolve a project to its papers, erroring with the Python message when the
/// project is missing. Empty `source_fks` yields no papers without a query.
fn project_papers(
    conn: &rusqlite::Connection,
    project_id: i64,
) -> Result<Vec<linxiv_core::models::PaperDetails>, ErrorData> {
    let details = svc_project::get(conn, &Project { project_fk: Some(project_id) })
        .map_err(map_core)?
        .ok_or_else(|| {
            ErrorData::invalid_params(format!("Project {project_id} not found."), None)
        })?;
    if details.source_fks.is_empty() {
        return Ok(Vec::new());
    }
    svc_paper::get_many(
        conn,
        &svc_paper::Papers { source_fks: Some(details.source_fks), ..Default::default() },
    )
    .map_err(map_core)
}

/// `Path(dest)` with `ext` forced only when the destination has no extension
/// (Python `out.with_suffix(...) if not out.suffix`).
fn with_default_ext(dest: &str, ext: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(dest);
    if p.extension().is_none() {
        p.with_extension(ext)
    } else {
        p
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportProjectParams {
    /// Numeric project id.
    pub project_id: i64,
    /// Destination file path (.lxproj added automatically if absent).
    pub dest: String,
    /// Include bundled PDFs in the archive (default False).
    #[serde(default)]
    pub include_pdfs: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportProjectParams {
    /// Path to the .lxproj archive.
    pub zip_path: String,
    /// How to handle papers that already exist — "merge" or "overwrite".
    #[serde(default = "default_on_conflict")]
    pub on_conflict: String,
    /// If True, return a summary without modifying the database.
    #[serde(default)]
    pub preview: bool,
}

fn default_on_conflict() -> String {
    "merge".to_string()
}

/// Export tools that take a project id plus an output path.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportProjectDestParams {
    /// Numeric project id.
    pub project_id: i64,
    /// Output file path (extension added automatically if none is given).
    pub dest: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DoiParams {
    /// DOI string (e.g. "10.1038/nature12373").
    pub doi: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AuthorIdParams {
    /// Numeric author id.
    pub author_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateAuthorParams {
    /// Numeric author id.
    pub author_id: i64,
    /// New full name.
    #[serde(default)]
    pub full_name: Option<String>,
    /// New first name.
    #[serde(default)]
    pub first_name: Option<String>,
    /// New last name.
    #[serde(default)]
    pub last_name: Option<String>,
    /// New ORCID identifier.
    #[serde(default)]
    pub orcid: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportBibtexParams {
    /// Path to the .bib file on disk.
    pub file: String,
    /// Optionally link all imported papers to this project.
    #[serde(default)]
    pub project_id: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateSettingParams {
    /// Setting key.
    pub key: String,
    /// New value (JSON-parsed when valid JSON, else stored as a string).
    pub value: String,
}

#[tool_router(router = tools_io_authors_misc, vis = "pub(crate)")]
impl Server {
    #[tool(description = "Export a project to a .lxproj archive file.")]
    pub async fn export_project(
        &self,
        params: Parameters<ExportProjectParams>,
    ) -> Result<String, ErrorData> {
        let ExportProjectParams { project_id, dest, include_pdfs } = params.0;
        let pdf_dir = self.pdf_dir.clone();
        let path = self.with_conn(|conn| -> Result<String, ErrorData> {
            if svc_project::get(conn, &Project { project_fk: Some(project_id) })
                .map_err(map_core)?
                .is_none()
            {
                return Err(ErrorData::invalid_params(
                    format!("Project {project_id} not found."),
                    None,
                ));
            }
            let out = svc_ei::export_project(conn, project_id, Path::new(&dest), include_pdfs, &pdf_dir)
                .map_err(map_core)?;
            Ok(out.to_string_lossy().into_owned())
        })?;
        json_ok(&json!({ "path": path, "project_id": project_id }))
    }

    #[tool(description = "Import a project from a .lxproj archive file.")]
    pub async fn import_project(
        &self,
        params: Parameters<ImportProjectParams>,
    ) -> Result<String, ErrorData> {
        let ImportProjectParams { zip_path, on_conflict, preview } = params.0;
        let path = std::path::PathBuf::from(&zip_path);
        if preview {
            let result = svc_ei::preview_import(&path).map_err(map_core)?;
            return json_ok(&result);
        }
        let on_conflict = if on_conflict == "overwrite" {
            OnConflict::Overwrite
        } else {
            OnConflict::Merge
        };
        let pdf_dir = self.pdf_dir.clone();
        let fk = self
            .with_conn(|conn| svc_ei::commit_import(conn, &path, on_conflict, &pdf_dir))
            .map_err(map_core)?;
        json_ok(&json!({ "project_id": fk }))
    }

    #[tool(description = "Export a project's papers to a BibTeX (.bib) file.")]
    pub async fn export_project_bibtex(
        &self,
        params: Parameters<ExportProjectDestParams>,
    ) -> Result<String, ErrorData> {
        let ExportProjectDestParams { project_id, dest } = params.0;
        let body = self.with_conn(|conn| -> Result<String, ErrorData> {
            let papers = project_papers(conn, project_id)?;
            Ok(formats::bibtex_export(&papers))
        })?;
        let out = with_default_ext(&dest, "bib");
        std::fs::write(&out, body)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        json_ok(&json!({ "path": out.to_string_lossy(), "project_id": project_id }))
    }

    #[tool(description = "Export a project's papers as Obsidian-style markdown notes.")]
    pub async fn export_project_obsidian(
        &self,
        params: Parameters<ExportProjectDestParams>,
    ) -> Result<String, ErrorData> {
        let ExportProjectDestParams { project_id, dest } = params.0;
        let body = self.with_conn(|conn| -> Result<String, ErrorData> {
            let papers = project_papers(conn, project_id)?;
            Ok(formats::obsidian_export(&papers))
        })?;
        let out = with_default_ext(&dest, "md");
        std::fs::write(&out, body)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        json_ok(&json!({ "path": out.to_string_lossy(), "project_id": project_id }))
    }

    #[tool(description = "Resolve a DOI to paper metadata without saving it to the library.")]
    pub async fn resolve_doi(
        &self,
        params: Parameters<DoiParams>,
    ) -> Result<String, ErrorData> {
        let meta = doi_resolve::resolve_doi(&params.0.doi, &config::data_dir())
            .await
            .map_err(map_core)?;
        json_ok(&meta)
    }

    #[tool(description = "Resolve a DOI and save the resulting paper to the local library.")]
    pub async fn save_doi(
        &self,
        params: Parameters<DoiParams>,
    ) -> Result<String, ErrorData> {
        let meta = doi_resolve::resolve_doi(&params.0.doi, &config::data_dir())
            .await
            .map_err(map_core)?;
        let (source_id, version) = self
            .with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))
            .map_err(map_core)?;
        json_ok(&json!({
            "source_id": source_id,
            "version": version,
            "title": meta.title,
        }))
    }

    #[tool(description = "List all authors in the library with their paper counts.")]
    pub async fn list_authors(&self) -> Result<String, ErrorData> {
        let authors = self
            .with_conn(|conn| svc_author::list_with_paper_count(conn, 0))
            .map_err(map_core)?;
        json_ok(&authors)
    }

    #[tool(description = "Get an author's details together with a preview of their papers.")]
    pub async fn get_author(
        &self,
        params: Parameters<AuthorIdParams>,
    ) -> Result<String, ErrorData> {
        let author_id = params.0.author_id;
        let value = self.with_conn(|conn| -> Result<Value, ErrorData> {
            let author = svc_author::get(conn, &Author { author_id: Some(author_id), ..Default::default() })
                .map_err(map_core)?
                .ok_or_else(|| {
                    ErrorData::invalid_params(format!("Author {author_id} not found."), None)
                })?;
            let previews = svc_author::get_paper_previews(conn, author_id).map_err(map_core)?;
            let mut value = serde_json::to_value(&author)
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            if let Value::Object(map) = &mut value {
                map.insert(
                    "papers".to_string(),
                    serde_json::to_value(&previews)
                        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
                );
            }
            Ok(value)
        })?;
        json_ok(&value)
    }

    #[tool(description = "Update an author's fields. At least one field must be provided.")]
    pub async fn update_author(
        &self,
        params: Parameters<UpdateAuthorParams>,
    ) -> Result<String, ErrorData> {
        let UpdateAuthorParams { author_id, full_name, first_name, last_name, orcid } = params.0;
        self.with_conn(|conn| -> Result<(), ErrorData> {
            if svc_author::get(conn, &Author { author_id: Some(author_id), ..Default::default() })
                .map_err(map_core)?
                .is_none()
            {
                return Err(ErrorData::invalid_params(
                    format!("Author {author_id} not found."),
                    None,
                ));
            }
            if full_name.is_none() && first_name.is_none() && last_name.is_none() && orcid.is_none() {
                return Err(ErrorData::invalid_params(
                    "At least one of full_name, first_name, last_name, or orcid must be provided.",
                    None,
                ));
            }
            svc_author::update_fields(
                conn,
                author_id,
                full_name.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                orcid.as_deref(),
            )
            .map_err(map_core)
        })?;
        json_ok(&json!({ "updated_author_id": author_id }))
    }

    #[tool(description = "Delete an author. Blocked if the author is still linked to any papers.")]
    pub async fn delete_author(
        &self,
        params: Parameters<AuthorIdParams>,
    ) -> Result<String, ErrorData> {
        let author_id = params.0.author_id;
        self.with_conn(|conn| -> Result<(), ErrorData> {
            let link_count = svc_author::count_paper_links(conn, author_id).map_err(map_core)?;
            if link_count > 0 {
                return Err(ErrorData::invalid_params(
                    format!("Author {author_id} is linked to {link_count} paper(s); unlink first."),
                    None,
                ));
            }
            svc_author::delete(conn, &Author { author_id: Some(author_id), ..Default::default() })
                .map_err(map_core)
        })?;
        json_ok(&json!({ "deleted_author_id": author_id }))
    }

    #[tool(description = "Bulk-import papers from a BibTeX (.bib) file into the library.")]
    pub async fn import_bibtex(
        &self,
        params: Parameters<ImportBibtexParams>,
    ) -> Result<String, ErrorData> {
        let ImportBibtexParams { file, project_id } = params.0;
        let text = std::fs::read_to_string(&file)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        let metas = formats::bibtex_import(&text)
            .map_err(|m| ErrorData::invalid_params(m, None))?;

        let value = self.with_conn(|conn| -> Result<Value, ErrorData> {
            // Guard before saving so a missing/deleted project fails the call
            // before the library is mutated.
            if let Some(pid) = project_id {
                match svc_project::ensure_membership_writable(conn, pid) {
                    Ok(()) => {}
                    Err(CoreError::ProjectNotFound) => {
                        return Err(ErrorData::invalid_params(
                            format!("Project {pid} not found."),
                            None,
                        ));
                    }
                    Err(e) => return Err(map_core(e)),
                }
            }
            let mut saved = Vec::new();
            for meta in &metas {
                let (source_id, version) =
                    svc_paper::save_paper_metadata(conn, meta, None).map_err(map_core)?;
                saved.push((source_id, version));
            }
            if let Some(pid) = project_id {
                if !saved.is_empty() {
                    let ids: Vec<String> = saved.iter().map(|(s, _)| s.clone()).collect();
                    if let Err(e) = svc_project::link_imported(conn, pid, &ids) {
                        return Err(ErrorData::invalid_params(
                            format!(
                                "{} paper(s) were imported but could not be linked: {e}",
                                saved.len()
                            ),
                            None,
                        ));
                    }
                }
            }
            let papers: Vec<Value> = saved
                .iter()
                .map(|(s, v)| json!({ "source_id": s, "version": v }))
                .collect();
            Ok(json!({ "imported": saved.len(), "papers": papers }))
        })?;
        json_ok(&value)
    }

    #[tool(description = "Report library statistics: paper, tag, category, and downloaded-PDF counts.")]
    pub async fn get_stats(&self) -> Result<String, ErrorData> {
        self.with_conn(|conn| -> Result<String, ErrorData> {
            let papers = svc_paper::list_papers(conn, true, None, 0, None).map_err(map_core)?;
            let categories = svc_paper::get_categories(conn).map_err(map_core)?;
            let all_tags = svc_tag::list_all_tags(conn).map_err(map_core)?;
            let pdf_count = papers.iter().filter(|p| p.has_pdf).count();
            json_ok(&json!({
                "paper_count": papers.len(),
                "tag_count": all_tags.len(),
                "category_count": categories.len(),
                "pdf_count": pdf_count,
            }))
        })
    }

    #[tool(description = "List all distinct paper categories present in the library.")]
    pub async fn list_categories(&self) -> Result<String, ErrorData> {
        let categories = self
            .with_conn(|conn| svc_paper::get_categories(conn))
            .map_err(map_core)?;
        json_ok(&categories)
    }

    #[tool(description = "Get all current user settings.")]
    pub async fn get_settings(&self) -> Result<String, ErrorData> {
        let settings = UserSettings::load().map_err(map_core)?;
        json_ok(&Value::Object(settings.all()))
    }

    #[tool(description = "Update a single user setting.")]
    pub async fn update_setting(
        &self,
        params: Parameters<UpdateSettingParams>,
    ) -> Result<String, ErrorData> {
        let UpdateSettingParams { key, value } = params.0;
        // Parse the value as JSON when valid, else store it verbatim as a string.
        let parsed = serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value));
        let mut settings = UserSettings::load().map_err(map_core)?;
        settings.set(key.clone(), parsed.clone()).map_err(map_core)?;
        let mut out = serde_json::Map::new();
        out.insert(key, parsed);
        json_ok(&Value::Object(out))
    }
}
