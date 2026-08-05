//! Group `library` — the flat top-level commands `search`, `fetch`, `list`.
//! cmd_search / cmd_fetch / cmd_list in `linxiv_cli.py`.

use clap::{Args, ValueEnum};

use linxiv_core::config;
use linxiv_core::service::paper::{self as svc_paper, PaperSort};
use linxiv_core::sources::fetch as svc_fetch;

use crate::ctx::Ctx;
use crate::output::{fail, output, render_paper, validate_arxiv_id};

/// Paper source backends (argparse `choices=list(_SOURCES)`).
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Source {
    Arxiv,
    Openalex,
    Crossref,
}

use linxiv_core::config::openalex_mailto as mailto;

#[derive(Args)]
pub struct SearchArgs {
    /// Search query string
    pub query: String,
    #[arg(long, value_enum, default_value_t = Source::Arxiv)]
    pub source: Source,
    /// Max results
    #[arg(long, default_value_t = 10)]
    pub max: i64,
}

#[derive(Args)]
pub struct FetchArgs {
    /// Paper ID (e.g. 2204.12985 or W3123456789)
    pub source_id: String,
    #[arg(long, value_enum, default_value_t = Source::Arxiv)]
    pub source: Source,
}

/// Library sort metrics — the variant names are the `PaperSort` wire keys.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SortKey {
    Published,
    Added,
    Title,
}

#[derive(Args)]
pub struct ListArgs {
    /// Max papers to return
    #[arg(long)]
    pub limit: Option<i64>,
    /// Offset for pagination
    #[arg(long, default_value_t = 0)]
    pub offset: i64,
    /// Filter by category
    #[arg(long)]
    pub category: Option<String>,
    /// Sort metric
    #[arg(long, value_enum, default_value_t = SortKey::Published)]
    pub sort: SortKey,
    /// Sort direction (default: newest first for dates, A–Z for titles)
    #[arg(long, value_enum)]
    pub dir: Option<Dir>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Dir {
    Asc,
    Desc,
}

// cmd_search: search the source, dump the metadata list. The `[search] {e}`
// prefix line + error JSON mirror Python's two-line stderr on failure.
pub async fn search(args: SearchArgs, _ctx: &mut Ctx) -> anyhow::Result<()> {
    // Python `source.search` defaults sort="relevance"; the CLI never overrides it.
    let results = match svc_fetch::search(
        args.source.to_possible_value().unwrap().get_name(),
        &args.query,
        args.max as u32,
        "relevance",
        &config::data_dir(),
        &mailto(),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[search] {e}");
            fail(e);
        }
    };
    output(&results);
    Ok(())
}

// cmd_fetch: validate (arxiv only), fetch, persist, then render-or-dump.
pub async fn fetch(args: FetchArgs, ctx: &mut Ctx) -> anyhow::Result<()> {
    if matches!(args.source, Source::Arxiv) {
        validate_arxiv_id(&args.source_id);
    }
    let meta = match svc_fetch::fetch_by_id(
        args.source.to_possible_value().unwrap().get_name(),
        &args.source_id,
        &config::data_dir(),
        &mailto(),
    )
    .await
    {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[fetch] {e}");
            fail(e);
        }
    };
    svc_paper::save_paper_metadata(&mut ctx.conn, &meta, None)?;
    match render_paper(&meta) {
        Some(rendered) => println!("{rendered}"),
        None => output(&meta),
    }
    Ok(())
}

// cmd_list: latest-version rows, optional category/limit/offset filter.
// Python dumps the RAW `latest_papers` view rows, not curated structs.
pub async fn list(args: ListArgs, ctx: &mut Ctx) -> anyhow::Result<()> {
    let sort = PaperSort::from_key(args.sort.to_possible_value().unwrap().get_name());
    let desc = match args.dir {
        Some(Dir::Desc) => true,
        Some(Dir::Asc) => false,
        None => sort.default_desc(),
    };
    let papers = list_papers_raw(
        &ctx.conn,
        args.limit,
        args.offset,
        args.category.as_deref(),
        sort,
        desc,
    )?;
    output(&papers);
    Ok(())
}

/// `cmd_list` body: `[{k: row[k] for k in row.keys()} for row in rows]` over the
/// RAW `latest_papers` view rows — NOT `PaperDetails`. Each column stays as its
/// raw SQLite value: integers (incl. has_pdf/downloaded_source 0/1) stay integers,
/// the categories/authors/tags JSON-TEXT columns stay as raw strings, created_at/
/// updated_at are included, NULLs serialize to null, and columns keep view order
/// (preserve_order). Same filter/order as `db.list_papers(latest_only=True)`.
///
/// One exception: `full_text` always reports null. `list_papers_sql` selects it
/// as NULL so a multi-row read doesn't haul every indexed TeX body into memory;
/// `paper get` still returns the real value.
fn list_papers_raw(
    conn: &rusqlite::Connection,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
    sort: PaperSort,
    desc: bool,
) -> rusqlite::Result<Vec<serde_json::Value>> {
    use rusqlite::types::ValueRef;

    let (sql, params) = linxiv_core::storage::queries::paper::list_papers_sql(
        true, limit, offset, category, sort, desc,
    );

    let mut stmt = conn.prepare(&sql)?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(rusqlite::params_from_iter(&params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            let val = match row.get_ref(i)? {
                ValueRef::Null => serde_json::Value::Null,
                ValueRef::Integer(n) => serde_json::Value::from(n),
                ValueRef::Real(f) => serde_json::Value::from(f),
                ValueRef::Text(t) => {
                    serde_json::Value::from(String::from_utf8_lossy(t).into_owned())
                }
                ValueRef::Blob(b) => {
                    serde_json::Value::from(String::from_utf8_lossy(b).into_owned())
                }
            };
            obj.insert(name.clone(), val);
        }
        out.push(serde_json::Value::Object(obj));
    }
    Ok(out)
}
