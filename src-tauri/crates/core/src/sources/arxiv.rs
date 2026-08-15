//! arxiv — port of `sources/arxiv_source.py`.
//!
//! The Python source delegates HTTP + Atom parsing to the `arxiv` PyPI client;
//! there is no Rust equivalent, so the Atom parse (the highest-risk piece) is
//! reimplemented here with quick-xml against export.arxiv.org's Atom feed and
//! the result is normalized into `models::PaperMetadata` exactly as
//! `_result_to_metadata` does. Plan §5.4.
//!
//! Field mapping (mirrors `_result_to_metadata`):
//!   <id>                       -> source_id/version via `parse_arxiv_id`
//!   <title>                    -> title (whitespace-collapsed, as the lib does)
//!   <author><name>             -> authors
//!   <published>/<updated>      -> published / updated (date only)
//!   <summary>                  -> summary
//!   <arxiv:primary_category>   -> category   (term attr)
//!   <category term=..>         -> categories
//!   <arxiv:doi> | link@doi     -> doi        (bare arxiv:doi preferred)
//!   <arxiv:journal_ref>        -> journal_ref
//!   <arxiv:comment>            -> comment
//!   <link title="pdf">         -> url        (== arxiv.Result.pdf_url)

use std::path::Path;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::error::{CoreError, Result};
use crate::models::{arxiv_source_id, strip_namespace, PaperMetadata};
use crate::service::version_monitor::MAX_VERSION_CHECK_BATCH;
use tracing::warn;

// ---------------------------------------------------------------------------
// _parse_arxiv_id
// ---------------------------------------------------------------------------

/// Split `"http://arxiv.org/abs/2204.12985v4"` -> `("arxiv:2204.12985", 4)`.
/// Also handles old-style ids: `"http://arxiv.org/abs/hep-th/9901001v1"` -> `("arxiv:hep-th/9901001", 1)`.
/// Port of `_parse_arxiv_id`'s `^(.+?)(?:v(\d+))?$` over the raw id (extracted by stripping URL):
/// a trailing `v<digits>` is the version (default 1), the rest is the bare id.
pub fn parse_arxiv_id(entry_id: &str) -> (String, i64) {
    // Strip URL prefix by finding the last /abs/ or /pdf/ and taking everything after.
    // If neither is present, fall back to the current rsplit behavior for bare ids.
    let raw = if let Some((_, after_abs)) = entry_id.rsplit_once("/abs/") {
        after_abs
    } else if let Some((_, after_pdf)) = entry_id.rsplit_once("/pdf/") {
        after_pdf
    } else {
        entry_id.rsplit('/').next().unwrap_or(entry_id)
    };

    if let Some(vpos) = raw.rfind('v') {
        let (base, vpart) = raw.split_at(vpos);
        let digits = &vpart[1..];
        if !base.is_empty() && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(v) = digits.parse::<i64>() {
                return (arxiv_source_id(base), v);
            }
        }
    }
    (arxiv_source_id(raw), 1)
}

// ---------------------------------------------------------------------------
// Atom parse
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Entry {
    entry_id: String,
    title: String,
    summary: String,
    published: String,
    updated: String,
    authors: Vec<String>,
    categories: Vec<String>,
    category: Option<String>,
    doi_bare: Option<String>,
    doi_link: Option<String>,
    journal_ref: Option<String>,
    comment: Option<String>,
    url: Option<String>,
}

/// Local part of a (possibly `arxiv:`-prefixed) qualified name.
pub(crate) fn local(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

pub(crate) fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn parse_date(s: &str) -> Result<chrono::NaiveDate> {
    let prefix = s.get(..10).unwrap_or(s);
    chrono::NaiveDate::parse_from_str(prefix, "%Y-%m-%d")
        .map_err(|e| CoreError::Upstream(format!("arXiv bad date {s:?}: {e}")))
}

fn finalize(b: Entry) -> Result<PaperMetadata> {
    let (source_id, version) = parse_arxiv_id(&b.entry_id);
    Ok(PaperMetadata {
        source_id,
        version,
        title: b.title.split_whitespace().collect::<Vec<_>>().join(" "),
        authors: b.authors,
        published: parse_date(&b.published)?,
        updated: if b.updated.trim().is_empty() {
            None
        } else {
            Some(parse_date(&b.updated)?)
        },
        summary: b.summary.trim().to_string(),
        category: b.category,
        categories: Some(b.categories),
        doi: b.doi_bare.or(b.doi_link),
        journal_ref: b.journal_ref,
        comment: b.comment,
        url: b.url,
        tags: None,
        source: Some("arxiv".to_string()),
        author_orcids: None,
    })
}

/// Parse an arXiv Atom feed into one `PaperMetadata` per `<entry>`.
/// Pure + sync — the highest-risk parser, fixture-tested below.
pub fn parse_atom(xml: &[u8]) -> Result<Vec<PaperMetadata>> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut cur: Option<Entry> = None;
    let mut in_author = false;
    let mut text = String::new();
    let mut last_err: Option<CoreError> = None;

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| CoreError::Upstream(format!("arXiv XML parse: {e}")))?;
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name();
                let n = name.as_ref();
                let l = local(n);
                if l == b"entry" {
                    cur = Some(Entry::default());
                } else if let Some(b) = cur.as_mut() {
                    match l {
                        // primary_category/category/link carry data in attributes (Start or Empty tags)
                        b"primary_category" => b.category = attr(&e, b"term"),
                        b"category" => {
                            if let Some(t) = attr(&e, b"term") {
                                b.categories.push(t);
                            }
                        }
                        b"link" => match attr(&e, b"title").as_deref() {
                            Some("pdf") => b.url = attr(&e, b"href"),
                            Some("doi") => b.doi_link = attr(&e, b"href"),
                            _ => {}
                        },
                        b"author" => in_author = true,
                        _ => {}
                    }
                }
                text.clear();
            }
            Event::Text(e) => {
                if cur.is_some() {
                    match e.decode() {
                        Ok(t) => text.push_str(&t),
                        Err(err) => {
                            warn!("arXiv XML text decode failed: {}", err);
                        }
                    }
                }
            }
            // quick-xml emits entities (`&amp;`, `&#38;`) as their own events.
            Event::GeneralRef(e) => {
                if cur.is_some() {
                    match e.resolve_char_ref() {
                        Ok(Some(c)) => text.push(c),
                        Ok(None) => match e.decode() {
                            Ok(name) => match quick_xml::escape::resolve_predefined_entity(&name) {
                                Some(s) => text.push_str(s),
                                None => warn!("Unknown XML entity: &{};", name),
                            },
                            Err(err) => warn!("arXiv XML entity decode failed: {}", err),
                        },
                        Err(err) => warn!("arXiv XML char ref resolution failed: {}", err),
                    }
                }
            }
            Event::End(e) => {
                let name = e.name();
                let n = name.as_ref();
                let l = local(n);
                if let Some(b) = cur.as_mut() {
                    if in_author && l == b"name" {
                        b.authors.push(text.trim().to_string());
                    } else if l == b"author" {
                        in_author = false;
                    } else {
                        match l {
                            b"id" => b.entry_id = text.trim().to_string(),
                            b"title" => b.title = text.clone(),
                            b"published" => b.published = text.trim().to_string(),
                            b"updated" => b.updated = text.trim().to_string(),
                            b"summary" => b.summary = text.clone(),
                            b"journal_ref" => b.journal_ref = Some(text.trim().to_string()),
                            b"comment" => b.comment = Some(text.trim().to_string()),
                            b"doi" => b.doi_bare = Some(text.trim().to_string()),
                            _ => {}
                        }
                    }
                }
                if l == b"entry" {
                    if let Some(b) = cur.take() {
                        let entry_id = b.entry_id.clone();
                        match finalize(b) {
                            Ok(metadata) => out.push(metadata),
                            Err(e) => {
                                warn!("Skipping malformed arXiv entry {entry_id:?}: {}", e);
                                last_err = Some(e);
                            }
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if out.is_empty() {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Network fetch (compiles against the http stub; integration-tested later)
// ---------------------------------------------------------------------------

const QUERY_URL: &str = "http://export.arxiv.org/api/query";

/// (sortBy, sortOrder) for the public sort keys — port of `_SORT_MAP`.
fn sort_params(sort: &str) -> Result<(&'static str, &'static str)> {
    Ok(match sort {
        "relevance" => ("relevance", "descending"),
        "newest" => ("submittedDate", "descending"),
        "oldest" => ("submittedDate", "ascending"),
        "lastUpdated" => ("lastUpdatedDate", "descending"),
        other => return Err(CoreError::Validation(format!("unknown sort {other:?}"))),
    })
}

async fn body(resp: reqwest::Response) -> Result<String> {
    resp.text()
        .await
        .map_err(|e| CoreError::Upstream(format!("arXiv read body: {e}")))
}

/// `ArxivSource.search` — query export.arxiv.org and parse the Atom feed.
pub async fn search(
    query: &str,
    max_results: u32,
    sort: &str,
    data_dir: &Path,
) -> Result<Vec<PaperMetadata>> {
    let (sort_by, sort_order) = sort_params(sort)?;
    let mut url = reqwest::Url::parse(QUERY_URL).expect("static URL");
    url.query_pairs_mut()
        .append_pair("search_query", query)
        .append_pair("start", "0")
        .append_pair("max_results", &max_results.to_string())
        .append_pair("sortBy", sort_by)
        .append_pair("sortOrder", sort_order);
    let resp = crate::sources::http::arxiv_get(url.as_str(), data_dir).await?;
    parse_atom(body(resp).await?.as_bytes())
}

/// `ArxivSource.fetch_by_id` — strip the `arxiv:` prefix, fetch by id_list,
/// raise `ArxivNotFound` on an empty feed.
pub async fn fetch_by_id(source_id: &str, data_dir: &Path) -> Result<PaperMetadata> {
    let bare = strip_namespace(source_id);
    if bare.is_empty() {
        return Err(CoreError::Validation(format!(
            "source_id '{source_id}' resolves to an empty arXiv ID."
        )));
    }
    let mut url = reqwest::Url::parse(QUERY_URL).expect("static URL");
    url.query_pairs_mut().append_pair("id_list", &bare);
    let resp = crate::sources::http::arxiv_get(url.as_str(), data_dir).await?;
    parse_atom(body(resp).await?.as_bytes())?
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::ArxivNotFound(format!("Paper '{source_id}' not found on arXiv.")))
}

/// Prepare the id_list parameter for a batch query: strip namespaces, filter
/// empty ids, join with commas. Returns None if all ids are empty after stripping.
/// Cap processing to 100 ids.
fn prepare_id_list(source_ids: &[String]) -> Option<(String, usize)> {
    let bare: Vec<String> = source_ids
        .iter()
        .take(MAX_VERSION_CHECK_BATCH as usize)
        .map(|s| strip_namespace(s))
        .filter(|s| !s.is_empty())
        .collect();
    if bare.is_empty() {
        None
    } else {
        let count = bare.len();
        Some((bare.join(","), count))
    }
}

/// Fetch metadata for many ids in ONE rate-limited request (arXiv `id_list` is
/// comma-separated). Ids that arXiv doesn't recognize are simply absent from the
/// returned feed — the caller matches results back by `source_id`.
pub async fn fetch_by_ids(source_ids: &[String], data_dir: &Path) -> Result<Vec<PaperMetadata>> {
    let (id_list, count) = match prepare_id_list(source_ids) {
        Some(pair) => pair,
        None => return Ok(Vec::new()),
    };
    let mut url = reqwest::Url::parse(QUERY_URL).expect("static URL");
    url.query_pairs_mut()
        .append_pair("id_list", &id_list)
        .append_pair("max_results", &count.to_string());
    let resp = crate::sources::http::arxiv_get(url.as_str(), data_dir).await?;
    parse_atom(body(resp).await?.as_bytes())
}

// ---------------------------------------------------------------------------
// Tests — parser against a representative recorded arXiv Atom feed.
// (The Python suite mocks `arxiv.Result`; the real wire format the `arxiv`
// client consumes is the Atom feed below, with every mapped field present.)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    const ATOM: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <title type="html">ArXiv Query</title>
  <id>http://arxiv.org/api/errors</id>
  <updated>2024-01-01T00:00:00-05:00</updated>
  <entry>
    <id>http://arxiv.org/abs/2204.12985v4</id>
    <updated>2023-08-14T15:00:00Z</updated>
    <published>2022-04-27T17:59:01Z</published>
    <title>Attention Is All You
      Need Again</title>
    <summary>  We propose a new architecture &amp; show it works.
Second line of the abstract.
</summary>
    <author><name>Alice Smith</name></author>
    <author><name>Bob Jones</name></author>
    <arxiv:doi xmlns:arxiv="http://arxiv.org/schemas/atom">10.1234/test.2022.12985</arxiv:doi>
    <link title="doi" href="http://dx.doi.org/10.1234/test.2022.12985" rel="related"/>
    <arxiv:comment xmlns:arxiv="http://arxiv.org/schemas/atom">15 pages, 3 figures</arxiv:comment>
    <arxiv:journal_ref xmlns:arxiv="http://arxiv.org/schemas/atom">J. Test 12 (2022) 1-15</arxiv:journal_ref>
    <link href="http://arxiv.org/abs/2204.12985v4" rel="alternate" type="text/html"/>
    <link title="pdf" href="http://arxiv.org/pdf/2204.12985v4" rel="related" type="application/pdf"/>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.AI" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    // A feed for a non-existent id: no <entry> elements.
    const ATOM_EMPTY: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>ArXiv Query</title>
  <id>http://arxiv.org/api/errors</id>
  <opensearch:totalResults xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">0</opensearch:totalResults>
</feed>"#;

    // A feed with two entries to verify non-cross-contamination.
    const ATOM_TWO: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <title type="html">ArXiv Query</title>
  <id>http://arxiv.org/api/errors</id>
  <updated>2024-01-01T00:00:00-05:00</updated>
  <entry>
    <id>http://arxiv.org/abs/2204.12985v1</id>
    <updated>2023-08-14T15:00:00Z</updated>
    <published>2022-04-27T17:59:01Z</published>
    <title>First Paper</title>
    <summary>First summary text.</summary>
    <author><name>Alice Smith</name></author>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2301.00123v2</id>
    <updated>2023-01-15T10:30:00Z</updated>
    <published>2023-01-01T12:00:00Z</published>
    <title>Second Paper</title>
    <summary>Second summary text.</summary>
    <author><name>Bob Jones</name></author>
    <author><name>Charlie Brown</name></author>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.AI" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    // A feed with one good entry and one malformed entry (missing published date).
    const ATOM_MIXED_GOOD_BAD: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <title>ArXiv Query</title>
  <id>http://arxiv.org/api/errors</id>
  <entry>
    <id>http://arxiv.org/abs/2204.12985v1</id>
    <published>2022-04-27T17:59:01Z</published>
    <title>Good Paper</title>
    <summary>Good summary.</summary>
    <author><name>Alice Smith</name></author>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2301.00123v1</id>
    <title>Bad Paper</title>
    <summary>Missing published date.</summary>
    <author><name>Bob Jones</name></author>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.AI" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    // A feed where all entries fail finalize (all missing published date).
    const ATOM_ALL_BAD: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <title>ArXiv Query</title>
  <id>http://arxiv.org/api/errors</id>
  <entry>
    <id>http://arxiv.org/abs/2204.12985v1</id>
    <title>Bad Paper 1</title>
    <summary>Missing published.</summary>
    <author><name>Alice</name></author>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2301.00123v1</id>
    <title>Bad Paper 2</title>
    <summary>Also missing published.</summary>
    <author><name>Bob</name></author>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.AI" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    #[test]
    fn parse_arxiv_id_matches_python_cases() {
        assert_eq!(
            parse_arxiv_id("http://arxiv.org/abs/2204.12985v4"),
            ("arxiv:2204.12985".into(), 4)
        );
        assert_eq!(
            parse_arxiv_id("https://arxiv.org/abs/2204.12985v2"),
            ("arxiv:2204.12985".into(), 2)
        );
        assert_eq!(
            parse_arxiv_id("2204.12985v3"),
            ("arxiv:2204.12985".into(), 3)
        );
        // no version -> default 1
        assert_eq!(parse_arxiv_id("2204.12985"), ("arxiv:2204.12985".into(), 1));
        // five+ digit id
        assert_eq!(
            parse_arxiv_id("http://arxiv.org/abs/2204.123456v1"),
            ("arxiv:2204.123456".into(), 1)
        );
        // old-style id with archive prefix
        assert_eq!(
            parse_arxiv_id("http://arxiv.org/abs/hep-th/9901001v1"),
            ("arxiv:hep-th/9901001".into(), 1)
        );
        // /pdf/ URL fallback
        assert_eq!(
            parse_arxiv_id("http://arxiv.org/pdf/2204.12985v4"),
            ("arxiv:2204.12985".into(), 4)
        );
    }

    #[test]
    fn parse_atom_maps_every_field() {
        let papers = parse_atom(ATOM).unwrap();
        assert_eq!(papers.len(), 1);
        let p = &papers[0];

        // id -> source_id + version
        assert_eq!(p.source_id, "arxiv:2204.12985");
        assert_eq!(p.version, 4);
        // title whitespace collapsed (the arxiv client does re.sub(r"\s+"," "))
        assert_eq!(p.title, "Attention Is All You Need Again");
        // authors in order
        assert_eq!(p.authors, vec!["Alice Smith", "Bob Jones"]);
        // dates are date-only
        assert_eq!(p.published, NaiveDate::from_ymd_opt(2022, 4, 27).unwrap());
        assert_eq!(
            p.updated,
            Some(NaiveDate::from_ymd_opt(2023, 8, 14).unwrap())
        );
        // summary: entities unescaped, surrounding ws trimmed, body kept
        assert!(p
            .summary
            .starts_with("We propose a new architecture & show it works."));
        assert!(p.summary.contains("Second line"));
        // arxiv: ext elements
        assert_eq!(p.category.as_deref(), Some("cs.LG"));
        assert_eq!(
            p.categories.as_deref(),
            Some(&["cs.LG".to_string(), "cs.AI".to_string()][..])
        );
        assert_eq!(p.journal_ref.as_deref(), Some("J. Test 12 (2022) 1-15"));
        assert_eq!(p.comment.as_deref(), Some("15 pages, 3 figures"));
        // bare arxiv:doi preferred over the doi.org link form
        assert_eq!(p.doi.as_deref(), Some("10.1234/test.2022.12985"));
        // url == pdf link href (arxiv.Result.pdf_url)
        assert_eq!(p.url.as_deref(), Some("http://arxiv.org/pdf/2204.12985v4"));
        // source tag
        assert_eq!(p.source.as_deref(), Some("arxiv"));
    }

    #[test]
    fn parse_atom_empty_feed_yields_no_entries() {
        // drives fetch_by_id's ArxivNotFound branch.
        assert!(parse_atom(ATOM_EMPTY).unwrap().is_empty());
    }

    #[test]
    fn parse_atom_multi_entry_no_cross_contamination() {
        let papers = parse_atom(ATOM_TWO).unwrap();
        assert_eq!(papers.len(), 2);

        // First entry
        let p1 = &papers[0];
        assert_eq!(p1.source_id, "arxiv:2204.12985");
        assert_eq!(p1.version, 1);
        assert_eq!(p1.title, "First Paper");
        assert_eq!(p1.authors, vec!["Alice Smith"]);
        assert_eq!(p1.summary, "First summary text.");
        assert_eq!(p1.category.as_deref(), Some("cs.LG"));

        // Second entry — verify no field leakage from first
        let p2 = &papers[1];
        assert_eq!(p2.source_id, "arxiv:2301.00123");
        assert_eq!(p2.version, 2);
        assert_eq!(p2.title, "Second Paper");
        assert_eq!(p2.authors, vec!["Bob Jones", "Charlie Brown"]);
        assert_eq!(p2.summary, "Second summary text.");
        assert_eq!(p2.category.as_deref(), Some("cs.AI"));
    }

    #[test]
    fn sort_params_rejects_unknown_sort() {
        assert!(sort_params("relevance").is_ok());
        assert!(sort_params("bogus").is_err());
    }

    #[test]
    fn prepare_id_list_strips_namespace() {
        let input = vec!["arxiv:2204.12985".to_string()];
        let (ids, count) = prepare_id_list(&input).unwrap();
        assert_eq!(ids, "2204.12985");
        assert_eq!(count, 1);
    }

    #[test]
    fn prepare_id_list_filters_empty() {
        let input = vec!["arxiv:2204.12985".to_string(), "".to_string()];
        let (ids, count) = prepare_id_list(&input).unwrap();
        assert_eq!(ids, "2204.12985");
        assert_eq!(count, 1);
    }

    #[test]
    fn prepare_id_list_joins_with_comma() {
        let input = vec![
            "arxiv:2204.12985".to_string(),
            "arxiv:2301.00123".to_string(),
        ];
        let (ids, count) = prepare_id_list(&input).unwrap();
        assert_eq!(ids, "2204.12985,2301.00123");
        assert_eq!(count, 2);
    }

    #[test]
    fn prepare_id_list_empty_input_returns_none() {
        let input: Vec<String> = vec![];
        assert!(prepare_id_list(&input).is_none());
    }

    #[test]
    fn prepare_id_list_all_empty_returns_none() {
        let input = vec!["".to_string(), "".to_string()];
        assert!(prepare_id_list(&input).is_none());
    }

    #[test]
    fn parse_atom_skips_malformed_entries_returns_good_ones() {
        let papers = parse_atom(ATOM_MIXED_GOOD_BAD).unwrap();
        assert_eq!(papers.len(), 1);
        let p = &papers[0];
        assert_eq!(p.source_id, "arxiv:2204.12985");
        assert_eq!(p.title, "Good Paper");
        assert_eq!(p.authors, vec!["Alice Smith"]);
    }

    #[test]
    fn parse_atom_all_entries_fail_returns_error() {
        let result = parse_atom(ATOM_ALL_BAD);
        assert!(result.is_err());
    }
}
