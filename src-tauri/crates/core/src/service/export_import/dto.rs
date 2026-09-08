//! Manifest wire model and the archive PDF-name codec.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::models::{PaperDetails, PaperMetadata};

pub(super) const FORMAT_VERSION: i64 = 1;

/// Merge vs. overwrite behaviour for papers whose source_id already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    /// Keep the stored paper metadata; just (re)link it to the imported project.
    Merge,
    /// Re-write stored paper metadata from the archive (`repair_paper`).
    Overwrite,
}

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

/// Archive paper record.
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
    /// `author_orcids` comes prefetched from the batched
    /// `author::paper_author_orcids` lookup in `build_manifest`.
    pub(super) fn from_details(p: &PaperDetails, author_orcids: Vec<Option<String>>) -> Self {
        PaperEntry {
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
        }
    }

    /// Archive record → `PaperMetadata`. Missing `published` falls back to today;
    /// empty list fields collapse to None.
    pub(super) fn to_metadata(&self) -> PaperMetadata {
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
    /// Decode an in-zip entry path. `None` for entries the import loop skips:
    /// non-`.pdf` names and stems without `_v`. Splits on the LAST `_v`, so
    /// source_ids containing `_v` round-trip; a non-numeric version falls back to 1.
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
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct ImportPreview {
    pub project_name: String,
    pub description: String,
    pub paper_count: usize,
    pub note_count: usize,
    pub annotation_count: usize,
    pub has_pdfs: bool,
    pub format_version: i64,
}

/// `POST /api/projects/import/preview` envelope (route/uploads.rs) — [`ImportPreview`] minus `annotation_count`.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct ImportPreviewResponse {
    pub project_name: String,
    pub description: String,
    pub paper_count: usize,
    pub note_count: usize,
    pub has_pdfs: bool,
    pub format_version: i64,
}

impl From<ImportPreview> for ImportPreviewResponse {
    fn from(p: ImportPreview) -> Self {
        ImportPreviewResponse {
            project_name: p.project_name,
            description: p.description,
            paper_count: p.paper_count,
            note_count: p.note_count,
            has_pdfs: p.has_pdfs,
            format_version: p.format_version,
        }
    }
}

/// `POST /api/projects/import/commit` and MCP `import_project` envelope — the created/merged project id.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct ImportedProject {
    pub project_id: i64,
}
