//! PAPER_TO_READING named queries — sparse per-paper reading status inside a
//! reading-list PROJECT. `Unread` is the default and is stored as the ABSENCE
//! of a row, so `set` deletes the row for `Unread` and upserts otherwise.
//!
//! Callers must pass a connection opened via `storage::db::open` (FK cascades
//! depend on its `foreign_keys` PRAGMA).

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{CoreError, Result};

/// A paper's reading status within a reading list. The default `Unread` is never
/// stored — it is the absence of a PAPER_TO_READING row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadingStatus {
    Unread,
    Reading,
    Read,
}

impl ReadingStatus {
    /// SQL token for the non-default variants; `None` for `Unread` (no row).
    fn to_sql(self) -> Option<&'static str> {
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
    match status {
        Some(s) => ReadingStatus::from_sql(&s),
        None => Ok(ReadingStatus::Unread),
    }
}

/// `PROJECT.IS_READING_LIST` for the given project, or `None` if the project
/// row doesn't exist.
pub fn is_reading_list_project(conn: &Connection, project_fk: i64) -> Result<Option<bool>> {
    let flag: Option<i64> = conn
        .query_row(
            "SELECT IS_READING_LIST FROM PROJECT WHERE PROJECT_FK = ?1",
            params![project_fk],
            |r| r.get(0),
        )
        .optional()?;
    Ok(flag.map(|v| v != 0))
}

/// Set this paper's reading status. `Unread` deletes the row (back to default);
/// any other status upserts.
pub fn set_reading_status(
    conn: &Connection,
    project_fk: i64,
    source_fk: i64,
    status: ReadingStatus,
) -> Result<()> {
    match status.to_sql() {
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
            "INSERT INTO PROJECT (PROJECT_FK, NAME, IS_READING_LIST) VALUES (10, 'RL', 1)",
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
    fn is_reading_list_project_reflects_flag_and_absence() {
        let conn = setup();
        assert_eq!(is_reading_list_project(&conn, 10).unwrap(), Some(true));
        assert_eq!(is_reading_list_project(&conn, 999).unwrap(), None);
    }
}
