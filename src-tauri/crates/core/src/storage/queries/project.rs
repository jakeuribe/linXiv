use rusqlite::{Connection, OptionalExtension, Row};

use crate::error::{CoreError, Result};
use crate::models::{ProjectDetails, Status};
use crate::storage::db::timestamp_from_sql;
use crate::storage::query::Q;

// Columns in fixed order; both queries share `raw_from_row` / `to_model`.
const SELECT_COLS: &str =
    "PROJECT_FK, NAME, DESCRIPTION, COLOR, STATUS, CREATED_AT, UPDATED_AT, ARCHIVED_AT FROM PROJECT";

/// Raw column values before decltype conversion (closure stays in rusqlite-error
/// land; `to_model` does the CoreError-returning conversions).
struct RawProject {
    id: i64,
    name: String,
    description: Option<String>,
    color: Option<i64>,
    status: String,
    created_at: String,
    updated_at: String,
    archived_at: Option<String>,
}

fn raw_from_row(r: &Row) -> rusqlite::Result<RawProject> {
    Ok(RawProject {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        color: r.get(3)?,
        status: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        archived_at: r.get(7)?,
    })
}

fn status_from_sql(s: &str) -> Result<Status> {
    match s {
        "active" => Ok(Status::Active),
        "archived" => Ok(Status::Archived),
        "deleted" => Ok(Status::Deleted),
        other => Err(CoreError::Internal(format!("unknown project status {other:?}"))),
    }
}

/// Maps a row to ProjectDetails. `source_fks` is left empty for the caller to
/// fill via `load_source_fks`; `project_tags` stays empty — Python's
/// `Project.from_row` does not load tags either.
fn to_model(raw: RawProject) -> Result<ProjectDetails> {
    Ok(ProjectDetails {
        id: Some(raw.id),
        name: raw.name,
        description: raw.description.unwrap_or_default(), // Python: DESCRIPTION or ""
        color: raw.color.map(|c| c as i32),
        project_tags: Vec::new(),
        source_fks: Vec::new(),
        status: status_from_sql(&raw.status)?,
        created_at: Some(timestamp_from_sql(&raw.created_at)?),
        updated_at: Some(timestamp_from_sql(&raw.updated_at)?),
        archived_at: raw.archived_at.as_deref().map(timestamp_from_sql).transpose()?,
    })
}

/// `storage/projects.py::_load_source_fks` — active-paper membership in
/// PROJECT_TO_PAPER_FK (insertion) order; soft-deleted roots are excluded.
fn load_source_fks(conn: &Connection, project_fk: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT p2p.SOURCE_FK FROM PROJECT_TO_PAPER p2p \
         JOIN PAPER_ROOTS r ON r.SOURCE_FK = p2p.SOURCE_FK \
         WHERE p2p.PROJECT_FK = ? AND r.STATUS = 'active' \
         ORDER BY p2p.PROJECT_TO_PAPER_FK",
    )?;
    let fks = stmt
        .query_map([project_fk], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(fks)
}

/// `storage/projects.py::get_project` — full project row. `load_sources` mirrors
/// Python (default true): when false, `source_fks` stays empty and the caller
/// fills counts via the bulk loader (port of `list_project_source_ids_bulk` in
/// storage/config/queries.py, deferred to the service phase).
pub fn get_project(conn: &Connection, project_id: i64, load_sources: bool) -> Result<Option<ProjectDetails>> {
    let raw = conn
        .query_row(
            &format!("SELECT {SELECT_COLS} WHERE PROJECT_FK = ?"),
            [project_id],
            raw_from_row,
        )
        .optional()?;
    match raw {
        None => Ok(None),
        Some(raw) => {
            let mut proj = to_model(raw)?;
            if load_sources {
                proj.source_fks = load_source_fks(conn, project_id)?;
            }
            Ok(Some(proj))
        }
    }
}

/// `storage/projects.py::filter_projects` — list projects by optional predicate.
/// `load_sources` mirrors Python (default true): false skips the per-row membership
/// query — the list/graph paths pass false and fill counts via the bulk loader
/// (port of `list_project_source_ids_bulk`, deferred to the service phase), which
/// avoids the N+1.
pub fn list_projects(conn: &Connection, condition: Option<Q>, load_sources: bool) -> Result<Vec<ProjectDetails>> {
    let sql = match &condition {
        None => format!("SELECT {SELECT_COLS}"),
        Some(q) => format!("SELECT {SELECT_COLS} WHERE {}", q.sql),
    };
    let params = condition.as_ref().map(|q| q.params_slice()).unwrap_or_default();

    let mut stmt = conn.prepare(&sql)?;
    let raws = stmt
        .query_map(params.as_slice(), raw_from_row)?
        .collect::<rusqlite::Result<Vec<RawProject>>>()?;

    let mut out = Vec::with_capacity(raws.len());
    for raw in raws {
        let id = raw.id;
        let mut proj = to_model(raw)?;
        if load_sources {
            proj.source_fks = load_source_fks(conn, id)?;
        }
        out.push(proj);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};

    fn seed(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO PROJECT (PROJECT_FK, NAME, DESCRIPTION, COLOR, STATUS, CREATED_AT, UPDATED_AT)
                 VALUES (1, 'My Proj', 'desc', 255, 'active', '2024-01-01T10:00:00', '2024-01-02T11:00:00');
             INSERT INTO PROJECT (PROJECT_FK, NAME, STATUS, CREATED_AT, UPDATED_AT)
                 VALUES (2, 'Archived', 'archived', '2024-01-01T10:00:00', '2024-01-02T11:00:00');
             INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES
                 (10, 'arxiv:1', 'active'), (11, 'arxiv:2', 'deleted'), (12, 'arxiv:3', 'active');
             -- inserted out of FK order to prove ORDER BY PROJECT_TO_PAPER_FK, with one deleted root
             INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK) VALUES
                 (100, 1, 12), (101, 1, 11), (102, 1, 10);",
        )
        .unwrap();
    }

    #[test]
    fn get_project_loads_row_and_active_membership_in_order() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(&conn);

        let p = get_project(&conn, 1, true).unwrap().expect("project exists");
        assert_eq!(p.id, Some(1));
        assert_eq!(p.name, "My Proj");
        assert_eq!(p.description, "desc");
        assert_eq!(p.color, Some(255));
        assert_eq!(p.status, Status::Active);
        assert!(p.archived_at.is_none());
        assert!(p.created_at.is_some());
        // sfk 11 dropped (deleted root); ordered by PROJECT_TO_PAPER_FK: 100->12, 102->10
        assert_eq!(p.source_fks, vec![12, 10]);

        assert!(get_project(&conn, 999, true).unwrap().is_none());
    }

    #[test]
    fn list_projects_honors_predicate() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed(&conn);

        let all = list_projects(&conn, None, true).unwrap();
        assert_eq!(all.len(), 2);

        // load_sources=false skips the membership query (source_fks stay empty).
        let lite = list_projects(&conn, None, false).unwrap();
        assert!(lite.iter().all(|p| p.source_fks.is_empty()));

        let active =
            list_projects(&conn, Some(Q::new("STATUS = ?", "active".to_string())), true).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, Some(1));
        assert_eq!(active[0].source_fks, vec![12, 10]);
    }
}
