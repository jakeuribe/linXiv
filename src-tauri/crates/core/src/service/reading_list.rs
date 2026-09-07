//! reading-list service — per-paper reading status in a reading-list project.
//!
//! Thin delegation over `storage::queries::reading_list`. Status is stored
//! sparsely: the default `Unread` is the absence of a row, so setting a paper
//! back to `Unread` deletes it. DB-touching fns take `conn: &Connection` first.
//!
//! `set` rejects a PROJECT_FK that is not a reading list (no reserved
//! `reading-list` tag — see `is_reading_list_project` for the source-of-truth
//! note); a missing PROJECT_FK still fails via the foreign key constraint.

use rusqlite::Connection;

use crate::error::{CoreError, Result};
use crate::storage::queries::reading_list as q;
pub use crate::storage::queries::reading_list::ReadingStatus;

/// This paper's reading status in the given reading list. `Unread` if unset.
pub fn get(conn: &Connection, project_fk: i64, source_fk: i64) -> Result<ReadingStatus> {
    q::get_reading_status(conn, project_fk, source_fk)
}

/// Set this paper's reading status. `Unread` clears it (row deleted).
pub fn set(
    conn: &Connection,
    project_fk: i64,
    source_fk: i64,
    status: ReadingStatus,
) -> Result<()> {
    if q::is_reading_list_project(conn, project_fk)? == Some(false) {
        return Err(CoreError::BadRequest(format!(
            "project {project_fk} is not a reading list"
        )));
    }
    q::set_reading_status(conn, project_fk, source_fk, status)
}

// ── Global-per-paper view (the wire surface) ─────────────────────────────────
//
// The two keying models meet here. The frontend shows ONE status per paper —
// the same pill in every list a paper appears in — while the table keys rows
// per (PROJECT_FK, SOURCE_FK), and its composite FK means a row can only exist
// where a membership row does. Resolution: `set_for_paper` fans one write out
// to every reading list the paper belongs to, and `statuses` aggregates back
// to one entry per paper (latest write wins where lists disagree, e.g. a paper
// added to a second list after being marked). This preserves the shipped UX
// (a status set in one list shows in all of them) without loosening the FK.

/// One status per paper across all reading lists, keyed by SOURCE_ID. Sparse:
/// `Unread` papers are absent.
pub fn statuses(conn: &Connection) -> Result<Vec<(String, ReadingStatus)>> {
    q::statuses_by_source_id(conn)
}

/// `GET /api/reading-status` envelope (route/reading_status.rs) — a sparse `SOURCE_ID → status` map: unread papers are absent.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
pub struct ReadingStatusesResponse {
    #[ts(type = "Record<string, \"reading\" | \"read\">")]
    pub statuses: serde_json::Map<String, serde_json::Value>,
}

/// [`statuses`] in the wire envelope shape.
pub fn statuses_response(conn: &Connection) -> Result<ReadingStatusesResponse> {
    let mut map = serde_json::Map::new();
    for (sid, status) in statuses(conn)? {
        // as_str is Some for every stored row (Unread is never stored).
        if let Some(s) = status.as_str() {
            map.insert(sid, serde_json::Value::String(s.to_string()));
        }
    }
    Ok(ReadingStatusesResponse { statuses: map })
}

/// `PUT /api/reading-status/{source_id}` envelope; `applied` = reading lists written (0 = no-op).
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
pub struct ReadingStatusReceipt {
    pub ok: bool,
    pub applied: usize,
}

/// Set `source_id`'s status in every non-trashed reading list it belongs to,
/// atomically. Returns the number of lists written — 0 (a no-op, not an error)
/// when the paper is on no reading list. `PaperNotFound` for an unknown id.
pub fn set_for_paper(
    conn: &mut Connection,
    source_id: &str,
    status: ReadingStatus,
) -> Result<usize> {
    let source_fk = crate::service::paper::resolve_source_fk(conn, source_id)?;
    let fks = q::reading_list_fks_for_paper(conn, source_fk)?;
    crate::storage::db::transaction(conn, |tx| {
        for &project_fk in &fks {
            q::set_reading_status(tx, project_fk, source_fk, status)?;
        }
        Ok(fks.len())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::open_in_memory;
    use crate::storage::init_db;

    fn setup() -> Connection {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (10, 'RL')",
            [],
        )
        .unwrap();
        // The reserved tag is what makes project 10 a reading list.
        conn.execute(
            "INSERT INTO TAG (TAG_FK, TAG) VALUES (1, 'reading-list')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_TAG (PROJECT_FK, TAG_FK) VALUES (10, 1)",
            [],
        )
        .unwrap();
        // PAPER_TO_READING's composite FK requires project membership to exist first.
        conn.execute(
            "INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK) VALUES (1, 10, 1)",
            [],
        )
        .unwrap();
        conn
    }

    fn row_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM PAPER_TO_READING", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn set_read_back_then_default_removes_row() {
        let conn = setup();
        // default is Unread with no row.
        assert_eq!(get(&conn, 10, 1).unwrap(), ReadingStatus::Unread);
        assert_eq!(row_count(&conn), 0);

        set(&conn, 10, 1, ReadingStatus::Read).unwrap();
        assert_eq!(get(&conn, 10, 1).unwrap(), ReadingStatus::Read);
        assert_eq!(row_count(&conn), 1);

        // back to default → row gone.
        set(&conn, 10, 1, ReadingStatus::Unread).unwrap();
        assert_eq!(get(&conn, 10, 1).unwrap(), ReadingStatus::Unread);
        assert_eq!(row_count(&conn), 0);
    }

    #[test]
    fn set_reading_then_read_upserts_single_row() {
        let conn = setup();
        set(&conn, 10, 1, ReadingStatus::Reading).unwrap();
        assert_eq!(get(&conn, 10, 1).unwrap(), ReadingStatus::Reading);
        assert_eq!(row_count(&conn), 1);

        set(&conn, 10, 1, ReadingStatus::Read).unwrap();
        assert_eq!(get(&conn, 10, 1).unwrap(), ReadingStatus::Read);
        assert_eq!(row_count(&conn), 1);
    }

    #[test]
    fn set_requires_reading_list_project() {
        let conn = setup();
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (20, 'Not RL')",
            [],
        )
        .unwrap();

        let err = set(&conn, 20, 1, ReadingStatus::Read).unwrap_err();
        assert!(matches!(err, CoreError::BadRequest(_)));

        // still works on a project carrying the reading-list tag.
        set(&conn, 10, 1, ReadingStatus::Read).unwrap();
        assert_eq!(get(&conn, 10, 1).unwrap(), ReadingStatus::Read);
    }

    #[test]
    fn set_for_paper_fans_out_to_every_reading_list() {
        let mut conn = setup();
        // A second reading list also holding the paper, plus a plain project
        // (holding it too) that must NOT receive a status row.
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (11, 'RL2')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_TAG (PROJECT_FK, TAG_FK) VALUES (11, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (20, 'P')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK) \
             VALUES (2, 11, 1), (3, 20, 1)",
            [],
        )
        .unwrap();

        assert_eq!(
            set_for_paper(&mut conn, "arxiv:1", ReadingStatus::Reading).unwrap(),
            2
        );
        assert_eq!(get(&conn, 10, 1).unwrap(), ReadingStatus::Reading);
        assert_eq!(get(&conn, 11, 1).unwrap(), ReadingStatus::Reading);
        assert_eq!(get(&conn, 20, 1).unwrap(), ReadingStatus::Unread);

        assert_eq!(
            statuses(&conn).unwrap(),
            vec![("arxiv:1".to_string(), ReadingStatus::Reading)]
        );

        // Unread clears both rows.
        assert_eq!(
            set_for_paper(&mut conn, "arxiv:1", ReadingStatus::Unread).unwrap(),
            2
        );
        assert_eq!(statuses(&conn).unwrap(), vec![]);
    }

    #[test]
    fn set_for_paper_unknown_paper_and_unlisted_paper() {
        let mut conn = setup();
        let err = set_for_paper(&mut conn, "ghost", ReadingStatus::Read).unwrap_err();
        assert!(matches!(err, CoreError::PaperNotFound(_)));

        // A real paper on no reading list: a 0-list no-op, not an error.
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (2, 'arxiv:2')",
            [],
        )
        .unwrap();
        assert_eq!(
            set_for_paper(&mut conn, "arxiv:2", ReadingStatus::Read).unwrap(),
            0
        );
    }
}
