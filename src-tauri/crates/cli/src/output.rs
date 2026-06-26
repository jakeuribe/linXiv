//! Output + error parity helpers. Byte-for-byte mirrors of `linxiv_cli.py`'s
//! `_output` / error-exit / `_validate_arxiv_id` / `_as_source_id` / `_render_paper`.
// Skeleton stage: consumed by the per-group `run` bodies (still `todo!()`), so
// every helper here reads as dead until those land.
#![allow(dead_code)]

use std::fmt::Display;
use std::io::Write;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use linxiv_core::models::PaperMetadata;

/// arXiv template, embedded so it ships with the binary. Only `arxiv` has one —
/// other sources fall back to a JSON dump (see `render_paper`).
const ARXIV_TEMPLATE: &str = include_str!("../assets/arxiv_paper.md");

/// `_ARXIV_ID_RE` from `linxiv_cli.py`, verbatim.
static ARXIV_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}\.\d{4,5}(v\d+)?$|^[a-z\-]+(\.[A-Z]{2})?/\d{7}(v\d+)?$")
        .expect("static arXiv-id regex is valid")
});

/// `_output`: pretty JSON (2-space indent, matching Python `indent=2`) + trailing "\n".
pub fn output<T: Serialize>(v: &T) {
    let mut stdout = std::io::stdout();
    serde_json::to_writer_pretty(&mut stdout, v).expect("serialize value to stdout");
    let _ = stdout.write_all(b"\n");
}

/// Print `{"error": MSG}` to stderr and `exit(1)`. Built by hand (not the compact
/// `serde_json` form) so the `": "` separator byte-matches Python `json.dumps`.
pub fn fail(msg: impl Display) -> ! {
    // serde_json escapes the message string exactly like json.dumps would.
    let body = serde_json::to_string(&msg.to_string()).unwrap_or_else(|_| "\"\"".to_string());
    eprintln!("{{\"error\": {}}}", body);
    std::process::exit(1);
}

/// Python `repr()` of a string, for `!r` error-message parity. Python defaults to single
/// quotes, switching to double only when the string holds a `'` but no `"`. Rust's `{:?}`
/// always uses double quotes, so it diverges byte-for-byte on every id in an error message.
pub fn pyrepr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
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

/// `_validate_arxiv_id`: on miss, fail with the `!r`-quoted id (Python single-quote repr).
pub fn validate_arxiv_id(source_id: &str) -> String {
    if !ARXIV_ID_RE.is_match(source_id) {
        fail(format!("Invalid arXiv ID format: {}", pyrepr(source_id)));
    }
    source_id.to_string()
}

/// `_as_source_id`: prefix a bare id with its namespace; already-prefixed ids pass through.
pub fn as_source_id(raw: &str, source: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("{source}:{raw}")
    }
}

/// `_render_paper`: only `arxiv` has a template; non-arxiv sources return `None`
/// (caller then JSON-dumps the metadata). Optional fields render as Python's
/// `str(None)` -> "None", matching `format_map` over `model_dump(mode="json")`.
pub fn render_paper(meta: &PaperMetadata) -> Option<String> {
    if meta.source.as_deref() != Some("arxiv") {
        return None;
    }
    let authors_inline = meta.authors.join(", ");
    let rendered = ARXIV_TEMPLATE
        .replace("{title}", &meta.title)
        .replace("{authors_inline}", &authors_inline)
        .replace("{published}", &meta.published.to_string())
        .replace("{category}", &opt(meta.category.as_deref()))
        .replace("{source_id}", &meta.source_id)
        .replace("{url}", &opt(meta.url.as_deref()))
        .replace("{doi}", &opt(meta.doi.as_deref()))
        .replace("{summary}", &meta.summary);
    Some(rendered)
}

fn opt(v: Option<&str>) -> String {
    v.map(str::to_string).unwrap_or_else(|| "None".to_string())
}

#[cfg(test)]
mod tests {
    use super::pyrepr;

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
}
