//! Group `library` — the flat top-level commands `search`, `fetch`, `list`.
//! cmd_search / cmd_fetch / cmd_list in `linxiv_cli.py`.

use clap::{Args, ValueEnum};

use linxiv_core::config;
use linxiv_core::models::SearchResultOut;
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

// cmd_search: search the source, dump the results as `SearchResultOut` — the
// canonical search wire shape all three surfaces emit (ADR-0011). The
// `[search] {e}` prefix line + error JSON mirror Python's two-line stderr on failure.
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
    let results: Vec<SearchResultOut> = results.into_iter().map(SearchResultOut::from).collect();
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

// cmd_list: latest-version rows, optional category/limit/offset filter, emitted
// as `PaperDetails` (models.rs SERIALIZER 2) — the same wire shape route and MCP
// list arms serialize. `full_text` never ships (skip_serializing on the model).
pub async fn list(args: ListArgs, ctx: &mut Ctx) -> anyhow::Result<()> {
    let sort = PaperSort::from_key(args.sort.to_possible_value().unwrap().get_name());
    let desc = match args.dir {
        Some(Dir::Desc) => true,
        Some(Dir::Asc) => false,
        None => sort.default_desc(),
    };
    let papers = svc_paper::list_papers_sorted(
        &ctx.conn,
        true,
        args.limit,
        args.offset,
        args.category.as_deref(),
        sort,
        desc,
    )?;
    output(&papers);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use linxiv_core::models::PaperMetadata;
    use linxiv_core::storage;
    use serde_json::json;

    /// `linxiv list` emits `PaperDetails` — pin the exact wire keys so a raw-row
    /// regression (JSON-string lists, 0/1 ints, created_at/updated_at extras,
    /// leaked full_text) can't come back.
    #[test]
    fn list_emits_paper_details_wire_shape() {
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let meta: PaperMetadata = serde_json::from_value(json!({
            "source_id": "arxiv:1234.5678",
            "version": 1,
            "title": "T",
            "authors": ["Ada Lovelace", "Alan Turing"],
            "published": "2024-01-01",
            "summary": "s",
            "category": "cs.LG",
            "categories": ["cs.LG", "stat.ML"],
            "source": "arxiv",
        }))
        .unwrap();
        svc_paper::save_paper_metadata(&mut conn, &meta, None).unwrap();
        svc_paper::set_full_text(&mut conn, "arxiv:1234.5678", 1, "tex body").unwrap();

        let papers =
            svc_paper::list_papers_sorted(&conn, true, None, 0, None, PaperSort::Published, true)
                .unwrap();
        let row = serde_json::to_value(&papers[0]).unwrap();
        let keys: Vec<&str> = row.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "paper_id",
                "source_id",
                "version",
                "title",
                "summary",
                "published",
                "updated",
                "url",
                "doi",
                "category",
                "categories",
                "journal_ref",
                "comment",
                "authors",
                "tags",
                "has_pdf",
                "pdf_path",
                "source",
                "downloaded_source",
                "source_fk",
            ]
        );
        assert_eq!(row["authors"], json!(["Ada Lovelace", "Alan Turing"]));
        assert_eq!(row["has_pdf"], json!(false)); // bool, not 0/1
        assert_eq!(row["downloaded_source"], json!(true));
    }

    /// `linxiv search` emits `SearchResultOut` (ADR-0011) — pin the exact wire
    /// shape so this surface can't drift back to raw `PaperMetadata`.
    #[test]
    fn search_results_pin_the_canonical_wire_shape() {
        let meta: PaperMetadata = serde_json::from_value(json!({
            "source_id": "arxiv:2204.12985",
            "version": 2,
            "title": "T",
            "authors": ["A", "B"],
            "published": "2024-01-15",
            "summary": "S",
            "category": "cs.LG",
            "url": "http://x",
        }))
        .unwrap();
        let v = serde_json::to_value(SearchResultOut::from(meta)).unwrap();
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"source_id":"2204.12985","version":2,"title":"T","summary":"S","authors":["A","B"],"published":"2024-01-15","paper_url":"http://x","primary_category":"cs.LG","entry_id":"arxiv:2204.12985"}"#
        );
    }
}
