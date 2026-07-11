//! crossref — CrossRef REST API source (port of `sources/crossref_source.py`).
//!
//! The pure parser (`parse_work` + the JATS tag-stripper + date-parts handling)
//! is the load-bearing piece and is fixture-tested below. `doi_resolve` reuses
//! `parse_work`. The async fetch wrappers route through `sources::http`
//! (integration-tested once the http unit lands).
//!
//! Plan §5.4. No auth required; `api.crossref.org` only.

use chrono::NaiveDate;
use serde_json::Value;

use super::http;
use crate::models::PaperMetadata;

const CROSSREF_BASE: &str = "https://api.crossref.org/works";
/// CrossRef is reached over exactly one host; the http guard enforces it.
const ALLOW: &[&str] = &["api.crossref.org"];

// ---------------------------------------------------------------------------
// Pure parsers (sync, fixture-tested) — reused by doi_resolve.
// ---------------------------------------------------------------------------

/// Strip well-formed `<...>` tags, matching Python's `re.sub(r"<[^>]+>", "", s)`:
/// a tag needs `>` after at least one non-`>` char, so a bare `<>` or an unclosed
/// `<` is left as a literal `<` (same as the regex not matching it).
pub fn strip_jats_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        match after.find('>') {
            // `>` at index >= 1 means >=1 non-`>` char between: a real tag, drop it.
            Some(gt) if gt >= 1 => rest = &after[gt + 1..],
            // bare `<>` or no closing `>`: keep the literal `<`, keep scanning.
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `date-parts: [[Y]]` / `[[Y, M]]` / `[[Y, M, D]]` -> a date, month/day default
/// to 1. Returns `None` (caller falls back to today) when absent, empty, or any
/// component is non-numeric — mirroring the Python try/except around `date(...)`.
fn parse_published(msg: &Value) -> Option<NaiveDate> {
    let parts = msg
        .get("published")?
        .get("date-parts")?
        .as_array()?
        .first()?
        .as_array()?;
    let y = parts.first()?.as_i64()?;
    let m = parts.get(1).and_then(Value::as_i64).unwrap_or(1);
    let d = parts.get(2).and_then(Value::as_i64).unwrap_or(1);
    NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32)
}

/// Convert one CrossRef work message into normalized `PaperMetadata`.
/// `doi` (when non-empty) wins over the message's own `DOI` field, matching the
/// Python `doi or msg.get("DOI", "")`.
pub fn parse_work(msg: &Value, doi: &str) -> PaperMetadata {
    let title = msg
        .get("title")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut authors = Vec::new();
    if let Some(arr) = msg.get("author").and_then(Value::as_array) {
        for a in arr {
            let given = a.get("given").and_then(Value::as_str).unwrap_or("");
            let family = a.get("family").and_then(Value::as_str).unwrap_or("");
            let name = format!("{given} {family}");
            let name = name.trim();
            if !name.is_empty() {
                authors.push(name.to_string());
            }
        }
    }

    let published = parse_published(msg).unwrap_or_else(|| chrono::Utc::now().date_naive());

    let abstract_raw = msg.get("abstract").and_then(Value::as_str).unwrap_or("");
    let summary = strip_jats_tags(abstract_raw).trim().to_string();

    let journal = msg
        .get("container-title")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .unwrap_or("");

    let paper_doi = Some(doi)
        .filter(|d| !d.is_empty())
        .or_else(|| msg.get("DOI").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();

    // URL from the message, else a doi.org link, else None.
    let url = msg
        .get("URL")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| (!paper_doi.is_empty()).then(|| format!("https://doi.org/{paper_doi}")));

    PaperMetadata {
        source_id: if paper_doi.is_empty() {
            String::new()
        } else {
            format!("doi:{paper_doi}")
        },
        version: 1,
        title,
        authors,
        published,
        updated: None,
        summary,
        category: (!journal.is_empty()).then(|| journal.to_string()),
        categories: None,
        doi: (!paper_doi.is_empty()).then(|| paper_doi.clone()),
        journal_ref: None,
        comment: None,
        url,
        tags: None,
        source: Some("crossref".to_string()),
    }
}

/// Parse a single-work response body (`{"message": {...}}`). `None` when the body
/// is unparseable or the work has no title — same gate as Python's
/// `if not msg.get("title"): return None`.
pub fn parse_doi_body(body: &[u8], doi: &str) -> Option<PaperMetadata> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let msg = v.get("message")?;
    let has_title = msg
        .get("title")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    has_title.then(|| parse_work(msg, doi))
}

/// Parse a search response body (`{"message": {"items": [...]}}`). Items with no
/// title or no DOI are skipped (Python `if item.get("title") and doi`).
pub fn parse_search_body(body: &[u8]) -> Vec<PaperMetadata> {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    let Some(items) = v
        .get("message")
        .and_then(|m| m.get("items"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let has_title = item
                .get("title")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty());
            let doi = item.get("DOI").and_then(Value::as_str).unwrap_or("");
            (has_title && !doi.is_empty()).then(|| parse_work(item, doi))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Async fetch wrappers (against the sources::http seam).
// ---------------------------------------------------------------------------

/// Fetch CrossRef metadata for a DOI. `None` on any non-200 / network / parse
/// error, matching the Python `except Exception: return None`.
pub async fn fetch_by_doi(doi: &str) -> Option<PaperMetadata> {
    let url = format!("{CROSSREF_BASE}/{doi}");
    let resp = http::get_guarded(&url, ALLOW).await.ok()?;
    if resp.status() != reqwest::StatusCode::OK {
        return None;
    }
    let body = resp.bytes().await.ok()?;
    parse_doi_body(&body, doi)
}

/// Search CrossRef by title. Empty vec on any error.
pub async fn search_by_title(title: &str, limit: u32) -> Vec<PaperMetadata> {
    let Ok(url) = reqwest::Url::parse_with_params(
        CROSSREF_BASE,
        [("query.title", title), ("rows", &limit.to_string())],
    ) else {
        return Vec::new();
    };
    let Ok(resp) = http::get_guarded(url.as_str(), ALLOW).await else {
        return Vec::new();
    };
    if resp.status() != reqwest::StatusCode::OK {
        return Vec::new();
    }
    let Ok(body) = resp.bytes().await else {
        return Vec::new();
    };
    parse_search_body(&body)
}

// ---------------------------------------------------------------------------
// Tests — recorded CrossRef wire shapes lifted from tests/test_crossref_source.py
// (the `_make_msg` dict and the search/items fixtures), committed as fixtures.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // The `{"message": {...}}` envelope api.crossref.org returns for one work.
    // Lifted from test_crossref_source.py::_make_msg.
    const WORK_BODY: &[u8] = br#"{"message":{
        "title":["Test Paper"],
        "author":[{"given":"Jane","family":"Doe"}],
        "published":{"date-parts":[[2023,6,15]]},
        "abstract":"<jats:p>Clean <b>text</b> here</jats:p>",
        "container-title":["Journal of Testing"],
        "URL":"https://doi.org/10.1000/xyz",
        "DOI":"10.1000/xyz"}}"#;

    // Search envelope with two items + one DOI-less item that must be skipped.
    // Lifted from TestSearchByTitle fixtures.
    const SEARCH_BODY: &[u8] = br#"{"message":{"items":[
        {"title":["Paper A"],"DOI":"10.1000/0","author":[],"published":{"date-parts":[[2023]]},"abstract":null,"container-title":[],"URL":null},
        {"title":["Paper B"],"DOI":"10.1000/1","author":[],"published":{"date-parts":[[2023]]},"abstract":null,"container-title":[],"URL":null},
        {"title":["Paper Without DOI"]}
    ]}}"#;

    fn work() -> Value {
        let v: Value = serde_json::from_slice(WORK_BODY).unwrap();
        v.get("message").unwrap().clone()
    }

    // ---- JATS / tag stripping ----
    #[test]
    fn strips_well_formed_tags_keeps_inner_text() {
        assert_eq!(
            strip_jats_tags("<jats:p>Clean <b>text</b> here</jats:p>"),
            "Clean text here"
        );
    }

    #[test]
    fn keeps_bare_lt_and_unclosed_tag_like_python_regex() {
        // `<>` has zero chars before `>`, `<unclosed` never closes: regex matches
        // neither, so both `<` survive.
        assert_eq!(strip_jats_tags("a<>b"), "a<>b");
        assert_eq!(strip_jats_tags("x < y"), "x < y");
        assert_eq!(strip_jats_tags("<unclosed"), "<unclosed");
    }

    // ---- parse_work mapping ----
    #[test]
    fn maps_core_fields() {
        let m = parse_work(&work(), "10.1000/xyz");
        assert_eq!(m.source, Some("crossref".into()));
        assert_eq!(m.source_id, "doi:10.1000/xyz");
        assert_eq!(m.doi, Some("10.1000/xyz".into()));
        assert_eq!(m.title, "Test Paper");
        assert_eq!(m.authors, vec!["Jane Doe".to_string()]);
        assert_eq!(m.version, 1);
        assert_eq!(m.category, Some("Journal of Testing".into()));
        assert_eq!(m.url, Some("https://doi.org/10.1000/xyz".into()));
        assert_eq!(m.published, NaiveDate::from_ymd_opt(2023, 6, 15).unwrap());
        assert!(!m.summary.contains('<'));
        assert_eq!(m.summary, "Clean text here");
    }

    #[test]
    fn author_only_family_and_skips_nameless() {
        let msg = serde_json::json!({
            "title": ["t"],
            "author": [{"family": "Einstein"}, {}, {"given": "Al", "family": "B"}],
        });
        let m = parse_work(&msg, "10.1/x");
        assert_eq!(m.authors, vec!["Einstein".to_string(), "Al B".to_string()]);
    }

    #[test]
    fn date_parts_year_only_and_year_month_default_to_1() {
        let yo = serde_json::json!({"title":["t"],"published":{"date-parts":[[2020]]}});
        assert_eq!(
            parse_work(&yo, "d").published,
            NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()
        );
        let ym = serde_json::json!({"title":["t"],"published":{"date-parts":[[2020,7]]}});
        assert_eq!(
            parse_work(&ym, "d").published,
            NaiveDate::from_ymd_opt(2020, 7, 1).unwrap()
        );
    }

    #[test]
    fn malformed_and_missing_date_fall_back_to_today() {
        let today = chrono::Utc::now().date_naive();
        let bad = serde_json::json!({"title":["t"],"published":{"date-parts":[["bad"]]}});
        assert_eq!(parse_work(&bad, "d").published, today);
        let missing = serde_json::json!({"title":["t"]});
        assert_eq!(parse_work(&missing, "d").published, today);
    }

    #[test]
    fn none_abstract_and_empty_container_title() {
        let msg = serde_json::json!({"title":["t"],"abstract":null,"container-title":[]});
        let m = parse_work(&msg, "10.1/x");
        assert_eq!(m.summary, "");
        assert_eq!(m.category, None);
    }

    #[test]
    fn url_falls_back_to_doi_when_absent() {
        let msg = serde_json::json!({"title":["t"],"URL":null});
        let m = parse_work(&msg, "10.5678/abc");
        assert_eq!(m.url, Some("https://doi.org/10.5678/abc".into()));
    }

    #[test]
    fn passed_doi_overrides_message_doi() {
        // message DOI is 10.1000/xyz; caller passes a different one (search path).
        let m = parse_work(&work(), "10.9999/test");
        assert_eq!(m.source_id, "doi:10.9999/test");
        assert_eq!(m.doi, Some("10.9999/test".into()));
    }

    // ---- response-body parsers (the None/skip gates the fetch wrappers rely on) ----
    #[test]
    fn doi_body_happy_and_no_title_returns_none() {
        let m = parse_doi_body(WORK_BODY, "10.1000/xyz").expect("title present");
        assert_eq!(m.source_id, "doi:10.1000/xyz");
        let no_title = br#"{"message":{"title":[]}}"#;
        assert!(parse_doi_body(no_title, "10.1000/xyz").is_none());
        let empty = br#"{"message":{}}"#;
        assert!(parse_doi_body(empty, "d").is_none());
        assert!(parse_doi_body(b"not json", "d").is_none());
    }

    #[test]
    fn search_body_keeps_titled_with_doi_skips_rest() {
        let results = parse_search_body(SEARCH_BODY);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Paper A");
        assert_eq!(results[0].doi, Some("10.1000/0".into()));
        assert_eq!(results[1].title, "Paper B");
        // empty body / no items -> empty.
        assert!(parse_search_body(br#"{"message":{"items":[]}}"#).is_empty());
        assert!(parse_search_body(b"garbage").is_empty());
    }
}
