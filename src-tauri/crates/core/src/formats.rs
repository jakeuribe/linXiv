//! BibTeX + Obsidian-markdown export formatters — leaf string transforms over
//! `PaperDetails`, no DB access. Port of `formats/bibtex.py::BibTeXFormat.export_papers`
//! (pybtex) and `formats/markdown.py::ObsidianFormat.export_papers`. Hoisted here so
//! the three thin binaries (Tauri app router, CLI, MCP) share one implementation
//! instead of each carrying its own copy.
//!
//! Deferred pybtex/pybtex-import strictness gaps (carried over from the original
//! CLI/MCP ports; not byte-exact vs Python, add if a golden needs it): export does
//! not LaTeX-encode field values (`%`→`\%`, `&`→`\&`, …) the way pybtex's
//! ulatex codec does, nor dedup case-insensitive citation keys; import
//! (`format_person`) emits "Given Last" rather than pybtex's "Last, Given"
//! normalization, drops literal braces, and accepts out-of-range years that
//! Python's `date()` would reject.

use std::collections::BTreeSet;

use biblatex::Bibliography;
use chrono::NaiveDate;

use crate::models::{PaperDetails, PaperMetadata};

/// Python `repr()` of a string, for `!r` error-message parity. Python defaults to single
/// quotes, switching to double only when the string holds a `'` but no `"`. Rust's `{:?}`
/// always uses double quotes, so it diverges byte-for-byte on every id in an error message.
pub fn pyrepr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `Path(dest)` + `with_suffix` only when the path has no extension
/// (Python `out.with_suffix(...) if not out.suffix`). Shared by the CLI and
/// MCP export commands.
pub fn with_default_ext(dest: &str, ext: &str) -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(dest);
    if p.extension().is_none() {
        p.set_extension(ext);
    }
    p
}

/// One `@article` entry per paper, byte-matching pybtex `bib.to_string("bibtex")`:
/// 4-space indent, `field = "value"`, no trailing comma on the last field, one
/// blank line between entries, single trailing newline.
pub fn bibtex_export(papers: &[PaperDetails]) -> String {
    let mut out = String::new();
    for (i, p) in papers.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let key = bib_key(&p.source_id);
        let year = p
            .published
            .map(|d| d.format("%Y").to_string())
            .unwrap_or_default();
        out.push_str(&format!("@article{{{key}"));
        let mut fields: Vec<(&str, &str)> = vec![
            ("title", p.title.as_str()),
            ("year", year.as_str()),
            ("abstract", p.summary.as_deref().unwrap_or("")),
        ];
        if let Some(doi) = p.doi.as_deref().filter(|s| !s.is_empty()) {
            fields.push(("doi", doi));
        }
        if let Some(journal) = p.journal_ref.as_deref().filter(|s| !s.is_empty()) {
            fields.push(("journal", journal));
        }
        if let Some(url) = p.url.as_deref().filter(|s| !s.is_empty()) {
            fields.push(("url", url));
        }
        for (name, value) in fields {
            out.push_str(&format!(",\n    {name} = {}", bib_quote(value)));
        }
        out.push_str("\n}\n");
    }
    out
}

/// pybtex `Writer.quote`: `"value"` unless the value contains a `"`, then `{value}`.
fn bib_quote(value: &str) -> String {
    if value.contains('"') {
        format!("{{{value}}}")
    } else {
        format!("\"{value}\"")
    }
}

/// `(source_id or "unknown").replace("/","_").replace(".","_")`.
fn bib_key(source_id: &str) -> String {
    if source_id.is_empty() {
        "unknown"
    } else {
        source_id
    }
    .replace(['/', '.'], "_")
}

/// `ObsidianFormat.export_papers` — YAML frontmatter + one `##` section per paper.
pub fn obsidian_export(papers: &[PaperDetails]) -> String {
    let all_tags: BTreeSet<&String> = papers.iter().flat_map(|p| &p.tags).collect();

    let mut lines: Vec<String> = vec!["---".into(), format!("papers: {}", papers.len())];
    if !all_tags.is_empty() {
        lines.push("tags:".into());
        for t in &all_tags {
            lines.push(format!("  - {t}"));
        }
    }
    lines.extend([
        "---".into(),
        "".into(),
        "# Selected Papers".into(),
        "".into(),
    ]);

    for p in papers {
        let sid = p.source_id.as_str();
        // Python `p.get("title", sid)`: title key always present, so an empty title stays empty.
        let title = p.title.as_str();
        let authors = p.authors.join(", ");
        let url = paper_url(sid, p.url.as_deref());
        lines.push(format!("## [{title}]({url})"));
        lines.push("".into());
        if !is_arxiv_id(sid) {
            lines.push(format!("**Paper-ID:** {sid}"));
        }
        if !authors.is_empty() {
            lines.push(format!("**Authors:** {authors}"));
        }
        if let Some(cat) = p.category.as_deref().filter(|s| !s.is_empty()) {
            lines.push(format!("**Category:** {cat}"));
        }
        if !p.tags.is_empty() {
            lines.push(format!("**Tags:** {}", p.tags.join(", ")));
        }
        lines.push("".into());
    }
    lines.join("\n")
}

/// Best URL for a paper: stored url > arXiv abs link > empty.
fn paper_url(sid: &str, stored_url: Option<&str>) -> String {
    if let Some(u) = stored_url.filter(|s| !s.is_empty()) {
        return u.to_string();
    }
    if is_arxiv_id(sid) {
        return format!("https://arxiv.org/abs/{sid}");
    }
    String::new()
}

/// Port of `_ARXIV_ID_RE`: `^\d{4}\.\d{4,5}(v\d+)?$ | ^[a-z\-]+(\.[A-Z]{2})?/\d{7}(v\d+)?$`.
/// Also the single source of truth for `linxiv-cli`'s `validate_arxiv_id` — pub
/// so the CLI doesn't need its own copy (was a duplicate `regex` crate + static).
pub fn is_arxiv_id(sid: &str) -> bool {
    new_style_arxiv(sid) || old_style_arxiv(sid)
}

fn new_style_arxiv(sid: &str) -> bool {
    let head = match sid.split_once('v') {
        Some((h, v)) if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()) => h,
        Some(_) => return false,
        None => sid,
    };
    let Some((a, b)) = head.split_once('.') else {
        return false;
    };
    a.len() == 4
        && a.chars().all(|c| c.is_ascii_digit())
        && (4..=5).contains(&b.len())
        && b.chars().all(|c| c.is_ascii_digit())
}

/// Strips an optional `.XX` archive-class suffix (e.g. "math.NT") from a
/// category part, matching the regex's `(\.[A-Z]{2})?` group: stripped only
/// when it's exactly 2 uppercase letters.
fn compute_cat(cat_part: &str) -> &str {
    match cat_part.rfind('.') {
        Some(i)
            if cat_part[i + 1..].len() == 2
                && cat_part[i + 1..].chars().all(|c| c.is_ascii_uppercase()) =>
        {
            &cat_part[..i]
        }
        _ => cat_part,
    }
}

fn old_style_arxiv(sid: &str) -> bool {
    let Some((cat_part, rest)) = sid.split_once('/') else {
        return false;
    };
    let cat = compute_cat(cat_part);
    if cat.is_empty() || !cat.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
        return false;
    }
    // Optional `vN` version suffix, matching the regex's trailing `(v\d+)?`.
    let num = match rest.split_once('v') {
        Some((n, v)) if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()) => n,
        Some(_) => return false,
        None => rest,
    };
    num.len() == 7 && num.chars().all(|c| c.is_ascii_digit())
}

// ── BibTeX import (`BibTeXFormat.import_string` / `_bib_to_metadata`) ────────

/// Parse a BibTeX document into `PaperMetadata`. Mirrors `_bib_to_metadata`:
/// source_id = doi or entry key, version 1, source "bibtex", year→Jan-1 date
/// (falling back to 1900-01-01).
pub fn bibtex_import(text: &str) -> Result<Vec<PaperMetadata>, String> {
    let bib = Bibliography::parse(text).map_err(|e| format!("BibTeX parse error: {e}"))?;
    let mut out = Vec::new();
    for entry in bib.into_iter() {
        let key = entry.key.clone();
        let authors: Vec<String> = entry
            .author()
            .unwrap_or_default()
            .iter()
            .map(format_person)
            .collect();
        let doi = field(&entry, "doi");
        let title = field(&entry, "title").unwrap_or_else(|| key.clone());
        let summary = field(&entry, "abstract").unwrap_or_default();
        let journal_ref = field(&entry, "journal").or_else(|| field(&entry, "booktitle"));
        let url = field(&entry, "url");
        let published = parse_year(&entry);
        out.push(PaperMetadata {
            // ADR 0002 / CONTEXT.md § source_id: always namespaced. A DOI keys the
            // root under `doi:`; an entry without one is unidentified, so its BibTeX
            // key goes under `local:`.
            source_id: match &doi {
                Some(d) => crate::models::doi_source_id(d),
                None => crate::models::local_source_id(&key),
            },
            version: 1,
            title,
            authors,
            published,
            updated: None,
            summary,
            category: None,
            categories: None,
            doi,
            journal_ref,
            comment: None,
            url,
            tags: None,
            source: Some("bibtex".into()),
            author_orcids: None,
        });
    }
    Ok(out)
}

/// A scalar field as plain text, or None when absent/empty.
fn field(entry: &biblatex::Entry, key: &str) -> Option<String> {
    entry
        .get_as::<String>(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn parse_year(entry: &biblatex::Entry) -> NaiveDate {
    let year = entry
        .get_as::<i64>("year")
        .ok()
        .or_else(|| field(entry, "year").and_then(|s| s.parse::<i64>().ok()));
    year.and_then(|y| NaiveDate::from_ymd_opt(y as i32, 1, 1))
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(1900, 1, 1).unwrap())
}

/// "Given Last" display name (prefix/suffix folded in), trimmed.
fn format_person(p: &biblatex::Person) -> String {
    [
        p.given_name.as_str(),
        p.prefix.as_str(),
        p.name.as_str(),
        p.suffix.as_str(),
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyrepr_matches_python_repr() {
        // Python: repr("arxiv:1234.5678") == "'arxiv:1234.5678'"
        assert_eq!(pyrepr("arxiv:1234.5678"), "'arxiv:1234.5678'");
        // repr of a string with a single quote and no double → switches to double quotes.
        assert_eq!(pyrepr("O'Brien"), "\"O'Brien\"");
        // Both quote kinds present → stays single, escapes the single.
        assert_eq!(pyrepr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(pyrepr("tab\there"), "'tab\\there'");
    }

    fn paper(source_id: &str, title: &str) -> PaperDetails {
        PaperDetails {
            paper_id: 1,
            source_id: source_id.into(),
            version: 1,
            title: title.into(),
            summary: Some("S".into()),
            published: NaiveDate::from_ymd_opt(2024, 1, 1),
            updated: None,
            url: None,
            doi: None,
            category: None,
            categories: vec![],
            journal_ref: None,
            comment: None,
            authors: vec!["Ada".into()],
            tags: vec![],
            has_pdf: false,
            pdf_path: None,
            source: None,
            full_text: None,
            downloaded_source: false,
            source_fk: 1,
        }
    }

    #[test]
    fn bibtex_export_matches_pybtex_layout() {
        let bib = bibtex_export(&[paper("2204.12985", "A Title")]);
        assert_eq!(
            bib,
            "@article{2204_12985,\n    title = \"A Title\",\n    year = \"2024\",\n    abstract = \"S\"\n}\n"
        );
    }

    #[test]
    fn obsidian_omits_paper_id_for_arxiv_and_builds_abs_url() {
        let md = obsidian_export(&[paper("2204.12985", "A Title")]);
        assert!(md.contains("## [A Title](https://arxiv.org/abs/2204.12985)"));
        assert!(!md.contains("**Paper-ID:**")); // arXiv id → omitted
        assert!(md.contains("**Authors:** Ada"));
    }

    #[test]
    fn bibtex_import_doi_wins_and_year_falls_back() {
        let metas = bibtex_import(
            "@article{smith2020, author = {John Smith and Jane Doe}, \
             title = {A Title}, year = {2020}, doi = {10.1/x}, journal = {J}}",
        )
        .unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].source_id, "doi:10.1/x"); // doi wins over key
        assert_eq!(
            metas[0].authors,
            vec!["John Smith".to_string(), "Jane Doe".to_string()]
        );
        assert_eq!(metas[0].source.as_deref(), Some("bibtex"));
        // no year/doi → the key under the `local:` namespace, 1900-01-01 fallback
        let m2 = &bibtex_import("@misc{k, title={T}}").unwrap()[0];
        assert_eq!(m2.source_id, "local:k");
        assert_eq!(m2.published, NaiveDate::from_ymd_opt(1900, 1, 1).unwrap());
    }

    #[test]
    fn arxiv_id_matcher() {
        assert!(is_arxiv_id("2204.12985"));
        assert!(is_arxiv_id("2204.12985v3"));
        assert!(is_arxiv_id("math-ph/0309136"));
        assert!(is_arxiv_id("math.NT/0309136")); // archive-class suffix
        assert!(is_arxiv_id("hep-th/9901001v2")); // old-style with version
        assert!(!is_arxiv_id("math.nt/0309136")); // suffix must be uppercase
        assert!(!is_arxiv_id("2204.123456")); // suffix too long
        assert!(!is_arxiv_id("openalex:W123"));
    }
}
