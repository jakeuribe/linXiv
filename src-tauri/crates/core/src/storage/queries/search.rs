use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::Result;
use crate::models::PaperDetails;

/// `storage/db.py::search_full_text` — FTS5 over TeX source AND note content.
/// Returns the latest version of each matching paper, ranked by bm25 (lower =
/// better); a paper matched by both its full text and a note takes its best score.
///
/// FTS misnomer: `papers_fts.paper_id` holds the SOURCE_ID *string*, so that half
/// joins on `latest_papers.source_id = papers_fts.paper_id` (NOT the int PAPER_ID).
/// notes_fts carries SOURCE_FK, joined back through PAPER_ROOTS to the same SOURCE_ID.
/// init_db always creates both FTS tables, so Python's "table missing" guard is moot.
///
/// The two FTS tables have different schemas, so a MATCH query using
/// column-filter syntax valid on one (e.g. `full_text:foo`) throws on the
/// other. Each branch runs as its own prepared statement and a prepare/query
/// error from either is treated as "no matches from that branch" rather than
/// aborting the whole search.
pub fn search_full_text(conn: &Connection, query: &str, limit: i64) -> Result<Vec<PaperDetails>> {
    let limit = limit.clamp(0, 1000);
    let mut best: HashMap<String, f64> = HashMap::new();
    for (sid, score) in fts_matches(
        conn,
        "SELECT fts.paper_id, bm25(papers_fts) FROM papers_fts fts WHERE papers_fts MATCH ?1",
        query,
    )
    .into_iter()
    .chain(fts_matches(
        conn,
        "SELECT r.SOURCE_ID, bm25(notes_fts) FROM notes_fts \
         JOIN PAPER_ROOTS r ON r.SOURCE_FK = notes_fts.source_fk \
         WHERE notes_fts MATCH ?1 AND r.STATUS = 'active'",
        query,
    )) {
        best.entry(sid)
            .and_modify(|s| {
                if score < *s {
                    *s = score;
                }
            })
            .or_insert(score);
    }

    let mut scored: Vec<(String, f64)> = best.into_iter().collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit as usize);
    if scored.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT p.* FROM latest_papers p WHERE p.source_id IN ({})",
        vec!["?"; scored.len()].join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids: Vec<&str> = scored.iter().map(|(sid, _)| sid.as_str()).collect();
    // Manual loop (not query_map): row_to_paper returns CoreError on a bad
    // DATE/LIST decode, which query_map's rusqlite::Result closure can't carry.
    let mut rows = stmt.query(rusqlite::params_from_iter(ids))?;
    let mut by_source_id: HashMap<String, PaperDetails> = HashMap::new();
    while let Some(row) = rows.next()? {
        let details = super::paper::row_to_paper(row)?;
        by_source_id.insert(details.source_id.clone(), details);
    }
    Ok(scored
        .into_iter()
        .filter_map(|(sid, _)| by_source_id.remove(&sid))
        .collect())
}

/// Run one FTS MATCH query, returning (source_id, bm25 score) pairs. A
/// prepare or execute error (e.g. column-filter syntax invalid for this
/// table's schema) yields an empty result instead of propagating.
fn fts_matches(conn: &Connection, sql: &str, query: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(e) => {
            tracing::warn!("fts_matches prepare failed for {sql:?}: {e}");
            return out;
        }
    };
    let mut rows = match stmt.query((query,)) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("fts_matches query failed for {sql:?}: {e}");
            return out;
        }
    };
    while let Ok(Some(row)) = rows.next() {
        let (Ok(sid), Ok(score)) = (row.get::<_, String>(0), row.get::<_, f64>(1)) else {
            continue;
        };
        out.push((sid, score));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};
    use chrono::NaiveDate;

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

    #[test]
    fn matches_on_note_content() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(&conn, "arxiv:2204.12985", "some tex source");

        // A note whose distinctive term appears in no paper's full text; the
        // notes_fts AFTER-INSERT trigger indexes it, so the FTS path finds it.
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE) \
             SELECT SOURCE_FK, 'n', 'zephyranthes reminder' \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:2204.12985"],
        )
        .unwrap();

        let hits = search_full_text(&conn, "zephyranthes", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "arxiv:2204.12985");
    }

    #[test]
    fn dedupes_paper_matched_in_both_indexes() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(
            &conn,
            "arxiv:2204.12985",
            "the manifold hypothesis in latent space",
        );
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE) \
             SELECT SOURCE_FK, 'n', 'manifold reading notes' \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:2204.12985"],
        )
        .unwrap();

        let hits = search_full_text(&conn, "manifold", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "arxiv:2204.12985");
    }

    #[test]
    fn column_filter_syntax_valid_only_on_papers_fts_still_matches() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(
            &conn,
            "arxiv:2204.12985",
            "the manifold hypothesis in latent space",
        );

        // `full_text:` is a papers_fts column; the same MATCH string throws
        // "no such column" against notes_fts, which must not zero out the result.
        let hits = search_full_text(&conn, "full_text:manifold", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "arxiv:2204.12985");
    }

    #[test]
    fn notes_fts_stays_in_sync_on_update_and_delete() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(&conn, "arxiv:2204.12985", "some tex source");

        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, TITLE, NOTE) \
             SELECT SOURCE_FK, 'n', 'aardvark reminder' \
             FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
            ["arxiv:2204.12985"],
        )
        .unwrap();
        let note_sk: i64 = conn
            .query_row(
                "SELECT NOTE_SK FROM NOTE WHERE NOTE = 'aardvark reminder'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(search_full_text(&conn, "aardvark", 20).unwrap().len(), 1);

        conn.execute(
            "UPDATE NOTE SET NOTE = 'buffalo reminder' WHERE NOTE_SK = ?1",
            [note_sk],
        )
        .unwrap();
        assert_eq!(search_full_text(&conn, "aardvark", 20).unwrap().len(), 0);
        assert_eq!(search_full_text(&conn, "buffalo", 20).unwrap().len(), 1);

        conn.execute("DELETE FROM NOTE WHERE NOTE_SK = ?1", [note_sk])
            .unwrap();
        assert_eq!(search_full_text(&conn, "buffalo", 20).unwrap().len(), 0);
    }
}
