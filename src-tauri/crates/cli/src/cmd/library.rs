//! Group `library` — the flat top-level commands `search`, `fetch`, `list`.
//! cmd_search / cmd_fetch / cmd_list in `linxiv_cli.py`.

use clap::{Args, Subcommand, ValueEnum};

use linxiv_core::config;
use linxiv_core::service::paper as svc_paper;
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

impl Source {
    /// The `_SOURCES` key the core dispatcher routes on.
    fn name(self) -> &'static str {
        match self {
            Source::Arxiv => "arxiv",
            Source::Openalex => "openalex",
            Source::Crossref => "crossref",
        }
    }
}

/// OpenAlex polite-pool address (`OPENALEX_MAILTO`); CR/LF are stripped downstream
/// in `openalex::user_agent`, matching `OpenAlexSource`.
fn mailto() -> String {
    std::env::var("OPENALEX_MAILTO").unwrap_or_default()
}

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
}

#[derive(Subcommand)]
pub enum LibraryCmd {
    Search(SearchArgs),
    Fetch(FetchArgs),
    List(ListArgs),
}

pub async fn run(cmd: LibraryCmd, ctx: &mut Ctx) -> anyhow::Result<()> {
    let data_dir = config::data_dir();
    match cmd {
        // cmd_search: search the source, dump the metadata list. The `[search] {e}`
        // prefix line + error JSON mirror Python's two-line stderr on failure.
        LibraryCmd::Search(args) => {
            // Python `source.search` defaults sort="relevance"; the CLI never overrides it.
            let results = match svc_fetch::search(
                args.source.name(),
                &args.query,
                args.max as u32,
                "relevance",
                &data_dir,
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
        }
        // cmd_fetch: validate (arxiv only), fetch, persist, then render-or-dump.
        LibraryCmd::Fetch(args) => {
            if matches!(args.source, Source::Arxiv) {
                validate_arxiv_id(&args.source_id);
            }
            let meta = match svc_fetch::fetch_by_id(
                args.source.name(),
                &args.source_id,
                &data_dir,
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
        }
        // cmd_list: latest-version rows, optional category/limit/offset filter.
        // Python dumps the RAW `latest_papers` view rows, not curated structs.
        LibraryCmd::List(args) => {
            let papers = list_papers_raw(
                &ctx.conn,
                args.limit,
                args.offset,
                args.category.as_deref(),
            )?;
            output(&papers);
        }
    }
    Ok(())
}

/// `cmd_list` body: `[{k: row[k] for k in row.keys()} for row in rows]` over the
/// RAW `latest_papers` view rows — NOT `PaperDetails`. Each column stays as its
/// raw SQLite value: integers (incl. has_pdf/downloaded_source 0/1) stay integers,
/// the categories/authors/tags JSON-TEXT columns stay as raw strings, created_at/
/// updated_at are included, NULLs serialize to null, and columns keep view order
/// (preserve_order). Same filter/order as `db.list_papers(latest_only=True)`.
fn list_papers_raw(
    conn: &rusqlite::Connection,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
) -> rusqlite::Result<Vec<serde_json::Value>> {
    use rusqlite::types::{Value, ValueRef};

    let mut sql = "SELECT * FROM latest_papers".to_string();
    let mut params: Vec<Value> = Vec::new();
    if let Some(cat) = category {
        sql.push_str(" WHERE category = ?");
        params.push(Value::Text(cat.to_string()));
    }
    sql.push_str(" ORDER BY published DESC");
    match limit {
        Some(l) => {
            sql.push_str(" LIMIT ? OFFSET ?");
            params.push(Value::Integer(l));
            params.push(Value::Integer(offset));
        }
        // No limit but a nonzero offset still needs LIMIT -1 (all rows) to skip.
        None if offset != 0 => {
            sql.push_str(" LIMIT -1 OFFSET ?");
            params.push(Value::Integer(offset));
        }
        None => {}
    }

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
