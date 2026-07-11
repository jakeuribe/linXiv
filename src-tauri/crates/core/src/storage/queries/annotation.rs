//! ANNOTATION named queries — PDF highlight storage, mirroring `queries::note`.
//!
//! Callers must pass a connection opened via `storage::db::open`; FK cascades
//! (SOURCE_FK → PAPER_ROOTS ON DELETE CASCADE) depend on the foreign_keys
//! PRAGMA set there.

use chrono::Utc;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use crate::error::Result;
use crate::models::AnnotationDetails;
use crate::storage::db::{timestamp_from_sql, timestamp_to_sql};

const COLS: &str = "ANNOTATION_SK, SOURCE_FK, PROJECT_FK, ANCHOR, COMMENT, CREATED_AT, UPDATED_AT";

fn annotation_from_row(row: &Row) -> rusqlite::Result<AnnotationDetails> {
    let created: String = row.get("CREATED_AT")?;
    let updated: String = row.get("UPDATED_AT")?;
    Ok(AnnotationDetails {
        annotation_id: row.get::<_, i64>("ANNOTATION_SK")?,
        source_fk: row.get("SOURCE_FK")?,
        project_id: row.get::<_, Option<i64>>("PROJECT_FK")?,
        anchor: row.get::<_, String>("ANCHOR")?,
        comment: row.get::<_, String>("COMMENT")?,
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

/// Annotations for a paper, 3-way scoped exactly like `note::get_notes`:
///   * `all_projects` true   → SOURCE_FK only (library + every project)
///   * `project_id` Some(id)  → `PROJECT_FK = id`
///   * `project_id` None      → `PROJECT_FK IS NULL` (library only)
/// Ordered CREATED_AT ASC.
pub fn get_annotations(
    conn: &Connection,
    source_fk: i64,
    project_id: Option<i64>,
    all_projects: bool,
) -> Result<Vec<AnnotationDetails>> {
    let project_clause = if all_projects {
        "1 = 1"
    } else if project_id.is_some() {
        "PROJECT_FK = ?2"
    } else {
        "PROJECT_FK IS NULL"
    };
    let sql = format!(
        "SELECT {COLS} FROM ANNOTATION \
         WHERE SOURCE_FK = ?1 AND {project_clause} ORDER BY CREATED_AT ASC"
    );

    let mut params: Vec<i64> = vec![source_fk];
    params.extend(if all_projects { None } else { project_id });
    query_annotations(conn, &sql, &params)
}

/// Single annotation by ANNOTATION_SK, None if absent.
pub fn get_annotation(conn: &Connection, annotation_id: i64) -> Result<Option<AnnotationDetails>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM ANNOTATION WHERE ANNOTATION_SK = ?1"
    ))?;
    Ok(stmt
        .query_row([annotation_id], annotation_from_row)
        .optional()?)
}

/// INSERT an annotation, returns the new ANNOTATION_SK. Both timestamps stamped now.
pub fn create_annotation(
    conn: &Connection,
    source_fk: i64,
    project_id: Option<i64>,
    anchor: &str,
    comment: &str,
) -> Result<i64> {
    let now = timestamp_to_sql(Utc::now().naive_utc());
    conn.execute(
        "INSERT INTO ANNOTATION (SOURCE_FK, PROJECT_FK, ANCHOR, COMMENT, CREATED_AT, UPDATED_AT) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![source_fk, project_id, anchor, comment, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Update the written comment (the only mutable field). Returns false if no row
/// matched ANNOTATION_SK.
pub fn patch_annotation(conn: &Connection, annotation_id: i64, comment: &str) -> Result<bool> {
    let now = timestamp_to_sql(Utc::now().naive_utc());
    let n = conn.execute(
        "UPDATE ANNOTATION SET COMMENT = ?1, UPDATED_AT = ?2 WHERE ANNOTATION_SK = ?3",
        params![comment, now, annotation_id],
    )?;
    Ok(n > 0)
}

/// Hard-delete one annotation row; false if absent.
pub fn delete_annotation(conn: &Connection, annotation_id: i64) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM ANNOTATION WHERE ANNOTATION_SK = ?1",
        [annotation_id],
    )?;
    Ok(n > 0)
}

fn query_annotations(
    conn: &Connection,
    sql: &str,
    params: &[i64],
) -> Result<Vec<AnnotationDetails>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params_from_iter(params), annotation_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every annotation, CREATED_AT ASC.
pub fn list_all_annotations(conn: &Connection) -> Result<Vec<AnnotationDetails>> {
    query_annotations(
        conn,
        &format!("SELECT {COLS} FROM ANNOTATION ORDER BY CREATED_AT ASC"),
        &[],
    )
}

/// All annotations scoped to a project, SOURCE_FK ASC then CREATED_AT ASC.
/// Feeds the export manifest and the share snapshot.
pub fn get_project_annotations(
    conn: &Connection,
    project_id: i64,
) -> Result<Vec<AnnotationDetails>> {
    query_annotations(
        conn,
        &format!(
            "SELECT {COLS} FROM ANNOTATION WHERE PROJECT_FK = ?1 ORDER BY SOURCE_FK ASC, CREATED_AT ASC"
        ),
        &[project_id],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::open_in_memory;
    use crate::storage::init_db;

    fn seed(conn: &Connection) -> (i64, i64) {
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
        (1, 10)
    }

    const ANCHOR: &str = r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##;

    #[test]
    fn create_get_patch_delete_roundtrip() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (src, proj) = seed(&conn);

        let id = create_annotation(&conn, src, Some(proj), ANCHOR, "first").unwrap();
        let got = get_annotation(&conn, id).unwrap().unwrap();
        assert_eq!(got.anchor, ANCHOR);
        assert_eq!(got.comment, "first");
        assert_eq!(got.project_id, Some(proj));
        assert!(got.created_at.is_some());

        // patch updates only the comment.
        assert!(patch_annotation(&conn, id, "edited").unwrap());
        assert_eq!(
            get_annotation(&conn, id).unwrap().unwrap().comment,
            "edited"
        );

        // patch/delete of an absent row report false.
        assert!(!patch_annotation(&conn, 999, "x").unwrap());
        assert!(!delete_annotation(&conn, 999).unwrap());

        assert!(delete_annotation(&conn, id).unwrap());
        assert!(get_annotation(&conn, id).unwrap().is_none());
    }

    #[test]
    fn scopes_by_project_and_lists() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (src, proj) = seed(&conn);

        // library (no project) + project-scoped, same paper.
        create_annotation(&conn, src, None, ANCHOR, "lib").unwrap();
        create_annotation(&conn, src, Some(proj), ANCHOR, "proj").unwrap();
        create_annotation(&conn, 2, Some(proj), ANCHOR, "other").unwrap();

        let lib = get_annotations(&conn, src, None, false).unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].comment, "lib");

        let all = get_annotations(&conn, src, None, true).unwrap();
        assert_eq!(all.len(), 2);

        let scoped = get_annotations(&conn, src, Some(proj), false).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].comment, "proj");

        assert_eq!(list_all_annotations(&conn).unwrap().len(), 3);

        // project annotations across papers, SOURCE_FK ASC.
        let pa = get_project_annotations(&conn, proj).unwrap();
        assert_eq!(pa.len(), 2);
        assert_eq!(pa[0].source_fk, 1);
        assert_eq!(pa[1].source_fk, 2);
    }

    #[test]
    fn cascades_on_paper_root_delete() {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let (src, _) = seed(&conn);
        create_annotation(&conn, src, None, ANCHOR, "c").unwrap();
        // Deleting the paper root cascades its annotations (FK ON DELETE CASCADE).
        conn.execute("DELETE FROM PAPER_ROOTS WHERE SOURCE_FK = ?1", [src])
            .unwrap();
        assert!(list_all_annotations(&conn).unwrap().is_empty());
    }
}
