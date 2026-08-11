//! Export/import + DOI + authors + bibtex import + system tools cluster.
//! Owned by the `io_authors_misc` Fill agent.
//!
//! Bodies use `self.with_conn(|conn| ...)`; export tools also use
//! `self.pdf_dir`. Call `linxiv_core::service::{export_import, author, paper,
//! tag}`, `::sources` for DOI resolution, and `linxiv_core::config::UserSettings`
//! for the settings tools. `import_bibtex` delegates to `linxiv_core::formats`.
//! Replicate the Python dict shapes EXACTLY, e.g. export returns
//! `{"path", "project_id"}`, get_stats returns
//! `{"paper_count", "tag_count", "category_count", "pdf_count"}`; save_doi
//! returns the route's `{"metadata", "saved"}` envelope. Map `ValueError` to
//! `Err(ErrorData::invalid_params(msg, None))` with the exact message.

use std::path::{Path, PathBuf};

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
use linxiv_core::storage;

use crate::Server;

/// Map a core error to the MCP error code Python's `ValueError` would surface:
/// user-facing validation (`BadRequest`/`Validation`) → `invalid_params`,
/// everything else (DB/FS) → `internal_error`.
fn map_core(e: CoreError) -> ErrorData {
    match e {
        CoreError::BadRequest(m) | CoreError::Validation(m) => ErrorData::invalid_params(m, None),
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

use crate::util::{blocking, guard_err, json_ok};

/// Resolve a project to its papers, erroring with the Python message when the
/// project is missing. Empty `source_fks` yields no papers without a query.
fn project_papers(
    conn: &rusqlite::Connection,
    project_id: i64,
) -> Result<Vec<linxiv_core::models::PaperDetails>, ErrorData> {
    let details = svc_project::get(
        conn,
        &Project {
            project_fk: Some(project_id),
        },
    )
    .map_err(map_core)?
    .ok_or_else(|| ErrorData::invalid_params(format!("Project {project_id} not found."), None))?;
    if details.source_fks.is_empty() {
        return Ok(Vec::new());
    }
    svc_paper::get_many(
        conn,
        &svc_paper::Papers {
            source_fks: Some(details.source_fks),
            ..Default::default()
        },
    )
    .map_err(map_core)
}

use linxiv_core::formats::with_default_ext;

/// Canonicalize a path's parent, keeping the filename, so a not-yet-existing
/// destination still compares against the live DB. Port of `route/storage.rs`.
fn canon_or_raw(path: &Path) -> PathBuf {
    path.parent()
        .and_then(|p| p.canonicalize().ok())
        .zip(path.file_name())
        .map(|(canon_parent, fname)| canon_parent.join(fname))
        .unwrap_or_else(|| path.to_path_buf())
}

/// Guard from `route/storage.rs::reject_live_db`: refuse a relative path, or one
/// that resolves to the live database file itself.
fn reject_live_db(path: &Path, field: &str, role: &str) -> Result<(), ErrorData> {
    if !path.is_absolute() {
        return Err(ErrorData::invalid_params(
            format!("{field} must be absolute"),
            None,
        ));
    }
    let (a, b) = (canon_or_raw(path), canon_or_raw(&config::db_path()));
    // Case-insensitive comparison only on case-insensitive filesystems.
    let same = if cfg!(windows) || cfg!(target_os = "macos") {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    };
    if same {
        return Err(ErrorData::invalid_params(
            format!("{role} is the live database itself — choose another file"),
            None,
        ));
    }
    Ok(())
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
pub struct MergeAuthorsParams {
    /// Canonical author id to keep.
    pub canonical_id: i64,
    /// Duplicate author ids to merge into the canonical author and delete.
    pub duplicate_ids: Vec<i64>,
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
pub struct BackupDatabaseParams {
    /// Absolute destination path for the snapshot. Must not already exist.
    pub dest: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreDatabaseParams {
    /// Absolute path to the backup snapshot to restore from.
    pub src: String,
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
        let ExportProjectParams {
            project_id,
            dest,
            include_pdfs,
        } = params.0;
        let pdf_dir = self.pdf_dir.clone();
        let path = self.with_conn(|conn| -> Result<String, ErrorData> {
            svc_project::get(
                conn,
                &Project {
                    project_fk: Some(project_id),
                },
            )
            .map_err(map_core)?
            .ok_or_else(|| {
                ErrorData::invalid_params(format!("Project {project_id} not found."), None)
            })?;
            let out =
                svc_ei::export_project(conn, project_id, Path::new(&dest), include_pdfs, &pdf_dir)
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
        let ImportProjectParams {
            zip_path,
            on_conflict,
            preview,
        } = params.0;
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
            Ok(linxiv_core::formats::bibtex_export(&papers))
        })?;
        let out = with_default_ext(&dest, "bib");
        std::fs::write(&out, body).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
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
            Ok(linxiv_core::formats::obsidian_export(&papers))
        })?;
        let out = with_default_ext(&dest, "md");
        std::fs::write(&out, body).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        json_ok(&json!({ "path": out.to_string_lossy(), "project_id": project_id }))
    }

    #[tool(description = "Resolve a DOI to paper metadata without saving it to the library.")]
    pub async fn resolve_doi(&self, params: Parameters<DoiParams>) -> Result<String, ErrorData> {
        let meta = doi_resolve::resolve_doi(&params.0.doi, &config::data_dir())
            .await
            .map_err(map_core)?;
        json_ok(&meta)
    }

    #[tool(description = "Resolve a DOI and save the resulting paper to the local library.")]
    pub async fn save_doi(&self, params: Parameters<DoiParams>) -> Result<String, ErrorData> {
        let meta = doi_resolve::resolve_doi(&params.0.doi, &config::data_dir())
            .await
            .map_err(map_core)?;
        self.with_conn(|conn| svc_paper::save_paper_metadata(conn, &meta, None))
            .map_err(map_core)?;
        // Route parity (`POST /api/doi/save`): the resolved metadata + saved flag.
        json_ok(&json!({ "metadata": meta, "saved": true }))
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
        // The canonical AuthorWithPapers composite, shared with route + CLI.
        let detail = self
            .with_conn(|conn| svc_author::get_with_papers(conn, author_id))
            .map_err(map_core)?
            .ok_or_else(|| {
                ErrorData::invalid_params(format!("Author {author_id} not found."), None)
            })?;
        json_ok(&detail)
    }

    #[tool(description = "Update an author's fields. At least one field must be provided.")]
    pub async fn update_author(
        &self,
        params: Parameters<UpdateAuthorParams>,
    ) -> Result<String, ErrorData> {
        let UpdateAuthorParams {
            author_id,
            full_name,
            first_name,
            last_name,
            orcid,
        } = params.0;
        // update_fields owns the "exists" + "at least one field" guards (core).
        self.with_conn(|conn| -> Result<(), ErrorData> {
            svc_author::update_fields(
                conn,
                author_id,
                full_name.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                orcid.as_deref(),
            )
            .map_err(guard_err)
        })?;
        json_ok(&json!({ "updated_author_id": author_id }))
    }

    #[tool(description = "Delete an author. Blocked if the author is still linked to any papers.")]
    pub async fn delete_author(
        &self,
        params: Parameters<AuthorIdParams>,
    ) -> Result<String, ErrorData> {
        let author_id = params.0.author_id;
        // svc_author::delete owns the "exists" + "still linked" guards (core).
        self.with_conn(|conn| -> Result<(), ErrorData> {
            svc_author::delete(
                conn,
                &Author {
                    author_id: Some(author_id),
                    ..Default::default()
                },
            )
            .map_err(guard_err)
        })?;
        json_ok(&json!({ "deleted_author_id": author_id }))
    }

    #[tool(
        description = "Merge duplicate authors into one canonical author, re-pointing all their papers."
    )]
    pub async fn merge_authors(
        &self,
        params: Parameters<MergeAuthorsParams>,
    ) -> Result<String, ErrorData> {
        let MergeAuthorsParams {
            canonical_id,
            duplicate_ids,
        } = params.0;
        let merged = self.with_conn(|conn| -> Result<Vec<i64>, ErrorData> {
            if svc_author::get(
                conn,
                &Author {
                    author_id: Some(canonical_id),
                    ..Default::default()
                },
            )
            .map_err(map_core)?
            .is_none()
            {
                return Err(ErrorData::invalid_params(
                    format!("Author {canonical_id} not found."),
                    None,
                ));
            }
            svc_author::merge(conn, canonical_id, &duplicate_ids).map_err(map_core)
        })?;
        json_ok(&json!({ "canonical_id": canonical_id, "merged_ids": merged }))
    }

    #[tool(
        description = "Find likely duplicate authors — other authors sharing this author's ORCID. \
                       Feed the result to merge_authors."
    )]
    pub async fn author_merge_candidates(
        &self,
        params: Parameters<AuthorIdParams>,
    ) -> Result<String, ErrorData> {
        let author_id = params.0.author_id;
        let candidates = self.with_conn(|conn| -> Result<_, ErrorData> {
            if svc_author::get(
                conn,
                &Author {
                    author_id: Some(author_id),
                    ..Default::default()
                },
            )
            .map_err(map_core)?
            .is_none()
            {
                return Err(ErrorData::invalid_params(
                    format!("Author {author_id} not found."),
                    None,
                ));
            }
            svc_author::orcid_merge_candidates(conn, author_id).map_err(map_core)
        })?;
        json_ok(&json!({ "author_id": author_id, "candidates": candidates }))
    }

    #[tool(
        description = "Snapshot the database to a backup file. `dest` must be an absolute path \
                       that does not already exist."
    )]
    pub async fn backup_database(
        &self,
        params: Parameters<BackupDatabaseParams>,
    ) -> Result<String, ErrorData> {
        let dest = PathBuf::from(params.0.dest);
        reject_live_db(&dest, "dest", "destination")?;
        // spawn_blocking: VACUUM INTO of a large library runs for seconds to minutes,
        // and with_conn would hold a tokio worker (and the shared mutex) for all of it.
        let conn = self.conn_handle();
        let info = blocking(move || {
            let guard = conn.lock().expect("db connection mutex poisoned");
            storage::backup(&guard, &dest).map_err(map_core)
        })
        .await??;
        json_ok(&info)
    }

    #[tool(
        description = "Restore the database from a backup snapshot, replacing the current library. \
                       Refused while another process holds the database open."
    )]
    pub async fn restore_database(
        &self,
        params: Parameters<RestoreDatabaseParams>,
    ) -> Result<String, ErrorData> {
        let src = PathBuf::from(params.0.src);
        reject_live_db(&src, "src", "source")?;
        let db_path = config::db_path();
        // Validate the snapshot before parking the live connection.
        storage::validate_backup_source(&src).map_err(map_core)?;
        // spawn_blocking for the same reason as backup_database: this holds the
        // mutex across two full-file copies and a rename.
        let handle = self.conn_handle();
        blocking(move || -> Result<String, ErrorData> {
            let mut guard = handle.lock().expect("db connection mutex poisoned");
            let conn = &mut *guard;
            // Park this server's handle on an in-memory DB so core's
            // `ensure_no_live_connections` only has to refuse OTHER processes.
            let parked = storage::open_in_memory().map_err(map_core)?;
            let live = std::mem::replace(conn, parked);
            if let Err((returned, e)) = live.close() {
                *conn = returned;
                return Err(ErrorData::internal_error(
                    format!("could not close the live database: {e}"),
                    None,
                ));
            }
            let result = storage::restore(&src, &db_path).map_err(map_core);
            // Reopen whatever now sits at db_path — the server needs a working
            // handle even when the restore itself was refused.
            *conn = storage::open(&db_path)
                .and_then(|fresh| storage::init_db(&fresh).map(|()| fresh))
                .map_err(|e| {
                    ErrorData::internal_error(
                        format!("could not reopen the database — restart linXiv: {e}"),
                        None,
                    )
                })?;
            result?;
            json_ok(&json!({ "ok": true, "restored": db_path.to_string_lossy() }))
        })
        .await?
    }

    #[tool(description = "Bulk-import papers from a BibTeX (.bib) file into the library.")]
    pub async fn import_bibtex(
        &self,
        params: Parameters<ImportBibtexParams>,
    ) -> Result<String, ErrorData> {
        let ImportBibtexParams { file, project_id } = params.0;
        let text = std::fs::read_to_string(&file)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        let metas = linxiv_core::formats::bibtex_import(&text)
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
            let saved: Vec<(String, i64)> = metas
                .iter()
                .map(|meta| svc_paper::save_paper_metadata(conn, meta, None).map_err(map_core))
                .collect::<Result<_, _>>()?;
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

    #[tool(
        description = "Report library statistics: paper, tag, category, and downloaded-PDF counts."
    )]
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
        settings
            .set(key.clone(), parsed.clone())
            .map_err(map_core)?;
        json_ok(&json!({ key: parsed }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Mirrors `papers.rs`'s test `server()`: an in-memory DB, tool methods
    /// called directly rather than dispatched through `tool_router`.
    fn server() -> Server {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        Server {
            conn: Arc::new(Mutex::new(conn)),
            pdf_dir: std::env::temp_dir(),
            tool_router: Server::tools_io_authors_misc(),
        }
    }

    fn scratch_dest() -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("linxiv_mcp_backup_{n}.db"))
    }

    /// A relative destination is refused before core is reached; an absolute one
    /// writes a snapshot. Restore is not exercised: it would overwrite the real
    /// `config::db_path()`.
    #[tokio::test]
    async fn backup_rejects_relative_paths_and_writes_a_snapshot() {
        let srv = server();
        let err = srv
            .backup_database(Parameters(BackupDatabaseParams {
                dest: "backup.db".to_string(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.message.as_ref(), "dest must be absolute");

        let dest = scratch_dest();
        let out = srv
            .backup_database(Parameters(BackupDatabaseParams {
                dest: dest.to_string_lossy().into_owned(),
            }))
            .await
            .unwrap();
        let out: Value = serde_json::from_str(&out).unwrap();
        assert!(dest.exists());
        assert!(out["bytes"].as_u64().unwrap() > 0);
        std::fs::remove_file(&dest).ok();
    }

    /// The existence check runs before the candidate query.
    #[tokio::test]
    async fn merge_candidates_rejects_unknown_authors() {
        let err = server()
            .author_merge_candidates(Parameters(AuthorIdParams { author_id: 999 }))
            .await
            .unwrap_err();
        assert_eq!(err.message.as_ref(), "Author 999 not found.");
    }
}
