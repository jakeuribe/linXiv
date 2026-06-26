//! Group `bibtex` — cmd_bibtex_* in `linxiv_cli.py`.
//!
//! The Python command delegates to `formats.bibtex.BibTeXFormat` (pybtex). The
//! cli crate can't depend on a BibTeX parser (no biblatex/core import fn), so the
//! parser is ported locally here — the same pattern `project.rs` uses for the
//! `bibtex_export`/`obsidian_export` formatters. Save/link delegate to core.

use std::collections::HashMap;

use clap::Subcommand;
use serde_json::json;

use linxiv_core::models::PaperMetadata;
use linxiv_core::service::{paper as svc_paper, project as svc_project};

use crate::ctx::Ctx;
use crate::output::{fail, output};

#[derive(Subcommand)]
pub enum BibtexCmd {
    /// Import papers from a .bib file
    Import {
        /// Path to .bib file
        file: String,
        /// Link imported papers to a project
        #[arg(long = "project-id")]
        project_id: Option<i64>,
    },
}

pub async fn run(cmd: BibtexCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    match cmd {
        // cmd_bibtex_import: guard the project (if any) before mutating the
        // library, parse the .bib, save every entry, then link the saved ids.
        BibtexCmd::Import { file, project_id } => {
            // Pre-parse membership guard: a missing/deleted project fails before
            // anything is imported (Python ensure_membership_writable).
            if let Some(pid) = project_id {
                if let Err(e) = svc_project::ensure_membership_writable(&ctx.conn, pid) {
                    fail(e);
                }
            }

            // BibTeXFormat().import_file: read + parse. Python catches any parse
            // exception and prints a two-line stderr (`[bibtex-import] {e}` then
            // the error JSON). ponytail: pybtex's exact error wording can't be
            // reproduced; the structure (prefix line + JSON) is preserved.
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[bibtex-import] {e}");
                    fail(e);
                }
            };
            let entries = parse_bib(&text);

            let mut metas: Vec<PaperMetadata> = Vec::with_capacity(entries.len());
            for e in &entries {
                metas.push(entry_to_meta(e)?);
            }

            // save_papers_metadata: per-entry save, collecting (source_id, version).
            let mut results: Vec<(String, i64)> = Vec::with_capacity(metas.len());
            for m in &metas {
                results.push(svc_paper::save_paper_metadata(&mut ctx.conn, m, None)?);
            }

            // link_imported only when something was actually saved. A project that
            // vanished between the guard and here leaves the papers imported.
            if let Some(pid) = project_id {
                if !results.is_empty() {
                    let ids: Vec<String> = results.iter().map(|(s, _)| s.clone()).collect();
                    if let Err(e) = svc_project::link_imported(&ctx.conn, pid, &ids) {
                        fail(format!(
                            "{} paper(s) were imported but could not be linked: {}",
                            results.len(),
                            e
                        ));
                    }
                }
            }

            output(&json!({
                "imported": results.len(),
                "papers": results
                    .iter()
                    .map(|(s, v)| json!({ "source_id": s, "version": v }))
                    .collect::<Vec<_>>(),
            }));
        }
    }
    Ok(())
}

// ── BibTeX parsing (port of pybtex `_bib_to_metadata`) ──────────────────────

/// One parsed `@type{...}` entry: the citation key plus case-insensitive fields
/// (values whitespace-collapsed, outer `{}`/`"` stripped) and pre-formatted
/// author display names.
struct BibEntry {
    key: String,
    fields: HashMap<String, String>,
    authors: Vec<String>,
}

/// `_bib_to_metadata`: source_id = doi or key, version 1, year→Jan-1 (fallback
/// 1900-01-01), `doi`/`journal`/`booktitle`/`url` treated as None when empty
/// (Python `or None`); `title` defaults to the key only when absent.
fn entry_to_meta(e: &BibEntry) -> anyhow::Result<PaperMetadata> {
    let nonempty = |k: &str| e.fields.get(k).filter(|s| !s.is_empty()).cloned();

    let doi = nonempty("doi");
    let title = e.fields.get("title").cloned().unwrap_or_else(|| e.key.clone());
    let summary = e.fields.get("abstract").cloned().unwrap_or_default();
    let journal_ref = nonempty("journal").or_else(|| nonempty("booktitle"));
    let url = nonempty("url");
    let source_id = doi.clone().unwrap_or_else(|| e.key.clone());

    // _parse_year: date(int(year), 1, 1) or the 1900-01-01 fallback. date() only
    // accepts years 1..=9999, so anything else falls back like Python's ValueError.
    let published = e
        .fields
        .get("year")
        .and_then(|y| y.trim().parse::<i32>().ok())
        .filter(|y| (1..=9999).contains(y))
        .map(|y| format!("{y:04}-01-01"))
        .unwrap_or_else(|| "1900-01-01".to_string());

    // Build via serde so `published` parses into a NaiveDate without naming chrono
    // (not a cli dep). The date string is always valid here, so this won't fail.
    let meta = serde_json::from_value(json!({
        "source_id": source_id,
        "version": 1,
        "title": title,
        "authors": e.authors,
        "published": published,
        "summary": summary,
        "doi": doi,
        "journal_ref": journal_ref,
        "url": url,
        "source": "bibtex",
    }))?;
    Ok(meta)
}

/// Scan a BibTeX document into entries. `@comment`/`@preamble`/`@string` are
/// skipped (string macros are not expanded — ponytail: rare in import files).
fn parse_bib(text: &str) -> Vec<BibEntry> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut entries = Vec::new();

    while i < n {
        if chars[i] != '@' {
            i += 1;
            continue;
        }
        i += 1;
        let mut typ = String::new();
        while i < n && chars[i].is_alphanumeric() {
            typ.push(chars[i].to_ascii_lowercase());
            i += 1;
        }
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        let close = match chars[i] {
            '{' => '}',
            '(' => ')',
            _ => continue,
        };
        i += 1;

        // Entry body ends at `close` while brace depth is 0 (bibtex requires
        // braces balanced even inside the body).
        let start = i;
        let mut depth = 0;
        while i < n {
            let c = chars[i];
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                if depth > 0 {
                    depth -= 1;
                } else if close == '}' {
                    break;
                }
            } else if c == close && depth == 0 {
                break;
            }
            i += 1;
        }
        let body = &chars[start..i];
        if i < n {
            i += 1; // consume the closing delimiter
        }

        if matches!(typ.as_str(), "comment" | "preamble" | "string") {
            continue;
        }
        if let Some(e) = parse_entry_body(body) {
            entries.push(e);
        }
    }
    entries
}

/// Parse the inside of `@type{ ... }`: leading citation key, then `name = value`
/// pairs (case-insensitive field names, whitespace-collapsed values).
fn parse_entry_body(body: &[char]) -> Option<BibEntry> {
    let n = body.len();
    let mut i = 0;
    while i < n && body[i].is_whitespace() {
        i += 1;
    }
    let mut key = String::new();
    while i < n && body[i] != ',' {
        key.push(body[i]);
        i += 1;
    }
    let key = key.trim().to_string();
    if i < n && body[i] == ',' {
        i += 1;
    }

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut authors: Vec<String> = Vec::new();
    loop {
        while i < n && (body[i].is_whitespace() || body[i] == ',') {
            i += 1;
        }
        if i >= n {
            break;
        }
        let mut name = String::new();
        while i < n && body[i] != '=' && !body[i].is_whitespace() {
            name.push(body[i]);
            i += 1;
        }
        while i < n && body[i].is_whitespace() {
            i += 1;
        }
        if i >= n || body[i] != '=' {
            break; // malformed tail
        }
        i += 1;
        while i < n && body[i].is_whitespace() {
            i += 1;
        }
        let value = read_value(body, &mut i);
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if name == "author" {
            authors = parse_authors(&value);
        }
        fields.insert(name, collapse_ws(&value));
    }

    if key.is_empty() && fields.is_empty() && authors.is_empty() {
        return None;
    }
    Some(BibEntry { key, fields, authors })
}

/// Read a field value at `*i`: `{...}` (outer braces stripped, inner kept),
/// `"..."` (depth-0 quote terminates), or a bare token up to the next comma.
fn read_value(body: &[char], i: &mut usize) -> String {
    let n = body.len();
    if *i >= n {
        return String::new();
    }
    match body[*i] {
        '{' => {
            *i += 1;
            let mut depth = 1;
            let mut s = String::new();
            while *i < n && depth > 0 {
                let c = body[*i];
                if c == '{' {
                    depth += 1;
                    s.push(c);
                } else if c == '}' {
                    depth -= 1;
                    if depth > 0 {
                        s.push(c);
                    }
                } else {
                    s.push(c);
                }
                *i += 1;
            }
            s
        }
        '"' => {
            *i += 1;
            let mut depth = 0;
            let mut s = String::new();
            while *i < n {
                let c = body[*i];
                if c == '{' {
                    depth += 1;
                    s.push(c);
                } else if c == '}' {
                    if depth > 0 {
                        depth -= 1;
                    }
                    s.push(c);
                } else if c == '"' && depth == 0 {
                    *i += 1;
                    break;
                } else {
                    s.push(c);
                }
                *i += 1;
            }
            s
        }
        _ => {
            let mut depth = 0;
            let mut s = String::new();
            while *i < n {
                let c = body[*i];
                if c == ',' && depth == 0 {
                    break;
                }
                if c == '{' {
                    depth += 1;
                } else if c == '}' && depth > 0 {
                    depth -= 1;
                }
                s.push(c);
                *i += 1;
            }
            s.trim().to_string()
        }
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── BibTeX author names (port of pybtex `Person.__str__`) ───────────────────

/// Split an author field on ` and ` (depth-0) and render each person as pybtex's
/// `von Last, Jr, First`. Empty groups (leading/trailing `and`) are dropped.
fn parse_authors(value: &str) -> Vec<String> {
    let mut persons: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for w in tokenize_words(value) {
        if w.eq_ignore_ascii_case("and") {
            persons.push(cur.join(" "));
            cur = Vec::new();
        } else {
            cur.push(w);
        }
    }
    persons.push(cur.join(" "));
    persons
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .map(|p| format_person(&p))
        .collect()
}

/// pybtex `Person.__str__`: `", ".join` of the non-empty parts
/// `von_last`, `jr`, `first`. Names are split on depth-0 commas into 1/2/3+
/// fields ("First von Last" / "von Last, First" / "von Last, Jr, First").
fn format_person(name: &str) -> String {
    let parts: Vec<Vec<String>> = comma_split(name)
        .into_iter()
        .map(|p| tokenize_words(&p))
        .collect();

    let (von, last, jr, first): (Vec<String>, Vec<String>, Vec<String>, Vec<String>) =
        match parts.len() {
            0 => return String::new(),
            1 => {
                let (f, v, l) = split_first_von_last(&parts[0]);
                (v, l, Vec::new(), f)
            }
            2 => {
                let (v, l) = split_von_last(&parts[0]);
                (v, l, Vec::new(), parts[1].clone())
            }
            _ => {
                let (v, l) = split_von_last(&parts[0]);
                (v, l, parts[1].clone(), parts[2].clone())
            }
        };

    let von_last = von.iter().chain(last.iter()).cloned().collect::<Vec<_>>().join(" ");
    let jr = jr.join(" ");
    let first = first.join(" ");
    [von_last, jr, first]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// "First von Last": von = the words from the first lowercase-leading word to the
/// last (excluding the final word, which is always Last). Returns (first,von,last).
fn split_first_von_last(words: &[String]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let n = words.len();
    if n == 0 {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    if n == 1 {
        return (Vec::new(), Vec::new(), words.to_vec());
    }
    let lowers: Vec<usize> = (0..n - 1).filter(|&k| starts_lower(&words[k])).collect();
    match (lowers.first(), lowers.last()) {
        (Some(&f), Some(&l)) => (
            words[..f].to_vec(),
            words[f..=l].to_vec(),
            words[l + 1..].to_vec(),
        ),
        _ => (words[..n - 1].to_vec(), Vec::new(), words[n - 1..].to_vec()),
    }
}

/// "von Last": leading lowercase words are von, the rest is Last (Last keeps at
/// least one word). Returns (von, last).
fn split_von_last(words: &[String]) -> (Vec<String>, Vec<String>) {
    let n = words.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    let mut k = 0;
    while k < n - 1 && starts_lower(&words[k]) {
        k += 1;
    }
    (words[..k].to_vec(), words[k..].to_vec())
}

/// Case of a word's first depth-0 letter (a `{...}`-led word counts as not-lower,
/// matching pybtex treating brace groups as caseless/last-name material).
fn starts_lower(word: &str) -> bool {
    let mut depth = 0;
    for c in word.chars() {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            if depth > 0 {
                depth -= 1;
            }
        } else if depth == 0 && c.is_alphabetic() {
            return c.is_lowercase();
        }
    }
    false
}

/// Whitespace-delimited words at brace depth 0; whitespace inside `{...}` is kept.
fn tokenize_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    for c in s.chars() {
        if c == '{' {
            depth += 1;
            cur.push(c);
        } else if c == '}' {
            if depth > 0 {
                depth -= 1;
            }
            cur.push(c);
        } else if c.is_whitespace() && depth == 0 {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Split on commas at brace depth 0, trimming each part.
fn comma_split(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    for c in s.chars() {
        if c == '{' {
            depth += 1;
            cur.push(c);
        } else if c == '}' {
            if depth > 0 {
                depth -= 1;
            }
            cur.push(c);
        } else if c == ',' && depth == 0 {
            parts.push(std::mem::take(&mut cur).trim().to_string());
        } else {
            cur.push(c);
        }
    }
    parts.push(cur.trim().to_string());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    // pybtex parity: str(Person) for the four canonical name shapes.
    #[test]
    fn person_names_match_pybtex() {
        assert_eq!(format_person("Knuth, Donald E."), "Knuth, Donald E.");
        assert_eq!(format_person("Jean-luc Picard"), "Picard, Jean-luc");
        assert_eq!(format_person("van der Berg, Jan"), "van der Berg, Jan");
        assert_eq!(format_person("von Beethoven, Jr, Ludwig"), "von Beethoven, Jr, Ludwig");
        assert_eq!(format_person("Smith"), "Smith");
        assert_eq!(format_person("{Barnes and Noble}"), "{Barnes and Noble}");
    }

    #[test]
    fn parses_fields_and_authors() {
        let src = r#"@Article{ Key1 ,
            title = {Hello {World} and  stuff},
            Year = 2021,
            DOI = {10.1/x},
            author = {Smith, John and {Barnes and Noble}},
            url = "http://x" }"#;
        let entries = parse_bib(src);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.key, "Key1");
        assert_eq!(e.fields.get("title").unwrap(), "Hello {World} and stuff");
        assert_eq!(e.fields.get("year").unwrap(), "2021");
        assert_eq!(e.authors, vec!["Smith, John", "{Barnes and Noble}"]);
        let meta = entry_to_meta(e).unwrap();
        assert_eq!(meta.source_id, "10.1/x"); // doi wins over key
        assert_eq!(meta.published.to_string(), "2021-01-01");
        assert_eq!(meta.source.as_deref(), Some("bibtex"));
    }

    #[test]
    fn bad_year_falls_back_to_1900() {
        let e = &parse_bib("@misc{k, title={T}, year={20xx}}")[0];
        assert_eq!(entry_to_meta(e).unwrap().published.to_string(), "1900-01-01");
        // No title field present -> defaults to the key.
        let e2 = &parse_bib("@misc{onlykey}")[0];
        assert_eq!(entry_to_meta(e2).unwrap().title, "onlykey");
    }
}
