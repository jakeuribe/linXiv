//! openalex — OpenAlex REST source (port of `sources/openalex_source.py`).
//!
//! Pure parsers (sync, on `serde_json::Value`) hold the load-bearing logic and
//! are fixture-tested below: inverted-index abstract reconstruction,
//! search-operator sanitization, polite-pool User-Agent, and Work→PaperMetadata
//! mapping. The async `search`/`fetch_by_id` wrappers reuse `sources::http`'s
//! guarded GET (host allowlist `api.openalex.org`) and are integration-tested
//! once the http unit fills its stubs. Plan §5.4.

use chrono::NaiveDate;
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::models::{
    date_min, normalize_orcid, openalex_source_id, strip_provider_prefix, PaperMetadata,
    OPENALEX_ID_PREFIX,
};
use crate::sources::http;

const BASE_URL: &str = "https://api.openalex.org";
const ALLOW: &[&str] = &["api.openalex.org"];

/// Fixed `select` field list — only the fields `work_to_metadata` reads.
const WORK_FIELDS: &str =
    "id,title,authorships,publication_date,doi,primary_topic,abstract_inverted_index";

/// OpenAlex reads `| ! * ?` in `search` as query operators (HTTP 400 on free
/// text); replace each with a space and collapse the resulting whitespace.
fn sanitize_search_query(query: &str) -> String {
    query
        .split(|c: char| c.is_whitespace() || matches!(c, '|' | '!' | '*' | '?'))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `^W\d+$` — bare OpenAlex Work ID.
fn is_work_id(s: &str) -> bool {
    s.len() > 1 && s.starts_with('W') && s[1..].chars().all(|c| c.is_ascii_digit())
}

/// Maps the UI sort key to the OpenAlex `sort` query value.
fn sort_param(sort: &str) -> Option<&'static str> {
    match sort {
        "relevance" => Some("relevance_score:desc"),
        "newest" => Some("publication_date:desc"),
        "oldest" => Some("publication_date:asc"),
        "citations" => Some("cited_by_count:desc"),
        _ => None,
    }
}

/// Reconstruct abstract text from OpenAlex's `abstract_inverted_index`
/// (`{word: [positions...]}`). Sorts by (position, word) and joins with spaces;
/// `None`/empty/non-object → `None`.
fn reconstruct_abstract(inverted: Option<&Value>) -> Option<String> {
    let obj = inverted?.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let mut pairs: Vec<(i64, &str)> = Vec::new();
    for (word, positions) in obj {
        if let Some(arr) = positions.as_array() {
            for p in arr {
                if let Some(pos) = p.as_i64() {
                    pairs.push((pos, word.as_str()));
                }
            }
        }
    }
    if pairs.is_empty() {
        return None;
    }
    pairs.sort();
    Some(pairs.iter().map(|(_, w)| *w).collect::<Vec<_>>().join(" "))
}

/// Convert an OpenAlex Work object to `PaperMetadata`. Errors (mapped to the
/// search skip-loop) only when the Work has no usable ID.
fn work_to_metadata(work: &Value) -> Result<PaperMetadata> {
    let openalex_id = work
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.rsplit('/').next())
        .unwrap_or("");
    if openalex_id.is_empty() {
        return Err(CoreError::OpenAlexInput(format!(
            "OpenAlex work has no valid ID: {work}"
        )));
    }

    // Paired so `author_orcids` stays index-aligned with `authors` after the
    // no-display-name filter drops entries.
    let author_pairs: Vec<(String, Option<String>)> = work
        .get("authorships")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let author = x.get("author")?;
            let name = author.get("display_name")?.as_str()?;
            (!name.is_empty()).then(|| {
                let orcid = author
                    .get("orcid")
                    .and_then(Value::as_str)
                    .and_then(normalize_orcid);
                (name.to_string(), orcid)
            })
        })
        .collect();
    let authors: Vec<String> = author_pairs.iter().map(|(n, _)| n.clone()).collect();
    let author_orcids =
        (!author_pairs.is_empty()).then(|| author_pairs.into_iter().map(|(_, o)| o).collect());

    let published = work
        .get("publication_date")
        .and_then(Value::as_str)
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or_else(date_min);

    // Category — the primary topic's subfield display name, if any.
    let category = work
        .get("primary_topic")
        .and_then(|pt| pt.get("subfield"))
        .and_then(|sf| sf.get("display_name"))
        .and_then(Value::as_str)
        .map(String::from);

    // URL — prefer the DOI landing page, fall back to the OpenAlex id URL.
    let doi = work
        .get("doi")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let url = doi
        .clone()
        .or_else(|| work.get("id").and_then(Value::as_str).map(String::from));

    let summary = reconstruct_abstract(work.get("abstract_inverted_index")).unwrap_or_default();

    Ok(PaperMetadata {
        source_id: openalex_source_id(openalex_id),
        version: 1,
        title: work
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        authors,
        published,
        updated: None,
        summary,
        category,
        categories: None,
        doi,
        journal_ref: None,
        comment: None,
        url,
        tags: None,
        source: Some("openalex".into()),
        author_orcids,
    })
}

/// Parse a `/works` search body, skipping malformed records (Python logs+skips).
fn parse_search_results(body: &Value) -> Vec<PaperMetadata> {
    body.get("results")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|w| work_to_metadata(w).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Search OpenAlex `/works`. `mailto` selects the polite pool (DI param, not env).
pub async fn search(
    query: &str,
    max_results: u32,
    sort: &str,
    mailto: &str,
) -> Result<Vec<PaperMetadata>> {
    search_at(BASE_URL, ALLOW, query, max_results, sort, mailto).await
}

/// `search` against an injected base URL + host allowlist (the seam wiremock
/// tests drive). Production passes `BASE_URL`/`ALLOW`.
async fn search_at(
    base_url: &str,
    allow: &[&str],
    query: &str,
    max_results: u32,
    sort: &str,
    mailto: &str,
) -> Result<Vec<PaperMetadata>> {
    let sort_value = sort_param(sort)
        .ok_or_else(|| CoreError::OpenAlexInput(format!("unknown sort '{sort}'")))?;
    let sanitized = sanitize_search_query(query);
    // An empty search returns OpenAlex's unfiltered list; skip the call.
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }
    // Polite-pool UA carried as a per-request header (Python sends it on every
    // request); CR/LF already stripped in `http::polite_user_agent`.
    let ua = http::polite_user_agent(mailto);
    let url = reqwest::Url::parse_with_params(
        &format!("{base_url}/works"),
        &[
            ("search", sanitized.as_str()),
            ("per_page", &max_results.to_string()),
            ("select", WORK_FIELDS),
            ("sort", sort_value),
        ],
    )
    .map_err(|e| CoreError::OpenAlexInput(format!("bad OpenAlex search URL: {e}")))?;

    let resp = http::get_guarded_with(url.as_str(), allow, &[("User-Agent", &ua)]).await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CoreError::OpenAlexHttp(format!(
            "OpenAlex search failed: HTTP {}",
            status.as_u16()
        )));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| CoreError::OpenAlexHttp(format!("OpenAlex search failed: {e}")))?;
    Ok(parse_search_results(&body))
}

/// Fetch a single Work by source_id (`openalex:Wnnn`, bare `Wnnn`, or a URL).
pub async fn fetch_by_id(source_id: &str, mailto: &str) -> Result<PaperMetadata> {
    fetch_by_id_at(BASE_URL, ALLOW, source_id, mailto).await
}

/// `fetch_by_id` against an injected base URL + host allowlist (test seam).
async fn fetch_by_id_at(
    base_url: &str,
    allow: &[&str],
    source_id: &str,
    mailto: &str,
) -> Result<PaperMetadata> {
    let mut bare = strip_provider_prefix(source_id, OPENALEX_ID_PREFIX).to_string();
    // Normalise any URL form (API or landing page) to a bare Work ID.
    if bare.starts_with("http://") || bare.starts_with("https://") {
        bare = bare.rsplit('/').next().unwrap_or("").to_string();
    }
    if bare.is_empty() {
        return Err(CoreError::OpenAlexInput(format!(
            "source_id '{source_id}' resolves to an empty work ID."
        )));
    }
    if !is_work_id(&bare) {
        return Err(CoreError::OpenAlexInput(format!(
            "Invalid OpenAlex work ID '{bare}': expected 'W' followed by digits."
        )));
    }
    let ua = http::polite_user_agent(mailto);
    let url = reqwest::Url::parse_with_params(
        &format!("{base_url}/works/{bare}"),
        &[("select", WORK_FIELDS)],
    )
    .map_err(|e| CoreError::OpenAlexInput(format!("bad OpenAlex work URL: {e}")))?;

    let resp = http::get_guarded_with(url.as_str(), allow, &[("User-Agent", &ua)]).await?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Err(CoreError::OpenAlexNotFound(format!(
            "Paper '{source_id}' not found on OpenAlex."
        )));
    }
    if !status.is_success() {
        return Err(CoreError::OpenAlexHttp(format!(
            "OpenAlex returned HTTP {} for '{source_id}'.",
            status.as_u16()
        )));
    }
    let work: Value = resp.json().await.map_err(|e| {
        CoreError::OpenAlexHttp(format!("OpenAlex fetch failed for '{source_id}': {e}"))
    })?;
    work_to_metadata(&work)
}

/// Look up a Work by DOI via `filter=doi:`, the closest OpenAlex offers to a
/// direct-by-DOI GET. Errors `OpenAlexNotFound` when the filter has no hits.
pub async fn fetch_by_doi(doi: &str, mailto: &str) -> Result<PaperMetadata> {
    fetch_by_doi_at(BASE_URL, ALLOW, doi, mailto).await
}

/// `fetch_by_doi` against an injected base URL + host allowlist (test seam).
async fn fetch_by_doi_at(
    base_url: &str,
    allow: &[&str],
    doi: &str,
    mailto: &str,
) -> Result<PaperMetadata> {
    let ua = http::polite_user_agent(mailto);
    let filter = format!("doi:{doi}");
    let url = reqwest::Url::parse_with_params(
        &format!("{base_url}/works"),
        &[
            ("filter", filter.as_str()),
            ("select", WORK_FIELDS),
            ("per_page", "1"),
        ],
    )
    .map_err(|e| CoreError::OpenAlexInput(format!("bad OpenAlex DOI filter URL: {e}")))?;

    let resp = http::get_guarded_with(url.as_str(), allow, &[("User-Agent", &ua)]).await?;
    if !resp.status().is_success() {
        return Err(CoreError::OpenAlexHttp(format!(
            "OpenAlex DOI lookup failed: HTTP {}",
            resp.status().as_u16()
        )));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| CoreError::OpenAlexHttp(format!("OpenAlex DOI lookup failed: {e}")))?;
    parse_search_results(&body)
        .into_iter()
        .next()
        .ok_or_else(|| {
            CoreError::OpenAlexNotFound(format!("No OpenAlex work found for DOI '{doi}'"))
        })
}

// ---------------------------------------------------------------------------
// Tests — fixtures lifted from tests/test_sources.py (the recorded Work shapes).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A representative /works search response (wire shape: {"results":[...]}).
    // Mirrors the OpenAlex Work objects recorded in tests/test_sources.py.
    const SEARCH_RESPONSE: &str = r#"{
      "meta": {"count": 2, "per_page": 10},
      "results": [
        {
          "id": "https://openalex.org/W3123456789",
          "title": "OpenAlex Paper",
          "authorships": [
            {"author": {"display_name": "Jane Doe"}},
            {"author": {}}
          ],
          "publication_date": "2023-06-01",
          "doi": "https://doi.org/10.1000/xyz",
          "primary_topic": {"subfield": {"display_name": "Machine Learning"}},
          "abstract_inverted_index": {"Hello": [0], "world": [1]}
        },
        {
          "id": "",
          "title": "Malformed — no id",
          "authorships": [],
          "publication_date": "2020-01-01",
          "doi": null,
          "primary_topic": null,
          "abstract_inverted_index": null
        }
      ]
    }"#;

    // ── reconstruct_abstract ──────────────────────────────────────────────
    #[test]
    fn reconstructs_simple_sentence() {
        let inv = json!({"The": [0], "quick": [1], "fox": [2]});
        assert_eq!(
            reconstruct_abstract(Some(&inv)).as_deref(),
            Some("The quick fox")
        );
    }

    #[test]
    fn handles_word_at_multiple_positions() {
        let inv = json!({"the": [0, 3], "cat": [1], "sat": [2]});
        assert_eq!(
            reconstruct_abstract(Some(&inv)).as_deref(),
            Some("the cat sat the")
        );
    }

    #[test]
    fn reconstruct_none_and_empty_yield_none() {
        assert_eq!(reconstruct_abstract(None), None);
        assert_eq!(reconstruct_abstract(Some(&json!({}))), None);
        assert_eq!(reconstruct_abstract(Some(&Value::Null)), None);
    }

    #[test]
    fn preserves_punctuation_as_words() {
        let inv = json!({"Hello": [0], "world.": [1]});
        assert_eq!(
            reconstruct_abstract(Some(&inv)).as_deref(),
            Some("Hello world.")
        );
    }

    // ── sanitize_search_query ─────────────────────────────────────────────
    #[test]
    fn sanitize_replaces_operators_and_collapses() {
        assert_eq!(sanitize_search_query("cats|dogs"), "cats dogs");
        assert_eq!(sanitize_search_query("*what is AI?"), "what is AI");
        assert_eq!(sanitize_search_query("a!!b"), "a b");
        assert_eq!(sanitize_search_query("|||"), "");
    }

    // The polite-pool UA builder is shared with CrossRef and unit-tested in
    // `http`; `search_200_parses_results_and_sends_polite_ua` below pins that
    // OpenAlex actually sends it.

    // ── is_work_id ────────────────────────────────────────────────────────
    #[test]
    fn work_id_validation() {
        assert!(is_work_id("W3123456789"));
        assert!(!is_work_id("W"));
        assert!(!is_work_id("3123"));
        assert!(!is_work_id("W31a23"));
        assert!(!is_work_id(""));
    }

    #[test]
    fn sort_param_known_and_unknown() {
        assert_eq!(sort_param("newest"), Some("publication_date:desc"));
        assert_eq!(sort_param("citations"), Some("cited_by_count:desc"));
        assert_eq!(sort_param("bogus"), None);
    }

    // ── work_to_metadata ──────────────────────────────────────────────────
    fn work(extra: Value) -> Value {
        let mut base = json!({
            "id": "https://openalex.org/W3123456789",
            "title": "OpenAlex Paper",
            "authorships": [{"author": {"display_name": "Jane Doe"}}],
            "publication_date": "2023-06-01",
            "doi": null,
            "primary_topic": null,
            "abstract_inverted_index": null
        });
        let obj = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap().clone() {
            obj.insert(k, v);
        }
        base
    }

    #[test]
    fn basic_conversion_and_id_from_url() {
        let m = work_to_metadata(&work(json!({}))).unwrap();
        assert_eq!(m.source_id, "openalex:W3123456789");
        assert_eq!(m.source.as_deref(), Some("openalex"));
        assert_eq!(m.title, "OpenAlex Paper");
        assert_eq!(m.version, 1);

        let m = work_to_metadata(&work(json!({"id": "https://openalex.org/W9876543210"}))).unwrap();
        assert_eq!(m.source_id, "openalex:W9876543210");
    }

    #[test]
    fn extracts_authors_skipping_missing_display_name() {
        let m = work_to_metadata(&work(json!({"authorships": [
            {"author": {"display_name": "Alice"}},
            {"author": {}},
            {"author": {"display_name": "Bob"}}
        ]})))
        .unwrap();
        assert_eq!(m.authors, vec!["Alice".to_string(), "Bob".to_string()]);
    }

    #[test]
    fn author_orcid_harvested_normalized_and_aligned_with_authors() {
        let m = work_to_metadata(&work(json!({"authorships": [
            {"author": {"display_name": "Alice", "orcid": "https://orcid.org/0000-0002-1825-0097"}},
            {"author": {}},
            {"author": {"display_name": "Bob"}}
        ]})))
        .unwrap();
        assert_eq!(
            m.author_orcids,
            Some(vec![Some("0000-0002-1825-0097".to_string()), None])
        );
    }

    #[test]
    fn no_authorships_means_no_orcids_list() {
        let m = work_to_metadata(&work(json!({"authorships": []}))).unwrap();
        assert_eq!(m.author_orcids, None);
    }

    #[test]
    fn category_from_primary_topic_subfield() {
        let m = work_to_metadata(&work(json!({
            "primary_topic": {"subfield": {"display_name": "Machine Learning"}}
        })))
        .unwrap();
        assert_eq!(m.category.as_deref(), Some("Machine Learning"));
        // None primary_topic -> no category.
        let m = work_to_metadata(&work(json!({}))).unwrap();
        assert_eq!(m.category, None);
    }

    #[test]
    fn date_parsing_and_fallbacks() {
        let m = work_to_metadata(&work(json!({"publication_date": "2021-09-15"}))).unwrap();
        assert_eq!(m.published, NaiveDate::from_ymd_opt(2021, 9, 15).unwrap());
        // Bad and empty dates both fall back to the date.min sentinel.
        let m = work_to_metadata(&work(json!({"publication_date": "not-a-date"}))).unwrap();
        assert_eq!(m.published, date_min());
        let m = work_to_metadata(&work(json!({"publication_date": ""}))).unwrap();
        assert_eq!(m.published, date_min());
    }

    #[test]
    fn missing_id_is_error() {
        let err = work_to_metadata(&work(json!({"id": ""}))).unwrap_err();
        assert!(matches!(err, CoreError::OpenAlexInput(_)));
        assert!(err.to_string().contains("no valid ID"));
    }

    #[test]
    fn abstract_and_doi_url() {
        let m = work_to_metadata(&work(json!({
            "abstract_inverted_index": {"Hello": [0], "world": [1]}
        })))
        .unwrap();
        assert_eq!(m.summary, "Hello world");

        let m = work_to_metadata(&work(json!({"doi": "https://doi.org/10.1000/xyz"}))).unwrap();
        assert_eq!(m.doi.as_deref(), Some("https://doi.org/10.1000/xyz"));
        assert_eq!(m.url.as_deref(), Some("https://doi.org/10.1000/xyz"));
    }

    #[test]
    fn no_doi_falls_back_to_id_url() {
        let m = work_to_metadata(&work(json!({}))).unwrap();
        assert_eq!(m.doi, None);
        assert_eq!(m.url.as_deref(), Some("https://openalex.org/W3123456789"));
    }

    // ── parse_search_results (skips malformed) ────────────────────────────
    #[test]
    fn search_response_parses_and_skips_malformed() {
        let body: Value = serde_json::from_str(SEARCH_RESPONSE).unwrap();
        let results = parse_search_results(&body);
        // Second result has an empty id -> skipped.
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.source_id, "openalex:W3123456789");
        assert_eq!(r.authors, vec!["Jane Doe".to_string()]);
        assert_eq!(r.category.as_deref(), Some("Machine Learning"));
        assert_eq!(r.summary, "Hello world");
        assert_eq!(r.url.as_deref(), Some("https://doi.org/10.1000/xyz"));
    }

    // ── async wrappers against a wiremock server (no live network) ─────────
    // ALLOW pointed at 127.0.0.1, like the http.rs guarded-GET tests.
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn search_200_parses_results_and_sends_polite_ua() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works"))
            // Polite-pool UA reached OpenAlex (the bug this fix closes).
            .and(wiremock::matchers::header(
                "User-Agent",
                "linXiv/1.0 (mailto:me@x.io)",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(SEARCH_RESPONSE))
            .mount(&server)
            .await;

        let out = search_at(
            &server.uri(),
            &["127.0.0.1"],
            "deep learning",
            10,
            "newest",
            "me@x.io",
        )
        .await
        .expect("200 search parses");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_id, "openalex:W3123456789");
    }

    #[tokio::test]
    async fn fetch_404_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works/W3123456789"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let err = fetch_by_id_at(&server.uri(), &["127.0.0.1"], "openalex:W3123456789", "")
            .await
            .expect_err("404 maps to OpenAlexNotFound");
        assert!(matches!(err, CoreError::OpenAlexNotFound(_)), "got {err}");
    }

    #[tokio::test]
    async fn search_503_is_http_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let err = search_at(&server.uri(), &["127.0.0.1"], "x", 5, "relevance", "")
            .await
            .expect_err("503 maps to OpenAlexHttp");
        assert!(matches!(err, CoreError::OpenAlexHttp(_)), "got {err}");
    }

    #[tokio::test]
    async fn fetch_by_doi_returns_first_match() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works"))
            .and(wiremock::matchers::query_param("filter", "doi:10.1000/xyz"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SEARCH_RESPONSE))
            .mount(&server)
            .await;

        let m = fetch_by_doi_at(&server.uri(), &["127.0.0.1"], "10.1000/xyz", "")
            .await
            .expect("filter hit parses");
        assert_eq!(m.source_id, "openalex:W3123456789");
    }

    #[tokio::test]
    async fn fetch_by_doi_no_match_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"results":[]}"#))
            .mount(&server)
            .await;

        let err = fetch_by_doi_at(&server.uri(), &["127.0.0.1"], "10.1000/none", "")
            .await
            .expect_err("empty results is not-found");
        assert!(matches!(err, CoreError::OpenAlexNotFound(_)), "got {err}");
    }
}
