//! Library stats — one receipt shared by route, CLI and MCP (SERIALIZER
//! convention: core owns the operation and the wire shape). Route shape wins:
//! the CLI/MCP envelopes gained `recent_papers` when this moved into core.

use crate::error::Result;
use crate::models::PaperDetails;
use crate::service::{paper, tag};
use rusqlite::Connection;
use serde::Serialize;
use ts_rs::TS;

/// `GET /api/stats` wire shape. `recent_papers` is the 10 newest latest-version
/// rows, full `PaperDetails`.
#[derive(Debug, Serialize, TS)]
pub struct Stats {
    pub paper_count: usize,
    pub tag_count: usize,
    pub category_count: usize,
    pub pdf_count: usize,
    pub recent_papers: Vec<PaperDetails>,
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    // Counts stay in SQL; only the 10 recent rows are materialized (this used
    // to haul the whole library into memory just to count it).
    let count = |sql: &str| -> Result<usize> {
        Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
    };
    let paper_count = count("SELECT COUNT(*) FROM latest_papers")?;
    let pdf_count = count("SELECT COUNT(*) FROM latest_papers WHERE has_pdf <> 0")?;
    let tag_count = tag::list_all_tags(conn)?.len();
    let category_count = paper::get_categories(conn)?.len();
    let recent_papers = paper::list_papers(conn, true, Some(10), 0, None)?;
    Ok(Stats {
        paper_count,
        tag_count,
        category_count,
        pdf_count,
        recent_papers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};

    /// Pins the wire shape: exact keys, exact order (preserve_order surfaces
    /// rely on it), empty DB values.
    #[test]
    fn stats_wire_shape() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let v = serde_json::to_value(stats(&conn).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"paper_count":0,"tag_count":0,"category_count":0,"pdf_count":0,"recent_papers":[]}"#
        );
    }

    /// The SQL counts and LIMIT 10 recent list match the old
    /// whole-library-scan-then-truncate behavior.
    #[test]
    fn stats_counts_and_recent_ten() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        for i in 0..12i64 {
            conn.execute(
                "INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?1)",
                [format!("arxiv:p{i}")],
            )
            .unwrap();
            let fk = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) \
                 VALUES (?1, 1, ?2, 'cs.LG', ?3, ?4)",
                rusqlite::params![
                    format!("arxiv:p{i}"),
                    format!("P{i}"),
                    (i % 2 == 0) as i64,
                    fk
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?1, ?2)",
                rusqlite::params![conn.last_insert_rowid(), format!("2024-01-{:02}", i + 1)],
            )
            .unwrap();
        }
        let s = stats(&conn).unwrap();
        assert_eq!(s.paper_count, 12);
        assert_eq!(s.pdf_count, 6);
        assert_eq!(s.category_count, 1);
        assert_eq!(s.recent_papers.len(), 10);
        // Newest published first — the same default ordering as the library list.
        assert_eq!(s.recent_papers[0].source_id, "arxiv:p11");
        assert_eq!(s.recent_papers[9].source_id, "arxiv:p2");
    }
}
