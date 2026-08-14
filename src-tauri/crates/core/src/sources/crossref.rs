//! crossref — CrossRef REST API source (port of `sources/crossref_source.py`).
//!
//! The pure parser (`parse_work` + the JATS tag-stripper + date-parts handling)
//! is the load-bearing piece and is fixture-tested below. `doi_resolve` reuses
//! `parse_work`. The async fetch wrappers route through `sources::http` and are
//! wiremock-tested below; the `PaperSource` adapter over them is covered in
//! `tests/paper_source.rs`.
//!
//! Plan §5.4. No auth required; `api.crossref.org` only.

use chrono::NaiveDate;
use serde_json::Value;

use super::http;
use crate::error::{CoreError, Result};
use crate::models::{doi_source_id, normalize_orcid, PaperMetadata};

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
    let mut author_orcids = Vec::new();
    if let Some(arr) = msg.get("author").and_then(Value::as_array) {
        for a in arr {
            let given = a.get("given").and_then(Value::as_str).unwrap_or("");
            let family = a.get("family").and_then(Value::as_str).unwrap_or("");
            let name = format!("{given} {family}");
            let name = name.trim();
            if !name.is_empty() {
                authors.push(name.to_string());
                author_orcids.push(
                    a.get("ORCID")
                        .and_then(Value::as_str)
                        .and_then(normalize_orcid),
                );
            }
        }
    }
    let author_orcids = (!authors.is_empty()).then_some(author_orcids);

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
            doi_source_id(&paper_doi)
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
        author_orcids,
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
    fetch_by_doi_checked(doi).await.ok().flatten()
}

/// Like `fetch_by_doi`, but distinguishes "no work found" (`Ok(None)`) from a
/// transport/HTTP/malformed-body failure (`Err`).
pub async fn fetch_by_doi_checked(doi: &str) -> Result<Option<PaperMetadata>> {
    fetch_by_doi_checked_at(CROSSREF_BASE, ALLOW, doi).await
}

/// `fetch_by_doi_checked` against an injected base URL + host allowlist (the
/// test seam, matching `openalex`'s `_at` pattern).
async fn fetch_by_doi_checked_at(
    base: &str,
    allow: &[&str],
    doi: &str,
) -> Result<Option<PaperMetadata>> {
    let url = format!("{base}/{doi}");
    let resp = http::get_guarded(&url, allow).await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if resp.status() != reqwest::StatusCode::OK {
        return Err(CoreError::Upstream(format!(
            "CrossRef DOI lookup failed: HTTP {}",
            resp.status().as_u16()
        )));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| CoreError::Upstream(format!("CrossRef DOI lookup failed: {e}")))?;
    // A 200 that isn't even valid JSON is a failure, not "no work found" —
    // matches how OpenAlex's side treats an unparseable 200 body.
    if serde_json::from_slice::<Value>(&body).is_err() {
        return Err(CoreError::Upstream(
            "CrossRef DOI lookup failed: invalid response body".to_string(),
        ));
    }
    Ok(parse_doi_body(&body, doi))
}

/// `(sort, order)` query values for the public sort keys — CrossRef's `score` is
/// its relevance metric. Unknown keys are refused rather than silently dropped
/// (the `PaperSource` contract); `search_by_title` pins "relevance" itself.
fn sort_params(sort: &str) -> Result<(&'static str, &'static str)> {
    Ok(match sort {
        "relevance" => ("score", "desc"),
        "newest" => ("published", "desc"),
        "oldest" => ("published", "asc"),
        "citations" => ("is-referenced-by-count", "desc"),
        other => {
            return Err(CoreError::Validation(format!(
                "CrossRef: unknown sort '{other}'"
            )))
        }
    })
}

/// Search CrossRef by title, relevance-ordered. Empty vec on any error.
pub async fn search_by_title(title: &str, limit: u32) -> Vec<PaperMetadata> {
    let Ok(url) = search_url(title, limit, "relevance") else {
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

/// The `/works` title-search URL for one `(title, limit, sort)`.
fn search_url(title: &str, limit: u32, sort: &str) -> Result<reqwest::Url> {
    let (sort_by, order) = sort_params(sort)?;
    reqwest::Url::parse_with_params(
        CROSSREF_BASE,
        [
            ("query.title", title),
            ("rows", &limit.to_string()),
            ("sort", sort_by),
            ("order", order),
        ],
    )
    .map_err(|e| CoreError::Upstream(format!("CrossRef search failed: {e}")))
}

/// `search_by_title` that reports transport/HTTP failures instead of folding them
/// into an empty result, so a caller can tell "CrossRef is down" from "no matches".
/// Same relationship to `search_by_title` as `fetch_by_doi_checked` has to `fetch_by_doi`.
pub async fn search_by_title_checked(
    title: &str,
    limit: u32,
    sort: &str,
) -> Result<Vec<PaperMetadata>> {
    let url = search_url(title, limit, sort)?;
    let resp = http::get_guarded(url.as_str(), ALLOW).await?;
    if resp.status() != reqwest::StatusCode::OK {
        return Err(CoreError::Upstream(format!(
            "CrossRef search failed: HTTP {}",
            resp.status().as_u16()
        )));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| CoreError::Upstream(format!("CrossRef search failed: {e}")))?;
    Ok(parse_search_body(&body))
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
    fn author_orcid_harvested_normalized_and_aligned_with_authors() {
        let msg = serde_json::json!({
            "title": ["t"],
            "author": [
                {"given": "Jane", "family": "Doe", "ORCID": "http://orcid.org/0000-0002-1825-0097"},
                {"given": "No", "family": "Orcid"},
                {},
            ],
        });
        let m = parse_work(&msg, "10.1/x");
        assert_eq!(
            m.authors,
            vec!["Jane Doe".to_string(), "No Orcid".to_string()]
        );
        assert_eq!(
            m.author_orcids,
            Some(vec![Some("0000-0002-1825-0097".to_string()), None])
        );
    }

    #[test]
    fn no_authors_means_no_orcids_list() {
        let msg = serde_json::json!({"title": ["t"], "author": []});
        assert_eq!(parse_work(&msg, "10.1/x").author_orcids, None);
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

    // ---- sort (honoured, not ignored — the PaperSource contract) ----
    #[test]
    fn search_url_carries_sort_and_order() {
        let url = search_url("attention", 5, "newest").unwrap();
        let q: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(q.contains(&("sort".into(), "published".into())), "{q:?}");
        assert!(q.contains(&("order".into(), "desc".into())), "{q:?}");
        assert_eq!(sort_params("oldest").unwrap(), ("published", "asc"));
        assert_eq!(sort_params("relevance").unwrap(), ("score", "desc"));
    }

    #[test]
    fn unknown_sort_is_refused_not_ignored() {
        let err = search_url("attention", 5, "bogus").unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)), "got {err}");
    }

    // ---- fetch_by_doi_checked: 404-vs-failure distinction the backfill route relies on ----
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn checked_200_parses_ok_some() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/10.1000/xyz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(WORK_BODY))
            .mount(&server)
            .await;

        let m = fetch_by_doi_checked_at(&server.uri(), &["127.0.0.1"], "10.1000/xyz")
            .await
            .expect("200 is Ok")
            .expect("title present is Some");
        assert_eq!(m.source_id, "doi:10.1000/xyz");
    }

    #[tokio::test]
    async fn checked_404_is_ok_none_not_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/10.1000/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let m = fetch_by_doi_checked_at(&server.uri(), &["127.0.0.1"], "10.1000/missing")
            .await
            .expect("404 is Ok, not Err");
        assert!(m.is_none());
    }

    #[tokio::test]
    async fn checked_200_with_garbage_body_is_err_not_silently_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/10.1000/xyz"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let err = fetch_by_doi_checked_at(&server.uri(), &["127.0.0.1"], "10.1000/xyz")
            .await
            .expect_err(
                "malformed 200 body must be Err, matching OpenAlex's json-parse-error path",
            );
        assert!(matches!(err, CoreError::Upstream(_)), "got {err}");
    }

    #[tokio::test]
    async fn checked_503_is_err_not_silently_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/10.1000/xyz"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let err = fetch_by_doi_checked_at(&server.uri(), &["127.0.0.1"], "10.1000/xyz")
            .await
            .expect_err("503 must be Err, not Ok(None)");
        assert!(matches!(err, CoreError::Upstream(_)), "got {err}");
    }
}
