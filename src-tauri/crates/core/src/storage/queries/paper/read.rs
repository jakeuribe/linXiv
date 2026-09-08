//! Row mapping, the shared column list, sort policy, and the read-side
//! queries over the `papers` / `latest_papers` views.

use chrono::NaiveDate;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, Row};
use serde::Serialize;

use crate::error::Result;
use crate::models::{PaperDetails, NO_PUBLISHED_DATE};
use crate::storage::db::{bool_from_sql, date_from_sql, list_from_sql};

// Every read selects `PAPER_COLUMNS_NO_TEXT` from the `papers` /
// `latest_papers` views (same column set), so one row->model mapper serves all.
// LIST/DATE/BOOL columns go through the storage::db decltype converters — no
// inline re-parsing.
pub(in crate::storage::queries) fn row_to_paper(row: &Row) -> Result<PaperDetails> {
    // LIST column (JSON TEXT) -> Vec<String>; NULL -> empty (model default).
    let list = |name: &str| -> Result<Vec<String>> {
        row.get::<_, Option<String>>(name)?
            .map_or(Ok(Vec::new()), |s| list_from_sql(&s))
    };
    // DATE column (ISO TEXT) -> NaiveDate; NULL -> None.
    let date = |name: &str| -> Result<Option<NaiveDate>> {
        row.get::<_, Option<String>>(name)?
            .as_deref()
            .map(date_from_sql)
            .transpose()
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
        downloaded_source: bool_from_sql(
            row.get::<_, Option<i64>>("downloaded_source")?.unwrap_or(0),
        ),
        source_fk: row.get("source_fk")?,
    })
}

/// A specific version, or the latest if `None`. Reads the list column set
/// (FULL_TEXT blanked); `has_full_text` answers the one stored-body question.
pub fn get_paper(
    conn: &Connection,
    source_id: &str,
    version: Option<i64>,
) -> Result<Option<PaperDetails>> {
    // A version of 0 is treated as unset -> fall through to latest.
    let (sql, params): (String, Vec<Value>) = match version.filter(|v| *v != 0) {
        Some(v) => (
            format!(
                "SELECT {PAPER_COLUMNS_NO_TEXT} FROM papers WHERE source_id = ? AND version = ?"
            ),
            vec![Value::Text(source_id.to_string()), Value::Integer(v)],
        ),
        None => (
            format!("SELECT {PAPER_COLUMNS_NO_TEXT} FROM latest_papers WHERE source_id = ?"),
            vec![Value::Text(source_id.to_string())],
        ),
    };
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(&params))?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_paper(row)?)),
        None => Ok(None),
    }
}

/// One exact PAPER version by PK. Reads the list column set (FULL_TEXT
/// blanked) so the lookup never hauls a full TeX corpus row into memory.
pub fn get_paper_by_id(conn: &Connection, paper_id: i64) -> Result<Option<PaperDetails>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PAPER_COLUMNS_NO_TEXT} FROM papers WHERE paper_id = ?"
    ))?;
    let mut rows = stmt.query([paper_id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_paper(row)?)),
        None => Ok(None),
    }
}

/// The `papers`/`latest_papers` column list with FULL_TEXT blanked out — every
/// column `row_to_paper` reads, in the view's order. Every `PaperDetails` read
/// uses this instead of `SELECT *`: nothing consumes the body through the
/// struct, and `SELECT *` would haul a multi-MB TeX body per read.
pub const PAPER_COLUMNS_NO_TEXT: &str = "paper_id, source_id, source_fk, version, title, url, \
     published, updated, category, categories, doi, journal_ref, comment, summary, authors, tags, \
     has_pdf, source, pdf_path, NULL AS full_text, downloaded_source, created_at, updated_at";

/// What the library list is ordered by. Each arm's column is indexed (migration
/// 18), so the ORDER BY drives its scan off the index instead of sorting the
/// whole library in a temp b-tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaperSort {
    /// Publication date — the historical default.
    #[default]
    Published,
    /// When the paper entered the library — keyed on `source_fk` (AUTOINCREMENT
    /// = insertion order, no one-second ties), NOT the per-version `created_at`,
    /// which jumps on every new version. A re-added paper keeps its old position.
    Added,
    /// Title, case-insensitively.
    Title,
}

impl PaperSort {
    /// Wire key (`?sort=`); anything unrecognised falls back to the default.
    /// Parsing here is what keeps the ORDER BY free of caller-supplied text.
    pub fn from_key(key: &str) -> Self {
        match key {
            "added" => Self::Added,
            "title" => Self::Title,
            _ => Self::Published,
        }
    }

    /// The direction to use when the caller names no explicit one: newest first
    /// for dates, A–Z for titles.
    pub fn default_desc(self) -> bool {
        self != Self::Title
    }

    /// `paper_id` breaks ties so paging is stable. Oldest-first leads with an
    /// extra term so undated papers sink; `idx_paper_meta_published_dated`
    /// indexes that exact expression — change one and the other must follow.
    fn order_by(self, desc: bool) -> String {
        let dir = if desc { "DESC" } else { "ASC" };
        let col = match self {
            Self::Published => "published",
            Self::Added => "source_fk",
            Self::Title => "title COLLATE NOCASE",
        };
        let undated_last = match (self, desc) {
            (Self::Published, false) => format!("(published > '{NO_PUBLISHED_DATE}') DESC, "),
            _ => String::new(),
        };
        format!(" ORDER BY {undated_last}{col} {dir}, paper_id {dir}")
    }
}

/// SQL + bind params for the list-papers filter/order/pagination.
fn list_papers_sql(
    latest_only: bool,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
    project: Option<i64>,
    sort: PaperSort,
    desc: bool,
) -> (String, Vec<Value>) {
    let view = if latest_only {
        "latest_papers"
    } else {
        "papers"
    };
    let mut sql = format!("SELECT {PAPER_COLUMNS_NO_TEXT} FROM {view}");
    let mut params: Vec<Value> = Vec::new();
    let mut wheres: Vec<&str> = Vec::new();
    if let Some(cat) = category {
        wheres.push("category = ?");
        params.push(Value::Text(cat.to_string()));
    }
    if let Some(pid) = project {
        wheres.push("source_fk IN (SELECT SOURCE_FK FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?)");
        params.push(Value::Integer(pid));
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    sql.push_str(&sort.order_by(desc));
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
    (sql, params)
}

/// Latest version per paper by default.
/// Optional exact-category filter; limit/offset apply to the filtered result.
pub fn list_papers(
    conn: &Connection,
    latest_only: bool,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
) -> Result<Vec<PaperDetails>> {
    list_papers_sorted(
        conn,
        latest_only,
        limit,
        offset,
        category,
        None,
        PaperSort::default(),
        true,
    )
}

/// `list_papers` under a caller-chosen ordering. `project` narrows to papers
/// linked to that project (PROJECT_TO_PAPER), filtered in SQL so a >200-paper
/// library never needs a client-side window.
pub fn list_papers_sorted(
    conn: &Connection,
    latest_only: bool,
    limit: Option<i64>,
    offset: i64,
    category: Option<&str>,
    project: Option<i64>,
    sort: PaperSort,
    desc: bool,
) -> Result<Vec<PaperDetails>> {
    let (sql, params) = list_papers_sql(latest_only, limit, offset, category, project, sort, desc);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(&params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_paper(row)?);
    }
    Ok(out)
}

/// Latest-version papers whose PDF flag is set — backs `GET /api/pdfs`. Filters
/// in SQL so the whole library is never materialized to find the PDF subset.
pub fn list_pdf_papers(conn: &Connection) -> Result<Vec<PaperDetails>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PAPER_COLUMNS_NO_TEXT} FROM latest_papers WHERE has_pdf = 1"
    ))?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_paper(row)?);
    }
    Ok(out)
}

/// Latest-version rows for the given roots, filtered in SQL so export/share
/// never materializes the whole library. Chunked under SQLite's bound-variable
/// limit, then sorted in Rust to the `list_papers` default order (chunking
/// would scramble a SQL ORDER BY).
pub fn get_papers_by_source_fks(
    conn: &Connection,
    source_fks: &[i64],
) -> Result<Vec<PaperDetails>> {
    let mut out = Vec::new();
    for chunk in source_fks.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT {PAPER_COLUMNS_NO_TEXT} FROM latest_papers \
             WHERE source_fk IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(chunk.iter()))?;
        while let Some(row) = rows.next()? {
            out.push(row_to_paper(row)?);
        }
    }
    // Option<NaiveDate> reversed = DESC with None (undated) last, like SQL DESC.
    out.sort_by(|a, b| {
        b.published
            .cmp(&a.published)
            .then(b.paper_id.cmp(&a.paper_id))
    });
    Ok(out)
}

/// Distinct primary categories across latest active papers, NULLs excluded,
/// ascending in BINARY (byte-order) collation.
pub fn get_categories(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT category FROM latest_papers \
         WHERE category IS NOT NULL ORDER BY category",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Latest papers whose JSON TAGS list holds `label`, matched whole and
/// case-insensitively (NOCASE folds ASCII only). Matches the JSON column, not
/// PAPER_TO_TAG — the relational half skips empty labels and case-folds into a
/// shared TAG row. `published DESC` (NULL dates last), `paper_id DESC` ties.
pub fn get_papers_by_json_tag(conn: &Connection, label: &str) -> Result<Vec<PaperDetails>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PAPER_COLUMNS_NO_TEXT} FROM latest_papers WHERE tags IS NOT NULL \
         AND EXISTS (SELECT 1 FROM json_each(latest_papers.tags) \
                     WHERE json_each.value = ? COLLATE NOCASE) \
         ORDER BY published DESC, paper_id DESC"
    ))?;
    let mut rows = stmt.query([label])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_paper(row)?);
    }
    Ok(out)
}

/// Which of `source_ids` are stored and active. Ids are namespaced
/// (`arxiv:2204.12985`); unknown ids are simply absent from the result.
/// Chunked to stay under SQLite's bound-variable limit; the seen-set keeps
/// the output duplicate-free when input duplicates span chunks.
pub fn existing_source_ids(conn: &Connection, source_ids: &[String]) -> Result<Vec<String>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for chunk in source_ids.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT DISTINCT source_id FROM papers WHERE source_id IN ({placeholders})"
        ))?;
        let rows = stmt.query_map(params_from_iter(chunk), |r| r.get::<_, String>(0))?;
        for row in rows {
            let sid = row?;
            if seen.insert(sid.clone()) {
                out.push(sid);
            }
        }
    }
    Ok(out)
}

/// `get_all_versions` — every stored (active) version, oldest-first.
pub fn get_all_versions(conn: &Connection, source_id: &str) -> Result<Vec<PaperDetails>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PAPER_COLUMNS_NO_TEXT} FROM papers WHERE source_id = ? ORDER BY version ASC"
    ))?;
    let mut rows = stmt.query([source_id])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row_to_paper(row)?);
    }
    Ok(out)
}

/// One `version_meta` row: the four scalars the versions listing needs.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct PaperVersionMeta {
    pub version: i64,
    pub published: Option<NaiveDate>,
    pub updated: Option<NaiveDate>,
    pub has_pdf: bool,
}

/// `version_meta` — [`get_all_versions`] minus the per-version full-row
/// hydration: every stored (active) version's four listing scalars, oldest-first.
pub fn version_meta(conn: &Connection, source_id: &str) -> Result<Vec<PaperVersionMeta>> {
    let mut stmt = conn.prepare(
        "SELECT version, published, updated, has_pdf FROM papers \
         WHERE source_id = ? ORDER BY version ASC",
    )?;
    let rows = stmt.query_map([source_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (version, published, updated, has_pdf) = row?;
        out.push(PaperVersionMeta {
            version,
            published: published.as_deref().map(date_from_sql).transpose()?,
            updated: updated.as_deref().map(date_from_sql).transpose()?,
            has_pdf: bool_from_sql(has_pdf),
        });
    }
    Ok(out)
}

/// Another paper root sharing this root's DOI — same underlying work resolved
/// independently by a different source (e.g. arXiv vs OpenAlex/Crossref).
/// Local struct (no model; models.rs out of scope this phase).
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct DoiVersionCandidate {
    pub source_fk: i64,
    pub source_id: String,
    pub title: String,
    pub source: Option<String>,
    pub published: Option<NaiveDate>,
    pub doi: String,
}

/// Other active paper roots whose latest version shares `source_fk`'s DOI —
/// likely the same work under a different source, for a "these look like the
/// same paper" suggestion. Empty if this root has no DOI (NULL/'' never
/// matches; mirrors `orcid_merge_candidates`'s self-join shape). Case-insensitive:
/// sources normalize DOI casing differently (e.g. OpenAlex lowercases).
// ponytail: unindexed self-join scan; fine at desktop-library scale, add a
// PAPER_META(DOI) index if this ever profiles hot.
pub fn find_doi_version_candidates(
    conn: &Connection,
    source_fk: i64,
) -> Result<Vec<DoiVersionCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT b.source_fk, b.source_id, b.title, b.source, b.published, b.doi \
         FROM latest_papers b, latest_papers a \
         WHERE a.source_fk = ? AND b.source_fk != a.source_fk \
           AND a.doi IS NOT NULL AND a.doi != '' AND b.doi = a.doi COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![source_fk], |r| {
        let published: Option<String> = r.get("published")?;
        Ok((
            r.get::<_, i64>("source_fk")?,
            r.get::<_, String>("source_id")?,
            r.get::<_, String>("title")?,
            r.get::<_, Option<String>>("source")?,
            published,
            r.get::<_, String>("doi")?,
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|(source_fk, source_id, title, source, published, doi)| {
            Ok(DoiVersionCandidate {
                source_fk,
                source_id,
                title,
                source,
                published: published.as_deref().map(date_from_sql).transpose()?,
                doi,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{meta, seed};
    use super::super::*;
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};
    use rusqlite::params;

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
        assert_eq!(
            latest.categories,
            vec!["cs.LG".to_string(), "cs.AI".to_string()]
        );
        assert_eq!(latest.tags, vec!["ml".to_string()]);
        assert!(latest.has_pdf);
        assert!(!latest.downloaded_source);
        assert_eq!(latest.source.as_deref(), Some("arxiv")); // PROVIDER default

        // Some(1) -> that exact version via papers view.
        let v1 = get_paper(&conn, "arxiv:2204.12985", Some(1))
            .unwrap()
            .unwrap();
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
        assert_eq!(
            list_papers(&conn, true, None, 0, Some("cs.LG"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            list_papers(&conn, true, None, 0, Some("nope"))
                .unwrap()
                .len(),
            0
        );

        // limit/offset apply to the (all-versions) filtered result.
        assert_eq!(
            list_papers(&conn, false, Some(1), 1, None).unwrap().len(),
            1
        );
    }

    /// The `project` filter narrows to PROJECT_TO_PAPER members in SQL —
    /// membership, not a client-side window over the first N rows.
    #[test]
    fn list_papers_project_filter_returns_only_members() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn); // arxiv:2204.12985
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:outside')",
            [],
        )
        .unwrap();
        let outside_fk = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) \
             VALUES ('arxiv:outside', 1, 'Outside', ?1)",
            [outside_fk],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID) VALUES (?1)",
            [conn.last_insert_rowid()],
        )
        .unwrap();

        conn.execute("INSERT INTO PROJECT (NAME) VALUES ('RL')", [])
            .unwrap();
        let project_fk = conn.last_insert_rowid();
        let member_fk: i64 = conn
            .query_row(
                "SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:2204.12985'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK) VALUES (?1, ?2)",
            params![project_fk, member_fk],
        )
        .unwrap();

        let member = list_papers_sorted(
            &conn,
            true,
            None,
            0,
            None,
            Some(project_fk),
            PaperSort::Published,
            true,
        )
        .unwrap();
        assert_eq!(member.len(), 1);
        assert_eq!(member[0].source_id, "arxiv:2204.12985");
        assert_eq!(member[0].version, 2); // latest_only still applies

        // Unknown project matches nothing; no filter returns both papers.
        assert!(list_papers_sorted(
            &conn,
            true,
            None,
            0,
            None,
            Some(9999),
            PaperSort::Published,
            true
        )
        .unwrap()
        .is_empty());
        assert_eq!(list_papers(&conn, true, None, 0, None).unwrap().len(), 2);

        // Composes with the category filter (WHERE ... AND ...): the member
        // paper is cs.LG, so a mismatched category empties the result.
        assert!(list_papers_sorted(
            &conn,
            true,
            None,
            0,
            Some("nope"),
            Some(project_fk),
            PaperSort::Published,
            true
        )
        .unwrap()
        .is_empty());
    }

    /// Pins `list_pdf_papers` ≡ `list_papers(latest_only)` filtered on has_pdf —
    /// the SQL-side filter must not change what the old scan-then-filter saw.
    #[test]
    fn list_pdf_papers_matches_filtered_full_list() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn); // arxiv:2204.12985, HAS_PDF=1, latest version 2
        for (sid, has_pdf) in [("arxiv:nopdf", 0), ("arxiv:withpdf", 1)] {
            conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?1)", [sid])
                .unwrap();
            let fk = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, HAS_PDF, SOURCE_FK) \
                 VALUES (?1, 1, ?1, ?2, ?3)",
                params![sid, has_pdf, fk],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID) VALUES (?1)",
                [conn.last_insert_rowid()],
            )
            .unwrap();
        }

        let expected: Vec<(String, i64)> = list_papers(&conn, true, None, 0, None)
            .unwrap()
            .into_iter()
            .filter(|p| p.has_pdf)
            .map(|p| (p.source_id, p.version))
            .collect();
        let mut got: Vec<(String, i64)> = list_pdf_papers(&conn)
            .unwrap()
            .into_iter()
            .map(|p| (p.source_id, p.version))
            .collect();
        // The new query carries no ORDER BY (its one caller re-sorts by file
        // size); compare as sets.
        got.sort();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(got, expected_sorted);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|(sid, _)| sid != "arxiv:nopdf"));
    }

    #[test]
    fn list_papers_sorted_orders_by_the_requested_metric() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        // Insertion order (= add order) matches neither the publication nor the
        // title order. Titles differ in case so a binary-collation ORDER BY
        // (Banana, apple, cherry) would fail.
        for (sid, title, published) in [
            ("arxiv:b", "Banana", "2024-01-01"),
            ("arxiv:a", "apple", "2024-06-01"),
            ("arxiv:c", "cherry", "2023-01-01"),
        ] {
            conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?1)", [sid])
                .unwrap();
            let fk = conn.last_insert_rowid();
            // Every VERSION row is stamped the same, later date: `added` must come
            // from the root, so p.CREATED_AT can't stand in for it.
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK, CREATED_AT) \
                 VALUES (?1, 1, ?2, ?3, '2026-01-01 00:00:00')",
                params![sid, title, fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?1, ?2)",
                params![pid, published],
            )
            .unwrap();
        }

        // The oldest-added paper gains a v2 today. Ordering by the version row's
        // timestamp would make it the "most recently added" paper.
        let apple_fk: i64 = conn
            .query_row(
                "SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK, CREATED_AT) \
             VALUES ('arxiv:a', 2, 'apple', ?1, '2026-06-01 00:00:00')",
            params![apple_fk],
        )
        .unwrap();
        let apple_v2 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?1, '2024-06-01')",
            params![apple_v2],
        )
        .unwrap();

        // An undated paper: stored as the 0001-01-01 sentinel, it must sink under
        // BOTH publication orders rather than heading "oldest first".
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:d')", [])
            .unwrap();
        let undated_fk = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK, CREATED_AT) \
             VALUES ('arxiv:d', 1, 'durian', ?1, '2026-01-01 00:00:00')",
            params![undated_fk],
        )
        .unwrap();
        let undated_pid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?1, '0001-01-01')",
            params![undated_pid],
        )
        .unwrap();

        let titles = |sort, desc| {
            list_papers_sorted(&conn, true, None, 0, None, None, sort, desc)
                .unwrap()
                .into_iter()
                .map(|p| p.title)
                .collect::<Vec<_>>()
        };

        // "durian" is undated: last under both publication orders.
        assert_eq!(
            titles(PaperSort::Published, true),
            ["apple", "Banana", "cherry", "durian"]
        );
        assert_eq!(
            titles(PaperSort::Published, false),
            ["cherry", "Banana", "apple", "durian"]
        );
        // Add order, not the v2 stored for "apple" in 2026 (which is the newest
        // PAPER row of all and would otherwise head "recently added").
        assert_eq!(
            titles(PaperSort::Added, true),
            ["durian", "cherry", "apple", "Banana"]
        );
        assert_eq!(
            titles(PaperSort::Added, false),
            ["Banana", "apple", "cherry", "durian"]
        );
        assert_eq!(
            titles(PaperSort::Title, false),
            ["apple", "Banana", "cherry", "durian"]
        );
        assert_eq!(
            titles(PaperSort::Title, true),
            ["durian", "cherry", "Banana", "apple"]
        );

        // Unknown wire keys can't reach the ORDER BY: they resolve to the default.
        assert_eq!(PaperSort::from_key("title; DROP"), PaperSort::Published);
        assert_eq!(PaperSort::from_key("added"), PaperSort::Added);
        assert!(PaperSort::Added.default_desc());
        assert!(!PaperSort::Title.default_desc());
    }

    /// Being indexed doesn't mean SQLite picks the index — a mismatched collation
    /// or an expression in the leading ORDER BY term silently drops it back to a
    /// full temp-b-tree sort of the whole library, which is exactly what these
    /// migration-18 indexes exist to prevent.
    #[test]
    fn list_papers_sorted_orderings_use_their_index() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let plan = |sort, desc| -> Vec<String> {
            let (sql, _) = list_papers_sql(true, None, 0, None, None, sort, desc);
            conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let uses = |sort, desc, index: &str| {
            let steps = plan(sort, desc);
            assert!(
                steps.iter().any(|s| s.contains(index)),
                "{sort:?} desc={desc} must scan {index}, got plan: {steps:?}"
            );
            // The tiebreak term may need a temp b-tree; the metric itself must not.
            assert!(
                !steps.iter().any(|s| s == "USE TEMP B-TREE FOR ORDER BY"),
                "{sort:?} desc={desc} sorted the whole library, plan: {steps:?}"
            );
        };

        uses(PaperSort::Published, true, "idx_paper_meta_published");
        // Oldest-first leads with the undated-sinking expression, so it needs the
        // expression index rather than the plain one.
        uses(
            PaperSort::Published,
            false,
            "idx_paper_meta_published_dated",
        );
        uses(PaperSort::Added, true, "idx_paper_source_fk");
        uses(PaperSort::Added, false, "idx_paper_source_fk");
        uses(PaperSort::Title, false, "idx_paper_title_nocase");
        uses(PaperSort::Title, true, "idx_paper_title_nocase");
    }

    #[test]
    fn find_doi_version_candidates_matches_same_doi_only() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let mut arxiv_meta = meta("arxiv:2204.12985", 1);
        arxiv_meta.doi = Some("10.1234/shared".into());
        arxiv_meta.source = Some("arxiv".into());
        save_paper_metadata(&mut conn, &arxiv_meta, None).unwrap();

        // Different casing than the arXiv record's DOI (sources normalize
        // differently, e.g. OpenAlex lowercases) — must still match.
        let mut openalex_meta = meta("openalex:W123", 1);
        openalex_meta.doi = Some("10.1234/SHARED".into());
        openalex_meta.source = Some("openalex".into());
        save_paper_metadata(&mut conn, &openalex_meta, None).unwrap();

        let mut no_doi_meta = meta("arxiv:9999.00001", 1);
        no_doi_meta.doi = None;
        save_paper_metadata(&mut conn, &no_doi_meta, None).unwrap();

        let arxiv_fk = ensure_paper_root(&mut conn, "arxiv:2204.12985").unwrap();
        let openalex_fk = ensure_paper_root(&mut conn, "openalex:W123").unwrap();
        let no_doi_fk = ensure_paper_root(&mut conn, "arxiv:9999.00001").unwrap();

        let candidates = find_doi_version_candidates(&conn, arxiv_fk).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].source_fk, openalex_fk);
        assert_eq!(candidates[0].source_id, "openalex:W123");
        assert_eq!(candidates[0].doi, "10.1234/SHARED");

        // Symmetric: the other root sees this one as a candidate too.
        let back = find_doi_version_candidates(&conn, openalex_fk).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].source_fk, arxiv_fk);

        // No DOI -> no candidates (NULL never equals NULL in SQL).
        assert!(find_doi_version_candidates(&conn, no_doi_fk)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn existing_source_ids_reports_only_active_stored_papers() {
        let mut conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:2204.12985", 1), None).unwrap();
        save_paper_metadata(&mut conn, &meta("arxiv:2204.12985", 2), None).unwrap();
        save_paper_metadata(&mut conn, &meta("openalex:W123", 1), None).unwrap();
        soft_delete_paper(&mut conn, "openalex:W123").unwrap();

        let found = existing_source_ids(
            &conn,
            &[
                "arxiv:2204.12985".into(), // stored, two versions -> reported once
                "openalex:W123".into(),    // trashed -> absent
                "arxiv:1111.00001".into(), // never saved -> absent
            ],
        )
        .unwrap();
        assert_eq!(found, vec!["arxiv:2204.12985".to_string()]);
        assert!(existing_source_ids(&conn, &[]).unwrap().is_empty());

        // Over the 900-per-chunk bound: the stored id in the last chunk is
        // still found, and a duplicate spanning chunks reports only once.
        let mut many: Vec<String> = (0..1000).map(|i| format!("arxiv:absent{i}")).collect();
        many[0] = "arxiv:2204.12985".into();
        many.push("arxiv:2204.12985".into());
        assert_eq!(
            existing_source_ids(&conn, &many).unwrap(),
            vec!["arxiv:2204.12985".to_string()]
        );
    }
}
