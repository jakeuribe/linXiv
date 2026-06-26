//! Group `project` — cmd_project_* (incl. export/import) in `linxiv_cli.py`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::json;

use linxiv_core::error::Result as CoreResult;
use linxiv_core::models::{PaperDetails, ProjectIn, ProjectUpdateIn, Status};
use linxiv_core::service::{export_import, paper, project};

use crate::ctx::Ctx;
use crate::output::{as_source_id, fail, output};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ProjectStatus {
    Active,
    Archived,
    Deleted,
}

impl ProjectStatus {
    fn to_status(self) -> Status {
        match self {
            ProjectStatus::Active => Status::Active,
            ProjectStatus::Archived => Status::Archived,
            ProjectStatus::Deleted => Status::Deleted,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OnConflict {
    Merge,
    Overwrite,
}

impl OnConflict {
    fn to_core(self) -> export_import::OnConflict {
        match self {
            OnConflict::Merge => export_import::OnConflict::Merge,
            OnConflict::Overwrite => export_import::OnConflict::Overwrite,
        }
    }
}

#[derive(Subcommand)]
pub enum ProjectCmd {
    /// List projects
    List {
        #[arg(long, value_enum)]
        status: Option<ProjectStatus>,
    },
    /// Get project details
    Get { project_id: i64 },
    /// Create a project
    Create {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Hex color (e.g. #4f86f7)
        #[arg(long)]
        color: Option<String>,
        #[arg(long, num_args = 0..)]
        tags: Option<Vec<String>>,
    },
    /// Update project fields
    Update {
        project_id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Hex color (e.g. #4f86f7)
        #[arg(long)]
        color: Option<String>,
        /// Project tags (replaces existing; pass no values to clear)
        #[arg(long, num_args = 0..)]
        tags: Option<Vec<String>>,
        #[arg(long, value_enum)]
        status: Option<ProjectStatus>,
    },
    /// Soft-delete a project
    Delete { project_id: i64 },
    /// Archive an active project
    Archive { project_id: i64 },
    /// Restore an archived or deleted project
    Restore { project_id: i64 },
    /// Permanently delete a project
    HardDelete { project_id: i64 },
    /// Add a paper to a project
    AddPaper { project_id: i64, source_id: String },
    /// Remove a paper from a project
    RemovePaper { project_id: i64, source_id: String },
    /// Export a project to a .lxproj archive
    Export {
        project_id: i64,
        /// Destination path (.lxproj extension added automatically)
        dest: String,
        /// Include bundled PDFs in the archive
        #[arg(long)]
        pdfs: bool,
    },
    /// Import a project from a .lxproj archive
    Import {
        zip_path: String,
        /// Show archive summary without modifying the database
        #[arg(long)]
        preview: bool,
        /// How to handle papers that already exist (default: merge)
        #[arg(long, value_enum, default_value_t = OnConflict::Merge)]
        on_conflict: OnConflict,
    },
    /// Export project papers as BibTeX
    ExportBibtex {
        project_id: i64,
        /// Output file path (.bib added if no extension)
        dest: String,
    },
    /// Export project papers as Obsidian markdown
    ExportObsidian {
        project_id: i64,
        /// Output file path (.md added if no extension)
        dest: String,
    },
}

/// `_resolve_project_or_exit`: fetch by id or fail with the exact Python message.
fn resolve_or_exit(ctx: &Ctx, project_id: i64) -> linxiv_core::models::ProjectDetails {
    match project::get(&ctx.conn, &project::Project { project_fk: Some(project_id) }) {
        Ok(Some(p)) => p,
        Ok(None) => fail(format!("Project {project_id} not found")),
        Err(e) => fail(e),
    }
}

fn status_str(s: Status) -> &'static str {
    match s {
        Status::Active => "active",
        Status::Archived => "archived",
        Status::Deleted => "deleted",
    }
}

/// Resolve a project's papers, mirroring `get_many(Papers(source_fks=...)) if source_fks else []`.
fn project_papers(ctx: &Ctx, source_fks: &[i64]) -> CoreResult<Vec<PaperDetails>> {
    if source_fks.is_empty() {
        return Ok(Vec::new());
    }
    paper::get_many(
        &ctx.conn,
        &paper::Papers { source_fks: Some(source_fks.to_vec()), ..Default::default() },
    )
}

pub async fn run(cmd: ProjectCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        ProjectCmd::List { status } => {
            let core_status = status.map(|s| s.to_status());
            let mut projects = project::get_many(
                &ctx.conn,
                &project::Projects { status: core_status, ..Default::default() },
            )?;
            if core_status.is_none() {
                projects.retain(|p| p.status != Status::Deleted);
            }
            #[derive(Serialize)]
            struct ListRow {
                id: Option<i64>,
                name: String,
                description: String,
                status: &'static str,
                paper_count: usize,
                color: Option<i32>,
                project_tags: Vec<String>,
            }
            let rows: Vec<ListRow> = projects
                .into_iter()
                .map(|p| ListRow {
                    id: p.id,
                    name: p.name,
                    description: p.description,
                    status: status_str(p.status),
                    paper_count: p.source_fks.len(),
                    color: p.color,
                    project_tags: p.project_tags,
                })
                .collect();
            output(&rows);
        }

        ProjectCmd::Get { project_id } => {
            let details = resolve_or_exit(ctx, project_id);
            output(&details);
        }

        ProjectCmd::Create { name, description, color, tags } => {
            // Python `color_from_hex(args.color) if args.color else None`: empty string -> no color.
            let color = match &color {
                Some(hex) if !hex.is_empty() => Some(project::color_from_hex(hex)?),
                _ => None,
            };
            let id = project::create(
                &mut ctx.conn,
                &ProjectIn {
                    name: name.clone(),
                    description,
                    color,
                    tags: tags.unwrap_or_default(),
                    source_fks: Vec::new(),
                },
            )?;
            #[derive(Serialize)]
            struct Out {
                id: i64,
                name: String,
                status: &'static str,
            }
            output(&Out { id, name, status: "active" });
        }

        ProjectCmd::Update { project_id, name, description, color, tags, status } => {
            // Mirror `_resolve_project_or_exit` before mutating.
            resolve_or_exit(ctx, project_id);
            let res = (|| -> CoreResult<()> {
                let color = match color {
                    Some(hex) => Some(Some(project::color_from_hex(&hex)?)),
                    None => None,
                };
                let upd = ProjectUpdateIn {
                    project_fk: project_id,
                    name,
                    description,
                    color,
                    project_tags: tags,
                    status: status.map(|s| s.to_status()),
                };
                project::update(&mut ctx.conn, &upd)
            })();
            if let Err(e) = res {
                fail(e);
            }
            let updated = resolve_or_exit(ctx, project_id);
            output(&updated);
        }

        ProjectCmd::Delete { project_id } => {
            resolve_or_exit(ctx, project_id);
            project::delete(&ctx.conn, &project::Project { project_fk: Some(project_id) })?;
            output(&json!({ "deleted_project_id": project_id }));
        }

        ProjectCmd::Archive { project_id } => {
            resolve_or_exit(ctx, project_id);
            project::archive(&ctx.conn, &project::Project { project_fk: Some(project_id) })?;
            output(&json!({ "archived_project_id": project_id }));
        }

        ProjectCmd::Restore { project_id } => {
            resolve_or_exit(ctx, project_id);
            project::restore(&ctx.conn, &project::Project { project_fk: Some(project_id) })?;
            output(&json!({ "restored_project_id": project_id }));
        }

        ProjectCmd::HardDelete { project_id } => {
            resolve_or_exit(ctx, project_id);
            project::hard_delete(&mut ctx.conn, &project::Project { project_fk: Some(project_id) })?;
            output(&json!({ "hard_deleted_project_id": project_id }));
        }

        ProjectCmd::AddPaper { project_id, source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            let failed = match project::add_papers(&ctx.conn, project_id, &[source_id.clone()]) {
                Ok(failed) => failed,
                Err(linxiv_core::error::CoreError::ProjectNotFound) => {
                    fail(format!("Project {project_id} not found"))
                }
                Err(e @ linxiv_core::error::CoreError::ProjectDeleted(_)) => fail(e),
                Err(e) => return Err(e.into()),
            };
            if !failed.is_empty() {
                fail(format!("Paper {source_id} not found in database"));
            }
            #[derive(Serialize)]
            struct Out {
                project_id: i64,
                source_id: String,
            }
            output(&Out { project_id, source_id });
        }

        ProjectCmd::RemovePaper { project_id, source_id } => {
            let source_id = as_source_id(&source_id, "arxiv");
            let failed = match project::remove_papers(&ctx.conn, project_id, &[source_id.clone()]) {
                Ok(failed) => failed,
                Err(linxiv_core::error::CoreError::ProjectNotFound) => {
                    fail(format!("Project {project_id} not found"))
                }
                Err(e @ linxiv_core::error::CoreError::ProjectDeleted(_)) => fail(e),
                Err(e) => return Err(e.into()),
            };
            if !failed.is_empty() {
                fail(format!("Paper {source_id} not found in database"));
            }
            #[derive(Serialize)]
            struct Out {
                project_id: i64,
                source_id: String,
                removed: bool,
            }
            output(&Out { project_id, source_id, removed: true });
        }

        ProjectCmd::Export { project_id, dest, pdfs } => {
            let out = match export_import::export_project(
                &ctx.conn,
                project_id,
                Path::new(&dest),
                pdfs,
                &ctx.pdf_dir,
            ) {
                Ok(out) => out,
                Err(e) => fail(e),
            };
            #[derive(Serialize)]
            struct Out {
                path: String,
                project_id: i64,
            }
            output(&Out { path: out.display().to_string(), project_id });
        }

        ProjectCmd::Import { zip_path, preview, on_conflict } => {
            let zip = Path::new(&zip_path);
            if preview {
                let prev = match export_import::preview_import(zip) {
                    Ok(p) => p,
                    Err(e) => fail(e),
                };
                output(&prev);
            } else {
                let fk = match export_import::commit_import(
                    &mut ctx.conn,
                    zip,
                    on_conflict.to_core(),
                    &ctx.pdf_dir,
                ) {
                    Ok(fk) => fk,
                    Err(e) => fail(e),
                };
                output(&json!({ "project_id": fk }));
            }
        }

        ProjectCmd::ExportBibtex { project_id, dest } => {
            let details = resolve_or_exit(ctx, project_id);
            let papers = project_papers(ctx, &details.source_fks)?;
            let bibtex = bibtex_export(&papers);
            let dest = with_default_ext(&dest, "bib");
            std::fs::write(&dest, bibtex)?;
            #[derive(Serialize)]
            struct Out {
                path: String,
                project_id: i64,
            }
            output(&Out { path: dest.display().to_string(), project_id });
        }

        ProjectCmd::ExportObsidian { project_id, dest } => {
            let details = resolve_or_exit(ctx, project_id);
            let papers = project_papers(ctx, &details.source_fks)?;
            let md = obsidian_export(&papers);
            let dest = with_default_ext(&dest, "md");
            std::fs::write(&dest, md)?;
            #[derive(Serialize)]
            struct Out {
                path: String,
                project_id: i64,
            }
            output(&Out { path: dest.display().to_string(), project_id });
        }
    }
    Ok(())
}

/// `Path(dest)` + `with_suffix` only when the path has no extension.
fn with_default_ext(dest: &str, ext: &str) -> PathBuf {
    let mut p = PathBuf::from(dest);
    if p.extension().is_none() {
        p.set_extension(ext);
    }
    p
}

// ── BibTeX / Obsidian formatters ─────────────────────────────────────────────
// Leaf string<->data transforms with no DB access. Mirrors `formats/bibtex.py`
// and `formats/markdown.py::ObsidianFormat`; the same port lives in linxiv-mcp's
// `formats` module. ponytail: duplicated until a shared `formats` crate exists
// (CLI cannot reach the mcp binary's module without a new dependency).

/// `BibTeXFormat.export_papers` — one `@article` entry per paper, byte-matching
/// pybtex `bib.to_string("bibtex")`: 4-space indent, `field = "value"`, no trailing
/// comma on the last field, one blank line between entries, single trailing newline.
/// ponytail: pybtex also LaTeX-encodes special chars (`%`->`\%`, accents); deferred
/// with the rest of the pybtex-strictness gap, add an encoder if goldens need it.
fn bibtex_export(papers: &[PaperDetails]) -> String {
    let mut out = String::new();
    for (i, p) in papers.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let key = bib_key(&p.source_id);
        let year = p.published.map(|d| d.format("%Y").to_string()).unwrap_or_default();
        out.push_str(&format!("@article{{{key}"));
        let mut fields: Vec<(&str, &str)> =
            vec![("title", p.title.as_str()), ("year", year.as_str()), ("abstract", p.summary.as_deref().unwrap_or(""))];
        if let Some(doi) = p.doi.as_deref().filter(|s| !s.is_empty()) {
            fields.push(("doi", doi));
        }
        if let Some(journal) = p.journal_ref.as_deref().filter(|s| !s.is_empty()) {
            fields.push(("journal", journal));
        }
        if let Some(url) = p.url.as_deref().filter(|s| !s.is_empty()) {
            fields.push(("url", url));
        }
        for (name, value) in fields {
            out.push_str(&format!(",\n    {name} = {}", bib_quote(value)));
        }
        out.push_str("\n}\n");
    }
    out
}

/// pybtex `Writer.quote`: `"value"` unless the value contains a `"`, then `{value}`.
fn bib_quote(value: &str) -> String {
    if value.contains('"') {
        format!("{{{value}}}")
    } else {
        format!("\"{value}\"")
    }
}

/// `(source_id or "unknown").replace("/","_").replace(".","_")`.
fn bib_key(source_id: &str) -> String {
    let base = if source_id.is_empty() { "unknown" } else { source_id };
    base.replace(['/', '.'], "_")
}

/// `ObsidianFormat.export_papers` — YAML frontmatter + one `##` section per paper.
fn obsidian_export(papers: &[PaperDetails]) -> String {
    let mut all_tags: BTreeSet<String> = BTreeSet::new();
    for p in papers {
        for t in &p.tags {
            all_tags.insert(t.clone());
        }
    }

    let mut lines: Vec<String> = vec!["---".into(), format!("papers: {}", papers.len())];
    if !all_tags.is_empty() {
        lines.push("tags:".into());
        for t in &all_tags {
            lines.push(format!("  - {t}"));
        }
    }
    lines.extend(["---".into(), "".into(), "# Selected Papers".into(), "".into()]);

    for p in papers {
        let sid = p.source_id.as_str();
        // Python `p.get("title", sid)`: title key always present, so an empty title stays empty.
        let title = p.title.as_str();
        let authors = p.authors.join(", ");
        let url = paper_url(sid, p.url.as_deref());
        lines.push(format!("## [{title}]({url})"));
        lines.push("".into());
        if !is_arxiv_id(sid) {
            lines.push(format!("**Paper-ID:** {sid}"));
        }
        if !authors.is_empty() {
            lines.push(format!("**Authors:** {authors}"));
        }
        if let Some(cat) = p.category.as_deref().filter(|s| !s.is_empty()) {
            lines.push(format!("**Category:** {cat}"));
        }
        if !p.tags.is_empty() {
            lines.push(format!("**Tags:** {}", p.tags.join(", ")));
        }
        lines.push("".into());
    }
    lines.join("\n")
}

/// Best URL for a paper: stored url > arXiv abs link > empty.
fn paper_url(sid: &str, stored_url: Option<&str>) -> String {
    if let Some(u) = stored_url.filter(|s| !s.is_empty()) {
        return u.to_string();
    }
    if is_arxiv_id(sid) {
        return format!("https://arxiv.org/abs/{sid}");
    }
    String::new()
}

/// Port of `_ARXIV_ID_RE`: `^\d{4}\.\d{4,5}(v\d+)?$ | ^[a-z-]+/\d{7}$`.
fn is_arxiv_id(sid: &str) -> bool {
    new_style_arxiv(sid) || old_style_arxiv(sid)
}

fn new_style_arxiv(sid: &str) -> bool {
    let head = match sid.split_once('v') {
        Some((h, v)) if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()) => h,
        Some(_) => return false,
        None => sid,
    };
    let Some((a, b)) = head.split_once('.') else { return false };
    a.len() == 4
        && a.chars().all(|c| c.is_ascii_digit())
        && (4..=5).contains(&b.len())
        && b.chars().all(|c| c.is_ascii_digit())
}

fn old_style_arxiv(sid: &str) -> bool {
    let Some((cat, num)) = sid.split_once('/') else { return false };
    !cat.is_empty()
        && cat.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        && num.len() == 7
        && num.chars().all(|c| c.is_ascii_digit())
}
