use std::collections::HashMap;

use chrono::{NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

use crate::error::{CoreError, Result};
use crate::models::{ProjectDetails, Status};
use crate::storage::db::{timestamp_from_sql, timestamp_to_sql, transaction};
use crate::storage::query::Q;

// Columns in fixed order; both queries share `raw_from_row` / `to_model`.
const SELECT_COLS: &str =
    "PROJECT_FK, NAME, DESCRIPTION, COLOR, STATUS, CREATED_AT, UPDATED_AT, ARCHIVED_AT, SHARE_ID FROM PROJECT";

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
    share_id: Option<String>,
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
        share_id: r.get(8)?,
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

pub(crate) fn status_to_sql(s: Status) -> &'static str {
    match s {
        Status::Active => "active",
        Status::Archived => "archived",
        Status::Deleted => "deleted",
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
        archived_at: raw
            .archived_at
            .as_deref()
            .map(timestamp_from_sql)
            .transpose()?,
        share_id: raw.share_id,
    })
}

/// Set SHARE_ID to `candidate` only if the row has none, then return the stored
/// value (existing wins over the candidate). `None` if the project is absent.
pub fn ensure_share_id(
    conn: &Connection,
    project_fk: i64,
    candidate: &str,
) -> Result<Option<String>> {
    conn.execute(
        "UPDATE PROJECT SET SHARE_ID = ?1 WHERE PROJECT_FK = ?2 AND SHARE_ID IS NULL",
        params![candidate, project_fk],
    )?;
    Ok(conn
        .query_row(
            "SELECT SHARE_ID FROM PROJECT WHERE PROJECT_FK = ?1",
            [project_fk],
            |r| r.get(0),
        )
        .optional()?)
}

/// PROJECT_FK of the project claiming this SHARE_ID (hoster- or reader-linked).
pub fn find_by_share_id(conn: &Connection, share_id: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT PROJECT_FK FROM PROJECT WHERE SHARE_ID = ?1 AND STATUS != 'deleted'",
            [share_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Check if another non-trashed project (not project_fk) has claimed this SHARE_ID
/// (mirrors idx_project_share_id_unique's WHERE STATUS != 'deleted' predicate).
pub fn share_id_claimed_by_other(
    conn: &Connection,
    project_fk: i64,
    share_id: &str,
) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM PROJECT WHERE SHARE_ID = ?1 AND PROJECT_FK != ?2 AND STATUS != 'deleted'",
            params![share_id, project_fk],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Clear SHARE_ID from any live (non-trashed) holder of this share_id.
pub fn release_share_id(conn: &Connection, share_id: &str) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE PROJECT SET SHARE_ID = NULL WHERE SHARE_ID = ?1 AND STATUS != 'deleted'",
        [share_id],
    )?)
}

/// Clear SHARE_ID from any trashed (STATUS = 'deleted') holder of this share_id.
pub fn release_share_id_from_deleted(conn: &Connection, share_id: &str) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE PROJECT SET SHARE_ID = NULL WHERE SHARE_ID = ?1 AND STATUS = 'deleted'",
        [share_id],
    )?)
}

/// `storage/projects.py::_load_source_fks` — active-paper membership in
/// PROJECT_TO_PAPER_FK (insertion) order; soft-deleted roots are excluded.
fn load_source_fks(conn: &Connection, project_fk: i64) -> Result<Vec<i64>> {
    Ok(source_fks_by_project(conn, &[project_fk])?
        .remove(&project_fk)
        .unwrap_or_default())
}

/// Batched [`load_source_fks`]: PROJECT_FK → active-membership SOURCE_FKs (in
/// PROJECT_TO_PAPER_FK order) for a set of projects in one chunked query, so
/// list paths are not a membership query per project. Projects with no active
/// papers are simply absent.
pub fn source_fks_by_project(
    conn: &Connection,
    project_fks: &[i64],
) -> Result<HashMap<i64, Vec<i64>>> {
    let mut by_project: HashMap<i64, Vec<i64>> = HashMap::with_capacity(project_fks.len());
    for chunk in project_fks.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        // Per-project ordering survives chunking: a project's rows never span chunks.
        let sql = format!(
            "SELECT p2p.PROJECT_FK, p2p.SOURCE_FK FROM PROJECT_TO_PAPER p2p \
             JOIN PAPER_ROOTS r ON r.SOURCE_FK = p2p.SOURCE_FK \
             WHERE p2p.PROJECT_FK IN ({placeholders}) AND r.STATUS = 'active' \
             ORDER BY p2p.PROJECT_TO_PAPER_FK"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (pfk, sfk) = row?;
            by_project.entry(pfk).or_default().push(sfk);
        }
    }
    Ok(by_project)
}

/// Bare existence probe, any status (mirrors `get_project`'s unfiltered row
/// lookup) — the narrow form for guard sites that discard the row.
pub fn project_exists(conn: &Connection, project_fk: i64) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM PROJECT WHERE PROJECT_FK = ?1",
            [project_fk],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Active-membership count for one project (same PAPER_ROOTS `active` join as
/// [`source_fks_by_project`]), `None` when the project is absent — the narrow
/// form of `get_project(..).source_fks.len()`.
pub fn active_paper_count(conn: &Connection, project_fk: i64) -> Result<Option<usize>> {
    Ok(conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM PROJECT_TO_PAPER p2p \
               JOIN PAPER_ROOTS r ON r.SOURCE_FK = p2p.SOURCE_FK \
               WHERE p2p.PROJECT_FK = PROJECT.PROJECT_FK AND r.STATUS = 'active') \
             FROM PROJECT WHERE PROJECT_FK = ?1",
            [project_fk],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .map(|n| n as usize))
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
    let Some(raw) = conn
        .query_row(
            &format!("SELECT {SELECT_COLS} WHERE PROJECT_FK = ?"),
            [project_id],
            raw_from_row,
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut proj = to_model(raw)?;
    if load_sources {
        proj.source_fks = load_source_fks(conn, project_id)?;
    }
    Ok(Some(proj))
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

    let mut by_project = if load_sources {
        source_fks_by_project(conn, &raws.iter().map(|r| r.id).collect::<Vec<_>>())?
    } else {
        HashMap::new()
    };
    let mut out = Vec::with_capacity(raws.len());
    for raw in raws {
        let id = raw.id;
        let mut proj = to_model(raw)?;
        if load_sources {
            proj.source_fks = by_project.remove(&id).unwrap_or_default();
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

/// Full membership replace, diffed against current rows (not a blanket
/// delete-then-reinsert) so a retained paper's PAPER_TO_READING cascade FK never
/// fires. Must run in the same transaction as `insert_project`.
pub fn save_source_fks(tx: &Transaction, project_fk: i64, source_fks: &[i64]) -> Result<()> {
    let existing: std::collections::HashSet<i64> = {
        let mut stmt =
            tx.prepare("SELECT SOURCE_FK FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?1")?;
        let rows = stmt
            .query_map([project_fk], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };
    let incoming: std::collections::HashSet<i64> = source_fks.iter().copied().collect();

    let to_remove: Vec<i64> = existing.difference(&incoming).copied().collect();
    if !to_remove.is_empty() {
        remove_papers(tx, project_fk, &to_remove)?;
    }
    let to_add: Vec<i64> = source_fks
        .iter()
        .copied()
        .filter(|s| !existing.contains(s))
        .collect();
    add_papers(tx, project_fk, &to_add)
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

/// Set membership to exactly `source_fks` (dedup), atomically. Order is only
/// honored for newly-added papers; reordering existing members is a no-op.
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
    Ok(project_fks_by_source_fk(conn, &[source_fk])?
        .remove(&source_fk)
        .unwrap_or_default())
}

/// Batched [`get_paper_project_fks`]: SOURCE_FK → PROJECT_FKs (any status, in
/// PROJECT_TO_PAPER_FK order) for a set of papers in one chunked query, so
/// trash listing is not a membership query per paper. Papers in no project are
/// simply absent.
pub fn project_fks_by_source_fk(
    conn: &Connection,
    source_fks: &[i64],
) -> Result<HashMap<i64, Vec<i64>>> {
    let mut by_paper: HashMap<i64, Vec<i64>> = HashMap::with_capacity(source_fks.len());
    for chunk in source_fks.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        // Per-paper ordering survives chunking: a paper's rows never span chunks.
        let sql = format!(
            "SELECT SOURCE_FK, PROJECT_FK FROM PROJECT_TO_PAPER \
             WHERE SOURCE_FK IN ({placeholders}) ORDER BY PROJECT_TO_PAPER_FK"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (sfk, pfk) = row?;
            by_paper.entry(sfk).or_default().push(pfk);
        }
    }
    Ok(by_paper)
}

/// Remove a paper from every project; returns the FKs it was removed from.
/// Python `remove_paper_from_all_projects` (select-then-delete, transactional).
pub fn remove_paper_from_all_projects(conn: &mut Connection, source_fk: i64) -> Result<Vec<i64>> {
    transaction(conn, |tx| {
        let fks = get_paper_project_fks(tx, source_fk)?;
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
        // same set as existing {10, 11} (dedup applies) — reorder is a no-op.
        replace_papers(&mut conn, id, &[11, 11, 10]).unwrap();
        assert_eq!(
            get_project(&conn, id, true).unwrap().unwrap().source_fks,
            vec![10, 11]
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

    fn seed_reading_list_project(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES (10, 'arxiv:1', 'active');
             INSERT INTO PROJECT (PROJECT_FK, NAME, IS_READING_LIST, STATUS, CREATED_AT, UPDATED_AT) VALUES
                 (1, 'RL', 1, 'active', '2024-01-01T00:00:00', '2024-01-01T00:00:00');",
        )
        .unwrap();
    }

    #[test]
    fn remove_papers_clears_reading_status_and_readd_does_not_resurrect_it() {
        use crate::storage::queries::reading_list::{
            get_reading_status, set_reading_status, ReadingStatus,
        };

        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed_reading_list_project(&conn);

        add_papers(&conn, 1, &[10]).unwrap();
        set_reading_status(&conn, 1, 10, ReadingStatus::Read).unwrap();
        assert_eq!(
            get_reading_status(&conn, 1, 10).unwrap(),
            ReadingStatus::Read
        );

        // (b) removing the paper from the project cascades to drop its reading row.
        remove_papers(&conn, 1, &[10]).unwrap();
        assert_eq!(
            get_reading_status(&conn, 1, 10).unwrap(),
            ReadingStatus::Unread
        );

        // (c) re-adding it starts fresh — no resurrected status from the orphaned row.
        add_papers(&conn, 1, &[10]).unwrap();
        assert_eq!(
            get_reading_status(&conn, 1, 10).unwrap(),
            ReadingStatus::Unread
        );
    }

    #[test]
    fn remove_paper_from_all_projects_clears_reading_status() {
        use crate::storage::queries::reading_list::{
            get_reading_status, set_reading_status, ReadingStatus,
        };

        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed_reading_list_project(&conn);

        add_papers(&conn, 1, &[10]).unwrap();
        set_reading_status(&conn, 1, 10, ReadingStatus::Reading).unwrap();

        remove_paper_from_all_projects(&mut conn, 10).unwrap();
        assert_eq!(
            get_reading_status(&conn, 1, 10).unwrap(),
            ReadingStatus::Unread
        );
    }

    /// A retained paper must keep its row (and reading status) — CASCADE fires
    /// per-statement, so even a same-set reinsert would otherwise wipe it.
    #[test]
    fn replace_papers_retains_untouched_papers_reading_status() {
        use crate::storage::queries::reading_list::{
            get_reading_status, set_reading_status, ReadingStatus,
        };

        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed_reading_list_project(&conn);

        add_papers(&conn, 1, &[10]).unwrap();
        set_reading_status(&conn, 1, 10, ReadingStatus::Read).unwrap();

        // paper 10 stays in the list — nothing "removed" from the caller's POV.
        replace_papers(&mut conn, 1, &[10]).unwrap();

        assert_eq!(get_paper_project_fks(&conn, 10).unwrap(), vec![1]);
        assert_eq!(
            get_reading_status(&conn, 1, 10).unwrap(),
            ReadingStatus::Read
        );
    }

    #[test]
    fn save_source_fks_partial_removal_only_touches_removed_paper() {
        use crate::storage::queries::reading_list::{
            get_reading_status, set_reading_status, ReadingStatus,
        };

        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES
                 (10, 'arxiv:1', 'active'), (11, 'arxiv:2', 'active'), (12, 'arxiv:3', 'active');
             INSERT INTO PROJECT (PROJECT_FK, NAME, IS_READING_LIST, STATUS, CREATED_AT, UPDATED_AT) VALUES
                 (1, 'RL', 1, 'active', '2024-01-01T00:00:00', '2024-01-01T00:00:00');",
        )
        .unwrap();

        add_papers(&conn, 1, &[10, 11]).unwrap();
        set_reading_status(&conn, 1, 10, ReadingStatus::Read).unwrap();
        set_reading_status(&conn, 1, 11, ReadingStatus::Reading).unwrap();

        // no-op save (same set, same order): reading status for both survives.
        replace_papers(&mut conn, 1, &[10, 11]).unwrap();
        assert_eq!(
            get_reading_status(&conn, 1, 10).unwrap(),
            ReadingStatus::Read
        );
        assert_eq!(
            get_reading_status(&conn, 1, 11).unwrap(),
            ReadingStatus::Reading
        );

        // partial removal: 11 drops out, 12 is new, 10 stays untouched.
        replace_papers(&mut conn, 1, &[10, 12]).unwrap();
        assert_eq!(
            get_project(&conn, 1, true).unwrap().unwrap().source_fks,
            vec![10, 12]
        );
        // 10 kept its row and reading status — not wiped by an incidental reinsert.
        assert_eq!(
            get_reading_status(&conn, 1, 10).unwrap(),
            ReadingStatus::Read
        );
        // 11 was genuinely removed: membership gone and its reading row cascaded away.
        assert!(get_paper_project_fks(&conn, 11).unwrap().is_empty());
        assert_eq!(
            get_reading_status(&conn, 1, 11).unwrap(),
            ReadingStatus::Unread
        );
        // 12 is new, starts unread.
        assert_eq!(
            get_reading_status(&conn, 1, 12).unwrap(),
            ReadingStatus::Unread
        );
    }

    #[test]
    fn project_fks_by_source_fk_batches_any_status_in_insertion_order() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID, STATUS) VALUES
                 (10, 'arxiv:1', 'active'), (11, 'arxiv:2', 'active'), (12, 'arxiv:3', 'active');",
        )
        .unwrap();
        let a = insert_project(&conn, "A", "d", None, Status::Active, None).unwrap();
        let b = insert_project(&conn, "B", "d", None, Status::Deleted, None).unwrap();
        add_papers(&conn, a, &[10, 11]).unwrap();
        add_papers(&conn, b, &[11]).unwrap();

        let by_paper = project_fks_by_source_fk(&conn, &[10, 11, 12, 999]).unwrap();
        assert_eq!(by_paper[&10], vec![a]);
        // Any status (deleted project b included), PROJECT_TO_PAPER_FK order.
        assert_eq!(by_paper[&11], vec![a, b]);
        assert!(!by_paper.contains_key(&12));
        assert!(!by_paper.contains_key(&999));
        assert!(project_fks_by_source_fk(&conn, &[]).unwrap().is_empty());
    }

    #[test]
    fn ensure_share_id_idempotent_first_candidate_wins() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        let id = insert_project(&conn, "P", "d", None, Status::Active, None).unwrap();

        // First call with "first_id" sets SHARE_ID.
        let result1 = ensure_share_id(&conn, id, "first_id").unwrap();
        assert_eq!(result1, Some("first_id".to_string()));

        // Second call with "second_id" sees existing SHARE_ID and returns it unchanged.
        let result2 = ensure_share_id(&conn, id, "second_id").unwrap();
        assert_eq!(result2, Some("first_id".to_string()));

        // Verify the project has the first candidate.
        let p = get_project(&conn, id, false).unwrap().unwrap();
        assert_eq!(p.share_id, Some("first_id".to_string()));
    }
}
