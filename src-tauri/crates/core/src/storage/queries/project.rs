use chrono::{NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::error::{CoreError, Result};
use crate::models::{ProjectDetails, Status};
use crate::storage::db::{timestamp_from_sql, timestamp_to_sql, transaction};
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
        other => Err(CoreError::Internal(format!(
            "unknown project status {other:?}"
        ))),
    }
}

fn status_to_sql(s: Status) -> &'static str {
    match s {
        Status::Active => "active",
        Status::Archived => "archived",
        Status::Deleted => "deleted",
    }
}

// ── Colour helpers — Python `projects.py::color_to_hex`/`color_from_hex`. ──────

pub fn color_to_hex(color: i32) -> String {
    format!("#{color:06x}")
}

pub fn color_from_hex(hex: &str) -> Result<i32> {
    i32::from_str_radix(hex.trim_start_matches('#'), 16)
        .map_err(|e| CoreError::Internal(format!("bad colour hex {hex:?}: {e}")))
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
        archived_at: raw
            .archived_at
            .as_deref()
            .map(timestamp_from_sql)
            .transpose()?,
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
pub fn get_project(
    conn: &Connection,
    project_id: i64,
    load_sources: bool,
) -> Result<Option<ProjectDetails>> {
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
pub fn list_projects(
    conn: &Connection,
    condition: Option<Q>,
    load_sources: bool,
) -> Result<Vec<ProjectDetails>> {
    let sql = match &condition {
        None => format!("SELECT {SELECT_COLS}"),
        Some(q) => format!("SELECT {SELECT_COLS} WHERE {}", q.sql),
    };
    let params = condition
        .as_ref()
        .map(|q| q.params_slice())
        .unwrap_or_default();

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

// ── Writes — Python `storage/projects.py`. ────────────────────────────────────

/// Insert a new PROJECT row (CREATED_AT = UPDATED_AT = now). Returns PROJECT_FK.
/// Membership is NOT written here — the caller composes `save_source_fks`
/// (mirrors Python `save()` on insert, which calls `_save_source_fks` next).
pub fn insert_project(
    conn: &Connection,
    name: &str,
    description: &str,
    color: Option<i32>,
    status: Status,
    archived_at: Option<NaiveDateTime>,
) -> Result<i64> {
    let now = timestamp_to_sql(Utc::now().naive_utc());
    conn.execute(
        "INSERT INTO PROJECT (NAME, DESCRIPTION, COLOR, STATUS, CREATED_AT, UPDATED_AT, ARCHIVED_AT) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
        params![name, description, color, status_to_sql(status), now, archived_at.map(timestamp_to_sql)],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fields-only UPDATE (NAME/DESCRIPTION/COLOR/STATUS/ARCHIVED_AT + UPDATED_AT).
/// NON-NEGOTIABLE: membership is deliberately NOT rewritten — Python `save()` on
/// update writes fields only, so a stale in-memory member list can't clobber rows
/// other requests wrote. Returns false if no row matched. Covers delete/archive/
/// restore (those just set STATUS/ARCHIVED_AT then call this).
pub fn update_project_fields(
    conn: &Connection,
    project_fk: i64,
    name: &str,
    description: &str,
    color: Option<i32>,
    status: Status,
    archived_at: Option<NaiveDateTime>,
) -> Result<bool> {
    let now = timestamp_to_sql(Utc::now().naive_utc());
    let n = conn.execute(
        "UPDATE PROJECT SET NAME = ?1, DESCRIPTION = ?2, COLOR = ?3, STATUS = ?4, \
         UPDATED_AT = ?5, ARCHIVED_AT = ?6 WHERE PROJECT_FK = ?7",
        params![
            name,
            description,
            color,
            status_to_sql(status),
            now,
            archived_at.map(timestamp_to_sql),
            project_fk
        ],
    )?;
    Ok(n > 0)
}

/// Full membership replace: DELETE all then re-INSERT (OR IGNORE) in list order —
/// Python `_save_source_fks`. Takes `&Transaction` (not `&Connection`) so it can
/// only be called inside a transaction — DELETE-then-INSERT on a bare autocommit
/// connection would wipe membership with no rollback if an INSERT failed. The
/// service-layer insert+membership composer (Python `save()` on insert) must run
/// `insert_project` and this in the SAME transaction.
pub fn save_source_fks(tx: &Transaction, project_fk: i64, source_fks: &[i64]) -> Result<()> {
    tx.execute(
        "DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?1",
        [project_fk],
    )?;
    let mut stmt = tx.prepare(
        "INSERT OR IGNORE INTO PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK) VALUES (?1, ?2)",
    )?;
    for &sfk in source_fks {
        stmt.execute(params![project_fk, sfk])?;
    }
    Ok(())
}

/// Incremental add — INSERT OR IGNORE per row (idx_project_to_paper_unique makes
/// dupes a no-op). Python `Project.add_papers`.
pub fn add_papers(conn: &Connection, project_fk: i64, source_fks: &[i64]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK) VALUES (?1, ?2)",
    )?;
    for &sfk in source_fks {
        stmt.execute(params![project_fk, sfk])?;
    }
    Ok(())
}

/// Incremental remove — DELETE per (project, paper). Python `Project.remove_papers`.
pub fn remove_papers(conn: &Connection, project_fk: i64, source_fks: &[i64]) -> Result<()> {
    let mut stmt =
        conn.prepare("DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?1 AND SOURCE_FK = ?2")?;
    for &sfk in source_fks {
        stmt.execute(params![project_fk, sfk])?;
    }
    Ok(())
}

/// Set membership to exactly `source_fks` (dedup, ordered), atomically. Python
/// `Project.replace_papers` → `_save_source_fks` inside one transaction.
pub fn replace_papers(conn: &mut Connection, project_fk: i64, source_fks: &[i64]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    let deduped: Vec<i64> = source_fks
        .iter()
        .copied()
        .filter(|s| seen.insert(*s))
        .collect();
    transaction(conn, |tx| save_source_fks(tx, project_fk, &deduped))
}

/// PROJECT_FKs of every project containing this paper — any status. Python
/// `get_paper_project_fks`. Callers filter to active themselves.
pub fn get_paper_project_fks(conn: &Connection, source_fk: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT PROJECT_FK FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?1")?;
    let fks = stmt
        .query_map([source_fk], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(fks)
}

/// Remove a paper from every project; returns the FKs it was removed from.
/// Python `remove_paper_from_all_projects` (select-then-delete, transactional).
pub fn remove_paper_from_all_projects(conn: &mut Connection, source_fk: i64) -> Result<Vec<i64>> {
    transaction(conn, |tx| {
        let fks: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT PROJECT_FK FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?1")?;
            let v = stmt
                .query_map([source_fk], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            v
        };
        if !fks.is_empty() {
            tx.execute(
                "DELETE FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?1",
                [source_fk],
            )?;
        }
        Ok(fks)
    })
}

/// Permanently remove a project + associations in ONE transaction (Python
/// `hard_delete_project`). NULLs NOTE.PROJECT_FK rather than deleting notes, and
/// leaves orphan TAG rows, per ADR-0009. No-ops cleanly if the project is absent.
pub fn hard_delete_project(conn: &mut Connection, project_fk: i64) -> Result<()> {
    transaction(conn, |tx| {
        tx.execute(
            "DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?1",
            [project_fk],
        )?;
        tx.execute(
            "DELETE FROM PROJECT_TO_TAG WHERE PROJECT_FK = ?1",
            [project_fk],
        )?;
        tx.execute(
            "UPDATE NOTE SET PROJECT_FK = NULL WHERE PROJECT_FK = ?1",
            [project_fk],
        )?;
        tx.execute(
            "UPDATE ANNOTATION SET PROJECT_FK = NULL WHERE PROJECT_FK = ?1",
            [project_fk],
        )?;
        tx.execute("DELETE FROM PROJECT WHERE PROJECT_FK = ?1", [project_fk])?;
        Ok(())
    })
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

        let p = get_project(&conn, 1, true)
            .unwrap()
            .expect("project exists");
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

        let active = list_projects(
            &conn,
            Some(Q::new("STATUS = ?", "active".to_string())),
            true,
        )
        .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, Some(1));
        assert_eq!(active[0].source_fks, vec![12, 10]);
    }

    #[test]
    fn insert_update_membership_and_hard_delete() {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES
                 (10, 'arxiv:1', 'active'), (11, 'arxiv:2', 'active');
             INSERT INTO TAG (TAG_FK, TAG) VALUES (5, 't');",
        )
        .unwrap();

        // insert → re-read confirms the row landed.
        let id = insert_project(&conn, "P", "d", Some(0x00ff00), Status::Active, None).unwrap();
        let p = get_project(&conn, id, true).unwrap().unwrap();
        assert_eq!(p.name, "P");
        assert_eq!(p.color, Some(0x00ff00));
        assert_eq!(p.status, Status::Active);
        assert_eq!(
            p.created_at, p.updated_at,
            "insert stamps both timestamps to now"
        );

        // membership via add/replace.
        add_papers(&conn, id, &[10, 11]).unwrap();
        assert_eq!(
            get_project(&conn, id, true).unwrap().unwrap().source_fks,
            vec![10, 11]
        );
        replace_papers(&mut conn, id, &[11, 11, 10]).unwrap(); // dedup, reorder
        assert_eq!(
            get_project(&conn, id, true).unwrap().unwrap().source_fks,
            vec![11, 10]
        );
        remove_papers(&conn, id, &[11]).unwrap();
        assert_eq!(
            get_project(&conn, id, true).unwrap().unwrap().source_fks,
            vec![10]
        );

        // fields-only update must NOT touch membership.
        assert!(
            update_project_fields(&conn, id, "P2", "d2", None, Status::Archived, None).unwrap()
        );
        let p = get_project(&conn, id, true).unwrap().unwrap();
        assert_eq!(p.name, "P2");
        assert_eq!(p.status, Status::Archived);
        assert_eq!(
            p.source_fks,
            vec![10],
            "fields-only update left membership intact"
        );
        assert!(!update_project_fields(&conn, 999, "x", "", None, Status::Active, None).unwrap());

        // a note + tag link, then hard_delete: project gone, note kept but unscoped.
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT) \
             VALUES (10, ?1, 't', 'c', '2024-01-01T00:00:00', '2024-01-01T00:00:00')",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_TAG (PROJECT_TO_TAG_FK, PROJECT_FK, TAG_FK) VALUES (1, ?1, 5)",
            [id],
        )
        .unwrap();
        // a project-scoped annotation (FK to PROJECT) must not block hard_delete.
        conn.execute(
            "INSERT INTO ANNOTATION (SOURCE_FK, PROJECT_FK, ANCHOR) VALUES (10, ?1, '{}')",
            [id],
        )
        .unwrap();
        assert_eq!(get_paper_project_fks(&conn, 10).unwrap(), vec![id]);

        hard_delete_project(&mut conn, id).unwrap();
        assert!(get_project(&conn, id, true).unwrap().is_none());
        assert!(get_paper_project_fks(&conn, 10).unwrap().is_empty());
        let note_proj: Option<i64> = conn
            .query_row(
                "SELECT PROJECT_FK FROM NOTE WHERE SOURCE_FK = 10",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(note_proj, None, "note kept, PROJECT_FK NULLed (ADR-0009)");
        let ann_proj: Option<i64> = conn
            .query_row(
                "SELECT PROJECT_FK FROM ANNOTATION WHERE SOURCE_FK = 10",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ann_proj, None, "annotation kept, PROJECT_FK NULLed");
        let tag_links: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM PROJECT_TO_TAG WHERE PROJECT_FK = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tag_links, 0, "hard_delete removed PROJECT_TO_TAG links");

        assert_eq!(color_to_hex(0x00ff00), "#00ff00");
        assert_eq!(color_from_hex("#00ff00").unwrap(), 0x00ff00);
    }

    #[test]
    fn remove_paper_from_all_projects_clears_membership_and_returns_fks() {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES
                 (10, 'arxiv:1', 'active'), (20, 'arxiv:2', 'active');
             INSERT INTO PROJECT (PROJECT_FK, NAME, STATUS, CREATED_AT, UPDATED_AT) VALUES
                 (1, 'A', 'active', '2024-01-01T00:00:00', '2024-01-01T00:00:00'),
                 (2, 'B', 'active', '2024-01-01T00:00:00', '2024-01-01T00:00:00');",
        )
        .unwrap();
        add_papers(&conn, 1, &[10, 20]).unwrap();
        add_papers(&conn, 2, &[10]).unwrap();

        // paper 10 is in projects 1 and 2; removal returns both, transactionally.
        let mut fks = remove_paper_from_all_projects(&mut conn, 10).unwrap();
        fks.sort();
        assert_eq!(fks, vec![1, 2]);
        assert!(get_paper_project_fks(&conn, 10).unwrap().is_empty());
        // unrelated paper 20 (in project 1) survives.
        assert_eq!(get_paper_project_fks(&conn, 20).unwrap(), vec![1]);
        // empty case: a paper in no project returns [] without error.
        assert!(remove_paper_from_all_projects(&mut conn, 999)
            .unwrap()
            .is_empty());
    }
}
