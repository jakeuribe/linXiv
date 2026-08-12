//! Library stats — one receipt shared by route, CLI and MCP (SERIALIZER
//! convention: core owns the operation and the wire shape). Route shape wins:
//! the CLI/MCP envelopes gained `recent_papers` when this moved into core.

use crate::error::Result;
use crate::models::PaperDetails;
use crate::service::{paper, tag};
use rusqlite::Connection;
use serde::Serialize;

/// `GET /api/stats` wire shape. `recent_papers` is the 10 newest latest-version
/// rows, full `PaperDetails`.
#[derive(Debug, Serialize)]
pub struct Stats {
    pub paper_count: usize,
    pub tag_count: usize,
    pub category_count: usize,
    pub pdf_count: usize,
    pub recent_papers: Vec<PaperDetails>,
}

pub fn stats(conn: &Connection) -> Result<Stats> {
    let mut papers = paper::list_papers(conn, true, None, 0, None)?;
    let tag_count = tag::list_all_tags(conn)?.len();
    let category_count = paper::get_categories(conn)?.len();
    let paper_count = papers.len();
    let pdf_count = papers.iter().filter(|p| p.has_pdf).count();
    papers.truncate(10);
    Ok(Stats {
        paper_count,
        tag_count,
        category_count,
        pdf_count,
        recent_papers: papers,
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
}
