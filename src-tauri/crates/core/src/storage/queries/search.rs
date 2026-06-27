use chrono::NaiveDate;
use rusqlite::{Connection, Row};

use crate::error::Result;
use crate::models::PaperDetails;
use crate::storage::db::{bool_from_sql, date_from_sql, list_from_sql};

/// `storage/db.py::search_full_text` — FTS5 over TeX source. Returns the latest
/// version of each matching paper, ranked by FTS relevance.
///
/// FTS misnomer: `papers_fts.paper_id` holds the SOURCE_ID *string*, so the join
/// is `latest_papers.source_id = papers_fts.paper_id` (NOT the int PAPER_ID).
/// init_db always creates papers_fts, so Python's "table missing" guard is moot.
pub fn search_full_text(conn: &Connection, query: &str, limit: i64) -> Result<Vec<PaperDetails>> {
    let mut stmt = conn.prepare(
        "SELECT p.* FROM latest_papers p \
         JOIN papers_fts fts ON p.source_id = fts.paper_id \
         WHERE papers_fts MATCH ?1 \
         ORDER BY rank \
         LIMIT ?2",
    )?;
    // Manual loop (not query_map): map_row returns CoreError on a bad
    // DATE/LIST decode, which query_map's rusqlite::Result closure can't carry.
    let mut rows = stmt.query((query, limit))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map_row(row)?);
    }
    Ok(out)
}

/// Map a `latest_papers` (== `papers`) view row into `PaperDetails`, using the
/// decltype converters in storage::db for LIST/DATE/BOOL columns.
fn map_row(row: &Row) -> Result<PaperDetails> {
    Ok(PaperDetails {
        paper_id: row.get("paper_id")?,
        source_id: row.get("source_id")?,
        version: row.get("version")?,
        title: row.get("title")?,
        summary: row.get("summary")?,
        published: opt_date(row.get("published")?)?,
        updated: opt_date(row.get("updated")?)?,
        url: row.get("url")?,
        doi: row.get("doi")?,
        category: row.get("category")?,
        categories: opt_list(row.get("categories")?)?,
        journal_ref: row.get("journal_ref")?,
        comment: row.get("comment")?,
        authors: opt_list(row.get("authors")?)?,
        tags: opt_list(row.get("tags")?)?,
        has_pdf: bool_from_sql(row.get("has_pdf")?),
        pdf_path: row.get("pdf_path")?,
        source: row.get("source")?,
        full_text: row.get("full_text")?,
        downloaded_source: bool_from_sql(
            row.get::<_, Option<i64>>("downloaded_source")?.unwrap_or(0),
        ),
        source_fk: row.get("source_fk")?,
    })
}

fn opt_date(s: Option<String>) -> Result<Option<NaiveDate>> {
    s.as_deref().map(date_from_sql).transpose()
}

fn opt_list(s: Option<String>) -> Result<Vec<String>> {
    Ok(s.as_deref()
        .map(list_from_sql)
        .transpose()?
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};

    fn seed(conn: &Connection, source_id: &str, full_text: &str) {
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?1)",
            [source_id],
        )
        .unwrap();
        let source_fk: i64 = conn
            .query_row(
                "SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
                [source_id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) \
             VALUES (?1, 1, ?2, 'cs.LG', 1, ?3)",
            rusqlite::params![source_id, "A Title", source_fk],
        )
        .unwrap();
        let paper_id: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED, AUTHORS, TAGS, SUMMARY, FULL_TEXT) \
             VALUES (?1, '2024-03-05', '[\"Ada\"]', '[\"ml\"]', 'sum', ?2)",
            rusqlite::params![paper_id, full_text],
        )
        .unwrap();
        // FTS misnomer: paper_id column stores the SOURCE_ID string.
        conn.execute(
            "INSERT INTO papers_fts (paper_id, full_text) VALUES (?1, ?2)",
            rusqlite::params![source_id, full_text],
        )
        .unwrap();
    }

    #[test]
    fn matches_on_tex_source_and_maps_fields() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(
            &conn,
            "arxiv:2204.12985",
            "the manifold hypothesis in latent space",
        );
        seed(
            &conn,
            "arxiv:1111.00000",
            "unrelated quantum chromodynamics",
        );

        let hits = search_full_text(&conn, "manifold", 20).unwrap();
        assert_eq!(hits.len(), 1);
        let p = &hits[0];
        assert_eq!(p.source_id, "arxiv:2204.12985");
        assert_eq!(p.version, 1);
        assert_eq!(p.title, "A Title");
        assert_eq!(p.published, NaiveDate::from_ymd_opt(2024, 3, 5));
        assert_eq!(p.authors, vec!["Ada".to_string()]);
        assert_eq!(p.tags, vec!["ml".to_string()]);
        assert!(p.has_pdf);

        assert_eq!(
            search_full_text(&conn, "nonexistentterm", 20)
                .unwrap()
                .len(),
            0
        );
    }
}
