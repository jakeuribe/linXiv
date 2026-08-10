//! Output + error parity helpers. Byte-for-byte mirrors of `linxiv_cli.py`'s
//! `_output` / error-exit / `_validate_arxiv_id` / `_as_source_id` / `_render_paper`.
use std::fmt::Display;
use std::io::Write;

use serde::Serialize;

use linxiv_core::formats::is_arxiv_id;
use linxiv_core::models::PaperMetadata;

/// arXiv template, embedded so it ships with the binary. Only `arxiv` has one —
/// other sources fall back to a JSON dump (see `render_paper`).
const ARXIV_TEMPLATE: &str = include_str!("../assets/arxiv_paper.md");

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

pub use linxiv_core::formats::pyrepr;

/// `_validate_arxiv_id`: on miss, fail with the `!r`-quoted id (Python single-quote repr).
pub fn validate_arxiv_id(source_id: &str) {
    if !is_arxiv_id(source_id) {
        fail(format!("Invalid arXiv ID format: {}", pyrepr(source_id)));
    }
}

/// `_as_source_id`: prefix a bare id with its namespace; already-prefixed ids pass through.
pub fn as_source_id(raw: &str, source: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("{source}:{raw}")
    }
}

/// Resolve an optional CLI paper id to its SOURCE_FK; `Err(NotFound)` on a miss
/// (`main` routes it through `fail`, same `{"error": ...}` body).
pub fn resolve_source_fk(
    conn: &rusqlite::Connection,
    raw: Option<String>,
) -> anyhow::Result<Option<i64>> {
    Ok(match raw {
        Some(raw) => Some(linxiv_core::service::paper::resolve_source_fk(
            conn,
            &as_source_id(&raw, "arxiv"),
        )?),
        None => None,
    })
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
