//! doi_resolve — port of `sources/doi_resolve.py`.
//!
//! Resolve a DOI to `models::PaperMetadata` via three strategies, in order:
//!   1. arXiv-issued DOI (`10.48550/arXiv.<id>`) -> fetch the arXiv record
//!   2. Semantic Scholar  -> any DOI; uses the arXiv id when S2 exposes one
//!   3. CrossRef          -> last resort (reuses `crossref::parse_doi_body`)
//!
//! The load-bearing, fixture-tested pieces are the PURE parsers: the arXiv-DOI
//! matcher (`arxiv_doi_id`), the Semantic Scholar field mapper (`parse_s2`,
//! incl. its publicationDate/year/today date ladder) and `strip_doi_url`. The
//! async wrappers route fetches through `sources::http` + `sources::arxiv`.
//!
//! NON-NEGOTIABLE (ported exactly): an arXiv 429 hit while resolving an arXiv
//! DOI (strategy 1, or strategy 2's arXiv-id branch) is RE-RAISED as a
//! user-facing error, never swallowed into the fallback chain. Plan §5.4.

use chrono::{NaiveDate, Utc};
use reqwest::StatusCode;
use serde_json::Value;

use std::path::Path;

use super::{arxiv, crossref, http};
use crate::error::{CoreError, Result};
use crate::models::PaperMetadata;

/// Fields requested from the Semantic Scholar graph API (`_S2_FIELDS`).
const S2_FIELDS: &str = "title,authors,year,abstract,externalIds,venue,publicationDate,url";
const S2_BASE: &str = "https://api.semanticscholar.org/graph/v1/paper/DOI:";
/// Semantic Scholar is reached over exactly one host; the http guard enforces it.
const S2_HOSTS: &[&str] = &["api.semanticscholar.org"];
const RATELIMIT_MSG: &str = "arXiv rate limit reached. Please wait ~60 s and try again.";

// ---------------------------------------------------------------------------
// Pure helpers (sync, fixture-tested).
// ---------------------------------------------------------------------------

/// Strip a leading `http(s)://(dx.)doi.org/` and surrounding whitespace,
/// mirroring `re.sub(r"^https?://(dx\.)?doi\.org/", "", doi.strip())`.
pub fn strip_doi_url(doi: &str) -> String {
    let t = doi.trim();
    [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
    ]
    .iter()
    .find_map(|p| t.strip_prefix(p))
    .unwrap_or(t)
    .to_string()
}

/// An arXiv error is a rate-limit iff its message mentions `429`, matching
/// Python's `"429" in str(e)`. `http::arxiv_get` surfaces a 429 as exactly that.
fn is_ratelimited(e: &CoreError) -> bool {
    e.to_string().contains("429")
}

/// If `doi` contains an arXiv-issued DOI (`10.48550/arXiv.<id>`, case-insensitive)
/// return the bare arXiv id; else `None`. Hand-port of `_ARXIV_DOI_RE`'s
/// `(\d{4}\.\d{4,5}|[a-z\-]+/\d+)` id alternatives (new-style id, then old-style
/// `category/number`), with the same first-alternative-wins ordering.
fn arxiv_doi_id(doi: &str) -> Option<String> {
    let prefix = "10.48550/arxiv.";
    let pos = doi.to_ascii_lowercase().find(prefix)?;
    let rest = &doi[pos + prefix.len()..];
    let b = rest.as_bytes();

    // new-style: NNNN.NNNN or NNNN.NNNNN (\d{4}\.\d{4,5})
    if b.len() >= 9
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'.'
        && b[5..9].iter().all(u8::is_ascii_digit)
    {
        let end = if b.len() >= 10 && b[9].is_ascii_digit() {
            10
        } else {
            9
        };
        return Some(rest[..end].to_string());
    }

    // old-style: [a-z-]+/\d+  (regex is IGNORECASE, so accept any letter)
    let cat = b
        .iter()
        .take_while(|&&c| c.is_ascii_alphabetic() || c == b'-')
        .count();
    if cat == 0 || b.get(cat) != Some(&b'/') {
        return None;
    }
    let digits = b[cat + 1..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    Some(rest[..cat + 1 + digits].to_string())
}

/// Pick the published date from a Semantic Scholar record: a valid ISO
/// `publicationDate` wins, else `year` -> Jan 1, else today. Mirrors the Python
/// date ladder (incl. the invalid-`publicationDate` -> year fallback).
fn s2_published(data: &Value) -> NaiveDate {
    let today = Utc::now().date_naive();
    let year_jan1 = || {
        data.get("year")
            .and_then(Value::as_i64)
            .and_then(|y| NaiveDate::from_ymd_opt(y as i32, 1, 1))
    };
    match data.get("publicationDate").and_then(Value::as_str) {
        Some(raw) if !raw.is_empty() => NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .ok()
            .or_else(year_jan1)
            .unwrap_or(today),
        _ => year_jan1().unwrap_or(today),
    }
}

/// Build `PaperMetadata` from Semantic Scholar's own fields (the non-arXiv path).
/// `None` when the record has no `title` (the Python `"title" not in data` gate).
/// The arXiv-id branch is handled by `try_semantic_scholar`, not here.
fn parse_s2(data: &Value, doi: &str) -> Option<PaperMetadata> {
    let title = data.get("title").and_then(Value::as_str)?;

    let authors = data
        .get("authors")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let venue = data
        .get("venue")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    let url = data
        .get("url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let pid = data.get("paperId").and_then(Value::as_str).unwrap_or("");
            format!("https://www.semanticscholar.org/paper/{pid}")
        });

    Some(PaperMetadata {
        source_id: format!("doi:{doi}"),
        version: 1,
        title: title.to_string(),
        authors,
        published: s2_published(data),
        updated: None,
        summary: data
            .get("abstract")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        category: venue.map(str::to_string),
        categories: None,
        doi: Some(doi.to_string()),
        journal_ref: None,
        comment: None,
        url: Some(url),
        tags: None,
        source: Some("semanticscholar".to_string()),
    })
}

// ---------------------------------------------------------------------------
// Async strategies (against the sources::http + sources::arxiv seams).
// ---------------------------------------------------------------------------

/// Strategy 1: if `doi` is an arXiv-issued DOI, fetch the arXiv record.
/// `Ok(None)` for a non-arXiv DOI or any non-429 arXiv error; an arXiv 429
/// is re-raised as the user-facing rate-limit error.
async fn try_arxiv_doi(doi: &str, data_dir: &Path) -> Result<Option<PaperMetadata>> {
    let Some(id) = arxiv_doi_id(doi) else {
        return Ok(None);
    };
    match arxiv::fetch_by_id(&id, data_dir).await {
        Ok(m) => Ok(Some(m)),
        Err(e) if is_ratelimited(&e) => Err(CoreError::BadRequest(RATELIMIT_MSG.to_string())),
        Err(_) => Ok(None),
    }
}

/// Strategy 2: look the DOI up on Semantic Scholar. If S2 exposes an arXiv id,
/// fetch the full arXiv record (re-raising a 429); otherwise build from S2's own
/// fields. Any fetch/parse failure resolves to `Ok(None)` (Python swallows them).
async fn try_semantic_scholar(doi: &str, data_dir: &Path) -> Result<Option<PaperMetadata>> {
    let url = format!("{S2_BASE}{doi}?fields={S2_FIELDS}");
    let resp = match http::get_guarded(&url, S2_HOSTS).await {
        Ok(r) if r.status() == StatusCode::OK => r,
        _ => return Ok(None),
    };
    let Ok(data) = resp.json::<Value>().await else {
        return Ok(None);
    };
    if data.get("title").is_none() {
        return Ok(None);
    }

    let arxiv_id = data
        .get("externalIds")
        .and_then(|e| e.get("ArXiv"))
        .and_then(Value::as_str);
    if let Some(id) = arxiv_id {
        match arxiv::fetch_by_id(id, data_dir).await {
            Ok(m) => return Ok(Some(m)),
            Err(e) if is_ratelimited(&e) => {
                return Err(CoreError::BadRequest(RATELIMIT_MSG.to_string()))
            }
            // Non-429 arXiv error: fall through and build from S2's own fields.
            Err(_) => {}
        }
    }
    Ok(parse_s2(&data, doi))
}

/// Resolve a DOI to `PaperMetadata`, trying arXiv -> Semantic Scholar ->
/// CrossRef in order. `data_dir` (DI) feeds the arXiv rate-limit cool-down.
/// Errors are user-facing: empty input, a propagated arXiv 429, or "could not
/// resolve" when every strategy comes up empty.
pub async fn resolve_doi(doi: &str, data_dir: &Path) -> Result<PaperMetadata> {
    let doi = strip_doi_url(doi);
    if doi.is_empty() {
        return Err(CoreError::BadRequest("Please enter a DOI.".to_string()));
    }

    if let Some(m) = try_arxiv_doi(&doi, data_dir).await? {
        return Ok(m);
    }
    if let Some(m) = try_semantic_scholar(&doi, data_dir).await? {
        return Ok(m);
    }
    if let Some(m) = crossref::fetch_by_doi(&doi).await {
        return Ok(m);
    }

    Err(CoreError::BadRequest(
        "Could not resolve this DOI.\n\
         • Check the DOI is correct\n\
         • The paper may not be indexed by Semantic Scholar or CrossRef\n\
         • arXiv-hosted papers use DOIs starting with 10.48550/arXiv."
            .to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Tests — pure parsers against the recorded S2/CrossRef shapes lifted from
// tests/test_doi_resolve.py (the `s2_data`/`cr_data` dicts), plus the network-
// free orchestration guards (empty / strips-to-empty DOI). No live network.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- strip_doi_url ----
    #[test]
    fn strips_doi_url_prefixes_and_whitespace() {
        assert_eq!(strip_doi_url("https://doi.org/10.1000/xyz"), "10.1000/xyz");
        assert_eq!(strip_doi_url("http://doi.org/10.1000/xyz"), "10.1000/xyz");
        assert_eq!(
            strip_doi_url("https://dx.doi.org/10.1000/xyz"),
            "10.1000/xyz"
        );
        assert_eq!(strip_doi_url("10.1000/xyz"), "10.1000/xyz");
        assert_eq!(strip_doi_url("  10.1000/xyz  "), "10.1000/xyz");
    }

    // ---- is_ratelimited ----
    #[test]
    fn ratelimit_detects_429_only() {
        assert!(is_ratelimited(&CoreError::Upstream(
            "arXiv returned 429 — rate limited; retry in 60s".into()
        )));
        assert!(!is_ratelimited(&CoreError::Upstream(
            "HTTP 404 Not Found".into()
        )));
        assert!(!is_ratelimited(&CoreError::Upstream(
            "Connection reset".into()
        )));
    }

    // ---- arxiv_doi_id ----
    #[test]
    fn arxiv_doi_id_matches_extracts_and_rejects() {
        assert_eq!(
            arxiv_doi_id("10.48550/arXiv.2204.12985").as_deref(),
            Some("2204.12985")
        );
        // case-insensitive prefix, like the IGNORECASE regex.
        assert_eq!(
            arxiv_doi_id("10.48550/ARXIV.2204.12985").as_deref(),
            Some("2204.12985")
        );
        // old-style category/number id.
        assert_eq!(
            arxiv_doi_id("10.48550/arXiv.hep-th/9901001").as_deref(),
            Some("hep-th/9901001")
        );
        // a 4-digit minor part is accepted (\d{4,5}); a 5th digit is greedily taken.
        assert_eq!(
            arxiv_doi_id("10.48550/arXiv.2101.00001").as_deref(),
            Some("2101.00001")
        );
        // non-arXiv DOI and blank -> no match.
        assert!(arxiv_doi_id("10.1000/xyz123").is_none());
        assert!(arxiv_doi_id("").is_none());
    }

    // ---- parse_s2 ----
    #[test]
    fn s2_builds_metadata_without_arxiv_id() {
        // Lifted from test_doi_resolve.py::test_builds_metadata_from_s2_without_arxiv_id
        let data: Value = serde_json::json!({
            "title": "A Test Paper",
            "authors": [{"name": "Jane Doe"}, {"name": ""}],
            "publicationDate": "2023-05-01",
            "abstract": "An abstract.",
            "externalIds": {},
            "url": "https://www.semanticscholar.org/paper/abc",
        });
        let m = parse_s2(&data, "10.1000/xyz").expect("title present");
        assert_eq!(m.title, "A Test Paper");
        assert_eq!(m.authors, vec!["Jane Doe".to_string()]); // empty name dropped
        assert_eq!(m.source, Some("semanticscholar".into()));
        assert_eq!(m.summary, "An abstract.");
        assert_eq!(m.source_id, "doi:10.1000/xyz");
        assert_eq!(m.doi, Some("10.1000/xyz".into()));
        assert_eq!(
            m.url,
            Some("https://www.semanticscholar.org/paper/abc".into())
        );
        assert_eq!(m.published, NaiveDate::from_ymd_opt(2023, 5, 1).unwrap());
    }

    #[test]
    fn s2_year_fallback_when_no_publication_date() {
        let data: Value = serde_json::json!({
            "title": "Old Paper", "authors": [], "publicationDate": null,
            "year": 2019, "abstract": null, "externalIds": {},
        });
        let m = parse_s2(&data, "10.1000/xyz").unwrap();
        assert_eq!(m.published, NaiveDate::from_ymd_opt(2019, 1, 1).unwrap());
        assert_eq!(m.summary, ""); // null abstract -> ""
    }

    #[test]
    fn s2_invalid_publication_date_falls_back_to_year() {
        let data: Value = serde_json::json!({
            "title": "t", "year": 2018, "publicationDate": "not-a-date",
        });
        let m = parse_s2(&data, "d").unwrap();
        assert_eq!(m.published, NaiveDate::from_ymd_opt(2018, 1, 1).unwrap());
    }

    #[test]
    fn s2_today_when_no_date_or_year() {
        let data: Value = serde_json::json!({
            "title": "Undated Paper", "authors": [], "publicationDate": null,
            "year": null, "abstract": null, "externalIds": {},
        });
        let m = parse_s2(&data, "10.1000/xyz").unwrap();
        assert_eq!(m.published, Utc::now().date_naive());
        // No url/paperId -> the semanticscholar.org/paper/ fallback (empty id).
        assert_eq!(m.url, Some("https://www.semanticscholar.org/paper/".into()));
    }

    #[test]
    fn s2_no_title_returns_none() {
        assert!(parse_s2(&serde_json::json!({"authors": []}), "10.1000/xyz").is_none());
        assert!(parse_s2(&serde_json::json!({}), "d").is_none());
    }

    // ---- resolve_doi network-free guards ----
    #[tokio::test]
    async fn resolve_rejects_empty_doi() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_doi("", dir.path())
            .await
            .expect_err("empty DOI must error");
        assert_eq!(err.http_status(), 400);
        assert!(err.to_string().contains("Please enter a DOI"), "got {err}");
    }

    #[tokio::test]
    async fn resolve_rejects_doi_url_that_strips_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_doi("https://doi.org/", dir.path())
            .await
            .expect_err("a bare doi.org URL strips to empty");
        assert!(err.to_string().contains("Please enter a DOI"), "got {err}");
    }
}
