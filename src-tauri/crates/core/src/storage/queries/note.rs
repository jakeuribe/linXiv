use rusqlite::{params_from_iter, Connection};

use crate::error::Result;
use crate::models::NoteDetails;
use crate::storage::db::timestamp_from_sql;

/// `storage/notes.py::get_notes` — notes for a paper, 3-way scoped:
///   * `all_projects` true   → SOURCE_FK only (every note: library + all projects)
///   * `project_id` Some(id)  → `PROJECT_FK = id`
///   * `project_id` None      → `PROJECT_FK IS NULL` (library notes only)
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
    let sql = format!(
        "SELECT NOTE_SK, SOURCE_FK, PAPER_ID_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT \
         FROM NOTE WHERE SOURCE_FK = ?1 AND {project_clause} ORDER BY CREATED_AT ASC"
    );

    let mut params: Vec<i64> = vec![source_fk];
    if !all_projects {
        if let Some(id) = project_id {
            params.push(id);
        }
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        // TITLE nullable, NOTE is a BLOB holding text — both coalesce to "" (Python `or ""`).
        Ok((
            row.get::<_, Option<i64>>("NOTE_SK")?,
            row.get::<_, i64>("SOURCE_FK")?,
            row.get::<_, Option<i64>>("PAPER_ID_FK")?,
            row.get::<_, Option<i64>>("PROJECT_FK")?,
            row.get::<_, Option<String>>("TITLE")?,
            row.get::<_, Option<String>>("NOTE")?,
            row.get::<_, String>("CREATED_AT")?,
            row.get::<_, String>("UPDATED_AT")?,
        ))
    })?;

    let mut out = Vec::new();
    for r in rows {
        let (note_id, src, paper_id_fk, proj, title, content, created, updated) = r?;
        out.push(NoteDetails {
            note_id,
            source_fk: src,
            paper_id_fk,
            project_id: proj,
            title: title.unwrap_or_default(),
            content: content.unwrap_or_default(),
            // TIMESTAMP cols are NOT NULL — parse via the db decltype converter.
            created_at: Some(timestamp_from_sql(&created)?),
            updated_at: Some(timestamp_from_sql(&updated)?),
        });
    }
    Ok(out)
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
        conn.execute("INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (10, 'P')", [])
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
        assert!(n.note_id.is_some());
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
}
