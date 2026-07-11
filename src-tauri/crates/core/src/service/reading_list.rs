//! reading-list service — per-paper reading status in a reading-list project.
//!
//! Thin delegation over `storage::queries::reading_list`. Status is stored
//! sparsely: the default `Unread` is the absence of a row, so setting a paper
//! back to `Unread` deletes it. DB-touching fns take `conn: &Connection` first.
//!
//! `set` rejects a PROJECT_FK whose PROJECT.IS_READING_LIST is 0; a missing
//! PROJECT_FK still fails via the foreign key constraint.

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
            "INSERT INTO PROJECT (PROJECT_FK, NAME, IS_READING_LIST) VALUES (20, 'Not RL', 0)",
            [],
        )
        .unwrap();

        let err = set(&conn, 20, 1, ReadingStatus::Read).unwrap_err();
        assert!(matches!(err, CoreError::BadRequest(_)));

        // still works on a project with IS_READING_LIST=1.
        set(&conn, 10, 1, ReadingStatus::Read).unwrap();
        assert_eq!(get(&conn, 10, 1).unwrap(), ReadingStatus::Read);
    }
}
