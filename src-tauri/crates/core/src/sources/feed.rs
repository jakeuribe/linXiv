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

use quick_xml::events::{BytesEnd, BytesRef, BytesStart, Event};
use quick_xml::Reader;
use reqwest::Url;
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::error::{CoreError, Result};
use crate::sources::arxiv::{attr, local};
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
    /// arXiv version parsed from the link's trailing `v<N>` (absent -> `1`).
    /// `None` when `arxiv_id` is `None`.
    pub version: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Feed {
    pub title: String,
    pub entries: Vec<FeedEntry>,
}

/// Extract a bare arXiv id + version from an abs/pdf link on an arxiv.org host.
/// `https://arxiv.org/abs/2401.12345v2` → `("2401.12345", 2)`;
/// old-style `http://arxiv.org/abs/math-ph/0309136` → `("math-ph/0309136", 1)`
/// (no explicit `vN` suffix means version 1 -- a fresh submission's first appearance).
fn parse_arxiv_link(link: &str) -> Option<(String, i64)> {
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
    let (base, version) = match id.rfind('v') {
        Some(i) if i + 1 < id.len() && id[i + 1..].bytes().all(|b| b.is_ascii_digit()) => {
            (&id[..i], id[i + 1..].parse().unwrap_or(1))
        }
        _ => (id, 1),
    };
    if base.is_empty() {
        return None;
    }
    // Validate against arXiv's two real id shapes: new (YYYY.XXXXX) or old (archive/NNNNNNN).
    if let Some((archive, digits)) = base.split_once('/') {
        // Old-style: archive/7digits
        if digits.len() != 7 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if archive.is_empty() || !archive.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
            return None;
        }
    } else {
        // New-style: 4digits.4-5digits
        let (year, number) = base.split_once('.')?;
        if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if !(4..=5).contains(&number.len()) || !number.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some((base.to_string(), version))
}

/// Extract a bare arXiv id from an abs/pdf link on an arxiv.org host.
/// `https://arxiv.org/abs/2401.12345v2` → `2401.12345`;
/// old-style `http://arxiv.org/abs/math-ph/0309136` → `math-ph/0309136`.
pub fn arxiv_id_from_link(link: &str) -> Option<String> {
    parse_arxiv_link(link).map(|(id, _)| id)
}

/// arXiv's RSS `<description>` is prefixed with its own announce-type boilerplate
/// (`arXiv:2401.12345v1 Announce Type: new\nAbstract: ...`) that the Atom API used
/// by search/save doesn't emit. Strip it, keeping just the actual abstract.
fn strip_announce_prefix(summary: &str) -> &str {
    let Some(rest) = summary.strip_prefix("arXiv:") else {
        return summary;
    };
    let Some(idx) = rest.find("Abstract:") else {
        return summary;
    };
    if !rest[..idx].contains("Announce Type:") {
        return summary;
    }
    rest[idx + "Abstract:".len()..].trim_start()
}

/// The base letter following a LaTeX accent command at `at`: `{X}` (braced) or,
/// when `allow_bare` is set, a bare `X`. Returns the letter and how many chars
/// (starting at `at`) it consumed.
fn accented_base(chars: &[char], at: usize, allow_bare: bool) -> Option<(char, usize)> {
    if chars.get(at) == Some(&'{') {
        let close = chars[at + 1..].iter().position(|&c| c == '}')?;
        let mut inner = chars[at + 1..at + 1 + close].iter();
        let base = *inner.next()?;
        if inner.next().is_some() {
            return None; // more than one char inside the braces -- not a simple accent
        }
        return base.is_ascii_alphabetic().then_some((base, 2 + close));
    }
    if !allow_bare {
        return None;
    }
    let base = *chars.get(at)?;
    base.is_ascii_alphabetic().then_some((base, 1))
}

/// Decode the common LaTeX accent/ligature macros (`\'e` -> é, `\"o` -> ö, `\o` -> ø, ...)
/// that arXiv's RSS feed occasionally leaks raw into author names -- unlike the Atom API
/// used by search/save, which is clean UTF-8. Accent macros map to their Unicode combining
/// mark and fold onto the base letter via NFC normalization, so any base letter works
/// without a per-letter lookup table.
///
/// Only safe to run on plain-text fields with no real TeX in them (author names) --
/// NOT on titles/abstracts, which legitimately carry math macros for MathJax (`\cos`,
/// `\rho`, `\vec{v}`, ...). A LaTeX accent command is a *control word* when letter-named
/// (`c`,`v`,`u`,`r`,`H`,`k`) -- terminated by braces, a single swallowed space, or a
/// non-letter/EOF, so a bare adjacent letter with none of those belongs to a longer
/// macro name and is left untouched. It's a *control symbol* when punctuation-named
/// (`'`,`` ` ``,`^`,`"`,`~`,`=`,`.`) -- exactly one char, unambiguously followed by its
/// base with no separator needed.
/// ponytail: covers accent marks + the handful of single-letter ligatures seen in real
/// arXiv author names; multi-letter ligatures (`\ss`, `\ae`, `\oe` + capitals) and nested
/// macros are out of scope -- extend the tables below if one shows up.
/// The single-letter ligature a no-argument LaTeX control word collapses to
/// (`\o` -> `ø`), or `None` if `cmd` isn't one of the four ligature commands.
fn compute_ligature(cmd: char) -> Option<char> {
    match cmd {
        'o' => Some('ø'),
        'O' => Some('Ø'),
        'l' => Some('ł'),
        'L' => Some('Ł'),
        _ => None,
    }
}

fn decode_latex_accents(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            let cmd = chars[i + 1];
            let ligature = compute_ligature(cmd);
            // No-argument control word: word-boundary check so `\oint`-style macros
            // aren't mistaken for `\o` + "int"; a boundary space is the terminator
            // and is swallowed too, not printed (`S\o rensen` -> "Sørensen").
            if let Some(lig) = ligature {
                if !chars.get(i + 2).is_some_and(|c| c.is_ascii_alphabetic()) {
                    out.push(lig);
                    i += 2;
                    if chars.get(i) == Some(&' ') {
                        i += 1;
                    }
                    continue;
                }
            }
            let combining = match cmd {
                '\'' => Some('\u{0301}'), // acute
                '`' => Some('\u{0300}'),  // grave
                '^' => Some('\u{0302}'),  // circumflex
                '"' => Some('\u{0308}'),  // diaeresis
                '~' => Some('\u{0303}'),  // tilde
                'c' => Some('\u{0327}'),  // cedilla
                'v' => Some('\u{030C}'),  // caron
                'u' => Some('\u{0306}'),  // breve
                '=' => Some('\u{0304}'),  // macron
                '.' => Some('\u{0307}'),  // dot above
                'r' => Some('\u{030A}'),  // ring above
                'H' => Some('\u{030B}'),  // double acute
                'k' => Some('\u{0328}'),  // ogonek
                _ => None,
            };
            if let Some(mark) = combining {
                let is_word = cmd.is_ascii_alphabetic();
                // Control symbol: bare adjacent base is always unambiguous. Control
                // word: only braced or space-terminated bare forms are unambiguous --
                // a bare adjacent letter with neither belongs to a longer macro name
                // (`\cos`, `\rho`, `\vec`, `\kappa`, `\check{x}`) and is left alone.
                let (base_at, allow_bare, cmd_len) = if !is_word {
                    (i + 2, true, 2)
                } else if chars.get(i + 2) == Some(&' ') {
                    (i + 3, true, 3)
                } else {
                    (i + 2, false, 2)
                };
                if let Some((base, consumed)) = accented_base(&chars, base_at, allow_bare) {
                    out.push(base);
                    out.push(mark);
                    i += cmd_len + consumed;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out.nfc().collect()
}

fn finalize(mut e: FeedEntry) -> FeedEntry {
    e.title = e.title.split_whitespace().collect::<Vec<_>>().join(" ");
    e.summary = strip_announce_prefix(e.summary.trim()).to_string();
    e.authors = e.authors.iter().map(|a| decode_latex_accents(a)).collect();
    // Reject non-http(s) links (guard against javascript:/data: URIs).
    if !e.link.is_empty() {
        let lower = e.link.to_ascii_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            e.link = String::new();
        }
    }
    match parse_arxiv_link(&e.link) {
        Some((id, version)) => {
            e.arxiv_id = Some(id);
            e.version = Some(version);
        }
        None => {
            e.arxiv_id = None;
            e.version = None;
        }
    }
    e
}

/// Mutable state threaded through the feed parse loop — one entry in progress,
/// the accumulated text of the current leaf element, and per-entry link/guid/
/// author bookkeeping.
#[derive(Default)]
struct ParseState {
    feed_title: String,
    entries: Vec<FeedEntry>,
    cur: Option<FeedEntry>,
    in_author: bool,
    author_name_found: bool,
    text: String,
    guid_is_permalink: bool,
    explicit_link: Option<String>,
    guid_permalink: Option<String>,
}

impl ParseState {
    /// `Event::Start`/`Event::Empty` handling: opens a new entry on `<item>`/`<entry>`,
    /// otherwise updates the in-progress entry's link/guid/author state, or clears
    /// `text` ahead of the top-level `<title>` when no entry is open.
    fn handle_start_event(&mut self, e: &BytesStart<'_>) {
        let name = e.name();
        let l = local(name.as_ref());
        if l == b"item" || l == b"entry" {
            self.cur = Some(FeedEntry::default());
            self.guid_is_permalink = true;
            self.explicit_link = None;
            self.guid_permalink = None;
        } else if self.cur.is_some() {
            match l {
                // Atom link carries its target in @href (prefer the
                // alternate/plain link over enclosure/self rels).
                b"link" => {
                    if let Some(href) = attr(e, b"href") {
                        if matches!(attr(e, b"rel").as_deref(), None | Some("alternate")) {
                            self.explicit_link = Some(href);
                        }
                    }
                    self.text.clear();
                }
                b"guid" => {
                    self.guid_is_permalink = attr(e, b"isPermaLink")
                        .map(|v| v != "false")
                        .unwrap_or(true);
                    self.text.clear();
                }
                b"author" => {
                    self.in_author = true;
                    self.author_name_found = false;
                    self.text.clear();
                }
                // Only clear text buffer for leaf elements we actually parse on End.
                b"title" | b"description" | b"summary" | b"content" | b"name" | b"creator"
                | b"pubDate" | b"published" | b"updated" => {
                    self.text.clear();
                }
                _ => {}
            }
        } else if l == b"title" {
            // Top-level title: clear any accumulated text from preceding sibling elements
            // (e.g., <id> before <title> in real-world Atom feeds).
            self.text.clear();
        }
    }
}

/// `Event::GeneralRef` handling: quick-xml emits entities (`&amp;`, `&#38;`) as
/// their own events. Resolves a numeric char ref directly; a named entity checks
/// XML predefined, then common HTML entities.
fn handle_general_ref_event(e: &BytesRef<'_>, text: &mut String) {
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

impl ParseState {
    /// `Event::End` handling: closes and finalizes the in-progress entry on
    /// `</item>`/`</entry>`, otherwise assigns the just-closed leaf element's
    /// trimmed text into the entry (or the top-level feed title when none is open).
    fn handle_end_event(&mut self, e: &BytesEnd<'_>) {
        let name = e.name();
        let l = local(name.as_ref());
        if l == b"item" || l == b"entry" {
            if let Some(mut b) = self.cur.take() {
                // Explicit <link>/Atom @href wins; <guid isPermaLink> is the fallback.
                b.link = self
                    .explicit_link
                    .take()
                    .filter(|l| !l.is_empty())
                    .or_else(|| self.guid_permalink.take().filter(|l| !l.is_empty()))
                    .unwrap_or_default();
                self.entries.push(finalize(b));
            }
        } else if let Some(b) = self.cur.as_mut() {
            let t = self.text.trim();
            match l {
                b"title" => b.title = t.to_string(),
                // RSS `<link>` is element text; Atom's @href was taken above.
                b"link" if !t.is_empty() => self.explicit_link = Some(t.to_string()),
                // RSS `<guid isPermaLink>` fallback when <link> is absent.
                b"guid" if self.guid_is_permalink && !t.is_empty() => {
                    self.guid_permalink = Some(t.to_string())
                }
                // RSS description / Atom summary; Atom content as fallback.
                b"description" | b"summary" => b.summary = t.to_string(),
                b"content" if b.summary.is_empty() => b.summary = t.to_string(),
                // Atom `<author><name>`.
                b"name" if self.in_author => {
                    b.authors.push(t.to_string());
                    self.author_name_found = true;
                }
                b"author" => {
                    // Capture RSS 2.0 plain-text `<author>` if no `<name>` child was found.
                    if !self.author_name_found && !t.is_empty() {
                        b.authors.push(t.to_string());
                    }
                    self.in_author = false;
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
        } else if l == b"title" && self.feed_title.is_empty() {
            // First title outside any item/entry: the channel/feed title.
            self.feed_title = self.text.trim().to_string();
        }
    }
}

/// Parse RSS 2.0 or Atom bytes into a `Feed`. Pure + sync, fixture-tested below.
pub fn parse_feed(xml: &[u8]) -> Result<Feed> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut st = ParseState {
        guid_is_permalink: true,
        ..Default::default()
    };
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
                st.cur = None;
                st.in_author = false;
                st.text.clear();
                buf.clear();
                continue;
            }
        };
        match ev {
            Event::Start(e) | Event::Empty(e) => st.handle_start_event(&e),
            Event::Text(e) => {
                if let Ok(t) = e.decode() {
                    st.text.push_str(&t);
                }
                // Decode errors on individual text elements are skipped; only hard
                // read_event_into errors abort the entire parse.
            }
            Event::CData(e) => {
                st.text.push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            // quick-xml emits entities (`&amp;`, `&#38;`) as their own events.
            Event::GeneralRef(e) => {
                handle_general_ref_event(&e, &mut st.text);
            }
            Event::End(e) => st.handle_end_event(&e),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(Feed {
        title: st.feed_title,
        entries: st.entries,
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

    // fixtures live in testdata/feed/, whitespace inside is load-bearing --
    // never run a formatter over them.
    /// Trimmed real shape of `https://rss.arxiv.org/rss/cs.LG` (RSS 2.0 + dc).
    const ARXIV_RSS: &str = include_str!("testdata/feed/arxiv_rss.xml");

    const ATOM: &str = include_str!("testdata/feed/atom.xml");

    #[test]
    fn parses_arxiv_rss_items() {
        let feed = parse_feed(ARXIV_RSS.as_bytes()).unwrap();
        assert_eq!(feed.title, "cs.LG updates on arXiv.org");
        assert_eq!(feed.entries.len(), 2);

        let e = &feed.entries[0];
        assert_eq!(e.title, "Attention & Memory in Deep Learning");
        assert_eq!(e.link, "https://arxiv.org/abs/2401.12345");
        assert_eq!(e.authors, vec!["Ada Lovelace", "Alan Turing"]);
        // "arXiv:2401.12345v1 Announce Type: new" boilerplate prefix is stripped.
        assert_eq!(e.summary, "We study attention.");
        assert_eq!(e.published, "Mon, 15 Jan 2024 00:00:00 -0500");
        assert_eq!(e.arxiv_id.as_deref(), Some("2401.12345"));
        // Link has no explicit vN suffix -> first appearance, version 1.
        assert_eq!(e.version, Some(1));

        let e = &feed.entries[1];
        assert_eq!(e.summary, "Abstract: CDATA body.");
        // Old-style id: version stripped, category prefix kept.
        assert_eq!(e.arxiv_id.as_deref(), Some("math-ph/0309136"));
        assert_eq!(e.version, Some(2));
    }

    #[test]
    fn version_none_for_non_arxiv_entries() {
        let feed = parse_feed(ATOM.as_bytes()).unwrap();
        assert_eq!(feed.entries[0].version, None);
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
    fn decode_latex_accents_covers_common_macros() {
        assert_eq!(decode_latex_accents("R\\^omulo"), "Rômulo");
        assert_eq!(decode_latex_accents("M\\\"oller"), "Möller");
        assert_eq!(decode_latex_accents("Erd\\H{o}s"), "Erdős");
        assert_eq!(decode_latex_accents("Ren\\'e"), "René");
        // Space-terminated control word: the terminator space is swallowed, not printed.
        assert_eq!(decode_latex_accents("S\\o rensen"), "Sørensen");
        assert_eq!(decode_latex_accents("Fran\\c{c}ois"), "François");
        // Space-separated bare accent argument (no braces).
        assert_eq!(decode_latex_accents("Fran\\c cois"), "François");
        // Braced multi-char content isn't a simple accent -- left alone.
        assert_eq!(decode_latex_accents("\\'{ab}"), "\\'{ab}");
        // A non-letter base (braced or bare) isn't a simple accent either.
        assert_eq!(decode_latex_accents("\\'{1}"), "\\'{1}");
        assert_eq!(decode_latex_accents("\\'1"), "\\'1");
        // No backslash -> untouched (and the common case, so it takes the fast path).
        assert_eq!(decode_latex_accents("Ada Lovelace"), "Ada Lovelace");
    }

    #[test]
    fn decode_latex_accents_leaves_real_math_macros_alone() {
        // Letter-named accent commands (c, v, u, r, H, k) must not eat the first
        // letter of a longer macro name with no space/braces to disambiguate.
        assert_eq!(decode_latex_accents("$\\cos x$"), "$\\cos x$");
        assert_eq!(decode_latex_accents("$\\rho$"), "$\\rho$");
        assert_eq!(decode_latex_accents("$\\vec{v}$"), "$\\vec{v}$");
        assert_eq!(decode_latex_accents("$\\kappa$"), "$\\kappa$");
        assert_eq!(decode_latex_accents("$\\check{x}$"), "$\\check{x}$");
        assert_eq!(decode_latex_accents("$\\underline{x}$"), "$\\underline{x}$");
    }

    #[test]
    fn strip_announce_prefix_only_matches_real_boilerplate() {
        assert_eq!(
            strip_announce_prefix("arXiv:2401.12345v1 Announce Type: new\nAbstract: Body text."),
            "Body text."
        );
        assert_eq!(
            strip_announce_prefix("arXiv:2401.12345v1 Announce Type: replace-cross\nAbstract: X."),
            "X."
        );
        // No "Announce Type:" -> not our boilerplate, left untouched.
        assert_eq!(
            strip_announce_prefix("arXiv preprint, Abstract: foo"),
            "arXiv preprint, Abstract: foo"
        );
        // Doesn't start with "arXiv:" -> untouched.
        assert_eq!(
            strip_announce_prefix("Abstract: CDATA body."),
            "Abstract: CDATA body."
        );
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
            .respond_with(ResponseTemplate::new(302).insert_header("location", loop_url.as_str()))
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
        const RSS_WITH_MARKUP: &str = include_str!("testdata/feed/rss_with_markup.xml");

        let feed = parse_feed(RSS_WITH_MARKUP.as_bytes()).unwrap();
        assert_eq!(feed.entries.len(), 1);
        // Nested markup should be ignored; only text content accumulates.
        assert_eq!(feed.entries[0].title, "Foo Bar Baz");
    }

    #[test]
    fn parses_feed_with_invalid_char_ref() {
        // Feed containing an invalid numeric character reference (e.g., &#0;) should parse successfully,
        // dropping just that reference instead of aborting the entire parse.
        const RSS_WITH_BAD_REF: &str = include_str!("testdata/feed/rss_with_bad_ref.xml");

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
        const ATOM_WITH_ID_BEFORE_TITLE: &str =
            include_str!("testdata/feed/atom_with_id_before_title.xml");

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
        const RSS_WITH_BAD_ITEM: &str = include_str!("testdata/feed/rss_with_bad_item.xml");

        let feed = parse_feed(RSS_WITH_BAD_ITEM.as_bytes()).unwrap();
        let titles: Vec<&str> = feed.entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["Good One", "Good Two"]);
        assert_eq!(feed.entries[1].link, "https://example.com/3");
    }

    #[test]
    fn guid_before_link_prefers_explicit_link() {
        // Regression: when <guid isPermaLink> appears before <link>,
        // the explicit <link> should take precedence, not the guid.
        const RSS_GUID_BEFORE_LINK: &str = include_str!("testdata/feed/rss_guid_before_link.xml");

        let feed = parse_feed(RSS_GUID_BEFORE_LINK.as_bytes()).unwrap();
        assert_eq!(feed.entries.len(), 1);
        // The explicit <link> should win, not the <guid>.
        assert_eq!(feed.entries[0].link, "https://example.com/real-link");
    }
}
