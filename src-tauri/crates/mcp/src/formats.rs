//! BibTeX + Obsidian-markdown formatters used by the export/import tools.
//!
//! Port of `formats/bibtex.py` (pybtex) and `formats/markdown.py::ObsidianFormat`.
//! These live in the binary, not in core: they are leaf string<->data transforms
//! with no DB access. The CLI can lift them into a shared `formats` crate later.
//! ponytail: keep here until a second consumer (CLI) needs them; then hoist.

use std::collections::BTreeSet;

use biblatex::Bibliography;
use chrono::NaiveDate;
use linxiv_core::models::{PaperDetails, PaperMetadata};

const FALLBACK_DATE: (i32, u32, u32) = (1900, 1, 1);

// ── BibTeX export (`BibTeXFormat.export_papers`) ────────────────────────────

/// Render papers as a BibTeX string (one `@article` entry each). Field order
/// mirrors the Python writer: title, year, abstract, then optional doi/journal/url.
pub fn bibtex_export(papers: &[PaperDetails]) -> String {
    let mut out = String::new();
    for p in papers {
        let key = bib_key(&p.source_id);
        let year = p
            .published
            .map(|d| d.format("%Y").to_string())
            .unwrap_or_default();
        out.push_str(&format!("@article{{{key},\n"));
        out.push_str(&format!("  title = {{{}}},\n", p.title));
        out.push_str(&format!("  year = {{{year}}},\n"));
        out.push_str(&format!(
            "  abstract = {{{}}},\n",
            p.summary.as_deref().unwrap_or("")
        ));
        if let Some(doi) = p.doi.as_deref().filter(|s| !s.is_empty()) {
            out.push_str(&format!("  doi = {{{doi}}},\n"));
        }
        if let Some(journal) = p.journal_ref.as_deref().filter(|s| !s.is_empty()) {
            out.push_str(&format!("  journal = {{{journal}}},\n"));
        }
        if let Some(url) = p.url.as_deref().filter(|s| !s.is_empty()) {
            out.push_str(&format!("  url = {{{url}}},\n"));
        }
        out.push_str("}\n\n");
    }
    out
}

/// `(source_id or "unknown").replace("/","_").replace(".","_")`.
fn bib_key(source_id: &str) -> String {
    let base = if source_id.is_empty() { "unknown" } else { source_id };
    base.replace(['/', '.'], "_")
}

// ── Obsidian export (`ObsidianFormat.export_papers`) ────────────────────────

/// YAML-frontmatter Markdown, one `##` section per paper. Byte-faithful to the
/// Python `ObsidianFormat.export_papers` line builder.
pub fn obsidian_export(papers: &[PaperDetails]) -> String {
    let mut all_tags: BTreeSet<String> = BTreeSet::new();
    for p in papers {
        for t in &p.tags {
            all_tags.insert(t.clone());
        }
    }

    let mut lines: Vec<String> = vec!["---".into(), format!("papers: {}", papers.len())];
    if !all_tags.is_empty() {
        lines.push("tags:".into());
        for t in &all_tags {
            lines.push(format!("  - {t}"));
        }
    }
    lines.extend(["---".into(), "".into(), "# Selected Papers".into(), "".into()]);

    for p in papers {
        let sid = p.source_id.as_str();
        let title = if p.title.is_empty() { sid } else { p.title.as_str() };
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

/// Port of `_ARXIV_ID_RE`: `^\d{4}\.\d{4,5}(v\d+)?$ | ^[a-z-]+/\d{7}$`.
fn is_arxiv_id(sid: &str) -> bool {
    new_style_arxiv(sid) || old_style_arxiv(sid)
}

fn new_style_arxiv(sid: &str) -> bool {
    // NNNN.NNNN[N][vN]
    let (head, ver) = match sid.split_once('v') {
        Some((h, v)) if v.chars().all(|c| c.is_ascii_digit()) && !v.is_empty() => (h, true),
        Some(_) => return false,
        None => (sid, false),
    };
    let _ = ver;
    let Some((a, b)) = head.split_once('.') else { return false };
    a.len() == 4
        && a.chars().all(|c| c.is_ascii_digit())
        && (4..=5).contains(&b.len())
        && b.chars().all(|c| c.is_ascii_digit())
}

fn old_style_arxiv(sid: &str) -> bool {
    // [a-z-]+/NNNNNNN
    let Some((cat, num)) = sid.split_once('/') else { return false };
    !cat.is_empty()
        && cat.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        && num.len() == 7
        && num.chars().all(|c| c.is_ascii_digit())
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
            source_id: doi.clone().unwrap_or_else(|| key.clone()),
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
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(FALLBACK_DATE.0, FALLBACK_DATE.1, FALLBACK_DATE.2).unwrap())
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
    fn arxiv_id_matcher_matches_python_regex() {
        assert!(is_arxiv_id("2204.12985"));
        assert!(is_arxiv_id("2204.12985v3"));
        assert!(!is_arxiv_id("2204.123456")); // 6-digit suffix too long (Python regex caps at \d{4,5})
        assert!(is_arxiv_id("math-ph/0309136"));
        assert!(!is_arxiv_id("arxiv:2204.12985")); // namespaced -> not bare
        assert!(!is_arxiv_id("2204.123")); // suffix too short
        assert!(!is_arxiv_id("openalex:W123"));
    }

    #[test]
    fn bibtex_roundtrips_key_doi_and_authors() {
        let src = "@article{smith2020, author = {John Smith and Jane Doe}, \
                   title = {A Title}, year = {2020}, doi = {10.1/x}, journal = {J}}";
        let metas = bibtex_import(src).unwrap();
        assert_eq!(metas.len(), 1);
        let m = &metas[0];
        assert_eq!(m.source_id, "10.1/x"); // doi wins over key
        assert_eq!(m.doi.as_deref(), Some("10.1/x"));
        assert_eq!(m.title, "A Title");
        assert_eq!(m.published, NaiveDate::from_ymd_opt(2020, 1, 1).unwrap());
        assert_eq!(m.authors, vec!["John Smith".to_string(), "Jane Doe".to_string()]);
        assert_eq!(m.source.as_deref(), Some("bibtex"));

        // No year -> 1900-01-01 fallback; no doi -> key as source_id.
        let m2 = &bibtex_import("@misc{k, title={T}}").unwrap()[0];
        assert_eq!(m2.source_id, "k");
        assert_eq!(m2.published, NaiveDate::from_ymd_opt(1900, 1, 1).unwrap());
    }
}
