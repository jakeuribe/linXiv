//! Paper service — Rust port of the non-import parts of `service/paper.py`.
//! Plan §5.2. Thin orchestration over `storage::queries::paper` (+ `::project`
//! for project_fks); no raw SQL, no duplicated storage logic.
//!
//! DI seam: every DB-touching fn takes `conn` first; FS-touching helpers
//! (`pdf_on_disk_name`) take only the source_id/version and return a name —
//! they never read config. `import_pdf` lives in `service::paper_import`.
//!
//! Two PDF filename formats stay DISTINCT (do not unify): the on-disk
//! `{safe}v{version}.pdf` here vs. the `.lxproj` archive `{source_id}_v{version}.pdf`
//! (export/import contract). PaperDetails (one version) / PaperDetailsAll (all
//! versions) / SearchResultOut stay distinct serializers (D16).
//!
//! One Python `storage.db` read has no dedicated Rust storage fn yet
//! (`get_paper_by_source_fk`); it is composed here from existing storage fns
//! rather than adding raw SQL to the service.

use crate::error::{CoreError, Result};
use crate::formats::pyrepr;
use crate::models::{
    is_arxiv_source_id, PaperDetails, PaperDetailsAll, PaperIn, PaperMetadata, ARXIV_PDF_MARKER,
};
use crate::service::files;
pub use crate::storage::queries::paper::PaperSort;
use crate::storage::queries::{
    note as note_store, paper as store, project as proj_store, search as search_store,
};
use chrono::{NaiveDate, NaiveDateTime};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub use store::DoiVersionCandidate;

// ── lookup query objects (defined in service/paper.py, not models.py) ────────

/// Identifies a single paper. Version pinning only exists for source_id
/// lookups; `Id` and `SourceFk` carry no version by construction.
#[derive(Debug, Clone)]
pub enum PaperRef {
    /// Exact PAPER row by PK (selects that exact version).
    Id(i64),
    /// Latest version for a root.
    SourceFk(i64),
    /// By source_id; `version` pins one stored version, `None` → latest.
    Source {
        source_id: String,
        version: Option<i64>,
    },
}

impl PaperRef {
    /// `Source` with no version pin — the overwhelmingly common lookup.
    pub fn source(source_id: String) -> Self {
        PaperRef::Source {
            source_id,
            version: None,
        }
    }
}

/// A soft-deleted paper enriched with its project memberships (Python
/// `DeletedPaperDetails`). Wraps storage's `DeletedPaper` + `project_fks`.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct DeletedPaperDetails {
    pub source_fk: i64,
    pub source_id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub published: Option<NaiveDate>,
    pub deleted_at: Option<NaiveDateTime>,
    pub pdf_path: Option<String>,
    pub had_pdf: bool,
    pub project_fks: Vec<i64>,
}

// ── PDF filename helpers (pure; no FS/DB) ────────────────────────────────────

/// `[/\\:*?"<>|]` → `_`, for embedding a source_id in a filename.
const UNSAFE_FNAME: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Filesystem-safe form of a source_id for PDF filenames.
pub fn pdf_filename_safe(source_id: &str) -> String {
    source_id
        .chars()
        .map(|c| if UNSAFE_FNAME.contains(&c) { '_' } else { c })
        .collect()
}

/// On-disk name for a directly-imported PDF: `{safe}v{version}.pdf` (NO
/// underscore before `v`). Distinct from the `.lxproj` archive format
/// `{source_id}_v{version}.pdf`, which is owned by
/// `service::export_import::ArchivePdfName` — the separate type keeps the two
/// formats unmixable; do not unify.
pub fn pdf_on_disk_name(source_id: &str, version: i64) -> String {
    format!("{}v{}.pdf", pdf_filename_safe(source_id), version)
}

// ── composed reads (no dedicated storage fn exists yet) ──────────────────────

/// `db.get_paper_by_id` — exact PAPER version by PK.
fn paper_by_id(conn: &Connection, paper_id: i64) -> Result<Option<PaperDetails>> {
    store::get_paper_by_id(conn, paper_id)
}

/// `db.get_paper_by_source_fk` — latest version for a root. Composed:
/// SOURCE_FK → SOURCE_ID → latest paper.
fn paper_by_source_fk(conn: &Connection, source_fk: i64) -> Result<Option<PaperDetails>> {
    match store::get_source_id(conn, source_fk)? {
        Some(sid) => store::get_paper(conn, &sid, None),
        None => Ok(None),
    }
}

// ── lookup seam ──────────────────────────────────────────────────────────────

/// Fetch a single paper version. `Id` is PK-exact; `SourceFk` and an unpinned
/// `Source` resolve to the latest version.
pub fn get(conn: &Connection, paper: &PaperRef) -> Result<Option<PaperDetails>> {
    match paper {
        PaperRef::Id(pid) => paper_by_id(conn, *pid),
        PaperRef::SourceFk(sfk) => paper_by_source_fk(conn, *sfk),
        PaperRef::Source { source_id, version } => {
            store::get_paper(conn, &canonical_source_id(conn, source_id), *version)
        }
    }
}

/// `get` by source_id where absence is an error: the one place the paper not-found
/// contract comes from (`CoreError::PaperNotFound` — route 404, CLI exit 1, MCP tool
/// error all word it identically). Mirrors `project::get_required`.
pub fn get_required(conn: &Connection, source_id: &str) -> Result<PaperDetails> {
    get(conn, &PaperRef::source(source_id.to_string()))?
        .ok_or_else(|| CoreError::PaperNotFound(source_id.to_string()))
}

/// Fetch every stored version, display fields from the latest. Key resolution
/// shares [`resolve_source_id`] with delete/restore/hard-delete. (`get` stays
/// PK-exact for `Id`: that key selects one version row.)
pub fn get_all(conn: &Connection, paper: &PaperRef) -> Result<Option<PaperDetailsAll>> {
    let Some(source_id) = resolve_source_id(conn, paper)? else {
        return Ok(None);
    };

    let rows = store::get_all_versions(conn, &source_id)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let latest = rows.last().expect("non-empty");
    Ok(Some(PaperDetailsAll {
        source_id: source_id.clone(),
        latest_version: latest.version,
        title: latest.title.clone(),
        authors: latest.authors.clone(),
        summary: latest.summary.clone(),
        published: rows[0].published, // oldest version's published date
        updated: latest.updated,
        doi: latest.doi.clone(),
        url: latest.url.clone(),
        category: latest.category.clone(),
        categories: latest.categories.clone(),
        journal_ref: latest.journal_ref.clone(),
        comment: latest.comment.clone(),
        tags: latest.tags.clone(),
        source: latest.source.clone(),
        versions: rows,
    }))
}

/// Latest-version rows for the given paper roots (project export/share),
/// filtered in SQL. Empty input → empty output — a project with no papers
/// yields no papers.
pub fn get_by_source_fks(conn: &Connection, source_fks: &[i64]) -> Result<Vec<PaperDetails>> {
    store::get_papers_by_source_fks(conn, source_fks)
}

// ── writes ───────────────────────────────────────────────────────────────────

/// Insert/update from the GUI `PaperIn` DTO. Returns (source_id, version).
pub fn upsert(
    conn: &mut Connection,
    paper: &PaperIn,
    tags: Option<&[String]>,
) -> Result<(String, i64)> {
    let meta = PaperMetadata {
        source_id: paper.source_id.clone().unwrap_or_default(),
        // Python `paper.version or 1`: 0 is falsy → 1.
        version: paper.version.filter(|v| *v != 0).unwrap_or(1),
        title: paper.title.clone(),
        authors: paper.authors.clone().unwrap_or_default(),
        published: paper.published,
        updated: None,
        summary: paper.summary.clone().unwrap_or_default(),
        category: paper.category.clone(),
        categories: None,
        doi: paper.doi.clone(),
        journal_ref: None,
        comment: None,
        url: paper.url.clone(),
        tags: paper.tags.clone(),
        source: paper.source.clone(),
        author_orcids: None,
    };
    store::save_paper_metadata(conn, &meta, tags)
}

/// Persist one normalized record. Returns (source_id, version).
pub fn save_paper_metadata(
    conn: &mut Connection,
    meta: &PaperMetadata,
    tags: Option<&[String]>,
) -> Result<(String, i64)> {
    store::save_paper_metadata(conn, meta, tags)
}

/// Persist many normalized records in one transaction (all-or-nothing).
/// Returns the source_ids in input order.
pub fn save_papers_metadata(conn: &mut Connection, metas: &[PaperMetadata]) -> Result<Vec<String>> {
    store::save_papers_metadata(conn, metas)
}

/// UNION `tags` onto a paper's existing tags (dual JSON + relational storage).
/// Import/share merge path. Returns the merged list.
pub fn add_paper_tags(
    conn: &mut Connection,
    source_id: &str,
    tags: &[String],
) -> Result<Vec<String>> {
    not_found_as_paper(store::add_paper_tags(conn, source_id, tags), source_id)
}

/// Remove `tags` from a paper across both halves of dual tag storage. Returns the
/// remaining list. Symmetric with [`add_paper_tags`].
pub fn remove_paper_tags(
    conn: &mut Connection,
    source_id: &str,
    tags: &[String],
) -> Result<Vec<String>> {
    not_found_as_paper(store::remove_paper_tags(conn, source_id, tags), source_id)
}

/// Restate storage's generic miss as the typed, id-carrying variant so every
/// surface words a tag write against an unknown paper identically.
fn not_found_as_paper<T>(r: Result<T>, source_id: &str) -> Result<T> {
    r.map_err(|e| match e {
        CoreError::NotFound(_) => CoreError::PaperNotFound(source_id.to_string()),
        other => other,
    })
}

/// Re-write a paper's metadata in-place (migrating SOURCE_ID if it changed).
/// Normalizes and validates first, so every Paper Repair front door (route, CLI,
/// MCP) rejects the same input. Archive import bypasses this via
/// [`repair_paper_unvalidated`].
pub fn repair_paper(conn: &mut Connection, source_fk: i64, meta: &PaperMetadata) -> Result<()> {
    store::repair_paper(conn, source_fk, &validate_repair(meta)?)
}

/// [`repair_paper`] minus normalization/validation — archive import replays
/// already-stored metadata, which is not held to the Paper Repair input rules
/// the front doors apply.
pub fn repair_paper_unvalidated(
    conn: &mut Connection,
    source_fk: i64,
    meta: &PaperMetadata,
) -> Result<()> {
    store::repair_paper(conn, source_fk, meta)
}

/// Parse a user-supplied `published` date for Paper Repair.
pub fn parse_published(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| CoreError::Validation(format!("Invalid date {}; use YYYY-MM-DD", pyrepr(s))))
}

/// The user-editable Paper Repair fields, shared by all three front doors:
/// the route PUT body (mirrors `PaperRepairBody` in `src/api/papers.ts`),
/// the CLI flags, and the MCP tool params. `published` stays a `String` so
/// the date is parsed here by [`parse_published`], identically per surface.
#[derive(Debug, Deserialize)]
pub struct RepairFields {
    pub title: String,
    pub authors: Vec<String>,
    /// Publication date (YYYY-MM-DD).
    pub published: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

impl RepairFields {
    /// Assemble repair metadata around the identity each surface resolves
    /// (source_id/version/source are not changeable here — ADR-0008). Fails
    /// with `Validation` on a bad `published` date.
    pub fn into_metadata(
        self,
        source_id: String,
        version: i64,
        source: Option<String>,
    ) -> Result<PaperMetadata> {
        Ok(PaperMetadata {
            source_id,
            version,
            title: self.title,
            authors: self.authors,
            published: parse_published(&self.published)?,
            updated: None,
            summary: self.summary,
            category: self.category,
            categories: None,
            doi: self.doi,
            journal_ref: None,
            comment: None,
            url: self.url,
            tags: self.tags,
            source,
            author_orcids: None,
        })
    }
}

/// Trim title/summary, dedup non-blank authors/tags (empty tag list → None), and
/// reject what Paper Repair must never persist: blank title, no authors, empty DOI.
fn validate_repair(meta: &PaperMetadata) -> Result<PaperMetadata> {
    let title = meta.title.trim().to_string();
    if title.is_empty() {
        return Err(CoreError::Validation("title must not be blank".into()));
    }
    let authors = dedup_nonblank(&meta.authors);
    if authors.is_empty() {
        return Err(CoreError::Validation(
            "at least one author is required".into(),
        ));
    }
    if matches!(meta.doi.as_deref(), Some("")) {
        return Err(CoreError::Validation(
            "doi must have at least 1 character".into(),
        ));
    }
    Ok(PaperMetadata {
        title,
        authors,
        summary: meta.summary.trim().to_string(),
        tags: meta
            .tags
            .as_ref()
            .map(|ts| dedup_nonblank(ts))
            .filter(|v| !v.is_empty()),
        ..meta.clone()
    })
}

/// Trim each entry, drop blanks, dedup preserving first-seen order.
fn dedup_nonblank(items: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .iter()
        .filter_map(|s| {
            let t = s.trim();
            (!t.is_empty() && seen.insert(t.to_string())).then(|| t.to_string())
        })
        .collect()
}

/// Library search: FTS hits (TeX source + note index) merged with the LIKE scan
/// over note text, deduped by source_id, FTS rows first then note-recency order —
/// bm25 relevance is the stronger signal, so LIKE-only extras trail it.
pub fn search_library(conn: &Connection, query: &str, limit: i64) -> Result<Vec<PaperDetails>> {
    // A missing or corrupt FTS index yields no rows rather than failing the whole
    // search, so note hits still populate.
    let limit = limit.max(0) as usize;
    let mut papers = search_store::search_full_text(conn, query, limit as i64).unwrap_or_default();
    // FTS already filled the limit: every LIKE extra below would be truncated.
    if papers.len() >= limit {
        papers.truncate(limit);
        return Ok(papers);
    }
    let mut seen: HashSet<String> = papers.iter().map(|p| p.source_id.clone()).collect();
    // One bulk fetch for every note hit (full_text pruned — it's skip_serializing
    // anyway), re-walked in note-recency order since the bulk read sorts by date.
    let note_fks = note_store::search_notes_source_fks(conn, query, limit as i64)?;
    let mut by_fk: HashMap<i64, PaperDetails> = store::get_papers_by_source_fks(conn, &note_fks)?
        .into_iter()
        .map(|p| (p.source_fk, p))
        .collect();
    for sfk in note_fks {
        if papers.len() >= limit {
            break;
        }
        if let Some(p) = by_fk.remove(&sfk) {
            if seen.insert(p.source_id.clone()) {
                papers.push(p);
            }
        }
    }
    Ok(papers)
}

// ── soft / hard delete ───────────────────────────────────────────────────────

/// Resolve a `PaperRef` to its text source_id (any variant → the root's id).
pub(crate) fn resolve_source_id(conn: &Connection, paper: &PaperRef) -> Result<Option<String>> {
    match paper {
        PaperRef::Source { source_id, .. } => Ok(Some(canonical_source_id(conn, source_id))),
        PaperRef::SourceFk(sfk) => store::get_source_id(conn, *sfk),
        PaperRef::Id(pid) => Ok(paper_by_id(conn, *pid)?.map(|p| p.source_id)),
    }
}

/// Soft-delete a paper. Returns its stored PDF_PATH (for the caller to unlink).
pub fn delete(conn: &mut Connection, paper: &PaperRef) -> Result<Option<String>> {
    match resolve_source_id(conn, paper)? {
        Some(sid) => store::soft_delete_paper(conn, &sid),
        None => Ok(None),
    }
}

/// Restore a soft-deleted paper. Returns (pdf_path, project_fks). The root's
/// SOURCE_FK is read before the restore write — it's an immutable PK, no TOCTOU.
pub fn restore(conn: &mut Connection, paper: &PaperRef) -> Result<(Option<String>, Vec<i64>)> {
    let Some(source_id) = resolve_source_id(conn, paper)? else {
        return Ok((None, Vec::new()));
    };
    let root = store::get_paper_root(conn, &source_id)?;
    let pdf_path = store::restore_paper(conn, &source_id)?;
    let project_fks = match root {
        Some(r) => proj_store::get_paper_project_fks(conn, r.source_fk)?,
        None => Vec::new(),
    };
    Ok((pdf_path, project_fks))
}

/// Guard for trash-only operations (restore-from-trash, hard delete): the paper
/// must be soft-deleted. `restore`/`hard_delete` stay unguarded — project import
/// checks the root's status itself, and `paper hard-delete` deletes at any status.
pub fn require_trashed(conn: &Connection, source_id: &str) -> Result<()> {
    if !store::is_paper_deleted(conn, source_id)? {
        return Err(CoreError::NotFound(format!(
            "Paper {} not found in trash",
            crate::formats::pyrepr(source_id)
        )));
    }
    Ok(())
}

/// Permanently remove a paper (children cascade off the FK).
pub fn hard_delete(conn: &mut Connection, paper: &PaperRef) -> Result<Option<String>> {
    match resolve_source_id(conn, paper)? {
        Some(sid) => store::hard_delete_paper(conn, &sid),
        None => Ok(None),
    }
}

/// All soft-deleted papers, each enriched with its project memberships.
pub fn list_deleted(conn: &Connection) -> Result<Vec<DeletedPaperDetails>> {
    let rows = store::list_deleted_papers(conn)?;
    let fks: Vec<i64> = rows.iter().map(|r| r.source_fk).collect();
    let mut by_paper = proj_store::project_fks_by_source_fk(conn, &fks)?;
    Ok(rows
        .into_iter()
        .map(|row| DeletedPaperDetails {
            project_fks: by_paper.remove(&row.source_fk).unwrap_or_default(),
            source_fk: row.source_fk,
            source_id: row.source_id,
            title: row.title,
            authors: row.authors,
            published: row.published,
            deleted_at: row.deleted_at,
            pdf_path: row.pdf_path,
            had_pdf: row.had_pdf,
        })
        .collect())
}

/// Which of `source_ids` (namespaced) are already in the library.
pub fn existing_source_ids(conn: &Connection, source_ids: &[String]) -> Result<Vec<String>> {
    store::existing_source_ids(conn, source_ids)
}

/// Which of `source_ids` are soft-deleted — the batched sibling of the
/// single-id `store::is_paper_deleted` check.
pub fn deleted_source_ids(
    conn: &Connection,
    source_ids: &[String],
) -> Result<std::collections::HashSet<String>> {
    store::deleted_source_ids(conn, source_ids)
}

// ── multi-paper reads ────────────────────────────────────────────────────────

/// Latest-version (default) paper rows, with optional exact-category filter and
/// limit/offset. In Rust storage already returns `PaperDetails`, so this is the
/// merge of Python's `list_papers` + `list_paper_details`.
pub fn list_papers(
    conn: &Connection,
    latest_only: bool,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
) -> Result<Vec<PaperDetails>> {
    store::list_papers(conn, latest_only, limit, offset, category)
}

/// `list_papers` under a caller-chosen ordering (the Library page's sort).
pub fn list_papers_sorted(
    conn: &Connection,
    latest_only: bool,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
    sort: PaperSort,
    desc: bool,
) -> Result<Vec<PaperDetails>> {
    store::list_papers_sorted(conn, latest_only, limit, offset, category, sort, desc)
}

/// Latest papers with a local PDF flag set, filtered in SQL (`GET /api/pdfs`).
pub fn list_pdf_papers(conn: &Connection) -> Result<Vec<PaperDetails>> {
    store::list_pdf_papers(conn)
}

/// Sorted distinct primary categories across latest papers (`db.get_categories`).
pub fn get_categories(conn: &Connection) -> Result<Vec<String>> {
    store::get_categories(conn)
}

/// Latest papers whose JSON tags include `label`, case-insensitively
/// (`db.get_papers_by_json_tag`). Order: published DESC (undated last), then
/// paper_id DESC so same-published-date papers are deterministic.
pub fn get_papers_by_tag(conn: &Connection, label: &str) -> Result<Vec<PaperDetails>> {
    store::get_papers_by_json_tag(conn, label)
}

// ── root / id helpers ────────────────────────────────────────────────────────

/// Insert PAPER_ROOTS row if absent (reactivating if soft-deleted). Returns SOURCE_FK.
pub fn ensure_paper_root(conn: &mut Connection, source_id: &str) -> Result<i64> {
    store::ensure_paper_root(conn, source_id)
}

/// The provider namespaces a user-typed bare id could belong to, tried in order.
/// arXiv first: it is the overwhelmingly common case and the only one the CLI used
/// to assume.
const ID_PREFIXES: [&str; 4] = [
    crate::models::ARXIV_ID_PREFIX,
    crate::models::DOI_ID_PREFIX,
    crate::models::OPENALEX_ID_PREFIX,
    crate::models::LOCAL_ID_PREFIX,
];

/// Map a user-supplied paper id onto the `source_id` actually stored.
///
/// A verbatim match always wins; only then is `raw` tried under each provider
/// namespace, so `2204.12985` finds `arxiv:2204.12985` and `10.1000/alpha` finds
/// either the bare row a pre-namespacing BibTeX import wrote or `doi:10.1000/alpha`.
/// Unresolvable ids come back unchanged, so callers keep whatever not-found or
/// empty-result behaviour they already had.
pub fn canonical_source_id(conn: &Connection, raw: &str) -> String {
    let raw = raw.trim();
    let mut candidates = vec![raw.to_string()];
    if !raw.contains(':') {
        candidates.extend(ID_PREFIXES.iter().map(|p| format!("{p}{raw}")));
    }
    let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    // Errors degrade to unresolved, matching the old per-candidate is_ok_and.
    let existing = store::source_fks_by_id(conn, &refs).unwrap_or_default();
    candidates
        .into_iter()
        .find(|c| existing.contains_key(c))
        .unwrap_or_else(|| raw.to_string())
}

/// SOURCE_FK for an existing paper root — the fail-if-absent counterpart to
/// [`ensure_paper_root`]. `PaperNotFound` (404) when the paper is not in the
/// library; the message is the variant's Display, same on every surface.
pub fn resolve_source_fk(conn: &Connection, source_id: &str) -> Result<i64> {
    let sid = canonical_source_id(conn, source_id);
    store::get_paper_root(conn, &sid)?
        .map(|root| root.source_fk)
        .ok_or(CoreError::PaperNotFound(sid))
}

/// PAPER_ROOTS row for a source_id, or None — the Option-returning sibling of
/// [`resolve_source_fk`] for callers that treat an unknown paper as empty, not 404.
pub fn get_paper_root(conn: &Connection, source_id: &str) -> Result<Option<store::PaperRoot>> {
    store::get_paper_root(conn, source_id)
}

/// Stored custom PDF_PATH for one (source_id, version) — the single-column
/// sibling of `get`'s `Source` arm (same key resolution, same version-0-means-
/// latest fallthrough), for callers that only need the path (share publish
/// loops) without materializing the full row incl. FULL_TEXT.
pub fn pdf_custom_path(
    conn: &Connection,
    source_id: &str,
    version: Option<i64>,
) -> Result<Option<String>> {
    store::pdf_path_for_source(conn, &canonical_source_id(conn, source_id), version)
}

/// SOURCE_ID for a SOURCE_FK, or None.
pub fn get_source_id(conn: &Connection, source_fk: i64) -> Result<Option<String>> {
    store::get_source_id(conn, source_fk)
}

/// SOURCE_FK → SOURCE_ID map (nonexistent fks absent) — the batched sibling of
/// `get_source_id` for callers resolving many rows in one pass.
pub fn source_ids_by_fk(
    conn: &Connection,
    source_fks: &[i64],
) -> Result<std::collections::HashMap<i64, String>> {
    store::source_ids_by_fk(conn, source_fks)
}

/// Other paper roots sharing this one's DOI — likely the same work resolved by
/// a different source, for a "these look like the same paper" suggestion.
pub fn find_doi_version_candidates(
    conn: &Connection,
    source_fk: i64,
) -> Result<Vec<DoiVersionCandidate>> {
    store::find_doi_version_candidates(conn, source_fk)
}

// ── PDF / full-text setters ──────────────────────────────────────────────────

/// Set HAS_PDF for one version.
pub fn set_has_pdf(conn: &Connection, source_id: &str, version: i64, has: bool) -> Result<()> {
    store::set_has_pdf(conn, source_id, version, has)
}

/// Set PDF_PATH for one version, or every version when `version` is None/0.
pub fn set_pdf_path(
    conn: &Connection,
    source_id: &str,
    path: &str,
    version: Option<i64>,
) -> Result<()> {
    store::set_pdf_path(conn, source_id, path, version)
}

/// Delete every stored version's local PDF for `source_id`, clearing
/// HAS_PDF/PDF_PATH per version as it goes, keeping the paper record. Returns
/// `Ok(false)` when a version's file resolves outside the managed dir
/// (deletion stops there; earlier versions' flags stay cleared) so each
/// surface can word its own conflict envelope. `source_id` is used verbatim
/// for path/flag lookups, exactly as the surfaces did. Backs
/// `DELETE /api/pdfs/{id}`, CLI `pdf delete`, and MCP `delete_pdf`.
pub fn delete_saved_pdfs(conn: &Connection, pdf_dir: &Path, source_id: &str) -> Result<bool> {
    let all = get_all(conn, &PaperRef::source(source_id.to_string()))?
        .ok_or_else(|| CoreError::PaperNotFound(source_id.to_string()))?;
    for ver in &all.versions {
        let path = files::pdf_path(pdf_dir, source_id, ver.version, ver.pdf_path.as_deref());
        if let Some(p) = &path {
            if !files::delete_pdf(pdf_dir, &p.to_string_lossy()) {
                return Ok(false);
            }
        }
        // Clear the flag/path before the next iteration may refuse.
        set_has_pdf(conn, source_id, ver.version, false)?;
        if path.is_some() {
            set_pdf_path(conn, source_id, "", Some(ver.version))?;
        }
    }
    Ok(true)
}

/// Atomically record PDF_PATH and set HAS_PDF=1 for one version.
pub fn mark_pdf_saved(
    conn: &mut Connection,
    source_id: &str,
    path: &str,
    version: i64,
) -> Result<()> {
    store::mark_pdf_saved(conn, source_id, path, version)
}

/// Store extracted full text + refresh the FTS index for one version.
/// Errors with `PaperNotFound` if the version no longer resolves (deleted or
/// pruned between resolving the paper and the network fetch completing).
pub fn set_full_text(
    conn: &mut Connection,
    source_id: &str,
    version: i64,
    full_text: &str,
) -> Result<()> {
    if store::get_paper(conn, source_id, Some(version))?.is_none() {
        return Err(CoreError::PaperNotFound(source_id.to_string()));
    }
    store::set_full_text(conn, source_id, version, Some(full_text))
}

/// The arXiv PDF URL a TeX-source fetch is derived from (`/pdf/` -> `/src/`), or
/// an error naming why this paper has no fetchable source. Pure, so the three
/// entry points that ingest full text (CLI, MCP, route) share one set of rules
/// without any of them holding a connection across the network await.
///
/// Only arXiv publishes source tarballs — OpenAlex/CrossRef/DOI and locally
/// imported PDFs are metadata-only, so they are refused here rather than each
/// caller re-deriving that.
///
/// The arXiv test is the `source_id` namespace, not `PAPER_META.PROVIDER`: the
/// provider column records provenance and arrives blank on some import paths.
pub fn source_fetch_url(paper: &PaperDetails) -> Result<&str> {
    if !is_arxiv_source_id(&paper.source_id) {
        return Err(CoreError::BadRequest(format!(
            "TeX source is only published by arXiv; {} is not an arXiv paper",
            paper.source_id
        )));
    }
    let url = paper.url.as_deref().unwrap_or_default();
    if !url.contains(ARXIV_PDF_MARKER) {
        return Err(CoreError::BadRequest(format!(
            "{} has no arXiv PDF URL to derive a source tarball from",
            paper.source_id
        )));
    }
    Ok(url)
}

/// Receipt for one full-text ingest attempt — the wire shape route, CLI and MCP
/// all emit (`{source_id, version, indexed}` + `chars` or `reason`).
#[derive(Debug, Serialize, ts_rs::TS)]
pub struct FullTextReceipt {
    pub source_id: String,
    pub version: i64,
    pub indexed: bool,
    // ts(optional) mirrors skip_serializing_if: the key is absent, not null.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub chars: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<&'static str>,
}

impl FullTextReceipt {
    /// The `downloaded_source && !force` skip, decided before any network work.
    pub fn already_indexed(paper: &PaperDetails) -> Self {
        FullTextReceipt {
            source_id: paper.source_id.clone(),
            version: paper.version,
            indexed: false,
            chars: None,
            reason: Some("source already indexed; pass force=true to re-fetch"),
        }
    }
}

/// Phase 1 of a full-text ingest — everything that must NOT hold the DB lock:
/// resolve the source URL, download the tarball, extract the TeX. Hand the
/// result to [`FetchedFullText::commit`] under the caller's lock (two-phase so
/// no surface holds its connection mutex across the network await).
pub async fn fetch_full_text(
    paper: &PaperDetails,
    data_dir: &std::path::Path,
) -> Result<FetchedFullText> {
    let url = source_fetch_url(paper)?.to_string();
    let text = crate::sources::arxiv_downloads::fetch_source_text(&url, data_dir).await?;
    Ok(FetchedFullText {
        source_id: paper.source_id.clone(),
        version: paper.version,
        text,
    })
}

/// A downloaded-and-extracted TeX body waiting to be committed (phase 2).
#[derive(Debug)]
pub struct FetchedFullText {
    source_id: String,
    version: i64,
    text: String,
}

impl FetchedFullText {
    /// Phase 2, under the caller's DB lock: store + index the body, or refuse to
    /// clobber an already-indexed one with an empty re-fetch. `extract_source`
    /// yields `""` for a corrupt, truncated, or PDF-only tarball, which is
    /// indistinguishable from "this paper genuinely has no TeX" — storing it is
    /// right the first time (it marks DOWNLOADED_SOURCE so the backfill stops
    /// retrying; `force` re-opens it), but overwriting a body that already
    /// indexes would silently drop the paper out of search results. The guard
    /// reads one stored column here, under the lock, so it sees the freshest
    /// state without ever hauling the body.
    pub fn commit(self, conn: &mut Connection) -> Result<FullTextReceipt> {
        if self.text.is_empty() && store::has_full_text(conn, &self.source_id, self.version)? {
            return Ok(FullTextReceipt {
                source_id: self.source_id,
                version: self.version,
                indexed: false,
                chars: None,
                reason: Some("re-fetch produced no TeX; kept the text already indexed"),
            });
        }
        set_full_text(conn, &self.source_id, self.version, &self.text)?;
        Ok(FullTextReceipt {
            source_id: self.source_id,
            version: self.version,
            indexed: true,
            chars: Some(self.text.chars().count()),
            reason: None,
        })
    }
}

/// SOURCE_IDs of stored arXiv papers with no TeX source yet, oldest-published
/// first — the backfill work list. Ids only; the caller loads each paper as it
/// goes. The query matches on the same `arxiv:` / `/pdf/` constants
/// `source_fetch_url` does, so a listed paper is one it accepts.
pub fn full_text_backfill_candidates(conn: &Connection) -> Result<Vec<String>> {
    store::full_text_backfill_candidates(conn)
}

/// Length of `full_text_backfill_candidates` without building the ids.
pub fn full_text_backfill_count(conn: &Connection) -> Result<i64> {
    store::full_text_backfill_count(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;

    fn meta(source_id: &str, version: i64, category: &str, tags: &[&str]) -> PaperMetadata {
        PaperMetadata {
            source_id: source_id.into(),
            version,
            title: format!("T{version}"),
            authors: vec!["Alice".into(), "Bob".into()],
            published: NaiveDate::from_ymd_opt(2024, 1, version as u32).unwrap(),
            updated: None,
            summary: "sum".into(),
            category: Some(category.into()),
            categories: Some(vec![category.into()]),
            doi: Some("10.1/x".into()),
            journal_ref: None,
            comment: None,
            url: Some("http://x".into()),
            tags: Some(tags.iter().map(|t| t.to_string()).collect()),
            source: Some("arxiv".into()),
            author_orcids: None,
        }
    }

    /// The fail-if-absent half of the pair every Note/Annotation consumer shares:
    /// resolving an unknown paper errors instead of creating a root.
    #[test]
    fn resolve_source_fk_fails_without_creating_a_root() {
        let mut conn = db();
        let err = resolve_source_fk(&conn, "arxiv:404").unwrap_err();
        assert_eq!(err.http_status(), 404);
        assert!(store::get_paper_root(&conn, "arxiv:404").unwrap().is_none());

        let fk = ensure_paper_root(&mut conn, "arxiv:1").unwrap();
        // The id is trimmed before lookup, like the route's ensure call site.
        assert_eq!(resolve_source_fk(&conn, " arxiv:1 ").unwrap(), fk);
    }

    /// Every surface must be able to address a paper by the id the user knows,
    /// namespaced or not — the CLI used to assume `arxiv:` and lost DOI/BibTeX rows.
    #[test]
    fn canonical_source_id_tries_verbatim_then_each_namespace() {
        let mut conn = db();
        for sid in ["arxiv:2204.12985", "doi:10.1000/beta", "10.1000/alpha"] {
            ensure_paper_root(&mut conn, sid).unwrap();
        }
        // bare arXiv id -> the namespaced root
        assert_eq!(canonical_source_id(&conn, "2204.12985"), "arxiv:2204.12985");
        // bare DOI -> the namespaced root
        assert_eq!(
            canonical_source_id(&conn, "10.1000/beta"),
            "doi:10.1000/beta"
        );
        // a pre-namespacing bare row still resolves verbatim, not as `arxiv:...`
        assert_eq!(canonical_source_id(&conn, "10.1000/alpha"), "10.1000/alpha");
        // when a bare id exists under two namespaces, arXiv (first prefix) wins
        ensure_paper_root(&mut conn, "arxiv:5555").unwrap();
        ensure_paper_root(&mut conn, "doi:5555").unwrap();
        assert_eq!(canonical_source_id(&conn, "5555"), "arxiv:5555");
        // already namespaced ids pass straight through
        assert_eq!(
            canonical_source_id(&conn, "arxiv:2204.12985"),
            "arxiv:2204.12985"
        );
        // nothing matches -> unchanged, so callers keep their own not-found path
        assert_eq!(canonical_source_id(&conn, "nope"), "nope");
        // and the lookup seam sees the same resolution
        assert_eq!(resolve_source_fk(&conn, "10.1000/beta").unwrap(), {
            let root = store::get_paper_root(&conn, "doi:10.1000/beta").unwrap();
            root.unwrap().source_fk
        });
    }

    /// `pdf_custom_path` is the single-column sibling of `get`'s `Source` arm:
    /// same key resolution (canonical id, version-0 → latest) and same
    /// visibility (trashed roots yield None), pinned against `get` itself.
    #[test]
    fn pdf_custom_path_matches_get_source_arm() {
        let mut conn = db();
        seed_two_versions(&mut conn);
        store::set_pdf_path(&conn, "arxiv:A", "/tmp/v1.pdf", Some(1)).unwrap();

        let get_path = |conn: &Connection, sid: &str, ver: Option<i64>| {
            get(
                conn,
                &PaperRef::Source {
                    source_id: sid.into(),
                    version: ver,
                },
            )
            .unwrap()
            .and_then(|p| p.pdf_path)
        };
        for (sid, ver) in [
            ("arxiv:A", Some(1)),   // custom path stored
            ("arxiv:A", Some(2)),   // version without a path
            ("arxiv:A", Some(9)),   // absent version
            ("arxiv:A", None),      // latest fallthrough
            ("arxiv:A", Some(0)),   // 0 means latest, like `get`
            ("A", Some(1)),         // bare id resolves via canonical_source_id
            ("arxiv:404", Some(1)), // unknown paper
        ] {
            assert_eq!(
                pdf_custom_path(&conn, sid, ver).unwrap(),
                get_path(&conn, sid, ver),
                "mismatch for ({sid:?}, {ver:?})"
            );
        }
        assert_eq!(
            pdf_custom_path(&conn, "A", Some(1)).unwrap().as_deref(),
            Some("/tmp/v1.pdf")
        );

        // Trashing hides the path from both, like the papers view does.
        delete(&mut conn, &PaperRef::source("arxiv:A".into())).unwrap();
        assert_eq!(pdf_custom_path(&conn, "arxiv:A", Some(1)).unwrap(), None);
    }

    // Two versions of one paper. Returns (source_fk, paper_id_v1, paper_id_v2).
    fn seed_two_versions(conn: &mut Connection) -> (i64, i64, i64) {
        save_paper_metadata(conn, &meta("arxiv:A", 1, "cs.LG", &["ml"]), None).unwrap();
        save_paper_metadata(conn, &meta("arxiv:A", 2, "cs.LG", &["ml"]), None).unwrap();
        let fk = ensure_paper_root(conn, "arxiv:A").unwrap();
        let v1 = get(
            conn,
            &PaperRef::Source {
                source_id: "arxiv:A".into(),
                version: Some(1),
            },
        )
        .unwrap()
        .unwrap()
        .paper_id;
        let v2 = get(conn, &PaperRef::source("arxiv:A".into()))
            .unwrap()
            .unwrap()
            .paper_id;
        (fk, v1, v2)
    }

    #[test]
    fn get_dispatch_and_version_handling() {
        let mut conn = db();
        let (fk, v1, _v2) = seed_two_versions(&mut conn);

        // source_fk -> latest version.
        assert_eq!(
            get(&conn, &PaperRef::SourceFk(fk))
                .unwrap()
                .unwrap()
                .version,
            2
        );
        // paper_id -> that exact version.
        assert_eq!(get(&conn, &PaperRef::Id(v1)).unwrap().unwrap().version, 1);
        // source_id + version -> pinned; source_id alone -> latest.
        assert_eq!(
            get(
                &conn,
                &PaperRef::Source {
                    source_id: "arxiv:A".into(),
                    version: Some(1)
                }
            )
            .unwrap()
            .unwrap()
            .version,
            1
        );
        assert_eq!(
            get(&conn, &PaperRef::source("arxiv:A".into()))
                .unwrap()
                .unwrap()
                .version,
            2
        );
    }

    #[test]
    fn get_all_aggregates_across_versions_each_dispatch() {
        let mut conn = db();
        let (fk, v1, _v2) = seed_two_versions(&mut conn);

        for key in [
            PaperRef::source("arxiv:A".into()),
            PaperRef::Id(v1),
            PaperRef::SourceFk(fk),
        ] {
            let all = get_all(&conn, &key).unwrap().unwrap();
            assert_eq!(all.source_id, "arxiv:A");
            assert_eq!(all.latest_version, 2);
            assert_eq!(all.title, "T2"); // display from latest
            assert_eq!(
                all.versions.iter().map(|v| v.version).collect::<Vec<_>>(),
                vec![1, 2]
            );
            // published comes from the OLDEST version (rows[0]).
            assert_eq!(all.published, NaiveDate::from_ymd_opt(2024, 1, 1));
        }

        assert!(get_all(&conn, &PaperRef::Id(424242)).unwrap().is_none());
    }

    #[test]
    fn get_by_source_fks_filters_latest_rows() {
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:A", 1, "cs.LG", &["ml"]), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:A", 2, "cs.LG", &["ml"]), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:B", 1, "math.CO", &["theory"]), None).unwrap();
        let fk_a = ensure_paper_root(&mut conn, "arxiv:A").unwrap();
        let fk_b = ensure_paper_root(&mut conn, "arxiv:B").unwrap();

        // Empty input -> empty output (an empty project exports no papers).
        assert!(get_by_source_fks(&conn, &[]).unwrap().is_empty());
        // One fk -> that root's latest version only.
        let r = get_by_source_fks(&conn, &[fk_a]).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].source_id.as_str(), r[0].version), ("arxiv:A", 2));
        // Unknown fks are dropped; result matches the list_papers default order.
        let r = get_by_source_fks(&conn, &[fk_b, fk_a, 999_999]).unwrap();
        let expected: Vec<i64> = list_papers(&conn, true, None, 0, None)
            .unwrap()
            .into_iter()
            .map(|p| p.paper_id)
            .collect();
        assert_eq!(
            r.iter().map(|p| p.paper_id).collect::<Vec<_>>(),
            expected,
            "same rows and order as the old whole-library scan-then-filter"
        );
    }

    #[test]
    fn upsert_roundtrip_and_categories_and_by_tag() {
        let mut conn = db();
        let p = PaperIn {
            title: "Manual".into(),
            published: NaiveDate::from_ymd_opt(2024, 5, 1).unwrap(),
            source_id: Some("local:abc".into()),
            version: None,
            authors: Some(vec!["Carol".into()]),
            summary: Some("s".into()),
            category: Some("cs.AI".into()),
            doi: None,
            url: None,
            tags: Some(vec!["Mine".into()]),
            source: Some("local".into()),
        };
        let (sid, ver) = upsert(&mut conn, &p, Some(&["shared".into()])).unwrap();
        assert_eq!((sid.as_str(), ver), ("local:abc", 1)); // version None/0 -> 1

        let got = get(&conn, &PaperRef::source("local:abc".into()))
            .unwrap()
            .unwrap();
        assert_eq!(got.title, "Manual");
        assert_eq!(got.authors, vec!["Carol".to_string()]);
        // upsert tags + extra tags merged.
        assert!(got.tags.contains(&"Mine".to_string()) && got.tags.contains(&"shared".to_string()));

        save_paper_metadata(&mut conn, &meta("arxiv:Z", 1, "cs.LG", &["other"]), None).unwrap();
        // categories: sorted distinct.
        assert_eq!(
            get_categories(&conn).unwrap(),
            vec!["cs.AI".to_string(), "cs.LG".to_string()]
        );
        // by_tag case-insensitive.
        let hits = get_papers_by_tag(&conn, "mine").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "local:abc");
        assert!(get_papers_by_tag(&conn, "nope").unwrap().is_empty());
    }

    /// Pins the paper_id lookup's exact semantics: the one version row that PK
    /// names, with FULL_TEXT blanked like every list read (the source_id path
    /// via `get_paper` does return the body — the difference is load-bearing).
    #[test]
    fn paper_by_id_returns_exact_version_with_full_text_blanked() {
        let mut conn = db();
        let (_fk, v1, _v2) = seed_two_versions(&mut conn);
        set_full_text(&mut conn, "arxiv:A", 1, "tex body").unwrap();

        let p = get(&conn, &PaperRef::Id(v1)).unwrap().unwrap();
        assert_eq!(p.paper_id, v1);
        assert_eq!(p.version, 1);
        assert_eq!(p.title, "T1");
        assert!(p.downloaded_source);
        assert_eq!(p.full_text, None);
        assert!(get(&conn, &PaperRef::Id(424242)).unwrap().is_none());
    }

    /// Pins get_categories: distinct, NULLs excluded, byte-order ascending
    /// (BTreeSet<String> order == SQL BINARY collation), latest versions only.
    #[test]
    fn get_categories_distinct_sorted_null_free_latest_only() {
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "cs.LG", &[]), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:2", 1, "cs.LG", &[]), None).unwrap(); // dup
        save_paper_metadata(&mut conn, &meta("arxiv:3", 1, "CS.AI", &[]), None).unwrap(); // 'C' < 'c'
                                                                                          // A superseded version's category must not surface.
        save_paper_metadata(&mut conn, &meta("arxiv:4", 1, "old.CAT", &[]), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:4", 2, "math.CO", &[]), None).unwrap();
        let no_cat = PaperMetadata {
            category: None,
            categories: None,
            ..meta("arxiv:5", 1, "unused", &[])
        };
        save_paper_metadata(&mut conn, &no_cat, None).unwrap();

        assert_eq!(
            get_categories(&conn).unwrap(),
            vec![
                "CS.AI".to_string(),
                "cs.LG".to_string(),
                "math.CO".to_string()
            ]
        );
    }

    /// Pins get_papers_by_tag: ASCII-case-insensitive whole-tag match on the
    /// JSON list, published DESC with NULL-published papers last, and papers
    /// with NULL TAGS skipped rather than erroring.
    #[test]
    fn get_papers_by_tag_matches_case_insensitively_and_sinks_null_published() {
        let mut conn = db();
        // Dated papers whose tag differs only in case (published day 1 vs 2).
        save_paper_metadata(&mut conn, &meta("arxiv:old", 1, "cs.LG", &["SHARED"]), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:new", 2, "cs.LG", &["shared"]), None).unwrap();
        let untagged = PaperMetadata {
            tags: None,
            ..meta("arxiv:untagged", 1, "cs.LG", &[])
        };
        save_paper_metadata(&mut conn, &untagged, None).unwrap();
        // NULL published is unreachable through save_paper_metadata; raw rows.
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:undated')",
            [],
        )
        .unwrap();
        let fk = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) \
             VALUES ('arxiv:undated', 1, 'U', ?)",
            [fk],
        )
        .unwrap();
        let pid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, TAGS) VALUES (?, '[\"Shared\"]')",
            [pid],
        )
        .unwrap();

        let hits = get_papers_by_tag(&conn, "sHaRed").unwrap();
        assert_eq!(
            hits.iter()
                .map(|p| p.source_id.as_str())
                .collect::<Vec<_>>(),
            // published DESC first, then the NULL-published paper last.
            ["arxiv:new", "arxiv:old", "arxiv:undated"]
        );
        // Whole-tag equality only — no substring matching.
        assert!(get_papers_by_tag(&conn, "share").unwrap().is_empty());
    }

    #[test]
    fn get_papers_by_tag_tiebreaks_same_date_by_paper_id_desc() {
        let mut conn = db();
        // Two papers share the same published date (version 1 -> 2024-01-01) and tag.
        save_paper_metadata(&mut conn, &meta("arxiv:A", 1, "cs.LG", &["shared"]), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:B", 1, "cs.LG", &["shared"]), None).unwrap();

        let hits = get_papers_by_tag(&conn, "shared").unwrap();
        assert_eq!(hits.len(), 2);
        // Same published date -> deterministic paper_id DESC (B saved after A).
        assert!(
            hits[0].paper_id > hits[1].paper_id,
            "same-date papers ordered by paper_id DESC"
        );
    }

    #[test]
    fn require_trashed_rejects_active_and_missing_papers() {
        let mut conn = db();
        let (fk, _v1, _v2) = seed_two_versions(&mut conn);
        // Active (never-trashed) and unknown papers both 404 out of trash-only ops.
        assert_eq!(
            require_trashed(&conn, "arxiv:A").unwrap_err().http_status(),
            404
        );
        assert_eq!(
            require_trashed(&conn, "arxiv:nope")
                .unwrap_err()
                .http_status(),
            404
        );
        delete(&mut conn, &PaperRef::SourceFk(fk)).unwrap();
        require_trashed(&conn, "arxiv:A").unwrap();
    }

    #[test]
    fn delete_restore_hard_delete_resolve_via_keys() {
        let mut conn = db();
        let (fk, v1, _v2) = seed_two_versions(&mut conn);
        // Link to a project so restore returns its membership.
        conn.execute(
            "INSERT INTO PROJECT (NAME, STATUS) VALUES ('P', 'active')",
            [],
        )
        .unwrap();
        let proj = conn.last_insert_rowid();
        proj_store::add_papers(&conn, proj, &[fk]).unwrap();

        // Soft-delete resolved via paper_id.
        delete(&mut conn, &PaperRef::Id(v1)).unwrap();
        assert!(store::is_paper_deleted(&conn, "arxiv:A").unwrap());
        let deleted = list_deleted(&conn).unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].source_id, "arxiv:A");
        assert_eq!(deleted[0].project_fks, vec![proj]); // enriched

        // Restore resolved via source_fk -> returns project_fks.
        let (_pdf, fks) = restore(&mut conn, &PaperRef::SourceFk(fk)).unwrap();
        assert_eq!(fks, vec![proj]);
        assert!(!store::is_paper_deleted(&conn, "arxiv:A").unwrap());

        // Hard-delete resolved via source_id.
        hard_delete(&mut conn, &PaperRef::source("arxiv:A".into())).unwrap();
        assert!(get(&conn, &PaperRef::source("arxiv:A".into()))
            .unwrap()
            .is_none());
    }

    #[test]
    fn pdf_and_fulltext_setters() {
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:p", 1, "cs.LG", &[]), None).unwrap();

        set_has_pdf(&conn, "arxiv:p", 1, true).unwrap();
        assert!(
            get(
                &conn,
                &PaperRef::Source {
                    source_id: "arxiv:p".into(),
                    version: Some(1)
                }
            )
            .unwrap()
            .unwrap()
            .has_pdf
        );

        set_pdf_path(&conn, "arxiv:p", "/tmp/a.pdf", Some(1)).unwrap();
        mark_pdf_saved(&mut conn, "arxiv:p", "/tmp/b.pdf", 1).unwrap();
        let got = get(
            &conn,
            &PaperRef::Source {
                source_id: "arxiv:p".into(),
                version: Some(1),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.pdf_path.as_deref(), Some("/tmp/b.pdf"));
        assert!(got.has_pdf);

        set_full_text(&mut conn, "arxiv:p", 1, "the full tex body").unwrap();
        let got = get(
            &conn,
            &PaperRef::Source {
                source_id: "arxiv:p".into(),
                version: Some(1),
            },
        )
        .unwrap()
        .unwrap();
        // The body is stored but never hydrated through `get` — every
        // PaperDetails read blanks FULL_TEXT.
        assert_eq!(got.full_text, None);
        assert!(store::has_full_text(&conn, "arxiv:p", 1).unwrap());
        assert!(got.downloaded_source);
    }

    /// Saving a paper then storing its text must make it findable — the claim the
    /// whole ingestion path exists to satisfy. Seeding `papers_fts` by hand (as the
    /// storage-layer search tests do) would not catch a break between the two.
    #[test]
    fn set_full_text_feeds_search_full_text() {
        use crate::storage::queries::search::search_full_text;
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:ft", 1, "cs.LG", &[]), None).unwrap();
        assert!(search_full_text(&conn, "zephyranthes", 10)
            .unwrap()
            .is_empty());

        set_full_text(&mut conn, "arxiv:ft", 1, "a study of zephyranthes blooms").unwrap();
        let hits = search_full_text(&conn, "zephyranthes", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "arxiv:ft");

        // The FTS row is keyed by SOURCE_ID, so a later version indexing empty
        // (corrupt tarball, PDF-only submission) must not take the paper out of
        // search — the clobber guard only ever sees one version's body.
        save_paper_metadata(&mut conn, &meta("arxiv:ft", 2, "cs.LG", &[]), None).unwrap();
        set_full_text(&mut conn, "arxiv:ft", 2, "").unwrap();
        assert_eq!(
            search_full_text(&conn, "zephyranthes", 10).unwrap().len(),
            1
        );

        // …and the rebuild after a delete has to find that older body too. It
        // would be unrecoverable otherwise: a forced re-fetch of v2 extracts
        // empty again and so stores nothing to rebuild from.
        let key = PaperRef::source("arxiv:ft".into());
        delete(&mut conn, &key).unwrap();
        assert!(search_full_text(&conn, "zephyranthes", 10)
            .unwrap()
            .is_empty());
        restore(&mut conn, &key).unwrap();
        assert_eq!(
            search_full_text(&conn, "zephyranthes", 10).unwrap().len(),
            1
        );
    }

    /// A re-fetch that extracts nothing must not wipe a body that already
    /// indexes — `extract_source` returns "" for a corrupt download too.
    #[test]
    fn commit_refuses_to_clobber_indexed_text_with_empty() {
        let commit = |conn: &mut Connection, text: &str| {
            FetchedFullText {
                source_id: "arxiv:clob".into(),
                version: 1,
                text: text.into(),
            }
            .commit(conn)
            .unwrap()
        };
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:clob", 1, "cs.LG", &[]), None).unwrap();

        // Nothing stored yet: an empty extract is written, marking it attempted.
        assert!(commit(&mut conn, "").indexed);
        // A paper stored as empty is not protected — retrying it is the point.
        assert!(commit(&mut conn, "real body text").indexed);

        // Now an empty extract is refused and the stored body survives…
        let refused = commit(&mut conn, "");
        assert!(!refused.indexed);
        assert!(refused.reason.unwrap().contains("kept the text"));
        assert!(store::has_full_text(&conn, "arxiv:clob", 1).unwrap());

        // …but a real re-fetch still replaces it.
        assert!(commit(&mut conn, "newer body text").indexed);
    }

    #[test]
    fn backfill_candidates_lists_only_unfetched_papers() {
        let mut conn = db();
        let fetchable = |sid: &str| PaperMetadata {
            url: Some(format!("http://arxiv.org/pdf/{sid}v1")),
            ..meta(sid, 1, "cs.LG", &[])
        };
        save_paper_metadata(&mut conn, &fetchable("arxiv:b1"), None).unwrap();
        save_paper_metadata(&mut conn, &fetchable("arxiv:b2"), None).unwrap();
        assert_eq!(full_text_backfill_candidates(&conn).unwrap().len(), 2);
        assert_eq!(full_text_backfill_count(&conn).unwrap(), 2);

        // set_full_text flips DOWNLOADED_SOURCE, so b1 drops off the work list.
        set_full_text(&mut conn, "arxiv:b1", 1, "indexed already").unwrap();
        assert_eq!(
            full_text_backfill_candidates(&conn).unwrap(),
            vec!["arxiv:b2".to_string()]
        );

        // Storing an empty extract also counts as fetched — the list empties out
        // instead of handing the same paper back on every run.
        set_full_text(&mut conn, "arxiv:b2", 1, "").unwrap();
        assert!(full_text_backfill_candidates(&conn).unwrap().is_empty());
        assert_eq!(full_text_backfill_count(&conn).unwrap(), 0);
    }

    #[test]
    fn backfill_candidates_skip_papers_with_no_tex_source_to_fetch() {
        // Everything source_fetch_url would refuse has to stay off the list:
        // the backfill would hand each one to the fetcher on every rebuild, and
        // the count backing the UI's progress line would never reach zero.
        let mut conn = db();
        let other_provider = PaperMetadata {
            url: Some("http://arxiv.org/pdf/1234v1".into()),
            source: Some("openalex".into()),
            ..meta("openalex:x", 1, "cs.LG", &[])
        };
        let abs_url_only = PaperMetadata {
            url: Some("http://arxiv.org/abs/2345v1".into()),
            ..meta("arxiv:noPdfLink", 1, "cs.LG", &[])
        };
        save_paper_metadata(&mut conn, &other_provider, None).unwrap();
        save_paper_metadata(&mut conn, &abs_url_only, None).unwrap();

        for sid in ["openalex:x", "arxiv:noPdfLink"] {
            let key = PaperRef::source(sid.into());
            let paper = get(&conn, &key).unwrap().unwrap();
            assert!(source_fetch_url(&paper).is_err());
        }
        assert!(full_text_backfill_candidates(&conn).unwrap().is_empty());
        assert_eq!(full_text_backfill_count(&conn).unwrap(), 0);
    }

    #[test]
    fn backfill_work_list_agrees_with_source_fetch_url() {
        // The work-list SQL and source_fetch_url must answer "is this arXiv"
        // identically on rows where the id namespace and the PROVIDER column
        // disagree — otherwise backfill queues papers the fetcher then skips.
        let mut conn = db();
        let row = |sid: &str, provider: &str, url: &str| PaperMetadata {
            url: Some(url.into()),
            source: Some(provider.into()),
            ..meta(sid, 1, "cs.LG", &[])
        };
        let cases = [
            // arxiv: id, PROVIDER blank as some import paths leave it.
            (row("arxiv:blank", "", "http://arxiv.org/pdf/1v1"), true),
            // arxiv: id with no /pdf/ link to derive the tarball URL from.
            (
                row("arxiv:absonly", "arxiv", "http://arxiv.org/abs/2v1"),
                false,
            ),
            // Local PDF import: PROVIDER says "pdf", and no arXiv id.
            (row("local:deadbeef", "pdf", "file:///x/pdf/y.pdf"), false),
            (row("openalex:W1", "openalex", "http://x.org/pdf/w1"), false),
            // Legacy row: migration 02 defaults PROVIDER to 'arxiv' for every
            // pre-existing paper, including imports arXiv never hosted.
            (
                row("local:legacy", "arxiv", "http://x.org/pdf/legacy"),
                false,
            ),
        ];
        for (m, _) in &cases {
            save_paper_metadata(&mut conn, m, None).unwrap();
        }

        let work_list = full_text_backfill_candidates(&conn).unwrap();
        assert_eq!(
            work_list.len() as i64,
            full_text_backfill_count(&conn).unwrap()
        );
        for (m, fetchable) in &cases {
            let key = PaperRef::source(m.source_id.clone());
            let paper = get(&conn, &key).unwrap().unwrap();
            assert_eq!(
                source_fetch_url(&paper).is_ok(),
                *fetchable,
                "source_fetch_url disagrees on {}",
                m.source_id
            );
            assert_eq!(
                work_list.contains(&m.source_id),
                *fetchable,
                "work list disagrees on {}",
                m.source_id
            );
        }
    }

    #[test]
    fn source_fetch_url_takes_arxiv_pdf_links_only() {
        let mut conn = db();
        let fetch_url_of = |conn: &Connection, sid: &str| {
            let p = get(conn, &PaperRef::source(sid.into())).unwrap().unwrap();
            source_fetch_url(&p).map(str::to_string)
        };

        let mut ok = meta("arxiv:s1", 1, "cs.LG", &[]);
        ok.url = Some("http://arxiv.org/pdf/2204.12985v4".into());
        save_paper_metadata(&mut conn, &ok, None).unwrap();
        assert_eq!(
            fetch_url_of(&conn, "arxiv:s1").unwrap(),
            "http://arxiv.org/pdf/2204.12985v4"
        );

        // An /abs/ link has no `/pdf/` segment to rewrite into `/src/`.
        let mut abs_only = meta("arxiv:s2", 1, "cs.LG", &[]);
        abs_only.url = Some("http://arxiv.org/abs/2204.12985v4".into());
        save_paper_metadata(&mut conn, &abs_only, None).unwrap();
        assert!(fetch_url_of(&conn, "arxiv:s2").is_err());

        // A stored paper with no URL at all (locally imported PDF).
        let mut no_url = meta("arxiv:s3", 1, "cs.LG", &[]);
        no_url.url = None;
        save_paper_metadata(&mut conn, &no_url, None).unwrap();
        assert!(fetch_url_of(&conn, "arxiv:s3").is_err());

        // Non-arXiv sources publish no source tarball, even at a /pdf/ URL.
        let mut crossref = meta("doi:10.1/y", 1, "cs.LG", &[]);
        crossref.source = Some("crossref".into());
        crossref.url = Some("http://example.com/pdf/y".into());
        save_paper_metadata(&mut conn, &crossref, None).unwrap();
        assert!(fetch_url_of(&conn, "doi:10.1/y").is_err());
    }

    #[test]
    fn root_and_id_helpers() {
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:v", 1, "cs.LG", &[]), None).unwrap();
        let fk = ensure_paper_root(&mut conn, "arxiv:v").unwrap();
        assert_eq!(
            get_source_id(&conn, fk).unwrap().as_deref(),
            Some("arxiv:v")
        );
        let by_fk = source_ids_by_fk(&conn, &[fk, 9_999]).unwrap();
        assert_eq!(by_fk.get(&fk).map(String::as_str), Some("arxiv:v"));
        assert!(!by_fk.contains_key(&9_999));
    }

    // Every front door goes through search_library, so a note-only hit that FTS
    // tokenization misses (substring) must still come back.
    #[test]
    fn search_library_merges_note_substring_hits() {
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:N", 1, "cs.LG", &[]), None).unwrap();
        ensure_paper_root(&mut conn, "arxiv:N").unwrap();
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE) \
             SELECT SOURCE_FK, 'n', 'on zephyranthes morphology' \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:N"],
        )
        .unwrap();

        // notes_fts tokenizes to whole words, so a substring only the LIKE scan
        // in search_notes_source_fks can reach.
        assert!(search_store::search_full_text(&conn, "orpholog", 10)
            .unwrap()
            .is_empty());
        let hits = search_library(&conn, "orpholog", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "arxiv:N");
    }

    // Note-only extras trail in note-recency order (MAX(UPDATED_AT) DESC), not
    // the bulk read's published/paper_id order — B is inserted first (lower
    // paper_id, same published date) so the bulk sort alone would yield [A, B].
    #[test]
    fn search_library_orders_note_hits_by_note_recency() {
        let mut conn = db();
        for sid in ["arxiv:noteB", "arxiv:noteA"] {
            save_paper_metadata(&mut conn, &meta(sid, 1, "cs.LG", &[]), None).unwrap();
            ensure_paper_root(&mut conn, sid).unwrap();
        }
        for (sid, updated_at) in [
            ("arxiv:noteA", "2024-01-01 00:00:00"),
            ("arxiv:noteB", "2024-06-01 00:00:00"),
        ] {
            conn.execute(
                "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE, UPDATED_AT) \
                 SELECT SOURCE_FK, 'n', 'on zephyranthes morphology', ?2 \
                 FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
                rusqlite::params![sid, updated_at],
            )
            .unwrap();
        }

        let hits = search_library(&conn, "orpholog", 10).unwrap();
        assert_eq!(
            hits.iter()
                .map(|p| p.source_id.as_str())
                .collect::<Vec<_>>(),
            ["arxiv:noteB", "arxiv:noteA"]
        );
    }

    // The case MCP used to drop before the merge was hoisted here: one query
    // where paper A matches via indexed full text and paper B only via note
    // content the FTS tokenizer can't reach. Both must return, FTS hit first.
    #[test]
    fn search_library_returns_full_text_and_note_hits_together() {
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:ftA", 1, "cs.LG", &[]), None).unwrap();
        set_full_text(&mut conn, "arxiv:ftA", 1, "a study of zephyranthes blooms").unwrap();

        save_paper_metadata(&mut conn, &meta("arxiv:ntB", 1, "cs.LG", &[]), None).unwrap();
        ensure_paper_root(&mut conn, "arxiv:ntB").unwrap();
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE) \
             SELECT SOURCE_FK, 'n', 'compare with megazephyranthes cultivars' \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:ntB"],
        )
        .unwrap();

        // "megazephyranthes" is one FTS token, so only the LIKE scan reaches B.
        let fts = search_store::search_full_text(&conn, "zephyranthes", 10).unwrap();
        assert_eq!(fts.len(), 1);
        assert_eq!(fts[0].source_id, "arxiv:ftA");

        let hits = search_library(&conn, "zephyranthes", 10).unwrap();
        assert_eq!(
            hits.iter()
                .map(|p| p.source_id.as_str())
                .collect::<Vec<_>>(),
            ["arxiv:ftA", "arxiv:ntB"]
        );
    }

    // Repair input rules live behind the service seam, not in one caller.
    #[test]
    fn repair_paper_validates_and_normalizes() {
        let mut conn = db();
        save_paper_metadata(&mut conn, &meta("arxiv:R", 1, "cs.LG", &[]), None).unwrap();
        let fk = ensure_paper_root(&mut conn, "arxiv:R").unwrap();

        let blank = PaperMetadata {
            title: "   ".into(),
            ..meta("arxiv:R", 1, "cs.LG", &[])
        };
        assert!(matches!(
            repair_paper(&mut conn, fk, &blank),
            Err(CoreError::Validation(_))
        ));

        let no_authors = PaperMetadata {
            authors: vec!["".into(), "  ".into()],
            ..meta("arxiv:R", 1, "cs.LG", &[])
        };
        assert!(matches!(
            repair_paper(&mut conn, fk, &no_authors),
            Err(CoreError::Validation(_))
        ));

        let empty_doi = PaperMetadata {
            doi: Some(String::new()),
            ..meta("arxiv:R", 1, "cs.LG", &[])
        };
        assert!(matches!(
            repair_paper(&mut conn, fk, &empty_doi),
            Err(CoreError::Validation(_))
        ));

        let dupes = PaperMetadata {
            title: "  Fixed  ".into(),
            authors: vec!["Ada".into(), " Ada ".into(), "".into(), "Bo".into()],
            tags: Some(vec!["nlp".into(), " nlp ".into(), "  ".into()]),
            ..meta("arxiv:R", 1, "cs.LG", &[])
        };
        repair_paper(&mut conn, fk, &dupes).unwrap();
        let got = get(&conn, &PaperRef::SourceFk(fk)).unwrap().unwrap();
        assert_eq!(got.title, "Fixed");
        assert_eq!(got.authors, vec!["Ada".to_string(), "Bo".to_string()]);
        assert_eq!(got.tags, vec!["nlp".to_string()]);

        // Blank-only tags collapse to None (Python `tags or None`) — every
        // front door hands raw tags to repair and relies on this.
        let blank_tags = PaperMetadata {
            tags: Some(vec!["  ".into()]),
            ..meta("arxiv:R", 1, "cs.LG", &[])
        };
        repair_paper(&mut conn, fk, &blank_tags).unwrap();
        let got = get(&conn, &PaperRef::SourceFk(fk)).unwrap().unwrap();
        assert!(got.tags.is_empty());

        assert!(parse_published("2024-13-01").is_err());
        assert_eq!(
            parse_published("2024-01-02").unwrap(),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap()
        );
    }

    #[test]
    fn pure_filename_and_parse_helpers() {
        assert_eq!(pdf_filename_safe("arxiv:2204.12985"), "arxiv_2204.12985");
        assert_eq!(
            pdf_filename_safe(r#"a/b\c:d*e?f"g<h>i|j"#),
            "a_b_c_d_e_f_g_h_i_j"
        );
        // On-disk format: no underscore before 'v'.
        assert_eq!(
            pdf_on_disk_name("arxiv:2204.12985", 4),
            "arxiv_2204.12985v4.pdf"
        );
    }
}
