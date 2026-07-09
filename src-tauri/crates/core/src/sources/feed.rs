//! feed — generic RSS 2.0 / Atom parser for the user-configurable home feed.
//! One quick-xml pass handles both dialects (RSS `<item>` / Atom `<entry>`),
//! following the event-loop shape of `sources::arxiv::parse_atom`. Entries whose
//! link points at arxiv.org carry an extracted `arxiv_id` so the UI can deep-link
//! into the existing arXiv save flow.
//!
//! The fetch takes an arbitrary user URL, so it is guarded: http(s) schemes only
//! (re-checked on every redirect hop), a total timeout, and a streamed body cap.

use std::path::Path;
use std::time::Duration;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use reqwest::Url;
use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::sources::http;

/// Body ceiling — arXiv daily feeds run ~1 MB; anything bigger is not a feed.
const MAX_FEED_BYTES: usize = 2 * 1024 * 1024;
/// Total fetch budget (connect + redirects + transfer).
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Default, Serialize)]
pub struct FeedEntry {
    pub title: String,
    pub link: String,
    pub authors: Vec<String>,
    pub summary: String,
    pub published: String,
    pub arxiv_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Feed {
    pub title: String,
    pub entries: Vec<FeedEntry>,
}

/// Extract a bare arXiv id from an abs/pdf link on an arxiv.org host.
/// `https://arxiv.org/abs/2401.12345v2` → `2401.12345`;
/// old-style `http://arxiv.org/abs/math-ph/0309136` → `math-ph/0309136`.
pub fn arxiv_id_from_link(link: &str) -> Option<String> {
    let url = Url::parse(link).ok()?;
    let host = url.host_str()?;
    if host != "arxiv.org" && !host.ends_with(".arxiv.org") {
        return None;
    }
    let path = url.path();
    let id = path
        .strip_prefix("/abs/")
        .or_else(|| path.strip_prefix("/pdf/"))?;
    let id = id.strip_suffix(".pdf").unwrap_or(id).trim_end_matches('/');
    // Strip a trailing `v<digits>` version; the digit check keeps archive names
    // containing 'v' (e.g. solv-int/9509007) intact.
    let base = match id.rfind('v') {
        Some(i) if i + 1 < id.len() && id[i + 1..].bytes().all(|b| b.is_ascii_digit()) => &id[..i],
        _ => id,
    };
    if base.is_empty() {
        return None;
    }
    // Validate against arXiv's two real id shapes: new (YYYY.XXXXX) or old (archive/NNNNNNN).
    if base.contains('/') {
        // Old-style: archive/7digits
        let parts: Vec<&str> = base.split('/').collect();
        if parts.len() != 2 {
            return None;
        }
        let (archive, digits) = (parts[0], parts[1]);
        if digits.len() != 7 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if archive.is_empty() || !archive.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
            return None;
        }
    } else {
        // New-style: 4digits.4-5digits
        let parts: Vec<&str> = base.split('.').collect();
        if parts.len() != 2 {
            return None;
        }
        let (year, number) = (parts[0], parts[1]);
        if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if (number.len() < 4 || number.len() > 5) || !number.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some(base.to_string())
}

fn attr(e: &BytesStart, key: &[u8], _decoder: quick_xml::encoding::Decoder) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

/// Local part of a (possibly namespaced) qualified name, e.g. `dc:creator` → `creator`.
fn local(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn finalize(mut e: FeedEntry) -> FeedEntry {
    e.title = e.title.split_whitespace().collect::<Vec<_>>().join(" ");
    e.summary = e.summary.trim().to_string();
    // Reject non-http(s) links (guard against javascript:/data: URIs).
    if !e.link.is_empty() {
        let lower = e.link.to_ascii_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            e.link = String::new();
        }
    }
    e.arxiv_id = arxiv_id_from_link(&e.link);
    e
}

/// Resolve link from explicit <link> / Atom @href or fallback to <guid isPermaLink>.
fn resolve_link(explicit: Option<String>, guid_fallback: Option<String>) -> String {
    explicit
        .filter(|l| !l.is_empty())
        .or_else(|| guid_fallback.filter(|l| !l.is_empty()))
        .unwrap_or_default()
}

/// Parse RSS 2.0 or Atom bytes into a `Feed`. Pure + sync, fixture-tested below.
pub fn parse_feed(xml: &[u8]) -> Result<Feed> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut feed_title = String::new();
    let mut entries: Vec<FeedEntry> = Vec::new();
    let mut cur: Option<FeedEntry> = None;
    let mut in_author = false;
    let mut author_name_found = false;
    let mut text = String::new();
    let mut guid_is_permalink = true;
    let mut explicit_link: Option<String> = None;
    let mut guid_permalink: Option<String> = None;
    let mut last_err_pos: Option<u64> = None;

    loop {
        let ev = match reader.read_event_into(&mut buf) {
            Ok(event) => event,
            Err(_e) => {
                // Malformed fragment: drop the in-progress entry and keep parsing
                // (mirror arxiv::parse_atom's skip); bail if the reader stalls.
                let pos = reader.buffer_position();
                if last_err_pos == Some(pos) {
                    break;
                }
                last_err_pos = Some(pos);
                cur = None;
                in_author = false;
                text.clear();
                buf.clear();
                continue;
            }
        };
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name();
                let l = local(name.as_ref());
                if l == b"item" || l == b"entry" {
                    cur = Some(FeedEntry::default());
                    guid_is_permalink = true;
                    explicit_link = None;
                    guid_permalink = None;
                } else if cur.is_some() {
                    match l {
                        // Atom link carries its target in @href (prefer the
                        // alternate/plain link over enclosure/self rels).
                        b"link" => {
                            if let Some(href) = attr(&e, b"href", reader.decoder()) {
                                if matches!(
                                    attr(&e, b"rel", reader.decoder()).as_deref(),
                                    None | Some("alternate")
                                ) {
                                    explicit_link = Some(href);
                                }
                            }
                            text.clear();
                        }
                        b"guid" => {
                            guid_is_permalink = attr(&e, b"isPermaLink", reader.decoder())
                                .map(|v| v != "false")
                                .unwrap_or(true);
                            text.clear();
                        }
                        b"author" => {
                            in_author = true;
                            author_name_found = false;
                            text.clear();
                        }
                        // Only clear text buffer for leaf elements we actually parse on End.
                        b"title" | b"description" | b"summary" | b"content" | b"name"
                        | b"creator" | b"pubDate" | b"published" | b"updated" => {
                            text.clear();
                        }
                        _ => {}
                    }
                } else if l == b"title" {
                    // Top-level title: clear any accumulated text from preceding sibling elements
                    // (e.g., <id> before <title> in real-world Atom feeds).
                    text.clear();
                }
            }
            Event::Text(e) => {
                if let Ok(t) = e.decode() {
                    text.push_str(&t);
                }
                // Decode errors on individual text elements are skipped; only hard
                // read_event_into errors abort the entire parse.
            }
            Event::CData(e) => {
                text.push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            // quick-xml emits entities (`&amp;`, `&#38;`) as their own events.
            Event::GeneralRef(e) => {
                match e.resolve_char_ref() {
                    Ok(Some(c)) => text.push(c),
                    Ok(None) => {
                        // Named entity: check XML predefined, then common HTML entities.
                        if let Ok(name) = e.decode() {
                            if let Some(s) = quick_xml::escape::resolve_predefined_entity(&name) {
                                text.push_str(s);
                            } else {
                                // Common HTML entities.
                                let expanded = match name.as_ref() {
                                    "nbsp" => Some("\u{00A0}"),
                                    "mdash" => Some("\u{2014}"),
                                    "ndash" => Some("\u{2013}"),
                                    "hellip" => Some("\u{2026}"),
                                    "rsquo" => Some("\u{2019}"),
                                    "lsquo" => Some("\u{2018}"),
                                    "ldquo" => Some("\u{201C}"),
                                    "rdquo" => Some("\u{201D}"),
                                    _ => None,
                                };
                                if let Some(s) = expanded {
                                    text.push_str(s);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Invalid numeric reference: drop it and continue.
                    }
                }
            }
            Event::End(e) => {
                let name = e.name();
                let l = local(name.as_ref());
                if l == b"item" || l == b"entry" {
                    if let Some(mut b) = cur.take() {
                        b.link = resolve_link(explicit_link.take(), guid_permalink.take());
                        entries.push(finalize(b));
                    }
                } else if let Some(b) = cur.as_mut() {
                    let t = text.trim();
                    match l {
                        b"title" => b.title = t.to_string(),
                        // RSS `<link>` is element text; Atom's @href was taken above.
                        b"link" if !t.is_empty() => explicit_link = Some(t.to_string()),
                        // RSS `<guid isPermaLink>` fallback when <link> is absent.
                        b"guid" if guid_is_permalink && !t.is_empty() => guid_permalink = Some(t.to_string()),
                        // RSS description / Atom summary; Atom content as fallback.
                        b"description" | b"summary" => b.summary = t.to_string(),
                        b"content" if b.summary.is_empty() => b.summary = t.to_string(),
                        // Atom `<author><name>`.
                        b"name" if in_author => {
                            b.authors.push(t.to_string());
                            author_name_found = true;
                        }
                        b"author" => {
                            // Capture RSS 2.0 plain-text `<author>` if no `<name>` child was found.
                            if !author_name_found && !t.is_empty() {
                                b.authors.push(t.to_string());
                            }
                            in_author = false;
                        }
                        // RSS `<dc:creator>` holds a comma-separated author list.
                        b"creator" => {
                            b.authors.extend(
                                t.split(',')
                                    .map(|s| s.trim())
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_string),
                            );
                        }
                        b"pubDate" | b"published" => b.published = t.to_string(),
                        b"updated" if b.published.is_empty() => b.published = t.to_string(),
                        _ => {}
                    }
                } else if l == b"title" && feed_title.is_empty() {
                    // First title outside any item/entry: the channel/feed title.
                    feed_title = text.trim().to_string();
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(Feed {
        title: feed_title,
        entries,
    })
}

/// Reject anything but http(s) — this endpoint fetches arbitrary user URLs.
fn assert_scheme_http(url: &str) -> Result<()> {
    let parsed =
        Url::parse(url).map_err(|e| CoreError::BadRequest(format!("invalid url {url:?}: {e}")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        s => Err(CoreError::BadRequest(format!(
            "unsupported scheme {s:?} — only http(s) feed URLs are allowed"
        ))),
    }
}

/// Fetch and parse a feed URL under `FETCH_TIMEOUT`. `data_dir` carries the
/// shared `.arxiv_ratelimit` file so arXiv-hosted feeds coordinate with every
/// other arXiv-bound call (search, version check, downloads).
pub async fn fetch_feed(url: &str, data_dir: &Path) -> Result<Feed> {
    let body = tokio::time::timeout(FETCH_TIMEOUT, fetch_body(url, data_dir))
        .await
        .map_err(|_| {
            CoreError::Upstream(format!("feed fetch timed out after {FETCH_TIMEOUT:?}"))
        })??;
    parse_feed(&body)
}

/// GET via the shared redirect-follow helper (scheme guard re-checked on every
/// hop; arXiv-host hops honour the shared cool-down + spacing), then stream the
/// body under a cap (Content-Length may be absent or lying).
async fn fetch_body(url: &str, data_dir: &Path) -> Result<Vec<u8>> {
    let mut resp = http::get_checked(url, &[], assert_scheme_http, Some(data_dir)).await?;
    if !resp.status().is_success() {
        return Err(CoreError::Upstream(format!(
            "feed GET {:?} returned {}",
            resp.url().as_str(),
            resp.status()
        )));
    }
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_FEED_BYTES as u64)
    {
        return Err(CoreError::Upstream(format!(
            "feed larger than {MAX_FEED_BYTES} bytes"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| CoreError::Upstream(format!("feed read body: {e}")))?
    {
        if body.len() + chunk.len() > MAX_FEED_BYTES {
            return Err(CoreError::Upstream(format!(
                "feed larger than {MAX_FEED_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Trimmed real shape of `https://rss.arxiv.org/rss/cs.LG` (RSS 2.0 + dc).
    const ARXIV_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <channel>
    <title>cs.LG updates on arXiv.org</title>
    <link>http://rss.arxiv.org/rss/cs.LG</link>
    <description>cs.LG updates on the arXiv.org e-print archive.</description>
    <item>
      <title>Attention &amp; Memory in Deep Learning</title>
      <link>https://arxiv.org/abs/2401.12345</link>
      <description>arXiv:2401.12345v1 Announce Type: new
Abstract: We study attention.</description>
      <dc:creator>Ada Lovelace, Alan Turing</dc:creator>
      <pubDate>Mon, 15 Jan 2024 00:00:00 -0500</pubDate>
      <guid isPermaLink="false">oai:arXiv.org:2401.12345v1</guid>
    </item>
    <item>
      <title>An Old-Style Paper</title>
      <link>https://arxiv.org/abs/math-ph/0309136v2</link>
      <description><![CDATA[Abstract: CDATA body.]]></description>
      <dc:creator>Emmy Noether</dc:creator>
      <pubDate>Tue, 16 Jan 2024 00:00:00 -0500</pubDate>
    </item>
  </channel>
</rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Example Atom Feed</title>
  <entry>
    <title>Non-arXiv Post</title>
    <link rel="alternate" href="https://example.com/post/1"/>
    <summary>A blog post.</summary>
    <author><name>Grace Hopper</name></author>
    <published>2024-02-01T00:00:00Z</published>
  </entry>
</feed>"#;

    #[test]
    fn parses_arxiv_rss_items() {
        let feed = parse_feed(ARXIV_RSS.as_bytes()).unwrap();
        assert_eq!(feed.title, "cs.LG updates on arXiv.org");
        assert_eq!(feed.entries.len(), 2);

        let e = &feed.entries[0];
        assert_eq!(e.title, "Attention & Memory in Deep Learning");
        assert_eq!(e.link, "https://arxiv.org/abs/2401.12345");
        assert_eq!(e.authors, vec!["Ada Lovelace", "Alan Turing"]);
        assert!(e.summary.starts_with("arXiv:2401.12345v1"));
        assert_eq!(e.published, "Mon, 15 Jan 2024 00:00:00 -0500");
        assert_eq!(e.arxiv_id.as_deref(), Some("2401.12345"));

        let e = &feed.entries[1];
        assert_eq!(e.summary, "Abstract: CDATA body.");
        // Old-style id: version stripped, category prefix kept.
        assert_eq!(e.arxiv_id.as_deref(), Some("math-ph/0309136"));
    }

    #[test]
    fn parses_atom_entries() {
        let feed = parse_feed(ATOM.as_bytes()).unwrap();
        assert_eq!(feed.title, "Example Atom Feed");
        assert_eq!(feed.entries.len(), 1);
        let e = &feed.entries[0];
        assert_eq!(e.title, "Non-arXiv Post");
        assert_eq!(e.link, "https://example.com/post/1");
        assert_eq!(e.authors, vec!["Grace Hopper"]);
        assert_eq!(e.summary, "A blog post.");
        assert_eq!(e.published, "2024-02-01T00:00:00Z");
        assert_eq!(e.arxiv_id, None);
    }

    #[test]
    fn arxiv_id_extraction_covers_edge_shapes() {
        assert_eq!(
            arxiv_id_from_link("https://arxiv.org/abs/2401.12345v2").as_deref(),
            Some("2401.12345")
        );
        assert_eq!(
            arxiv_id_from_link("https://arxiv.org/pdf/2401.12345v2.pdf").as_deref(),
            Some("2401.12345")
        );
        // Archive name containing 'v' must not be truncated.
        assert_eq!(
            arxiv_id_from_link("https://arxiv.org/abs/solv-int/9509007").as_deref(),
            Some("solv-int/9509007")
        );
        assert_eq!(
            arxiv_id_from_link("https://export.arxiv.org/abs/2401.12345").as_deref(),
            Some("2401.12345")
        );
        assert_eq!(
            arxiv_id_from_link("https://example.com/abs/2401.12345"),
            None
        );
        assert_eq!(
            arxiv_id_from_link("https://arxiv.org/list/cs.LG/recent"),
            None
        );
        assert_eq!(arxiv_id_from_link("not a url"), None);
    }

    #[test]
    fn scheme_guard_rejects_non_http() {
        assert!(assert_scheme_http("https://rss.arxiv.org/rss/cs.LG").is_ok());
        assert!(assert_scheme_http("http://example.com/feed").is_ok());
        for bad in ["file:///etc/passwd", "ftp://x/y", "gopher://x", "not a url"] {
            let err = assert_scheme_http(bad).unwrap_err();
            assert_eq!(err.http_status(), 400, "{bad}");
        }
    }

    #[tokio::test]
    async fn fetch_follows_redirect_and_parses() {
        let server = MockServer::start().await;
        let final_url = format!("{}/feed.xml", server.uri());
        Mock::given(method("GET"))
            .and(path("/feed"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", final_url.as_str()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/feed.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(ARXIV_RSS))
            .mount(&server)
            .await;

        let feed = fetch_feed(&format!("{}/feed", server.uri()), &std::env::temp_dir())
            .await
            .unwrap();
        assert_eq!(feed.entries.len(), 2);
    }

    #[tokio::test]
    async fn fetch_rejects_oversized_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; MAX_FEED_BYTES + 1]))
            .mount(&server)
            .await;

        let err = fetch_feed(&format!("{}/big", server.uri()), &std::env::temp_dir())
            .await
            .unwrap_err();
        assert_eq!(err.http_status(), 502);
    }

    #[tokio::test]
    async fn fetch_rejects_redirect_loop() {
        let server = MockServer::start().await;
        let loop_url = format!("{}/loop", server.uri());
        Mock::given(method("GET"))
            .and(path("/loop"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", loop_url.as_str())
            )
            .mount(&server)
            .await;

        let err = fetch_feed(&format!("{}/loop", server.uri()), &std::env::temp_dir())
            .await
            .unwrap_err();
        assert_eq!(err.http_status(), 502);
    }

    #[test]
    fn parses_title_with_nested_markup() {
        // Title containing inline nested markup should accumulate text from before and after the nested tag.
        const RSS_WITH_MARKUP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Feed Title</title>
    <item>
      <title>Foo <b>Bar</b> Baz</title>
      <link>https://example.com/item</link>
      <description>Test item with markup in title.</description>
    </item>
  </channel>
</rss>"#;

        let feed = parse_feed(RSS_WITH_MARKUP.as_bytes()).unwrap();
        assert_eq!(feed.entries.len(), 1);
        // Nested markup should be ignored; only text content accumulates.
        assert_eq!(feed.entries[0].title, "Foo Bar Baz");
    }

    #[test]
    fn parses_feed_with_invalid_char_ref() {
        // Feed containing an invalid numeric character reference (e.g., &#0;) should parse successfully,
        // dropping just that reference instead of aborting the entire parse.
        const RSS_WITH_BAD_REF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Feed Title</title>
    <item>
      <title>Test Item</title>
      <link>https://example.com/item</link>
      <description>Before &#0; after</description>
    </item>
  </channel>
</rss>"#;

        let feed = parse_feed(RSS_WITH_BAD_REF.as_bytes()).unwrap();
        assert_eq!(feed.entries.len(), 1);
        // Invalid ref &#0; should be dropped; adjacent text glues without space.
        assert_eq!(feed.entries[0].summary, "Before  after");
    }

    #[test]
    fn feed_title_not_corrupted_by_preceding_sibling_elements() {
        // Real Atom feeds (e.g., GitHub's commits.atom) may have <id>, <link>, etc. before <title>.
        // The text buffer must be cleared when we encounter the top-level title element
        // to prevent accumulated text from preceding siblings from leaking into the feed title.
        const ATOM_WITH_ID_BEFORE_TITLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>urn:uuid:60a76c80-d399-11d9-b91C-0003939e0af6</id>
  <link href="https://example.org/"/>
  <title>My Feed</title>
  <entry>
    <title>Entry One</title>
    <link href="https://example.org/entry1"/>
    <summary>First entry.</summary>
  </entry>
</feed>"#;

        let feed = parse_feed(ATOM_WITH_ID_BEFORE_TITLE.as_bytes()).unwrap();
        // Feed title must be exactly "My Feed", not corrupted with the preceding id/link text.
        assert_eq!(feed.title, "My Feed");
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].title, "Entry One");
    }

    #[test]
    fn malformed_entry_is_skipped_and_the_rest_survive() {
        // A stray end tag inside the middle item is a hard quick-xml error; the
        // parser must skip that entry and still return its good siblings.
        const RSS_WITH_BAD_ITEM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Feed Title</title>
    <item>
      <title>Good One</title>
      <link>https://example.com/1</link>
    </item>
    <item>
      <title>Broken</title>
      <bad></b>
      <link>https://example.com/2</link>
    </item>
    <item>
      <title>Good Two</title>
      <link>https://example.com/3</link>
    </item>
  </channel>
</rss>"#;

        let feed = parse_feed(RSS_WITH_BAD_ITEM.as_bytes()).unwrap();
        let titles: Vec<&str> = feed.entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["Good One", "Good Two"]);
        assert_eq!(feed.entries[1].link, "https://example.com/3");
    }

    #[test]
    fn guid_before_link_prefers_explicit_link() {
        // Regression: when <guid isPermaLink> appears before <link>,
        // the explicit <link> should take precedence, not the guid.
        const RSS_GUID_BEFORE_LINK: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <item>
      <title>Item Title</title>
      <guid isPermaLink="true">https://example.com/guid-url</guid>
      <link>https://example.com/real-link</link>
      <description>Test item.</description>
    </item>
  </channel>
</rss>"#;

        let feed = parse_feed(RSS_GUID_BEFORE_LINK.as_bytes()).unwrap();
        assert_eq!(feed.entries.len(), 1);
        // The explicit <link> should win, not the <guid>.
        assert_eq!(feed.entries[0].link, "https://example.com/real-link");
    }
}
