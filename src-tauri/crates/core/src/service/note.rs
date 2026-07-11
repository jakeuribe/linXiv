//! note service — Rust port of `service/note.py`.
//!
//! Thin delegation over `storage::queries::note`. DB-touching fns take
//! `conn: &Connection` as their first param (DI seam — never open from config).
//! The `Note`/`Notes` query objects below are the ONE lookup seam (D17); the
//! redundant 1-line named wrappers Python kept are dropped.

use crate::error::{CoreError, Result};
use crate::models::{NoteDetails, NoteIn, NoteUpdateIn};
use crate::storage::queries::note as q;
use rusqlite::Connection;

/// `service/note.py::Note` — single-note lookup key (NOTE_SK). `None` short-circuits
/// to a None/false result, matching Python's `if note.note_id is None`.
#[derive(Debug, Clone, Default)]
pub struct Note {
    pub note_id: Option<i64>,
}

/// `service/note.py::Notes` — multi-note filter. Valid combos: `source_fk`
/// (+ optional `project_fk`/`all_projects`), `paper_id` alone, or `project_fk`
/// alone. Priority order (source_fk > paper_id > project_fk) is load-bearing.
#[derive(Debug, Clone, Default)]
pub struct Notes {
    pub source_fk: Option<i64>,
    pub paper_id: Option<i64>,
    pub project_fk: Option<i64>,
    pub all_projects: bool,
}

/// Fetch a single note by note_id. `Ok(None)` if absent or note_id unset.
pub fn get(conn: &Connection, note: &Note) -> Result<Option<NoteDetails>> {
    match note.note_id {
        Some(id) => Ok(q::get_note(conn, id)?),
        None => Ok(None),
    }
}

/// Every note, CREATED_AT ASC.
pub fn list_all(conn: &Connection) -> Result<Vec<NoteDetails>> {
    Ok(q::list_all_notes(conn)?)
}

/// Fetch notes by filter. Invalid combinations raise `Validation` (Python's
/// `ValueError`). Use `list_all` to fetch every note unfiltered.
pub fn get_many(conn: &Connection, notes: &Notes) -> Result<Vec<NoteDetails>> {
    if notes.paper_id.is_some() && (notes.source_fk.is_some() || notes.project_fk.is_some()) {
        return Err(CoreError::Validation(
            "paper_id cannot be combined with source_fk or project_fk".into(),
        ));
    }
    if notes.all_projects && notes.project_fk.is_some() {
        return Err(CoreError::Validation(
            "all_projects=True cannot be combined with a specific project_fk".into(),
        ));
    }
    if notes.all_projects && notes.paper_id.is_some() {
        return Err(CoreError::Validation(
            "all_projects=True cannot be combined with paper_id".into(),
        ));
    }
    if let Some(source_fk) = notes.source_fk {
        Ok(q::get_notes(
            conn,
            source_fk,
            notes.project_fk,
            notes.all_projects,
        )?)
    } else if let Some(paper_id) = notes.paper_id {
        Ok(q::get_notes_by_paper_id(conn, paper_id)?)
    } else if let Some(project_fk) = notes.project_fk {
        Ok(q::get_project_notes(conn, project_fk)?)
    } else {
        Err(CoreError::Validation(
            "at least one filter field must be set on Notes".into(),
        ))
    }
}

/// Insert a new note. Returns NOTE_SK.
pub fn create(conn: &Connection, note: &NoteIn) -> Result<i64> {
    Ok(q::create_note(
        conn,
        note.source_fk,
        note.paper_id,
        note.project_fk,
        &note.title,
        &note.content,
    )?)
}

/// Delete a note by note_id. `false` if absent or note_id unset.
pub fn delete(conn: &Connection, note: &Note) -> Result<bool> {
    match note.note_id {
        Some(id) => Ok(q::delete_note(conn, id)?),
        None => Ok(false),
    }
}

/// Partial update via COALESCE (a `None` field leaves its column unchanged).
/// `false` if no row matched. Enforces "at least one of title/content provided"
/// (Python `NoteUpdateIn.__post_init__`).
pub fn update(conn: &Connection, note: &NoteUpdateIn) -> Result<bool> {
    if note.title.is_none() && note.content.is_none() {
        return Err(CoreError::Validation(
            "at least one of title or content must be provided".into(),
        ));
    }
    Ok(q::patch_note(
        conn,
        note.note_id,
        note.title.as_deref(),
        note.content.as_deref(),
    )?)
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
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1'), (2, 'arxiv:2')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (10, 'P')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn create_get_update_delete_roundtrip() {
        let conn = setup();
        let id = create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "t1".into(),
                content: "body1".into(),
                paper_id: None,
                project_fk: Some(10),
            },
        )
        .unwrap();

        let got = get(&conn, &Note { note_id: Some(id) }).unwrap().unwrap();
        assert_eq!(got.title, "t1");
        assert_eq!(got.project_id, Some(10));

        // partial update: None leaves title; Some replaces content.
        assert!(update(
            &conn,
            &NoteUpdateIn {
                note_id: id,
                title: None,
                content: Some("body2".into())
            },
        )
        .unwrap());
        let got = get(&conn, &Note { note_id: Some(id) }).unwrap().unwrap();
        assert_eq!(got.title, "t1");
        assert_eq!(got.content, "body2");

        assert!(delete(&conn, &Note { note_id: Some(id) }).unwrap());
        assert!(get(&conn, &Note { note_id: Some(id) }).unwrap().is_none());
    }

    #[test]
    fn unset_note_id_short_circuits() {
        let conn = setup();
        assert!(get(&conn, &Note { note_id: None }).unwrap().is_none());
        assert!(!delete(&conn, &Note { note_id: None }).unwrap());
        // absent row also reports false / None, not an error.
        assert!(get(&conn, &Note { note_id: Some(999) }).unwrap().is_none());
        assert!(!delete(&conn, &Note { note_id: Some(999) }).unwrap());
    }

    #[test]
    fn update_requires_a_field() {
        let conn = setup();
        let err = update(
            &conn,
            &NoteUpdateIn {
                note_id: 1,
                title: None,
                content: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
    }

    #[test]
    fn get_many_filters_and_priority() {
        let conn = setup();
        // PAPER row so PAPER_ID_FK has a referent.
        conn.execute(
            "INSERT INTO PAPER (PAPER_ID, SOURCE_FK, SOURCE_ID, VERSION, TITLE) \
             VALUES (100, 1, 'arxiv:1', 1, 'T')",
            [],
        )
        .unwrap();
        create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "lib".into(),
                content: "x".into(),
                paper_id: None,
                project_fk: None,
            },
        )
        .unwrap();
        create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "proj".into(),
                content: "y".into(),
                paper_id: None,
                project_fk: Some(10),
            },
        )
        .unwrap();
        create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "pinned".into(),
                content: "z".into(),
                paper_id: Some(100),
                project_fk: None,
            },
        )
        .unwrap();

        // source_fk + no project => library notes (PROJECT_FK NULL).
        let lib = get_many(
            &conn,
            &Notes {
                source_fk: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(lib.len(), 2); // lib + pinned, both PROJECT_FK NULL

        // all_projects => every note for the source.
        let all = get_many(
            &conn,
            &Notes {
                source_fk: Some(1),
                all_projects: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 3);

        // project_fk alone.
        let proj = get_many(
            &conn,
            &Notes {
                project_fk: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(proj.len(), 1);

        // paper_id alone.
        let pinned = get_many(
            &conn,
            &Notes {
                paper_id: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].title, "pinned");

        // source_fk takes priority over paper_id — but the combo is rejected.
        let err = get_many(
            &conn,
            &Notes {
                source_fk: Some(1),
                paper_id: Some(100),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
    }

    #[test]
    fn get_many_rejects_bad_combos_and_empty() {
        let conn = setup();
        assert!(matches!(
            get_many(
                &conn,
                &Notes {
                    all_projects: true,
                    project_fk: Some(10),
                    ..Default::default()
                }
            )
            .unwrap_err(),
            CoreError::Validation(_)
        ));
        assert!(matches!(
            get_many(
                &conn,
                &Notes {
                    all_projects: true,
                    paper_id: Some(1),
                    ..Default::default()
                }
            )
            .unwrap_err(),
            CoreError::Validation(_)
        ));
        assert!(matches!(
            get_many(&conn, &Notes::default()).unwrap_err(),
            CoreError::Validation(_)
        ));
    }

    #[test]
    fn counts_and_list_all() {
        let conn = setup();
        create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "a".into(),
                content: "x".into(),
                paper_id: None,
                project_fk: Some(10),
            },
        )
        .unwrap();
        create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "b".into(),
                content: "y".into(),
                paper_id: None,
                project_fk: None,
            },
        )
        .unwrap();
        create(
            &conn,
            &NoteIn {
                source_fk: 2,
                title: "c".into(),
                content: "z".into(),
                paper_id: None,
                project_fk: Some(10),
            },
        )
        .unwrap();

        assert_eq!(list_all(&conn).unwrap().len(), 3);
    }
}
