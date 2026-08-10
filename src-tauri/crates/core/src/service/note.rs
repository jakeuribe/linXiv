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

/// Notes for an optional paper and/or project — the CLI/MCP `list` scoping rule.
/// A paper with no project filter spans the library and every project
/// (`all_projects`); neither filter set lists everything.
pub fn list_filtered(
    conn: &Connection,
    source_fk: Option<i64>,
    project_fk: Option<i64>,
) -> Result<Vec<NoteDetails>> {
    if source_fk.is_none() && project_fk.is_none() {
        return list_all(conn);
    }
    get_many(
        conn,
        &Notes {
            source_fk,
            project_fk,
            all_projects: source_fk.is_some() && project_fk.is_none(),
            ..Default::default()
        },
    )
}

/// Insert a new note. Returns NOTE_SK.
pub fn create(conn: &Connection, note: &NoteIn) -> Result<i64> {
    let uuid: Option<String> = match &note.uuid {
        Some(u) => crate::models::resolve_uuid(u, |n| q::uuid_taken(conn, n).map_err(Into::into))?,
        None => None,
    };
    Ok(q::create_note(
        conn,
        note.source_fk,
        note.paper_id,
        note.project_fk,
        &note.title,
        &note.content,
        uuid.as_deref(),
    )?)
}

/// Whether a note with this uuid already exists.
pub fn uuid_taken(conn: &Connection, uuid: &str) -> Result<bool> {
    Ok(q::uuid_taken(conn, uuid)?)
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
                uuid: None,
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
                uuid: None,
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
                uuid: None,
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
                uuid: None,
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
                uuid: None,
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
                uuid: None,
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
                uuid: None,
            },
        )
        .unwrap();

        assert_eq!(list_all(&conn).unwrap().len(), 3);
    }

    #[test]
    fn uuid_preserved_on_first_create_and_fallback_on_duplicate() {
        let conn = setup();
        let fixed_uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

        let id1 = create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "first".into(),
                content: "x".into(),
                paper_id: None,
                project_fk: Some(10),
                uuid: Some(fixed_uuid.into()),
            },
        )
        .unwrap();

        let got1 = get(&conn, &Note { note_id: Some(id1) }).unwrap().unwrap();
        assert_eq!(got1.uuid, fixed_uuid);

        let id2 = create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "second".into(),
                content: "y".into(),
                paper_id: None,
                project_fk: Some(10),
                uuid: Some(fixed_uuid.into()),
            },
        )
        .unwrap();

        let got2 = get(&conn, &Note { note_id: Some(id2) }).unwrap().unwrap();
        assert_ne!(got2.uuid, fixed_uuid);
        assert!(!got2.uuid.is_empty());

        // uppercase spelling of the stored uuid: collision detected on the
        // normalized form, fresh-uuid fallback fires.
        let id3 = create(
            &conn,
            &NoteIn {
                source_fk: 1,
                title: "third".into(),
                content: "z".into(),
                paper_id: None,
                project_fk: Some(10),
                uuid: Some(fixed_uuid.to_uppercase()),
            },
        )
        .unwrap();

        let got3 = get(&conn, &Note { note_id: Some(id3) }).unwrap().unwrap();
        assert_ne!(got3.uuid.to_lowercase(), fixed_uuid);
        assert!(!got3.uuid.is_empty());
    }

    /// `note list` and `annotation list` are sibling commands: the same
    /// (paper, project) pair must scope both the same way.
    #[test]
    fn list_filtered_scopes_notes_and_annotations_alike() {
        use crate::models::AnnotationIn;
        use crate::service::annotation as ann;

        const ANCHOR: &str = r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##;
        let conn = setup();
        for project_fk in [None, Some(10)] {
            create(
                &conn,
                &NoteIn {
                    source_fk: 1,
                    title: "n".into(),
                    content: "c".into(),
                    paper_id: None,
                    project_fk,
                    uuid: None,
                },
            )
            .unwrap();
            ann::create(
                &conn,
                &AnnotationIn {
                    source_fk: 1,
                    anchor: ANCHOR.into(),
                    comment: String::new(),
                    project_fk,
                    uuid: None,
                },
            )
            .unwrap();
        }

        // Paper with no project filter spans library + every project.
        assert_eq!(list_filtered(&conn, Some(1), None).unwrap().len(), 2);
        assert_eq!(ann::list_filtered(&conn, Some(1), None).unwrap().len(), 2);
        // Paper + project narrows to that project on both.
        assert_eq!(list_filtered(&conn, Some(1), Some(10)).unwrap().len(), 1);
        assert_eq!(
            ann::list_filtered(&conn, Some(1), Some(10)).unwrap().len(),
            1
        );
        // Neither filter lists everything.
        assert_eq!(list_filtered(&conn, None, None).unwrap().len(), 2);
        assert_eq!(ann::list_filtered(&conn, None, None).unwrap().len(), 2);
    }
}
