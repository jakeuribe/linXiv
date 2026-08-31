//! PAPER_TO_READING named queries — sparse per-paper reading status inside a
//! reading-list PROJECT. `Unread` is the default and is stored as the ABSENCE
//! of a row, so `set` deletes the row for `Unread` and upserts otherwise.
//!
//! Callers must pass a connection opened via `storage::db::open` (FK cascades
//! depend on its `foreign_keys` PRAGMA).

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{CoreError, Result};
use crate::storage::queries::tag::READING_LIST_TAG;

/// A paper's reading status within a reading list. The default `Unread` is never
/// stored — it is the absence of a PAPER_TO_READING row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadingStatus {
    Unread,
    Reading,
    Read,
}

impl ReadingStatus {
    /// SQL/wire token for the non-default variants; `None` for `Unread` (no row,
    /// and never emitted on the wire — the statuses map is sparse).
    pub fn as_str(self) -> Option<&'static str> {
        match self {
            ReadingStatus::Unread => None,
            ReadingStatus::Reading => Some("reading"),
            ReadingStatus::Read => Some("read"),
        }
    }

    fn from_sql(s: &str) -> Result<Self> {
        match s {
            "reading" => Ok(ReadingStatus::Reading),
            "read" => Ok(ReadingStatus::Read),
            other => Err(CoreError::Internal(format!(
                "unknown reading status {other:?}"
            ))),
        }
    }
}

impl std::str::FromStr for ReadingStatus {
    type Err = CoreError;

    /// Wire parse; unlike the row values, `"unread"` is accepted (a request to
    /// clear the status).
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "unread" => Ok(ReadingStatus::Unread),
            "reading" => Ok(ReadingStatus::Reading),
            "read" => Ok(ReadingStatus::Read),
            other => Err(CoreError::Validation(format!(
                "Invalid reading status {other:?}. Use 'unread', 'reading', or 'read'."
            ))),
        }
    }
}

/// This paper's reading status in the given reading list. `Unread` if no row.
pub fn get_reading_status(
    conn: &Connection,
    project_fk: i64,
    source_fk: i64,
) -> Result<ReadingStatus> {
    let status: Option<String> = conn
        .query_row(
            "SELECT STATUS FROM PAPER_TO_READING WHERE PROJECT_FK = ?1 AND SOURCE_FK = ?2",
            params![project_fk, source_fk],
            |r| r.get(0),
        )
        .optional()?;
    status.map_or(Ok(ReadingStatus::Unread), |s| ReadingStatus::from_sql(&s))
}

/// Whether the project is a reading list, or `None` if the project row doesn't
/// exist.
///
/// Source of truth: the reserved `reading-list` project tag — the same signal
/// the frontend's `isReadingListProject` reads and the only one any code path
/// ever sets (project create/edit with `READING_LIST_TAG` in `project_tags`).
/// `PROJECT.IS_READING_LIST` is superseded: no write path ever set it, so
/// deriving from it would have rejected every real reading list; the column
/// remains in the schema (dropping it is a table rebuild for no gain) but has
/// no readers.
pub fn is_reading_list_project(conn: &Connection, project_fk: i64) -> Result<Option<bool>> {
    let flag: Option<bool> = conn
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM PROJECT_TO_TAG pt JOIN TAG t ON t.TAG_FK = pt.TAG_FK
                WHERE pt.PROJECT_FK = p.PROJECT_FK AND t.TAG = ?2 COLLATE NOCASE)
             FROM PROJECT p WHERE p.PROJECT_FK = ?1",
            params![project_fk, READING_LIST_TAG],
            |r| r.get(0),
        )
        .optional()?;
    Ok(flag)
}

/// PROJECT_FKs of every non-trashed reading list this paper is a member of —
/// the write fan-out set for [`set_reading_status`]. Trashed (`deleted`)
/// projects are skipped so a status write can't silently touch rows the UI
/// hides; archived lists are included (their statuses stay visible).
pub fn reading_list_fks_for_paper(conn: &Connection, source_fk: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT ptp.PROJECT_FK FROM PROJECT_TO_PAPER ptp
         JOIN PROJECT p ON p.PROJECT_FK = ptp.PROJECT_FK
         WHERE ptp.SOURCE_FK = ?1 AND p.STATUS != 'deleted'
           AND EXISTS (
             SELECT 1 FROM PROJECT_TO_TAG pt JOIN TAG t ON t.TAG_FK = pt.TAG_FK
             WHERE pt.PROJECT_FK = ptp.PROJECT_FK AND t.TAG = ?2 COLLATE NOCASE)",
    )?;
    let fks = stmt
        .query_map(params![source_fk, READING_LIST_TAG], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(fks)
}

/// One aggregated status per paper across all non-trashed reading lists, keyed
/// by SOURCE_ID — the read side of the frontend's global-per-paper keying (see
/// `service::reading_list`). Rows normally agree because writes fan out to
/// every list; where they don't (a paper added to a second list after being
/// marked), the most recently updated row wins. The bare STATUS column is
/// SQLite's documented MIN/MAX bare-column behavior: it comes from the
/// MAX(UPDATED_AT) row.
pub fn statuses_by_source_id(conn: &Connection) -> Result<Vec<(String, ReadingStatus)>> {
    let mut stmt = conn.prepare(
        "SELECT r.SOURCE_ID, ptr.STATUS, MAX(ptr.UPDATED_AT)
         FROM PAPER_TO_READING ptr
         JOIN PAPER_ROOTS r ON r.SOURCE_FK = ptr.SOURCE_FK
         JOIN PROJECT p ON p.PROJECT_FK = ptr.PROJECT_FK AND p.STATUS != 'deleted'
         GROUP BY ptr.SOURCE_FK",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (sid, status) = row?;
        out.push((sid, ReadingStatus::from_sql(&status)?));
    }
    Ok(out)
}

/// Set this paper's reading status. `Unread` deletes the row (back to default);
/// any other status upserts.
pub fn set_reading_status(
    conn: &Connection,
    project_fk: i64,
    source_fk: i64,
    status: ReadingStatus,
) -> Result<()> {
    match status.as_str() {
        None => {
            conn.execute(
                "DELETE FROM PAPER_TO_READING WHERE PROJECT_FK = ?1 AND SOURCE_FK = ?2",
                params![project_fk, source_fk],
            )?;
        }
        Some(sql) => {
            conn.execute(
                "INSERT INTO PAPER_TO_READING (PROJECT_FK, SOURCE_FK, STATUS, UPDATED_AT) \
                 VALUES (?1, ?2, ?3, datetime('now')) \
                 ON CONFLICT (PROJECT_FK, SOURCE_FK) \
                 DO UPDATE SET STATUS = ?3, UPDATED_AT = datetime('now')",
                params![project_fk, source_fk, sql],
            )?;
        }
    }
    Ok(())
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
        assert_eq!(
            get_reading_status(&conn, 10, 1).unwrap(),
            ReadingStatus::Unread
        );
        assert_eq!(row_count(&conn), 0);

        set_reading_status(&conn, 10, 1, ReadingStatus::Read).unwrap();
        assert_eq!(
            get_reading_status(&conn, 10, 1).unwrap(),
            ReadingStatus::Read
        );
        assert_eq!(row_count(&conn), 1);

        // back to default → row gone.
        set_reading_status(&conn, 10, 1, ReadingStatus::Unread).unwrap();
        assert_eq!(
            get_reading_status(&conn, 10, 1).unwrap(),
            ReadingStatus::Unread
        );
        assert_eq!(row_count(&conn), 0);
    }

    #[test]
    fn set_reading_then_read_upserts_single_row() {
        let conn = setup();
        set_reading_status(&conn, 10, 1, ReadingStatus::Reading).unwrap();
        assert_eq!(
            get_reading_status(&conn, 10, 1).unwrap(),
            ReadingStatus::Reading
        );
        assert_eq!(row_count(&conn), 1);

        set_reading_status(&conn, 10, 1, ReadingStatus::Read).unwrap();
        assert_eq!(
            get_reading_status(&conn, 10, 1).unwrap(),
            ReadingStatus::Read
        );
        assert_eq!(row_count(&conn), 1);
    }

    #[test]
    fn is_reading_list_project_reflects_tag_and_absence() {
        let conn = setup();
        assert_eq!(is_reading_list_project(&conn, 10).unwrap(), Some(true));
        // Untagged project → not a reading list (the IS_READING_LIST column is ignored).
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME, IS_READING_LIST) VALUES (20, 'P', 1)",
            [],
        )
        .unwrap();
        assert_eq!(is_reading_list_project(&conn, 20).unwrap(), Some(false));
        assert_eq!(is_reading_list_project(&conn, 999).unwrap(), None);
    }

    #[test]
    fn reading_list_fks_skip_non_lists_and_trashed_lists() {
        let conn = setup();
        // A plain project and a trashed reading list, both holding the paper.
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (20, 'P')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME, STATUS) VALUES (30, 'RL2', 'deleted')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_TAG (PROJECT_FK, TAG_FK) VALUES (30, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK) \
             VALUES (2, 20, 1), (3, 30, 1)",
            [],
        )
        .unwrap();

        assert_eq!(reading_list_fks_for_paper(&conn, 1).unwrap(), vec![10]);
        assert_eq!(
            reading_list_fks_for_paper(&conn, 999).unwrap(),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn statuses_aggregate_per_paper_and_hide_trashed_lists() {
        let conn = setup();
        set_reading_status(&conn, 10, 1, ReadingStatus::Reading).unwrap();
        assert_eq!(
            statuses_by_source_id(&conn).unwrap(),
            vec![("arxiv:1".to_string(), ReadingStatus::Reading)]
        );

        // A second reading list with a later-updated row for the same paper wins.
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (30, 'RL2')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_TAG (PROJECT_FK, TAG_FK) VALUES (30, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT_TO_PAPER (PROJECT_TO_PAPER_FK, PROJECT_FK, SOURCE_FK) VALUES (3, 30, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PAPER_TO_READING (PROJECT_FK, SOURCE_FK, STATUS, UPDATED_AT) \
             VALUES (30, 1, 'read', datetime('now', '+1 hour'))",
            [],
        )
        .unwrap();
        assert_eq!(
            statuses_by_source_id(&conn).unwrap(),
            vec![("arxiv:1".to_string(), ReadingStatus::Read)]
        );

        // Trashing the winning list hides its row; the older one resurfaces.
        conn.execute(
            "UPDATE PROJECT SET STATUS = 'deleted' WHERE PROJECT_FK = 30",
            [],
        )
        .unwrap();
        assert_eq!(
            statuses_by_source_id(&conn).unwrap(),
            vec![("arxiv:1".to_string(), ReadingStatus::Reading)]
        );
    }
}
