use chrono::Utc;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use crate::error::Result;
use crate::models::NoteDetails;
use crate::storage::db::{timestamp_from_sql, timestamp_to_sql};

/// Column list matching `Note.from_row` — the named SELECT for every note reader
/// (Python's `_fetch_*`/`get_notes` use `SELECT *`; we spell it out so the
/// decltype converters line up with the model).
const NOTE_COLS: &str =
    "NOTE_SK, NOTE_UUID, SOURCE_FK, PAPER_ID_FK, PROJECT_FK, TITLE, NOTE, MEDIA_TIME_MS, MEDIA_ITEM_ID, CREATED_AT, UPDATED_AT";

/// `Note.from_row().to_details()` — TITLE nullable, NOTE is a BLOB holding text,
/// both coalesce to "" (Python `or ""`); TIMESTAMP cols are NOT NULL.
fn note_from_row(row: &Row) -> rusqlite::Result<NoteDetails> {
    let created: String = row.get("CREATED_AT")?;
    let updated: String = row.get("UPDATED_AT")?;
    Ok(NoteDetails {
        note_id: row.get("NOTE_SK")?,
        uuid: row
            .get::<_, Option<String>>("NOTE_UUID")?
            .unwrap_or_default(),
        source_fk: row.get("SOURCE_FK")?,
        paper_id_fk: row.get::<_, Option<i64>>("PAPER_ID_FK")?,
        project_id: row.get::<_, Option<i64>>("PROJECT_FK")?,
        title: row.get::<_, Option<String>>("TITLE")?.unwrap_or_default(),
        content: row.get::<_, Option<String>>("NOTE")?.unwrap_or_default(),
        media_time_ms: row.get("MEDIA_TIME_MS")?,
        media_item_id: row.get("MEDIA_ITEM_ID")?,
        created_at: Some(
            timestamp_from_sql(&created)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
        ),
        updated_at: Some(
            timestamp_from_sql(&updated)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
        ),
    })
}

/// `storage/notes.py::get_notes` — notes for a paper, 3-way scoped:
///   * `all_projects` true   → SOURCE_FK only (every note: library + all projects)
///   * `project_id` Some(id)  → `PROJECT_FK = id`
///   * `project_id` None      → `PROJECT_FK IS NULL` (library notes only)
///
/// `all_projects` is a DISTINCT branch, not the same as `project_id = None` — MCP
/// `get_notes_for_paper` passes `all_projects = project_id.is_none()`, the opposite
/// of the None branch. Ordered CREATED_AT ASC.
pub fn get_notes(
    conn: &Connection,
    source_fk: i64,
    project_id: Option<i64>,
    all_projects: bool,
) -> Result<Vec<NoteDetails>> {
    // NON-NEGOTIABLE: conn comes from storage::db::open (FK PRAGMA on); no raw Connection.
    let project_clause = if all_projects {
        "1 = 1"
    } else if project_id.is_some() {
        "PROJECT_FK = ?2"
    } else {
        "PROJECT_FK IS NULL"
    };
    let mut params: Vec<i64> = vec![source_fk];
    if !all_projects {
        if let Some(id) = project_id {
            params.push(id);
        }
    }
    query_notes(
        conn,
        &format!(
            "SELECT {NOTE_COLS} FROM NOTE WHERE SOURCE_FK = ?1 AND {project_clause} \
             ORDER BY CREATED_AT ASC"
        ),
        &params,
    )
}

/// `storage/notes.py::get_note` — single note by NOTE_SK, None if absent.
pub fn get_note(conn: &Connection, note_id: i64) -> Result<Option<NoteDetails>> {
    Ok(conn
        .query_row(
            &format!("SELECT {NOTE_COLS} FROM NOTE WHERE NOTE_SK = ?1"),
            [note_id],
            note_from_row,
        )
        .optional()?)
}

/// `storage/notes.py::create_note` — INSERT a note, returns the new NOTE_SK.
/// CREATED_AT/UPDATED_AT both stamped now (Python `datetime.now(utc)`).
/// `uuid` None generates a fresh v4; Some preserves an imported identity.
pub fn create_note(
    conn: &Connection,
    source_fk: i64,
    paper_id_fk: Option<i64>,
    project_id: Option<i64>,
    title: &str,
    content: &str,
    media_time_ms: Option<i64>,
    media_item_id: Option<&str>,
    uuid: Option<&str>,
) -> Result<i64> {
    let now = timestamp_to_sql(Utc::now().naive_utc());
    let uuid = uuid
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    conn.execute(
        "INSERT INTO NOTE (SOURCE_FK, PAPER_ID_FK, PROJECT_FK, TITLE, NOTE, MEDIA_TIME_MS, MEDIA_ITEM_ID, NOTE_UUID, CREATED_AT, UPDATED_AT) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![source_fk, paper_id_fk, project_id, title, content, media_time_ms, media_item_id, uuid, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// `storage/notes.py::patch_note` — partial update via COALESCE: a None arg
/// leaves the column unchanged. Returns false if no row matched NOTE_SK.
pub fn patch_note(
    conn: &Connection,
    note_id: i64,
    title: Option<&str>,
    content: Option<&str>,
) -> Result<bool> {
    let now = timestamp_to_sql(Utc::now().naive_utc());
    let n = conn.execute(
        "UPDATE NOTE SET TITLE = COALESCE(?1, TITLE), NOTE = COALESCE(?2, NOTE), \
         UPDATED_AT = ?3 WHERE NOTE_SK = ?4",
        params![title, content, now, note_id],
    )?;
    Ok(n > 0)
}

/// `storage/notes.py::delete_note` — hard-delete one note row; false if absent.
pub fn delete_note(conn: &Connection, note_id: i64) -> Result<bool> {
    let n = conn.execute("DELETE FROM NOTE WHERE NOTE_SK = ?1", [note_id])?;
    Ok(n > 0)
}

/// `storage/notes.py::count_paper_notes` (`config.queries.count_notes`) — count
/// notes on a paper, optionally narrowed to a project.
pub fn count_notes(conn: &Connection, source_fk: i64, project_id: Option<i64>) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM NOTE WHERE SOURCE_FK = ?1 AND (?2 IS NULL OR PROJECT_FK = ?2)",
        params![source_fk, project_id],
        |r| r.get(0),
    )?)
}

/// `storage/notes.py::count_project_notes` — total notes scoped to a project.
pub fn count_project_notes(conn: &Connection, project_id: i64) -> Result<i64> {
    let n = conn.query_row(
        "SELECT COUNT(*) FROM NOTE WHERE PROJECT_FK = ?1",
        [project_id],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Check if a NOTE_UUID is already taken (exists in the database).
pub fn uuid_taken(conn: &Connection, uuid: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM NOTE WHERE NOTE_UUID = ?1",
            [uuid],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Map a `SELECT {NOTE_COLS}` statement (with its params) to NoteDetails rows.
fn query_notes(conn: &Connection, sql: &str, params: &[i64]) -> Result<Vec<NoteDetails>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter().copied()), note_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `storage/notes.py::list_all_notes` — every note, CREATED_AT ASC.
pub fn list_all_notes(conn: &Connection) -> Result<Vec<NoteDetails>> {
    query_notes(
        conn,
        &format!("SELECT {NOTE_COLS} FROM NOTE ORDER BY CREATED_AT ASC"),
        &[],
    )
}

/// `storage/notes.py::get_notes_by_paper_id` — notes pinned to a specific paper
/// version (PAPER_ID_FK), CREATED_AT ASC.
pub fn get_notes_by_paper_id(conn: &Connection, paper_id: i64) -> Result<Vec<NoteDetails>> {
    query_notes(
        conn,
        &format!("SELECT {NOTE_COLS} FROM NOTE WHERE PAPER_ID_FK = ?1 ORDER BY CREATED_AT ASC"),
        &[paper_id],
    )
}

/// `storage/notes.py::get_project_notes` — all notes in a project,
/// SOURCE_FK ASC then CREATED_AT ASC.
pub fn get_project_notes(conn: &Connection, project_id: i64) -> Result<Vec<NoteDetails>> {
    query_notes(
        conn,
        &format!(
            "SELECT {NOTE_COLS} FROM NOTE WHERE PROJECT_FK = ?1 \
             ORDER BY SOURCE_FK ASC, CREATED_AT ASC"
        ),
        &[project_id],
    )
}

/// `storage/notes.py::note_counts_by_paper_for_project` — (SOURCE_FK, count) for
/// each active paper in the project; papers with no notes get 0. Returns a Vec in
/// PROJECT_TO_PAPER_FK order (Python returns an ordered dict).
pub fn note_counts_by_paper_for_project(
    conn: &Connection,
    project_id: i64,
) -> Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT pp.SOURCE_FK AS source_fk, COALESCE(n.cnt, 0) AS note_count \
         FROM PROJECT_TO_PAPER pp \
         JOIN PAPER_ROOTS r ON r.SOURCE_FK = pp.SOURCE_FK \
         LEFT JOIN ( \
             SELECT SOURCE_FK, COUNT(*) AS cnt FROM NOTE \
             WHERE PROJECT_FK = ?1 GROUP BY SOURCE_FK \
         ) AS n ON n.SOURCE_FK = pp.SOURCE_FK \
         WHERE pp.PROJECT_FK = ?1 AND r.STATUS = 'active' \
         ORDER BY pp.PROJECT_TO_PAPER_FK",
    )?;
    let rows = stmt.query_map([project_id], |r| {
        Ok((
            r.get::<_, i64>("source_fk")?,
            r.get::<_, i64>("note_count")?,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Escape LIKE wildcards so a literal query matches literally (`\` is the ESCAPE
/// char). Mirrors `storage/notes.py::_escape_like`.
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// `storage/notes.py::search_notes_source_fks` — distinct SOURCE_FKs of active
/// papers whose note title or body contains `query`, most-recently-updated first.
pub fn search_notes_source_fks(conn: &Connection, query: &str, limit: i64) -> Result<Vec<i64>> {
    let pattern = format!("%{}%", escape_like(query));
    let mut stmt = conn.prepare(
        "SELECT n.SOURCE_FK FROM NOTE n \
         JOIN PAPER_ROOTS r ON r.SOURCE_FK = n.SOURCE_FK \
         WHERE r.STATUS = 'active' \
           AND (n.TITLE LIKE ?1 ESCAPE '\\' OR n.NOTE LIKE ?1 ESCAPE '\\') \
         GROUP BY n.SOURCE_FK \
         ORDER BY MAX(n.UPDATED_AT) DESC \
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::{open_in_memory, timestamp_to_sql};
    use crate::storage::init_db;
    use chrono::NaiveDate;

    #[test]
    fn get_notes_scopes_by_project_and_maps_fields() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Parents required by NOTE's FKs (FK PRAGMA is ON).
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:2204.12985')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (10, 'P')",
            [],
        )
        .unwrap();

        let ts = timestamp_to_sql(
            NaiveDate::from_ymd_opt(2024, 3, 5)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        );
        // Library note (PROJECT_FK NULL) + project-scoped note on the same paper.
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT) \
             VALUES (1, NULL, 'lib', 'library note', ?1, ?1)",
            [&ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT) \
             VALUES (1, 10, 'proj', 'project note', ?1, ?1)",
            [&ts],
        )
        .unwrap();

        // Unscoped: only the PROJECT_FK IS NULL note.
        let lib = get_notes(&conn, 1, None, false).unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].title, "lib");
        assert_eq!(lib[0].project_id, None);

        // all_projects: both the library and the project note (SOURCE_FK only).
        let all = get_notes(&conn, 1, None, true).unwrap();
        assert_eq!(all.len(), 2);

        // Project-scoped: only the matching project's note, fields mapped through.
        let proj = get_notes(&conn, 1, Some(10), false).unwrap();
        assert_eq!(proj.len(), 1);
        let n = &proj[0];
        assert_eq!(n.source_fk, 1);
        assert_eq!(n.project_id, Some(10));
        assert_eq!(n.title, "proj");
        assert_eq!(n.content, "project note");
        assert!(n.note_id > 0);
        assert_eq!(
            n.created_at,
            Some(
                NaiveDate::from_ymd_opt(2024, 3, 5)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap()
            )
        );
    }

    /// Seed two papers (active + one extra) and a project, return their SOURCE_FKs.
    fn seed(conn: &Connection) -> (i64, i64, i64) {
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1'), (2, 'arxiv:2')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (10, 'P')",
            [],
        )
        .unwrap();
        (1, 2, 10)
    }

    #[test]
    fn create_get_patch_delete_roundtrip() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (src, _, proj) = seed(&conn);

        let id = create_note(
            &conn,
            src,
            None,
            Some(proj),
            "t1",
            "body1",
            None,
            None,
            None,
        )
        .unwrap();
        let got = get_note(&conn, id).unwrap().unwrap();
        assert_eq!(got.title, "t1");
        assert_eq!(got.content, "body1");
        assert_eq!(got.project_id, Some(proj));
        assert!(got.created_at.is_some());

        // patch: None leaves column unchanged; Some replaces it.
        assert!(patch_note(&conn, id, None, Some("body2")).unwrap());
        let got = get_note(&conn, id).unwrap().unwrap();
        assert_eq!(got.title, "t1"); // unchanged
        assert_eq!(got.content, "body2");

        assert!(patch_note(&conn, id, Some("t2"), None).unwrap());
        assert_eq!(get_note(&conn, id).unwrap().unwrap().title, "t2");

        // patch/delete of an absent row report false.
        assert!(!patch_note(&conn, 999, Some("x"), None).unwrap());
        assert!(!delete_note(&conn, 999).unwrap());

        // delete actually removes the row.
        assert!(delete_note(&conn, id).unwrap());
        assert!(get_note(&conn, id).unwrap().is_none());
        assert!(!delete_note(&conn, id).unwrap());
    }

    #[test]
    fn counts_lists_and_paper_pinning() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (src, src2, proj) = seed(&conn);
        // a PAPER row so PAPER_ID_FK has a valid referent.
        conn.execute(
            "INSERT INTO PAPER (PAPER_ID, SOURCE_FK, SOURCE_ID, VERSION, TITLE) \
             VALUES (100, 1, 'arxiv:1', 1, 'T')",
            [],
        )
        .unwrap();

        create_note(&conn, src, None, Some(proj), "a", "x", None, None, None).unwrap();
        create_note(&conn, src, Some(100), None, "b", "y", None, None, None).unwrap();
        create_note(&conn, src2, None, Some(proj), "c", "z", None, None, None).unwrap();

        // count_notes: by paper, and narrowed to a project.
        assert_eq!(count_notes(&conn, src, None).unwrap(), 2);
        assert_eq!(count_notes(&conn, src, Some(proj)).unwrap(), 1);
        assert_eq!(count_project_notes(&conn, proj).unwrap(), 2);

        assert_eq!(list_all_notes(&conn).unwrap().len(), 3);

        let pinned = get_notes_by_paper_id(&conn, 100).unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].title, "b");

        // project notes ordered SOURCE_FK ASC then CREATED_AT ASC.
        let pn = get_project_notes(&conn, proj).unwrap();
        assert_eq!(pn.len(), 2);
        assert_eq!(pn[0].source_fk, src);
        assert_eq!(pn[1].source_fk, src2);
    }

    #[test]
    fn counts_by_paper_includes_zero_and_only_active() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (src, src2, proj) = seed(&conn);
        // trashed paper must be excluded.
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES (3, 'arxiv:3', 'trashed')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK) VALUES (10, 1), (10, 2), (10, 3)",
            [],
        )
        .unwrap();
        create_note(&conn, src, None, Some(proj), "a", "x", None, None, None).unwrap();

        let counts = note_counts_by_paper_for_project(&conn, proj).unwrap();
        assert_eq!(counts, vec![(src, 1), (src2, 0)]); // src=1 has one, src2=2 has zero, trashed absent
    }

    #[test]
    fn search_escapes_like_and_filters_active() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed(&conn);
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES (3, 'arxiv:3', 'trashed')",
            [],
        )
        .unwrap();

        create_note(
            &conn,
            1,
            None,
            None,
            "title 50%",
            "neural nets",
            None,
            None,
            None,
        )
        .unwrap();
        create_note(
            &conn,
            2,
            None,
            None,
            "other",
            "no match here",
            None,
            None,
            None,
        )
        .unwrap();
        create_note(
            &conn,
            3,
            None,
            None,
            "neural",
            "trashed paper note",
            None,
            None,
            None,
        )
        .unwrap(); // excluded: not active

        // body match on active paper only.
        let hits = search_notes_source_fks(&conn, "neural", 50).unwrap();
        assert_eq!(hits, vec![1]);

        // literal "%" must not act as a wildcard: matches the title, not everything.
        let pct = search_notes_source_fks(&conn, "50%", 50).unwrap();
        assert_eq!(pct, vec![1]);
        // a bare "%" would match all-if-wildcard; escaped it matches nothing.
        assert!(search_notes_source_fks(&conn, "zzz%zzz", 50)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn uuid_fixed_roundtrip_and_unique_constraint() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (src, _, proj) = seed(&conn);

        let id = create_note(
            &conn,
            src,
            None,
            Some(proj),
            "t",
            "body",
            None,
            None,
            Some("fixed"),
        )
        .unwrap();
        let got = get_note(&conn, id).unwrap().unwrap();
        assert_eq!(got.uuid, "fixed");

        // Second create_note with the same uuid should fail (unique constraint).
        let result = create_note(
            &conn,
            src,
            None,
            Some(proj),
            "t2",
            "body2",
            None,
            None,
            Some("fixed"),
        );
        assert!(result.is_err());
    }
}
