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

/// The `date.min` sentinel (`0001-01-01`) used to mark "no published date".
fn date_min() -> NaiveDate {
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
    #[serde(default)]
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

/// `FullAuthorDetails(BasicAuthorDetails)` — flattened (no Rust inheritance);
/// `to_dict` emits a flat object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullAuthorDetails {
    #[serde(flatten)]
    pub base: BasicAuthorDetails,
    #[serde(default)]
    pub paper_ids: Option<Vec<i64>>,
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

// ---------------------------------------------------------------------------
// Project (service/models/project.py)
// ---------------------------------------------------------------------------

/// Literal["active","archived","deleted"] — validates at deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    Archived,
    Deleted,
}

impl Default for Status {
    fn default() -> Self {
        Status::Active
    }
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
}

impl ProjectDetails {
    /// Derived `paper_count = len(source_fks)`; emitted by the `Serialize` impl
    /// below to match `service/models/project.py::to_dict` (plan §5.2, D16).
    pub fn paper_count(&self) -> usize {
        self.source_fks.len()
    }
}

// Manual Serialize so the JSON matches Python's to_dict: 11 keys in the same
// order, with the derived `paper_count` between `source_fks` and `status`.
// A `#[derive(Serialize)]` would silently drop paper_count (it is a method).
impl Serialize for ProjectDetails {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("ProjectDetails", 11)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("description", &self.description)?;
        st.serialize_field("color", &self.color)?;
        st.serialize_field("project_tags", &self.project_tags)?;
        st.serialize_field("source_fks", &self.source_fks)?;
        st.serialize_field("paper_count", &self.paper_count())?;
        st.serialize_field("status", &self.status)?;
        st.serialize_field("created_at", &self.created_at)?;
        st.serialize_field("updated_at", &self.updated_at)?;
        st.serialize_field("archived_at", &self.archived_at)?;
        st.end()
    }
}

// ---------------------------------------------------------------------------
// Note (service/models/note.py) — `note_id` serializes as "id"
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteDetails {
    #[serde(rename = "id")]
    pub note_id: Option<i64>,
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
    fn project_details_emits_paper_count_in_order() {
        let p = ProjectDetails {
            source_fks: vec![1, 2, 3],
            ..ProjectDetails {
                id: Some(5),
                name: "n".into(),
                description: String::new(),
                color: None,
                project_tags: vec![],
                source_fks: vec![],
                status: Status::Active,
                created_at: None,
                updated_at: None,
                archived_at: None,
            }
        };
        let s = serde_json::to_string(&p).unwrap();
        // derived field present and correct (a derive(Serialize) would drop it)
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&s).unwrap()["paper_count"],
            3
        );
        // and emitted between source_fks and status, matching to_dict order
        let fks = s.find("source_fks").unwrap();
        let pc = s.find("paper_count").unwrap();
        let st = s.find("\"status\"").unwrap();
        assert!(fks < pc && pc < st);
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
