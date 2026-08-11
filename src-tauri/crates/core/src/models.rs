//! Domain models + the two API serializers, ported from `service/models/*`,
//! `sources/base.py` (PaperMetadata), and `api/app.py`.
//!
//! Plan refs: §5.1 (serializers), §5.2 (domain models), D16.
//! D16 NON-NEGOTIABLE: `SearchResultOut` and `PaperDetails` are TWO DISTINCT
//! serializers and must NOT be unified — their JSON contracts differ
//! (`paper_url`/`primary_category`/`entry_id` + `published=""` sentinel vs.
//! `url`/`category` + `published: null`).
//!
//! Dates/datetimes use chrono and serialize as ISO strings (matching Python's
//! `.isoformat()`). LIST columns (categories/authors/tags) are `Vec<String>`
//! here — their JSON-string (de)serialization is a storage-layer concern.

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Bare paper ID with the source namespace prefix removed.
/// Mirrors `_strip_namespace`: `"arxiv:2204.12985"` -> `"2204.12985"`,
/// `"2204.12985"` (no colon) -> `"2204.12985"`.
pub fn strip_namespace(source_id: &str) -> String {
    source_id
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(source_id)
        .to_string()
}

/// The namespace every arXiv `source_id` carries. Identity, not provenance:
/// `PAPER_META.PROVIDER` is blank on some import paths and defaults to `'arxiv'`
/// for rows predating the column, so it cannot answer "is this an arXiv paper".
pub const ARXIV_ID_PREFIX: &str = "arxiv:";

/// The path segment an arXiv PDF link carries; the TeX tarball URL is derived
/// by swapping it for `/src/`.
pub const ARXIV_PDF_MARKER: &str = "/pdf/";

/// Whether `source_id` names a paper arXiv hosts. The one home for the test —
/// SQL callers build their pattern from [`ARXIV_ID_PREFIX`].
pub fn is_arxiv_source_id(source_id: &str) -> bool {
    source_id.starts_with(ARXIV_ID_PREFIX)
}

pub const OPENALEX_ID_PREFIX: &str = "openalex:";
pub const DOI_ID_PREFIX: &str = "doi:";
pub const LOCAL_ID_PREFIX: &str = "local:";

/// The one home for `source_id` namespace construction (CONTEXT.md § source_id,
/// ADR 0002). Sources build ids here so [`strip_namespace`] is the exact inverse.
pub fn arxiv_source_id(bare_id: &str) -> String {
    format!("{ARXIV_ID_PREFIX}{bare_id}")
}

pub fn openalex_source_id(work_id: &str) -> String {
    format!("{OPENALEX_ID_PREFIX}{work_id}")
}

pub fn doi_source_id(doi: &str) -> String {
    format!("{DOI_ID_PREFIX}{doi}")
}

pub fn local_source_id(hash: &str) -> String {
    format!("{LOCAL_ID_PREFIX}{hash}")
}

/// Strip one leading provider prefix if present (`removeprefix` semantics —
/// at most once, never mid-string), else return the id unchanged.
pub fn strip_provider_prefix<'a>(source_id: &'a str, prefix: &str) -> &'a str {
    source_id.strip_prefix(prefix).unwrap_or(source_id)
}

/// The `date.min` sentinel (`0001-01-01`) used to mark "no published date".
pub(crate) fn date_min() -> NaiveDate {
    NaiveDate::from_ymd_opt(1, 1, 1).expect("0001-01-01 is a valid date")
}

// ---------------------------------------------------------------------------
// PaperMetadata — normalized, source-agnostic record (sources/base.py)
// ---------------------------------------------------------------------------

/// Normalized paper representation produced by every `PaperSource`.
/// `categories`/`tags` stay `Option` here (pydantic `list[str] | None`),
/// unlike the DB-row `PaperDetails` where they default to an empty `Vec`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperMetadata {
    /// Namespaced ID, e.g. "arxiv:2204.12985", "openalex:W31...", "local:{hash}".
    pub source_id: String,
    /// Defaults to 1 for non-arxiv sources.
    pub version: i64,
    pub title: String,
    pub authors: Vec<String>,
    pub published: NaiveDate,
    #[serde(default)]
    pub updated: Option<NaiveDate>,
    pub summary: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub categories: Option<Vec<String>>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub journal_ref: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Backend that produced this record (must equal that source's `source_name`).
    #[serde(default)]
    pub source: Option<String>,
    /// Index-aligned with `authors` (same length when present); `None` per-author
    /// where the source didn't carry one. Only crossref/openalex populate this.
    #[serde(default)]
    pub author_orcids: Option<Vec<Option<String>>>,
}

/// Strip an `http(s)://orcid.org/` prefix + trailing slash/query/fragment,
/// uppercase the checksum digit. `None` if not `\d{4}-\d{4}-\d{4}-\d{3}[\dX]`.
pub fn normalize_orcid(raw: &str) -> Option<String> {
    let t = raw.trim();
    let t = ["https://orcid.org/", "http://orcid.org/"]
        .iter()
        .find_map(|p| t.strip_prefix(p))
        .unwrap_or(t);
    let t = t.split(['?', '#']).next().unwrap_or(t);
    let t = t.trim_end_matches('/');
    let up = t.to_uppercase();
    is_orcid_shaped(&up).then_some(up)
}

/// `\d{4}-\d{4}-\d{4}-\d{3}[\dX]` — the ORCID iD checksum-digit shape.
fn is_orcid_shaped(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 19
        && b[4] == b'-'
        && b[9] == b'-'
        && b[14] == b'-'
        && b[0..4].iter().all(u8::is_ascii_digit)
        && b[5..9].iter().all(u8::is_ascii_digit)
        && b[10..14].iter().all(u8::is_ascii_digit)
        && b[15..18].iter().all(u8::is_ascii_digit)
        && (b[18].is_ascii_digit() || b[18] == b'X')
}

// ---------------------------------------------------------------------------
// SERIALIZER 1 — SearchResultOut (api/app.py SearchResultOut.from_metadata)
// ---------------------------------------------------------------------------

/// Search-result wire shape. D16: distinct from `PaperDetails`; do not unify.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultOut {
    pub source_id: String,
    pub version: i64,
    pub title: String,
    pub summary: String,
    pub authors: Vec<String>,
    /// "" when the published date is the `date.min` sentinel; else ISO date.
    pub published: String,
    pub paper_url: String,
    pub primary_category: String,
    /// The full namespaced source_id (kept, unlike the stripped `source_id`).
    pub entry_id: String,
}

impl From<PaperMetadata> for SearchResultOut {
    fn from(m: PaperMetadata) -> Self {
        SearchResultOut {
            source_id: strip_namespace(&m.source_id),
            version: m.version,
            title: m.title,
            summary: m.summary,
            authors: m.authors,
            published: if m.published == date_min() {
                String::new()
            } else {
                m.published.to_string() // ISO "YYYY-MM-DD"
            },
            paper_url: m.url.unwrap_or_default(),
            primary_category: m.category.unwrap_or_default(),
            entry_id: m.source_id,
        }
    }
}

// ---------------------------------------------------------------------------
// SERIALIZER 2 — PaperDetails (service/models/paper.py PaperDetails.to_dict)
// ---------------------------------------------------------------------------

/// Full paper view. D16: distinct from `SearchResultOut`; do not unify.
/// `published`/`updated` are `Option<NaiveDate>` -> ISO string or `null`,
/// matching `to_dict`'s `.isoformat() if d else None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperDetails {
    pub paper_id: i64,
    pub source_id: String,
    pub version: i64,
    pub title: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub published: Option<NaiveDate>,
    #[serde(default)]
    pub updated: Option<NaiveDate>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub journal_ref: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub has_pdf: bool,
    #[serde(default)]
    pub pdf_path: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    /// Search-index payload, not a display field: megabytes of TeX per paper
    /// once ingestion runs.
    #[serde(default, skip_serializing)]
    pub full_text: Option<String>,
    #[serde(default)]
    pub downloaded_source: bool,
    #[serde(default)]
    pub source_fk: Option<i64>,
}

/// Aggregate view of a paper across all stored versions (PaperDetailsAll).
/// Display fields come from the latest version; `versions` is oldest-first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperDetailsAll {
    pub source_id: String,
    pub latest_version: i64,
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub published: Option<NaiveDate>,
    #[serde(default)]
    pub updated: Option<NaiveDate>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub journal_ref: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub versions: Vec<PaperDetails>,
}

// ---------------------------------------------------------------------------
// Authors (service/models/author.py)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicAuthorDetails {
    pub author_id: i64,
    #[serde(default)]
    pub orcid: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

/// `AuthorWithCount(BasicAuthorDetails)` — flattened base + `paper_count`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorWithCount {
    #[serde(flatten)]
    pub base: BasicAuthorDetails,
    #[serde(default)]
    pub paper_count: i64,
}

/// A tag label with its distinct *active*-paper count, for the Tags index table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagWithCount {
    pub label: String,
    pub paper_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorPaperPreview {
    pub paper_id: i64,
    pub source_id: String,
    pub source_fk: i64,
    pub version: i64,
    #[serde(default)]
    pub title: Option<String>,
}

/// `{**author, paper_count, papers}` — the author-detail composite every surface
/// (route GET/PATCH, `linxiv author get`, MCP `get_author`) emits.
#[derive(Debug, Clone, Serialize)]
pub struct AuthorWithPapers {
    #[serde(flatten)]
    pub base: BasicAuthorDetails,
    pub paper_count: usize,
    pub papers: Vec<AuthorPaperPreview>,
}

// ---------------------------------------------------------------------------
// Project (service/models/project.py)
// ---------------------------------------------------------------------------

/// Literal["active","archived","deleted"] — validates at deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Active,
    Archived,
    Deleted,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectDetails {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub color: Option<i32>,
    #[serde(default)]
    pub project_tags: Vec<String>,
    #[serde(default)]
    pub source_fks: Vec<i64>,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub created_at: Option<NaiveDateTime>,
    #[serde(default)]
    pub updated_at: Option<NaiveDateTime>,
    #[serde(default)]
    pub archived_at: Option<NaiveDateTime>,
    /// Persisted share identity (uuid v4); NULL until first publish.
    #[serde(default)]
    pub share_id: Option<String>,
}

// SERIALIZER 3 — ProjectOut: the one project wire shape, emitted identically by
// the route, the CLI and MCP (ADR-0011 scope). `ProjectDetails` itself is
// deliberately NOT Serialize so no surface can bypass this shape. Produced only
// via `service::project::to_out`, which resolves `source_fks` → namespaced
// `source_ids` and renders `color` as `color_hex`.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectOut {
    pub id: Option<i64>,
    pub name: String,
    pub description: String,
    pub color_hex: Option<String>,
    pub project_tags: Vec<String>,
    pub source_ids: Vec<String>,
    pub paper_count: usize,
    pub status: Status,
    pub created_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
    pub archived_at: Option<NaiveDateTime>,
    pub share_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Note (service/models/note.py) — `note_id` serializes as "id"
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteDetails {
    #[serde(rename = "id")]
    pub note_id: Option<i64>,
    /// Stable identity (uuid v4) surviving export/import + share.
    #[serde(default)]
    pub uuid: String,
    pub source_fk: i64,
    #[serde(default)]
    pub paper_id_fk: Option<i64>,
    #[serde(default)]
    pub project_id: Option<i64>,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub created_at: Option<NaiveDateTime>,
    #[serde(default)]
    pub updated_at: Option<NaiveDateTime>,
}

// ---------------------------------------------------------------------------
// Annotation (PDF highlights) — `annotation_id` serializes as "id", like
// NoteDetails. ANCHOR is opaque JSON (size-capped by validate_anchor; the
// frontend renderer reads its structure); COMMENT defaults "".
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationDetails {
    #[serde(rename = "id")]
    pub annotation_id: i64,
    /// Stable identity (uuid v4) surviving export/import + share.
    #[serde(default)]
    pub uuid: String,
    pub source_fk: i64,
    #[serde(default)]
    pub project_id: Option<i64>,
    pub anchor: String,
    pub comment: String,
    #[serde(default)]
    pub created_at: Option<NaiveDateTime>,
    #[serde(default)]
    pub updated_at: Option<NaiveDateTime>,
}

/// Reject an empty/whitespace-only or over-cap ANCHOR. Returns the message each
/// write boundary surfaces in its own error type; cap is 64 KiB.
pub fn validate_anchor(anchor: &str) -> std::result::Result<(), &'static str> {
    if anchor.trim().is_empty() {
        return Err("anchor must not be empty");
    }
    if anchor.len() > 65_536 {
        return Err("anchor exceeds maximum size");
    }
    Ok(())
}

/// Resolve a UUID string to `Some(canonical_uuid)` only if it parses and the
/// normalized form is not taken; `taken` is checked against the canonical string.
pub(crate) fn resolve_uuid(
    u: &str,
    taken: impl FnOnce(&str) -> crate::error::Result<bool>,
) -> crate::error::Result<Option<String>> {
    let Ok(parsed) = uuid::Uuid::parse_str(u) else {
        tracing::warn!("invalid uuid format: {}", u);
        return Ok(None);
    };
    let normalized = parsed.to_string();
    if taken(&normalized)? {
        tracing::debug!("uuid collision: {}", normalized);
        return Ok(None);
    }
    Ok(Some(normalized))
}

/// Insert DTO. `comment` defaults to "" (highlight with no written comment).
#[derive(Debug, Clone, Deserialize)]
pub struct AnnotationIn {
    pub source_fk: i64,
    pub anchor: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub project_fk: Option<i64>,
    /// None = generate a fresh uuid at insert; Some = preserve (import path).
    #[serde(default)]
    pub uuid: Option<String>,
}

/// PATCH DTO. Only the written comment is mutable — the anchor (geometry/quote)
/// is immutable; re-highlighting creates a new annotation.
#[derive(Debug, Clone, Deserialize)]
pub struct AnnotationUpdateIn {
    pub annotation_id: i64,
    pub comment: String,
}

// ---------------------------------------------------------------------------
// Tag (service/models/tag.py)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagDetails {
    pub tag_id: i64,
    #[serde(default)]
    pub label: Option<String>,
}

// ---------------------------------------------------------------------------
// Service input DTOs (service/{author,tag,note,paper,project}.py *In classes)
// ---------------------------------------------------------------------------

/// D16 UNSET sentinel deserializer. Maps a JSON field's three states onto
/// `Option<Option<T>>`: ABSENT -> `None` (unchanged), `null` -> `Some(None)`
/// (clear), value -> `Some(Some(v))`. Pair with `#[serde(default, ...)]` so an
/// absent key yields `None` (plain `Option<Option<T>>` would swallow `null`
/// into `None`, collapsing clear and unchanged). Mirrors `project.py::Unset`.
fn de_unset<'de, T, D>(de: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<T>::deserialize(de)?))
}

/// `service/author.py::AuthorIn`.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthorIn {
    pub full_name: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub orcid: Option<String>,
}

/// `service/tag.py::TagIn`.
#[derive(Debug, Clone, Deserialize)]
pub struct TagIn {
    pub label: String,
}

/// `service/note.py::NoteIn`.
#[derive(Debug, Clone, Deserialize)]
pub struct NoteIn {
    pub source_fk: i64,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub paper_id: Option<i64>,
    #[serde(default)]
    pub project_fk: Option<i64>,
    /// None = generate a fresh uuid at insert; Some = preserve (import path).
    #[serde(default)]
    pub uuid: Option<String>,
}

/// `service/note.py::NoteUpdateIn`. title/content are non-nullable columns:
/// absent and null both mean "unchanged" (plain `Option`), so no UNSET sentinel
/// here — mirrors Python where `None` means "not provided". Service enforces
/// "at least one of title/content provided" (Python `__post_init__`).
#[derive(Debug, Clone, Deserialize)]
pub struct NoteUpdateIn {
    pub note_id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

/// `service/paper.py::PaperIn`.
#[derive(Debug, Clone, Deserialize)]
pub struct PaperIn {
    pub title: String,
    pub published: NaiveDate,
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub authors: Option<Vec<String>>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub source: Option<String>,
}

/// `service/project.py::ProjectIn`.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectIn {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub color: Option<i32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source_fks: Vec<i64>,
}

/// `service/project.py::update(...)` as a PATCH DTO. `color` is the D16 UNSET
/// case: absent -> unchanged, `null` -> clear, value -> set (mirrors the
/// `color: int | None | Unset = UNSET` signature). Other fields are plain
/// `Option` (absent/null -> unchanged).
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectUpdateIn {
    pub project_fk: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "de_unset")]
    pub color: Option<Option<i32>>,
    #[serde(default)]
    pub project_tags: Option<Vec<String>>,
    #[serde(default)]
    pub status: Option<Status>,
}

// ---------------------------------------------------------------------------
// Checks — the only non-trivial logic here is SearchResultOut::from_metadata
// (namespace strip, date.min sentinel, "" coalescing of url/category).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_constructors_round_trip() {
        assert_eq!(strip_namespace(&arxiv_source_id("2204.12985")), "2204.12985");
        assert_eq!(strip_namespace(&openalex_source_id("W123")), "W123");
        assert_eq!(strip_namespace(&doi_source_id("10.1000/xyz")), "10.1000/xyz");
        assert_eq!(strip_namespace(&local_source_id("deadbeef")), "deadbeef");
        assert!(is_arxiv_source_id(&arxiv_source_id("2204.12985")));
        assert!(!is_arxiv_source_id(&openalex_source_id("W123")));
        assert!(!is_arxiv_source_id(&doi_source_id("10.1000/xyz")));
        assert!(!is_arxiv_source_id(&local_source_id("deadbeef")));
        // removeprefix semantics: at most one leading prefix comes off.
        assert_eq!(strip_provider_prefix("doi:doi:1", DOI_ID_PREFIX), "doi:1");
        assert_eq!(strip_provider_prefix("10.1000/xyz", DOI_ID_PREFIX), "10.1000/xyz");
    }

    #[test]
    fn normalize_orcid_strips_prefix_trailing_slash_and_uppercases() {
        assert_eq!(
            normalize_orcid("https://orcid.org/0000-0002-1825-0097"),
            Some("0000-0002-1825-0097".to_string())
        );
        assert_eq!(
            normalize_orcid("http://orcid.org/0000-0002-1825-0097/"),
            Some("0000-0002-1825-0097".to_string())
        );
        assert_eq!(
            normalize_orcid("https://orcid.org/0000-0002-1825-0097/?foo=1"),
            Some("0000-0002-1825-0097".to_string())
        );
        assert_eq!(
            normalize_orcid("0000-0002-1825-009x"),
            Some("0000-0002-1825-009X".to_string())
        );
    }

    #[test]
    fn normalize_orcid_rejects_malformed_values() {
        assert_eq!(normalize_orcid(""), None);
        assert_eq!(normalize_orcid("not-an-orcid"), None);
        assert_eq!(normalize_orcid("0000-0002-1825-00977"), None); // too long
        assert_eq!(normalize_orcid("0000-0002-1825-009"), None); // too short
    }

    fn meta(source_id: &str, published: NaiveDate) -> PaperMetadata {
        PaperMetadata {
            source_id: source_id.into(),
            version: 1,
            title: "T".into(),
            authors: vec!["A".into()],
            published,
            updated: None,
            summary: "S".into(),
            category: None,
            categories: None,
            doi: None,
            journal_ref: None,
            comment: None,
            url: None,
            tags: None,
            source: None,
            author_orcids: None,
        }
    }

    #[test]
    fn search_result_strips_namespace_keeps_entry_id_and_sentinels() {
        let out = SearchResultOut::from(meta("arxiv:2204.12985", date_min()));
        assert_eq!(out.source_id, "2204.12985");
        assert_eq!(out.entry_id, "arxiv:2204.12985");
        assert_eq!(out.published, ""); // date.min -> ""
        assert_eq!(out.paper_url, ""); // None -> ""
        assert_eq!(out.primary_category, ""); // None -> ""
    }

    #[test]
    fn search_result_passes_through_bare_id_and_real_values() {
        let mut m = meta("1234.5678", NaiveDate::from_ymd_opt(2024, 3, 5).unwrap());
        m.url = Some("http://x".into());
        m.category = Some("cs.LG".into());
        let out = SearchResultOut::from(m);
        assert_eq!(out.source_id, "1234.5678");
        assert_eq!(out.entry_id, "1234.5678"); // no namespace to strip
        assert_eq!(out.published, "2024-03-05");
        assert_eq!(out.paper_url, "http://x");
        assert_eq!(out.primary_category, "cs.LG");
    }

    #[test]
    fn status_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Status::Archived).unwrap(),
            "\"archived\""
        );
        assert_eq!(
            serde_json::from_str::<Status>("\"deleted\"").unwrap(),
            Status::Deleted
        );
    }

    #[test]
    fn note_id_field_serializes_as_id() {
        let n = NoteDetails {
            note_id: Some(7),
            uuid: "u-7".into(),
            source_fk: 1,
            paper_id_fk: None,
            project_id: None,
            title: "t".into(),
            content: "c".into(),
            created_at: None,
            updated_at: None,
        };
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["id"], 7);
        assert!(v.get("note_id").is_none());
    }

    #[test]
    fn project_out_wire_shape_is_pinned() {
        let p = ProjectOut {
            id: Some(5),
            name: "n".into(),
            description: String::new(),
            color_hex: Some("#00ff00".into()),
            project_tags: vec![],
            source_ids: vec!["arxiv:2204.12985".into()],
            paper_count: 1,
            status: Status::Active,
            created_at: None,
            updated_at: None,
            archived_at: None,
            share_id: None,
        };
        let v = serde_json::to_value(&p).unwrap();
        // Exact keys in exact order — the canonical shape all three surfaces emit.
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "id",
                "name",
                "description",
                "color_hex",
                "project_tags",
                "source_ids",
                "paper_count",
                "status",
                "created_at",
                "updated_at",
                "archived_at",
                "share_id",
            ]
        );
        assert_eq!(v["color_hex"], serde_json::json!("#00ff00"));
        assert_eq!(v["source_ids"], serde_json::json!(["arxiv:2204.12985"]));
        assert_eq!(v["paper_count"], serde_json::json!(1));
        assert_eq!(v["status"], serde_json::json!("active"));
    }

    #[test]
    fn project_update_color_distinguishes_absent_null_value() {
        // ABSENT key -> None (unchanged)
        let absent: ProjectUpdateIn = serde_json::from_str(r#"{"project_fk":1}"#).unwrap();
        assert_eq!(absent.color, None);
        // explicit null -> Some(None) (clear)
        let null: ProjectUpdateIn =
            serde_json::from_str(r#"{"project_fk":1,"color":null}"#).unwrap();
        assert_eq!(null.color, Some(None));
        // value -> Some(Some(v)) (set)
        let val: ProjectUpdateIn = serde_json::from_str(r#"{"project_fk":1,"color":42}"#).unwrap();
        assert_eq!(val.color, Some(Some(42)));
    }
}
