use chrono::NaiveDate;
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection, Row};

use crate::error::Result;
use crate::models::PaperDetails;
use crate::storage::db::{bool_from_sql, date_from_sql, list_from_sql};

// Both functions select `*` from the `papers` / `latest_papers` views (same
// column set), so one row->model mapper serves both. LIST/DATE/BOOL columns go
// through the storage::db decltype converters — no inline re-parsing.
fn row_to_paper(row: &Row) -> Result<PaperDetails> {
    // LIST column (JSON TEXT) -> Vec<String>; NULL -> empty (model default).
    let list = |name: &str| -> Result<Vec<String>> {
        match row.get::<_, Option<String>>(name)? {
            Some(s) => list_from_sql(&s),
            None => Ok(Vec::new()),
        }
    };
    // DATE column (ISO TEXT) -> NaiveDate; NULL -> None.
    let date = |name: &str| -> Result<Option<NaiveDate>> {
        match row.get::<_, Option<String>>(name)? {
            Some(s) => Ok(Some(date_from_sql(&s)?)),
            None => Ok(None),
        }
    };
    Ok(PaperDetails {
        paper_id: row.get("paper_id")?,
        source_id: row.get("source_id")?,
        version: row.get("version")?,
        title: row.get("title")?,
        summary: row.get("summary")?,
        published: date("published")?,
        updated: date("updated")?,
        url: row.get("url")?,
        doi: row.get("doi")?,
        category: row.get("category")?,
        categories: list("categories")?,
        journal_ref: row.get("journal_ref")?,
        comment: row.get("comment")?,
        authors: list("authors")?,
        tags: list("tags")?,
        has_pdf: bool_from_sql(row.get::<_, i64>("has_pdf")?),
        pdf_path: row.get("pdf_path")?,
        source: row.get("source")?,
        full_text: row.get("full_text")?,
        downloaded_source: bool_from_sql(row.get::<_, Option<i64>>("downloaded_source")?.unwrap_or(0)),
        source_fk: row.get("source_fk")?,
    })
}

/// `storage/db.py::get_paper` — a specific version, or the latest if `None`.
/// `conn` is an opened storage::db connection (FK PRAGMA already ON).
pub fn get_paper(conn: &Connection, source_id: &str, version: Option<i64>) -> Result<Option<PaperDetails>> {
    // Python `if version:` treats 0 as falsy too -> fall through to latest.
    let (sql, params): (&str, Vec<Value>) = match version.filter(|v| *v != 0) {
        Some(v) => (
            "SELECT * FROM papers WHERE source_id = ? AND version = ?",
            vec![Value::Text(source_id.to_string()), Value::Integer(v)],
        ),
        None => (
            "SELECT * FROM latest_papers WHERE source_id = ?",
            vec![Value::Text(source_id.to_string())],
        ),
    };
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params_from_iter(&params))?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_paper(row)?)),
        None => Ok(None),
    }
}

/// `storage/db.py::list_papers` — latest version per paper by default.
/// Optional exact-category filter; limit/offset apply to the filtered result.
pub fn list_papers(
    conn: &Connection,
    latest_only: bool,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
) -> Result<Vec<PaperDetails>> {
    let mut sql = if latest_only {
        "SELECT * FROM latest_papers".to_string()
    } else {
        "SELECT * FROM papers".to_string()
    };
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
    let mut rows = stmt.query(params_from_iter(&params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_paper(row)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};
    use rusqlite::params;

    fn seed(conn: &Connection) {
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:2204.12985')", [])
            .unwrap();
        let fk = conn.last_insert_rowid();
        for (ver, title, pub_date) in [(1, "V1", "2024-01-01"), (2, "V2", "2024-03-05")] {
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) \
                 VALUES ('arxiv:2204.12985', ?1, ?2, 'cs.LG', 1, ?3)",
                params![ver, title, fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID, URL, PUBLISHED, CATEGORIES, SUMMARY, AUTHORS, TAGS, DOI) \
                 VALUES (?1, 'http://x', ?2, '[\"cs.LG\",\"cs.AI\"]', 'sum', '[\"Alice\",\"Bob\"]', '[\"ml\"]', '10.1/x')",
                params![pid, pub_date],
            )
            .unwrap();
        }
    }

    #[test]
    fn get_paper_latest_and_specific_version() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn);

        // None -> latest version via latest_papers view.
        let latest = get_paper(&conn, "arxiv:2204.12985", None).unwrap().unwrap();
        assert_eq!(latest.version, 2);
        assert_eq!(latest.title, "V2");
        assert_eq!(latest.published, NaiveDate::from_ymd_opt(2024, 3, 5));
        assert_eq!(latest.authors, vec!["Alice".to_string(), "Bob".to_string()]);
        assert_eq!(latest.categories, vec!["cs.LG".to_string(), "cs.AI".to_string()]);
        assert_eq!(latest.tags, vec!["ml".to_string()]);
        assert!(latest.has_pdf);
        assert!(!latest.downloaded_source);
        assert_eq!(latest.source.as_deref(), Some("arxiv")); // PROVIDER default

        // Some(1) -> that exact version via papers view.
        let v1 = get_paper(&conn, "arxiv:2204.12985", Some(1)).unwrap().unwrap();
        assert_eq!(v1.version, 1);
        assert_eq!(v1.title, "V1");

        assert!(get_paper(&conn, "arxiv:nope", None).unwrap().is_none());
    }

    #[test]
    fn list_papers_latest_only_and_category_filter() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn);

        // Default: latest version only -> one row.
        let latest = list_papers(&conn, true, None, 0, None).unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].version, 2);

        // latest_only=false -> both versions.
        let all = list_papers(&conn, false, None, 0, None).unwrap();
        assert_eq!(all.len(), 2);

        // Category filter passes the seeded category, misses on a wrong one.
        assert_eq!(list_papers(&conn, true, None, 0, Some("cs.LG")).unwrap().len(), 1);
        assert_eq!(list_papers(&conn, true, None, 0, Some("nope")).unwrap().len(), 0);

        // limit/offset apply to the (all-versions) filtered result.
        assert_eq!(list_papers(&conn, false, Some(1), 1, None).unwrap().len(), 1);
    }
}
