//! export_import service — Rust port of `service/export_import.py`.
//!
//! A `.lxproj` file is a zip archive: `manifest.json` (project + papers + notes,
//! all keyed by source_id — no local DB ids) plus optional `pdfs/{source_id}_v{n}.pdf`
//! entries. That archive PDF name (`{source_id}_v{version}.pdf`, WITH the `_v`
//! separator) is owned by [`ArchivePdfName`] and is DISTINCT from the on-disk
//! managed name `{safe}v{version}.pdf` (`service::paper::pdf_on_disk_name`) —
//! the separate type/helper keep the two formats unmixable; do NOT unify them.
//!
//! DI seam: every DB-touching fn takes `conn` first; every FS-touching fn takes the
//! resolved `pdf_dir: &Path` as a param. Nothing here reads config.
//!
//! ZIP I/O: `export_project`/`preview_import`/`commit_import` read and write the
//! `.lxproj` archive via the `zip` crate (ZIP_DEFLATED). They wrap the in-memory
//! manifest layer — manifest construction, preview, and the two-phase commit with
//! whole-project rollback — which is independently tested on a decoded `Manifest`
//! + PDF bytes.
//!
//! Rollback parity: a mid-import failure SOFT-DELETES the project (Python
//! `_project.delete`, i.e. trash) and surfaces `CoreError::ProjectImport`. Papers
//! saved before the failure stay in the library exactly as in Python — the
//! "rollback" is scoped to the project, undone by trashing it.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::models::{
    validate_anchor, AnnotationIn, NoteIn, PaperDetails, PaperMetadata, ProjectIn,
};
use crate::service::{annotation, note, paper, project};

const FORMAT_VERSION: i64 = 1;

/// Path-safety validator for share IDs (re-exported by transport).
pub fn valid_share_id(id: &str) -> bool {
    // ':' — Windows drive-relative ids like "C:evil" escape share_dir via PathBuf::join.
    !id.is_empty()
        && !id.starts_with('.')
        && !id.contains(['/', '\\', ':'])
        && !id.contains("..")
        && !std::path::Path::new(id).is_absolute()
}

/// Merge vs. overwrite behaviour for papers whose source_id already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    /// Keep the stored paper metadata; just (re)link it to the imported project.
    Merge,
    /// Re-write stored paper metadata from the archive (`repair_paper`).
    Overwrite,
}

// ── Manifest wire model (mirrors the Python manifest dict) ───────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "default_format_version")]
    pub format_version: i64,
    #[serde(default)]
    pub exported_at: Option<String>,
    #[serde(default)]
    pub summary: Summary,
    pub project: ProjectEntry,
    #[serde(default)]
    pub papers: Vec<PaperEntry>,
    #[serde(default)]
    pub notes: Vec<NoteEntry>,
    /// PDF highlight annotations. `#[serde(default)]` so archives written before
    /// annotations existed still import (empty list).
    #[serde(default)]
    pub annotations: Vec<AnnotationEntry>,
}

fn default_format_version() -> i64 {
    1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    #[serde(default)]
    pub paper_count: usize,
    #[serde(default)]
    pub note_count: usize,
    #[serde(default)]
    pub annotation_count: usize,
    #[serde(default)]
    pub has_pdfs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub color_hex: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Persisted share identity; restored on import when the target has none.
    #[serde(default)]
    pub share_id: Option<String>,
}

/// Archive paper record — mirrors `_serialize_paper`/`_deserialize_paper`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperEntry {
    pub source_id: String,
    #[serde(default = "default_version")]
    pub version: i64,
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    /// Index-aligned with `authors`; empty on old exports (no ORCID data to fill).
    #[serde(default)]
    pub author_orcids: Vec<Option<String>>,
    #[serde(default)]
    pub published: Option<chrono::NaiveDate>,
    #[serde(default)]
    pub updated: Option<chrono::NaiveDate>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub journal_ref: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
}

fn default_version() -> i64 {
    1
}

impl PaperEntry {
    fn from_details(conn: &Connection, p: &PaperDetails) -> Result<Self> {
        let author_orcids = crate::storage::queries::author::get_paper_authors(conn, p.paper_id)?
            .into_iter()
            .map(|a| a.orcid)
            .collect();
        Ok(PaperEntry {
            source_id: p.source_id.clone(),
            version: p.version,
            title: p.title.clone(),
            authors: p.authors.clone(),
            author_orcids,
            published: p.published,
            updated: p.updated,
            summary: p.summary.clone().unwrap_or_default(),
            category: p.category.clone(),
            categories: p.categories.clone(),
            doi: p.doi.clone(),
            journal_ref: p.journal_ref.clone(),
            comment: p.comment.clone(),
            url: p.url.clone(),
            tags: p.tags.clone(),
            source: p.source.clone(),
        })
    }

    /// `_deserialize_paper` — archive record → `PaperMetadata`. Missing `published`
    /// falls back to today (Python `date.today()`); empty list fields collapse to None.
    fn to_metadata(&self) -> PaperMetadata {
        PaperMetadata {
            source_id: self.source_id.clone(),
            version: self.version,
            title: self.title.clone(),
            authors: self.authors.clone(),
            published: self.published.unwrap_or_else(|| Utc::now().date_naive()),
            updated: self.updated,
            summary: self.summary.clone(),
            category: self.category.clone(),
            categories: (!self.categories.is_empty()).then(|| self.categories.clone()),
            doi: self.doi.clone(),
            journal_ref: self.journal_ref.clone(),
            comment: self.comment.clone(),
            url: self.url.clone(),
            tags: (!self.tags.is_empty()).then(|| self.tags.clone()),
            source: self.source.clone(),
            author_orcids: (!self.author_orcids.is_empty()).then(|| self.author_orcids.clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteEntry {
    #[serde(default)]
    pub paper_source_id: Option<String>,
    #[serde(default)]
    pub paper_version: Option<i64>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    /// Stable note identity; None on pre-uuid archives (a fresh one is generated).
    #[serde(default)]
    pub uuid: Option<String>,
}

/// Archive PDF-annotation record — keyed by source_id like notes. The version the
/// coords were measured against lives inside the opaque `anchor` JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationEntry {
    pub paper_source_id: String,
    pub anchor: String,
    #[serde(default)]
    pub comment: String,
    /// Stable annotation identity; None on pre-uuid archives.
    #[serde(default)]
    pub uuid: Option<String>,
}

/// Archive PDF name: in-zip path `pdfs/{source_id}_v{version}.pdf` (WITH the
/// `_v` separator). Owns both directions of the archive format. DISTINCT from
/// the on-disk managed name `{safe}v{version}.pdf`
/// (`service::paper::pdf_on_disk_name`) — the two types keep the formats
/// unmixable; do NOT unify them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePdfName {
    pub source_id: String,
    pub version: i64,
}

impl ArchivePdfName {
    /// Decode an in-zip entry path. Returns `None` for entries the import
    /// loop skips: non-`.pdf` names and stems without `_v`. (The `pdfs/`
    /// prefix is filtered at the zip layer; here any directory prefix is
    /// dropped via the basename.) Splits on the LAST `_v` — the encoded
    /// `_v{version}` suffix is always the last one, so source_ids that
    /// themselves contain `_v` round-trip. A non-numeric version falls back
    /// to 1 (Python import parity).
    pub fn parse_entry(archive_name: &str) -> Option<Self> {
        let basename = archive_name.rsplit('/').next().unwrap_or(archive_name);
        let stem = basename.strip_suffix(".pdf")?;
        let sep = stem.rfind("_v")?;
        Some(Self {
            source_id: stem[..sep].to_string(),
            version: stem[sep + 2..].parse().unwrap_or(1),
        })
    }
}

impl std::fmt::Display for ArchivePdfName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pdfs/{}_v{}.pdf", self.source_id, self.version)
    }
}

/// A decoded archive PDF entry. `archive_name` is the in-zip path, e.g.
/// `pdfs/2204.12985_v1.pdf`; the zip layer fills `bytes` from the archive.
#[derive(Debug, Clone)]
pub struct ArchivePdf {
    pub archive_name: String,
    pub bytes: Vec<u8>,
}

/// `ImportPreview` — what `commit_import` would do, read without touching the DB.
#[derive(Debug, Clone, Serialize)]
pub struct ImportPreview {
    pub project_name: String,
    pub description: String,
    pub paper_count: usize,
    pub note_count: usize,
    pub annotation_count: usize,
    pub has_pdfs: bool,
    pub format_version: i64,
}

// ── Export ───────────────────────────────────────────────────────────────────

/// Build the in-memory manifest for a project plus the list of
/// `(archive_name, local_pdf_path)` entries to bundle. This is all of
/// `export_project` except the final zip packaging. `pdf_dir` resolves any
/// relative `PDF_PATH` stored on a paper row.
pub fn build_manifest(
    conn: &Connection,
    project_fk: i64,
    include_pdfs: bool,
    pdf_dir: &Path,
) -> Result<(Manifest, Vec<(String, PathBuf)>)> {
    let details = project::get_required(conn, project_fk)?;

    let papers = paper::get_many(
        conn,
        &paper::Papers {
            source_fks: Some(details.source_fks.clone()),
            ..Default::default()
        },
    )?;

    let paper_entries: Vec<PaperEntry> = papers
        .iter()
        .map(|p| PaperEntry::from_details(conn, p))
        .collect::<Result<Vec<_>>>()?;
    let note_entries = collect_note_entries(conn, project_fk)?;
    let annotation_entries = collect_annotation_entries(conn, project_fk)?;
    let pdf_files = if include_pdfs {
        collect_pdf_files(&papers, pdf_dir)
    } else {
        Vec::new()
    };

    // Python: `color_to_hex(details.color) if details.color else None` — 0 is falsy.
    let color_hex = details.color.filter(|&c| c != 0).map(project::color_to_hex);

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        exported_at: Some(Utc::now().to_rfc3339()),
        summary: Summary {
            paper_count: paper_entries.len(),
            note_count: note_entries.len(),
            annotation_count: annotation_entries.len(),
            has_pdfs: !pdf_files.is_empty(),
        },
        project: ProjectEntry {
            name: details.name,
            description: details.description,
            color_hex,
            tags: details.project_tags,
            share_id: details.share_id,
        },
        papers: paper_entries,
        notes: note_entries,
        annotations: annotation_entries,
    };
    Ok((manifest, pdf_files))
}

/// Archive note records for a project's notes, keyed by source_id.
fn collect_note_entries(conn: &Connection, project_fk: i64) -> Result<Vec<NoteEntry>> {
    let notes = note::get_many(
        conn,
        &note::Notes {
            project_fk: Some(project_fk),
            ..Default::default()
        },
    )?;
    let mut note_entries = Vec::new();
    for n in &notes {
        let Some(source_id) = paper::get_source_id(conn, n.source_fk)? else {
            continue; // Python skips notes whose source_id no longer resolves.
        };
        let version = match n.paper_id_fk {
            Some(pid) => paper::get(conn, &paper::PaperRef::Id(pid))?.map(|p| p.version),
            None => None,
        };
        note_entries.push(NoteEntry {
            paper_source_id: Some(source_id),
            paper_version: version,
            title: n.title.clone(),
            content: n.content.clone(),
            uuid: Some(n.uuid.clone()),
        });
    }
    Ok(note_entries)
}

/// Archive annotation records for a project's annotations, keyed by source_id.
fn collect_annotation_entries(conn: &Connection, project_fk: i64) -> Result<Vec<AnnotationEntry>> {
    let annotations = annotation::get_many(
        conn,
        &annotation::Annotations {
            project_fk: Some(project_fk),
            ..Default::default()
        },
    )?;
    let mut annotation_entries = Vec::new();
    for a in &annotations {
        let Some(source_id) = paper::get_source_id(conn, a.source_fk)? else {
            continue; // skip annotations whose source_id no longer resolves.
        };
        annotation_entries.push(AnnotationEntry {
            paper_source_id: source_id,
            anchor: a.anchor.clone(),
            comment: a.comment.clone(),
            uuid: Some(a.uuid.clone()),
        });
    }
    Ok(annotation_entries)
}

/// archive_name -> local path; only files that actually exist on disk.
fn collect_pdf_files(papers: &[PaperDetails], pdf_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut pdf_files: Vec<(String, PathBuf)> = Vec::new();
    for p in papers {
        let Some(stored) = &p.pdf_path else { continue };
        let local = pdf_dir.join(stored);
        if local.is_file() {
            let name = ArchivePdfName {
                source_id: p.source_id.clone(),
                version: p.version,
            };
            pdf_files.push((name.to_string(), local));
        }
    }
    pdf_files
}

/// Write a project to a `.lxproj` ZIP_DEFLATED archive at `dest_path` (`.lxproj`
/// is forced as the extension): `manifest.json` (pretty JSON) plus each bundled
/// PDF under its archive name `pdfs/{source_id}_v{version}.pdf`.
pub fn export_project(
    conn: &Connection,
    project_fk: i64,
    dest_path: &Path,
    include_pdfs: bool,
    pdf_dir: &Path,
) -> Result<PathBuf> {
    let (manifest, pdf_files) = build_manifest(conn, project_fk, include_pdfs, pdf_dir)?;
    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|e| CoreError::Internal(e.to_string()))?;
    let dest = dest_path.with_extension("lxproj");

    let file = std::fs::File::create(&dest).map_err(|e| CoreError::Internal(e.to_string()))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let zerr = |e: zip::result::ZipError| CoreError::Internal(e.to_string());
    let werr = |e: std::io::Error| CoreError::Internal(e.to_string());

    zw.start_file("manifest.json", opts).map_err(zerr)?;
    zw.write_all(manifest_json.as_bytes()).map_err(werr)?;

    for (archive_name, local_path) in &pdf_files {
        let bytes = std::fs::read(local_path).map_err(werr)?;
        zw.start_file(archive_name.as_str(), opts).map_err(zerr)?;
        zw.write_all(&bytes).map_err(werr)?;
    }
    zw.finish().map_err(zerr)?;
    Ok(dest)
}

// ── Import — two-phase ───────────────────────────────────────────────────────

/// `preview_import` over an already-decoded manifest (no DB, no zip).
pub fn preview_from_manifest(manifest: &Manifest) -> ImportPreview {
    // Archives predating the summary counts store 0; fall back to the array length.
    ImportPreview {
        project_name: manifest.project.name.clone(),
        description: manifest.project.description.clone(),
        paper_count: if manifest.summary.paper_count > 0 {
            manifest.summary.paper_count
        } else {
            manifest.papers.len()
        },
        note_count: if manifest.summary.note_count > 0 {
            manifest.summary.note_count
        } else {
            manifest.notes.len()
        },
        // The annotations array is the ground truth at preview time.
        annotation_count: manifest.annotations.len(),
        has_pdfs: manifest.summary.has_pdfs,
        format_version: manifest.format_version,
    }
}

/// Read + parse `manifest.json` from an open archive. Mirrors Python `_read_manifest`:
/// a missing entry is a "not a valid .lxproj file" error.
fn read_manifest<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    label: &str,
) -> Result<Manifest> {
    let mut entry = archive.by_name("manifest.json").map_err(|_| {
        CoreError::BadRequest(format!(
            "{label} is not a valid .lxproj file: manifest.json missing"
        ))
    })?;
    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|e| CoreError::Internal(e.to_string()))?;
    serde_json::from_slice(&buf).map_err(|e| CoreError::Internal(e.to_string()))
}

/// Open a `.lxproj` archive at `zip_path`.
fn open_archive(zip_path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    let file = std::fs::File::open(zip_path).map_err(|e| CoreError::Internal(e.to_string()))?;
    zip::ZipArchive::new(file).map_err(|e| CoreError::Internal(e.to_string()))
}

/// Parse a `.lxproj` archive and return a preview without touching the DB.
pub fn preview_import(zip_path: &Path) -> Result<ImportPreview> {
    let mut archive = open_archive(zip_path)?;
    let manifest = read_manifest(&mut archive, &zip_path.display().to_string())?;
    Ok(preview_from_manifest(&manifest))
}

/// Two-phase commit over an already-decoded manifest + decoded PDF bytes. Creates a
/// fresh project, imports papers (merge/overwrite), links them, writes bundled PDFs,
/// then imports notes. On ANY failure the project is soft-deleted (trash) and a
/// `CoreError::ProjectImport` is returned — papers saved before the failure remain
/// (Python parity). Returns the new project_fk.
pub fn commit_from_manifest(
    conn: &mut Connection,
    manifest: &Manifest,
    pdfs: &[ArchivePdf],
    on_conflict: OnConflict,
    pdf_dir: &Path,
) -> Result<i64> {
    let color = match &manifest.project.color_hex {
        Some(hex) => Some(project::color_from_hex(hex)?),
        None => None,
    };

    let project_fk = project::create(
        conn,
        &ProjectIn {
            name: manifest.project.name.clone(),
            description: manifest.project.description.clone(),
            color,
            tags: manifest.project.tags.clone(),
            source_fks: Vec::new(),
        },
    )?;
    match commit_body(conn, project_fk, manifest, pdfs, on_conflict, pdf_dir) {
        Ok(()) => {
            // Restore the archived share identity after a successful import.
            if let Some(share_id) = &manifest.project.share_id {
                if let Ok(u) = uuid::Uuid::parse_str(share_id) {
                    if let Err(e) = project::adopt_share_id(conn, project_fk, &u.to_string()) {
                        tracing::warn!("share_id adoption failed for project {project_fk}: {e}");
                    }
                }
            }
            Ok(project_fk)
        }
        Err(e) => {
            // Trash the partially-built project (Python `_project.delete`).
            tracing::warn!("import failed, trashing project {project_fk}: {e}");
            let _ = project::delete(
                conn,
                &project::Project {
                    project_fk: Some(project_fk),
                },
            );
            Err(CoreError::ProjectImport(e.to_string()))
        }
    }
}

fn commit_body(
    conn: &mut Connection,
    project_fk: i64,
    manifest: &Manifest,
    pdfs: &[ArchivePdf],
    on_conflict: OnConflict,
    pdf_dir: &Path,
) -> Result<()> {
    // Resolve every archived paper to a SOURCE_FK, in first-seen order.
    let mut source_ids: Vec<String> = Vec::new();
    for pe in &manifest.papers {
        let source_id = pe.source_id.clone();
        match paper::get_paper_root(conn, &source_id)? {
            Some(root) => {
                if root.status == "deleted" {
                    paper::restore(conn, &paper::PaperRef::source(source_id.clone()))?;
                }
                if on_conflict == OnConflict::Overwrite {
                    // Unvalidated: an archive replays already-stored metadata, so it
                    // is not held to the Paper Repair input rules the front doors apply.
                    paper::repair_paper_unvalidated(conn, root.source_fk, &pe.to_metadata())?;
                }
                // UNION the archive paper's tags onto the existing paper (Python
                // `_paper.add_paper_tags`) so a merge-import never discards them.
                if !pe.tags.is_empty() {
                    paper::add_paper_tags(conn, &source_id, &pe.tags)?;
                }
            }
            None => {
                let meta = pe.to_metadata();
                let extra = (!pe.tags.is_empty()).then(|| pe.tags.clone());
                paper::save_paper_metadata(conn, &meta, extra.as_deref())?;
                paper::ensure_paper_root(conn, &source_id)?;
            }
        }
        source_ids.push(source_id);
    }

    // Link every imported paper to the new project. An unresolved id here means the
    // saved id and the membership lookup disagree on id form (e.g. surrounding
    // whitespace) — fail so the project rolls back rather than linking partially.
    if !source_ids.is_empty() {
        let failed = project::add_papers(conn, project_fk, &source_ids)?;
        if !failed.is_empty() {
            let shown: Vec<_> = failed.iter().take(5).cloned().collect();
            return Err(CoreError::ProjectImport(format!(
                "{} imported paper(s) could not be linked to the project: {:?}",
                failed.len(),
                shown
            )));
        }
    }

    import_pdfs(conn, pdfs, &source_ids, pdf_dir)?;
    import_notes(conn, project_fk, manifest, &source_ids)?;
    import_annotations(conn, project_fk, manifest, &source_ids)?;
    Ok(())
}

/// Write bundled PDFs into `pdf_dir` under their ARCHIVE basename
/// (`{source_id}_v{version}.pdf` — kept verbatim, NOT the on-disk `v` form) and
/// record the path on the matching paper version. A PDF naming a version that
/// wasn't imported is skipped and its extracted file removed (Python parity).
fn import_pdfs(
    conn: &mut Connection,
    pdfs: &[ArchivePdf],
    source_ids: &[String],
    pdf_dir: &Path,
) -> Result<()> {
    if pdfs.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(pdf_dir).map_err(|e| CoreError::Internal(e.to_string()))?;

    for entry in pdfs {
        let Some(name) = ArchivePdfName::parse_entry(&entry.archive_name) else {
            continue;
        };
        if !source_ids.iter().any(|s| s == &name.source_id) {
            continue;
        }

        // Dest keeps the archive basename VERBATIM (not re-encoded): a
        // non-canonical stem like `a_v01.pdf` must land on disk unchanged.
        let basename = entry
            .archive_name
            .rsplit('/')
            .next()
            .unwrap_or(&entry.archive_name);
        let dest = pdf_dir.join(basename);
        std::fs::write(&dest, &entry.bytes).map_err(|e| CoreError::Internal(e.to_string()))?;
        let dest_str = dest.to_string_lossy().to_string();

        if let Err(e) = paper::mark_pdf_saved(conn, &name.source_id, &dest_str, name.version) {
            // Bundled PDF names a version that wasn't imported: drop the file, log, skip.
            let _ = std::fs::remove_file(&dest);
            tracing::warn!("import: skipping PDF {basename}: {e}");
        }
    }
    Ok(())
}

fn import_notes(
    conn: &Connection,
    project_fk: i64,
    manifest: &Manifest,
    source_ids: &[String],
) -> Result<()> {
    for nd in &manifest.notes {
        let Some(paper_source_id) = &nd.paper_source_id else {
            continue;
        };
        if !source_ids.iter().any(|s| s == paper_source_id) {
            continue;
        }
        let source_fk = match paper::get_paper_root(conn, paper_source_id)? {
            Some(r) => r.source_fk,
            None => continue,
        };

        // Re-pin to a specific PAPER version if the note named one.
        let paper_id = match nd.paper_version {
            Some(v) if v != 0 => paper::get(
                conn,
                &paper::PaperRef::Source {
                    source_id: paper_source_id.clone(),
                    version: Some(v),
                },
            )?
            .map(|p| p.paper_id),
            _ => None,
        };

        note::create(
            conn,
            &NoteIn {
                source_fk,
                title: nd.title.clone(),
                content: nd.content.clone(),
                paper_id,
                project_fk: Some(project_fk),
                uuid: nd.uuid.clone(),
            },
        )?;
    }
    Ok(())
}

fn import_annotations(
    conn: &Connection,
    project_fk: i64,
    manifest: &Manifest,
    source_ids: &[String],
) -> Result<()> {
    for ad in &manifest.annotations {
        // Same anchor rule as the live write boundaries, but skip-not-fail: one
        // bad archived annotation must not abort the whole import.
        if let Err(msg) = validate_anchor(&ad.anchor) {
            tracing::warn!(
                "import: skipping annotation for '{}': {msg}",
                ad.paper_source_id
            );
            continue;
        }
        if !source_ids.iter().any(|s| s == &ad.paper_source_id) {
            continue;
        }
        let source_fk = match paper::get_paper_root(conn, &ad.paper_source_id)? {
            Some(r) => r.source_fk,
            None => continue,
        };
        annotation::create(
            conn,
            &AnnotationIn {
                source_fk,
                anchor: ad.anchor.clone(),
                comment: ad.comment.clone(),
                project_fk: Some(project_fk),
                uuid: ad.uuid.clone(),
            },
        )?;
    }
    Ok(())
}

/// Parse a `.lxproj` archive (manifest + every `pdfs/*.pdf` entry) and import it.
/// Returns the new project_fk.
pub fn commit_import(
    conn: &mut Connection,
    zip_path: &Path,
    on_conflict: OnConflict,
    pdf_dir: &Path,
) -> Result<i64> {
    let mut archive = open_archive(zip_path)?;
    let manifest = read_manifest(&mut archive, &zip_path.display().to_string())?;

    // Collect every bundled PDF entry (basename parsing happens in import_pdfs).
    let mut pdfs = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        let name = entry.name().to_string();
        if name.starts_with("pdfs/") && name.ends_with(".pdf") {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| CoreError::Internal(e.to_string()))?;
            pdfs.push(ArchivePdf {
                archive_name: name,
                bytes,
            });
        }
    }

    commit_from_manifest(conn, &manifest, &pdfs, on_conflict, pdf_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AnnotationIn, Status};
    use crate::test_support::db;
    use chrono::NaiveDate;

    // ── ArchivePdfName (pins the exact legacy import-loop behavior) ─────────

    #[test]
    fn archive_pdf_name_display() {
        let n = ArchivePdfName {
            source_id: "2204.12985".into(),
            version: 1,
        };
        assert_eq!(n.to_string(), "pdfs/2204.12985_v1.pdf");
    }

    #[test]
    fn archive_pdf_name_parse_skips_and_accepts_like_import_loop() {
        // Skipped: non-.pdf suffix, stem without "_v".
        assert_eq!(ArchivePdfName::parse_entry("pdfs/a_v1.txt"), None);
        assert_eq!(ArchivePdfName::parse_entry("pdfs/av1.pdf"), None);
        // Accepted regardless of directory prefix — only the basename is
        // parsed (the `pdfs/` filter lives at the zip layer).
        for path in [
            "pdfs/a_v2.pdf",
            "other/a_v2.pdf",
            "a_v2.pdf",
            "x/y/a_v2.pdf",
        ] {
            assert_eq!(
                ArchivePdfName::parse_entry(path),
                Some(ArchivePdfName {
                    source_id: "a".into(),
                    version: 2,
                }),
                "{path}"
            );
        }
        // Non-numeric or empty version falls back to 1.
        assert_eq!(
            ArchivePdfName::parse_entry("pdfs/a_vX.pdf")
                .unwrap()
                .version,
            1
        );
        assert_eq!(
            ArchivePdfName::parse_entry("pdfs/a_v.pdf").unwrap().version,
            1
        );
    }

    #[test]
    fn archive_pdf_name_round_trips() {
        // Property-style: parse(display(n)) == n. Includes source_ids that
        // themselves contain "_v" — rfind splits on the LAST "_v", which is
        // always the encoded `_v{version}` suffix.
        for sid in [
            "2204.12985",
            "arxiv:1",
            "foo_v2_bar",
            "a_v",
            "_v",
            "x_v5",
            "",
        ] {
            for version in [0, 1, 7, 12, 130] {
                let n = ArchivePdfName {
                    source_id: sid.into(),
                    version,
                };
                assert_eq!(ArchivePdfName::parse_entry(&n.to_string()), Some(n));
            }
        }
    }

    fn meta(source_id: &str, version: i64, title: &str, tags: &[&str]) -> PaperMetadata {
        PaperMetadata {
            source_id: source_id.into(),
            version,
            title: title.into(),
            authors: vec!["Alice".into()],
            published: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            updated: None,
            summary: "sum".into(),
            category: Some("cs.LG".into()),
            categories: Some(vec!["cs.LG".into()]),
            doi: None,
            journal_ref: None,
            comment: None,
            url: Some("http://x".into()),
            tags: (!tags.is_empty()).then(|| tags.iter().map(|t| t.to_string()).collect()),
            source: Some("arxiv".into()),
            author_orcids: None,
        }
    }

    fn paper_entry(source_id: &str, version: i64, title: &str, tags: &[&str]) -> PaperEntry {
        PaperEntry {
            source_id: source_id.into(),
            version,
            title: title.into(),
            authors: vec!["Bob".into()],
            author_orcids: vec![],
            published: NaiveDate::from_ymd_opt(2023, 5, 5),
            updated: None,
            summary: "s".into(),
            category: Some("cs.AI".into()),
            categories: vec!["cs.AI".into()],
            doi: None,
            journal_ref: None,
            comment: None,
            url: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            source: Some("arxiv".into()),
        }
    }

    fn base_manifest(name: &str, papers: Vec<PaperEntry>, notes: Vec<NoteEntry>) -> Manifest {
        Manifest {
            format_version: 1,
            exported_at: None,
            summary: Summary::default(),
            project: ProjectEntry {
                name: name.into(),
                description: "desc".into(),
                color_hex: Some("#00ff00".into()),
                tags: vec!["imported".into()],
                share_id: None,
            },
            papers,
            notes,
            annotations: Vec::new(),
        }
    }

    const ANCHOR: &str = r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##;

    #[test]
    fn build_and_commit_round_trip_annotations() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();

        // Seed a paper + project + a project-scoped annotation on it.
        paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "P", &[]), None).unwrap();
        let fk1 = paper::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
        let pid = project::create(
            &mut conn,
            &ProjectIn {
                name: "Proj".into(),
                description: "d".into(),
                color: None,
                tags: vec![],
                source_fks: vec![fk1],
            },
        )
        .unwrap();
        annotation::create(
            &conn,
            &AnnotationIn {
                source_fk: fk1,
                anchor: ANCHOR.into(),
                comment: "hi".into(),
                project_fk: Some(pid),
                uuid: None,
            },
        )
        .unwrap();

        let (m, _) = build_manifest(&conn, pid, false, tmp.path()).unwrap();
        assert_eq!(m.annotations.len(), 1);
        assert_eq!(m.annotations[0].paper_source_id, "arxiv:1");
        assert_eq!(m.annotations[0].comment, "hi");
        let exported_uuid = m.annotations[0].uuid.clone().expect("annotation uuid");
        assert_eq!(exported_uuid.len(), 36);

        // Commit into a fresh DB and confirm the annotation lands project-scoped.
        let mut conn2 = db();
        let new_pid =
            commit_from_manifest(&mut conn2, &m, &[], OnConflict::Merge, tmp.path()).unwrap();
        let anns = annotation::get_many(
            &conn2,
            &annotation::Annotations {
                project_fk: Some(new_pid),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(anns.len(), 1);
        assert_eq!(anns[0].anchor, ANCHOR);
        assert_eq!(anns[0].comment, "hi");
        assert_eq!(anns[0].uuid, exported_uuid);
    }

    #[test]
    fn build_manifest_round_trips_project_papers_notes_and_pdfs() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();
        let pdf_dir = tmp.path();

        // Two papers; one has a PDF on disk (relative path resolved against pdf_dir).
        paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "Paper One", &["ml"]), None)
            .unwrap();
        paper::save_paper_metadata(&mut conn, &meta("arxiv:2", 1, "Paper Two", &[]), None).unwrap();
        let fk1 = paper::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
        let fk2 = paper::ensure_paper_root(&mut conn, "arxiv:2").unwrap();
        std::fs::write(pdf_dir.join("arxiv_1v1.pdf"), b"PDFBYTES").unwrap();
        paper::set_pdf_path(&conn, "arxiv:1", "arxiv_1v1.pdf", Some(1)).unwrap();

        let pid = project::create(
            &mut conn,
            &ProjectIn {
                name: "My Proj".into(),
                description: "d".into(),
                color: Some(0x00ff00),
                tags: vec!["t".into()],
                source_fks: vec![fk1, fk2],
            },
        )
        .unwrap();

        // A note pinned to arxiv:1 v1.
        let pid_v1 = paper::get(
            &conn,
            &paper::PaperRef::Source {
                source_id: "arxiv:1".into(),
                version: Some(1),
            },
        )
        .unwrap()
        .unwrap()
        .paper_id;
        note::create(
            &conn,
            &NoteIn {
                source_fk: fk1,
                title: "n".into(),
                content: "body".into(),
                paper_id: Some(pid_v1),
                project_fk: Some(pid),
                uuid: None,
            },
        )
        .unwrap();

        let (m, pdf_files) = build_manifest(&conn, pid, true, pdf_dir).unwrap();
        assert_eq!(m.project.name, "My Proj");
        assert_eq!(m.project.color_hex.as_deref(), Some("#00ff00"));
        assert_eq!(m.papers.len(), 2);
        assert_eq!(m.summary.paper_count, 2);
        assert!(m.summary.has_pdfs);
        assert_eq!(m.notes.len(), 1);
        assert_eq!(m.notes[0].paper_source_id.as_deref(), Some("arxiv:1"));
        assert_eq!(m.notes[0].paper_version, Some(1));
        // Archive PDF name uses the `_v` separator, not the on-disk `v` form.
        assert_eq!(pdf_files.len(), 1);
        assert_eq!(pdf_files[0].0, "pdfs/arxiv:1_v1.pdf");
        let _ = fk2;
    }

    #[test]
    fn build_manifest_missing_project_is_typed_not_found() {
        let conn = db();
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            build_manifest(&conn, 999, false, tmp.path()).unwrap_err(),
            CoreError::ProjectNotFound(999)
        ));
    }

    #[test]
    fn preview_from_manifest_uses_summary_then_falls_back_to_lengths() {
        let mut m = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
        // summary zeroed -> fall back to vec lengths
        let p = preview_from_manifest(&m);
        assert_eq!(p.paper_count, 1);
        assert_eq!(p.note_count, 0);
        assert_eq!(p.project_name, "P");
        // explicit summary wins
        m.summary = Summary {
            paper_count: 7,
            note_count: 3,
            annotation_count: 0,
            has_pdfs: true,
        };
        let p = preview_from_manifest(&m);
        assert_eq!(p.paper_count, 7);
        assert_eq!(p.note_count, 3);
        assert!(p.has_pdfs);
    }

    #[test]
    fn commit_creates_project_links_papers_notes_and_writes_pdf() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();
        let pdf_dir = tmp.path();

        let manifest = base_manifest(
            "Imported",
            vec![paper_entry("arxiv:1", 1, "New Paper", &["ml"])],
            vec![NoteEntry {
                paper_source_id: Some("arxiv:1".into()),
                paper_version: Some(1),
                title: "note".into(),
                content: "c".into(),
                uuid: None,
            }],
        );
        let pdfs = vec![ArchivePdf {
            archive_name: "pdfs/arxiv:1_v1.pdf".into(),
            bytes: b"BYTES".to_vec(),
        }];

        let pid =
            commit_from_manifest(&mut conn, &manifest, &pdfs, OnConflict::Merge, pdf_dir).unwrap();

        // Project created with tags + colour from the manifest.
        let got = project::get(
            &conn,
            &project::Project {
                project_fk: Some(pid),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.name, "Imported");
        assert_eq!(got.project_tags, vec!["imported"]);
        assert_eq!(got.color, Some(0x00ff00));
        assert_eq!(got.source_fks.len(), 1, "paper linked to the new project");

        // Paper metadata saved.
        let p = paper::get(&conn, &paper::PaperRef::source("arxiv:1".into()))
            .unwrap()
            .unwrap();
        assert_eq!(p.title, "New Paper");

        // PDF written under the archive basename + recorded on the paper.
        let on_disk = pdf_dir.join("arxiv:1_v1.pdf");
        assert!(on_disk.is_file(), "archive-named pdf written to disk");
        assert_eq!(
            p.pdf_path.as_deref(),
            Some(on_disk.to_string_lossy().as_ref())
        );
        assert!(p.has_pdf);

        // Note imported and pinned.
        let notes = note::get_many(
            &conn,
            &note::Notes {
                project_fk: Some(pid),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "note");
        assert!(notes[0].paper_id_fk.is_some());
    }

    #[test]
    fn commit_merge_keeps_existing_metadata_overwrite_replaces_it() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = base_manifest(
            "P",
            vec![paper_entry("arxiv:1", 1, "Archive Title", &[])],
            vec![],
        );

        // MERGE: pre-existing paper keeps its stored title, still gets linked.
        {
            let mut conn = db();
            paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "Stored Title", &[]), None)
                .unwrap();
            let pid =
                commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path())
                    .unwrap();
            let p = paper::get(&conn, &paper::PaperRef::source("arxiv:1".into()))
                .unwrap()
                .unwrap();
            assert_eq!(p.title, "Stored Title", "merge does not overwrite metadata");
            assert_eq!(
                project::get(
                    &conn,
                    &project::Project {
                        project_fk: Some(pid)
                    }
                )
                .unwrap()
                .unwrap()
                .source_fks
                .len(),
                1
            );
        }
        // OVERWRITE: stored paper is repaired to the archive's title.
        {
            let mut conn = db();
            paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "Stored Title", &[]), None)
                .unwrap();
            commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Overwrite, tmp.path())
                .unwrap();
            let p = paper::get(&conn, &paper::PaperRef::source("arxiv:1".into()))
                .unwrap()
                .unwrap();
            assert_eq!(
                p.title, "Archive Title",
                "overwrite repairs metadata from the archive"
            );
        }
    }

    #[test]
    fn commit_restores_soft_deleted_paper_on_merge() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();
        paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "T", &[]), None).unwrap();
        paper::delete(&mut conn, &paper::PaperRef::source("arxiv:1".into())).unwrap();
        assert!(paper::is_paper_deleted(&conn, "arxiv:1").unwrap());

        let manifest = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
        commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
        assert!(
            !paper::is_paper_deleted(&conn, "arxiv:1").unwrap(),
            "import restored the trashed paper"
        );
    }

    #[test]
    fn commit_rolls_back_project_when_a_paper_cannot_be_linked() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();
        // Whitespace in the source_id: it's saved verbatim, but add_papers trims it,
        // so the membership lookup misses -> link fails -> whole project rolled back.
        let manifest = base_manifest(
            "Doomed",
            vec![paper_entry("  spacey:1  ", 1, "T", &[])],
            vec![],
        );

        let err = commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path())
            .unwrap_err();
        assert!(matches!(err, CoreError::ProjectImport(_)));

        // The project must NOT survive as active — it was trashed (Python `_project.delete`).
        let active = project::get_many(
            &conn,
            &project::Projects {
                status: Some(Status::Active),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            active.iter().all(|p| p.name != "Doomed"),
            "failed import leaves no active project"
        );
        // It IS present in trash.
        let trashed = project::list_deleted(&conn).unwrap();
        assert!(
            trashed.iter().any(|p| p.name == "Doomed"),
            "rolled-back project is in trash"
        );
    }

    #[test]
    fn import_pdfs_skips_unknown_version_and_removes_file() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();
        let pdf_dir = tmp.path();
        paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "T", &[]), None).unwrap();

        // v9 was never imported for this paper -> mark_pdf_saved errors, file removed.
        let pdfs = vec![ArchivePdf {
            archive_name: "pdfs/arxiv:1_v9.pdf".into(),
            bytes: b"X".to_vec(),
        }];
        import_pdfs(&mut conn, &pdfs, &["arxiv:1".into()], pdf_dir).unwrap();
        assert!(
            !pdf_dir.join("arxiv:1_v9.pdf").exists(),
            "orphan pdf removed after failed mark"
        );

        // A pdf whose source_id wasn't imported is ignored entirely.
        let pdfs = vec![ArchivePdf {
            archive_name: "pdfs/other:2_v1.pdf".into(),
            bytes: b"X".to_vec(),
        }];
        import_pdfs(&mut conn, &pdfs, &["arxiv:1".into()], pdf_dir).unwrap();
        assert!(!pdf_dir.join("other:2_v1.pdf").exists());
    }

    #[test]
    fn paper_entry_to_metadata_defaults_published_today() {
        let mut pe = paper_entry("arxiv:1", 1, "T", &[]);
        pe.published = None;
        let m = pe.to_metadata();
        assert_eq!(m.published, Utc::now().date_naive());
    }

    #[test]
    fn zip_export_then_import_round_trips_to_a_fresh_db() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();
        let export_pdf_dir = tmp.path().join("export_pdfs");
        std::fs::create_dir_all(&export_pdf_dir).unwrap();

        // Seed: one paper with a real PDF on disk + a note pinned to it.
        paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "Paper One", &["ml"]), None)
            .unwrap();
        let fk1 = paper::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
        let pdf_path = export_pdf_dir.join("arxiv_1v1.pdf");
        std::fs::write(&pdf_path, b"PDFCONTENT").unwrap();
        paper::set_pdf_path(&conn, "arxiv:1", pdf_path.to_str().unwrap(), Some(1)).unwrap();
        paper::set_has_pdf(&conn, "arxiv:1", 1, true).unwrap();

        let pid = project::create(
            &mut conn,
            &ProjectIn {
                name: "Round Trip".into(),
                description: "d".into(),
                color: Some(0x00ff00),
                tags: vec!["t".into()],
                source_fks: vec![fk1],
            },
        )
        .unwrap();
        let pv1 = paper::get(
            &conn,
            &paper::PaperRef::Source {
                source_id: "arxiv:1".into(),
                version: Some(1),
            },
        )
        .unwrap()
        .unwrap()
        .paper_id;
        note::create(
            &conn,
            &NoteIn {
                source_fk: fk1,
                title: "n".into(),
                content: "body".into(),
                paper_id: Some(pv1),
                project_fk: Some(pid),
                uuid: None,
            },
        )
        .unwrap();

        // Export -> .lxproj is forced as the extension.
        let dest = tmp.path().join("out");
        let written = export_project(&conn, pid, &dest, true, &export_pdf_dir).unwrap();
        assert_eq!(written.extension().and_then(|e| e.to_str()), Some("lxproj"));
        assert!(written.is_file());

        // Preview without touching the DB.
        let preview = preview_import(&written).unwrap();
        assert_eq!(preview.project_name, "Round Trip");
        assert_eq!(preview.paper_count, 1);
        assert_eq!(preview.note_count, 1);
        assert!(preview.has_pdfs);

        // Commit into a FRESH db + fresh pdf dir.
        let mut conn2 = db();
        let import_pdf_dir = tmp.path().join("import_pdfs");
        let new_pid =
            commit_import(&mut conn2, &written, OnConflict::Merge, &import_pdf_dir).unwrap();

        let proj = project::get(
            &conn2,
            &project::Project {
                project_fk: Some(new_pid),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(proj.name, "Round Trip");
        assert_eq!(proj.source_fks.len(), 1);

        let p = paper::get(&conn2, &paper::PaperRef::source("arxiv:1".into()))
            .unwrap()
            .unwrap();
        assert_eq!(p.title, "Paper One");
        assert!(p.has_pdf);

        // PDF written under the ARCHIVE basename (`_v`), bytes intact, path recorded.
        let imported_pdf = import_pdf_dir.join("arxiv:1_v1.pdf");
        assert!(imported_pdf.is_file());
        assert_eq!(std::fs::read(&imported_pdf).unwrap(), b"PDFCONTENT");
        assert_eq!(
            p.pdf_path.as_deref(),
            Some(imported_pdf.to_string_lossy().as_ref())
        );

        let notes = note::get_many(
            &conn2,
            &note::Notes {
                project_fk: Some(new_pid),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "n");
        assert!(notes[0].paper_id_fk.is_some());
        let orig = note::get_many(
            &conn,
            &note::Notes {
                project_fk: Some(pid),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(notes[0].uuid, orig[0].uuid);
        assert_eq!(notes[0].uuid.len(), 36);
    }

    #[test]
    fn build_and_commit_round_trip_author_orcids() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();

        let mut m = meta("arxiv:1", 1, "P", &[]);
        m.authors = vec!["Alice".into(), "Bob".into()];
        m.author_orcids = Some(vec![Some("0000-0001".into()), None]);
        paper::save_paper_metadata(&mut conn, &m, None).unwrap();
        let fk1 = paper::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
        let pid = project::create(
            &mut conn,
            &ProjectIn {
                name: "Proj".into(),
                description: "d".into(),
                color: None,
                tags: vec![],
                source_fks: vec![fk1],
            },
        )
        .unwrap();

        let (manifest, _) = build_manifest(&conn, pid, false, tmp.path()).unwrap();
        assert_eq!(manifest.papers.len(), 1);
        assert_eq!(
            manifest.papers[0].author_orcids,
            vec![Some("0000-0001".to_string()), None]
        );

        // Commit into a fresh DB and confirm AUTHOR_ORCID lands (fill-if-null).
        let mut conn2 = db();
        commit_from_manifest(&mut conn2, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
        fn orcid(conn: &Connection, name: &str) -> Option<String> {
            conn.query_row(
                "SELECT AUTHOR_ORCID FROM AUTHOR WHERE AUTHOR_FULL_NAME = ?",
                [name],
                |r| r.get(0),
            )
            .unwrap()
        }
        assert_eq!(orcid(&conn2, "Alice").as_deref(), Some("0000-0001"));
        assert_eq!(orcid(&conn2, "Bob"), None);
    }

    #[test]
    fn commit_merge_unions_tags_onto_existing_paper() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();

        // Pre-existing paper carries tag A.
        paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "T", &["A"]), None).unwrap();

        // Merge-import the same paper carrying tag B.
        let manifest = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &["B"])], vec![]);
        commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();

        let p = paper::get(&conn, &paper::PaperRef::source("arxiv:1".into()))
            .unwrap()
            .unwrap();
        let mut tags = p.tags.clone();
        tags.sort();
        assert_eq!(
            tags,
            vec!["A".to_string(), "B".to_string()],
            "merge unions tags, not discard"
        );

        // Relational half re-synced too.
        let relational: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM PAPER_TO_TAG WHERE SOURCE_ID = ?",
                ["arxiv:1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(relational, 2);
    }

    /// SECURITY BOUNDARY. `route/share.rs` joins share ids onto `share_dir` at six
    /// call sites, so anything this accepts becomes a path component. The
    /// commit_adopts/rejects pair below only exercises it through the importer;
    /// this pins the validator itself.
    #[test]
    fn valid_share_id_rejects_everything_that_could_escape_share_dir() {
        // A real share id — a uuid v4 — is the only shape that must pass.
        assert!(valid_share_id("3f2b8c1a-9d4e-4f6a-8b2c-1d5e7f9a0b3c"));
        assert!(valid_share_id("plain-id_123"));

        for bad in [
            "",                      // empty — joins to the share_dir itself
            "..",                    // parent
            "../etc/passwd",         // classic traversal
            "..\\windows\\system32", // traversal, Windows separator
            "a/../../b",             // traversal mid-string
            "a..b",                  // any embedded `..`, conservatively
            ".",                     // current dir
            ".hidden",               // any leading dot
            "sub/dir",               // separator
            "sub\\dir",              // separator, Windows
            "/etc/passwd",           // absolute, Unix
            "C:evil",                // Windows drive-relative — join() escapes
            "C:\\Windows",           // absolute, Windows
            "\\\\server\\share",     // UNC
            "..\u{202e}gnp.exe",     // RTL override still carries `..`
            "\u{ff0e}\u{ff0e}/etc",  // fullwidth dots: not `..`, but `/` catches it
        ] {
            assert!(!valid_share_id(bad), "must reject {bad:?}");
        }

        // Documented gap, not an escape: unicode lookalikes with no separator and no
        // ASCII `..` are accepted as opaque names. They stay inside share_dir.
        assert!(valid_share_id("\u{ff0e}\u{ff0e}"));
    }

    #[test]
    fn commit_adopts_valid_share_id() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();

        let share_id = "3f2b8c1a-9d4e-4f6a-8b2c-1d5e7f9a0b3c";
        let mut manifest = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
        manifest.project.share_id = Some(share_id.into());

        let pid =
            commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();

        let proj = project::get(
            &conn,
            &project::Project {
                project_fk: Some(pid),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            proj.share_id,
            Some(share_id.into()),
            "share_id adopted from manifest"
        );
    }

    #[test]
    fn commit_rejects_invalid_share_id() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();

        // Traversal, absolute, and dotfile share_ids must NOT be adopted.
        let invalid_ids = vec!["../evil", "/etc/passwd", ".hidden"];
        for invalid_id in invalid_ids {
            let mut manifest =
                base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
            manifest.project.share_id = Some(invalid_id.into());

            let pid =
                commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path())
                    .unwrap();
            let proj = project::get(
                &conn,
                &project::Project {
                    project_fk: Some(pid),
                },
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                proj.share_id, None,
                "invalid share_id '{}' rejected",
                invalid_id
            );
        }
    }

    #[test]
    fn commit_rejects_duplicate_share_id() {
        let mut conn = db();
        let tmp = tempfile::tempdir().unwrap();

        let share_id = "7a1c4e2d-6b3f-4a8e-9c0d-2e5f7a9b1c4d";
        let mut manifest = base_manifest("P1", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
        manifest.project.share_id = Some(share_id.into());

        // First import claims the share_id.
        let pid1 =
            commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
        let proj1 = project::get(
            &conn,
            &project::Project {
                project_fk: Some(pid1),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(proj1.share_id, Some(share_id.into()));

        // Second import with the same manifest (same share_id): adopt fails on the
        // live claimant, the import still succeeds, and the project has no share_id.
        manifest.project.name = "P2".into();
        let pid2 =
            commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
        let proj2 = project::get(
            &conn,
            &project::Project {
                project_fk: Some(pid2),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            proj2.share_id, None,
            "second project does not claim the duplicate share_id"
        );
    }
}
