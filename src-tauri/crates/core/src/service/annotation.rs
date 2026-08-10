//! annotation service — PDF highlight CRUD, mirroring `service::note`.
//!
//! Thin delegation over `storage::queries::annotation`. DB-touching fns take
//! `conn: &Connection` first (DI seam — never open from config). The
//! `Annotations` query object is the one lookup seam.

use crate::error::{CoreError, Result};
use crate::models::{validate_anchor, AnnotationDetails, AnnotationIn, AnnotationUpdateIn};
use crate::storage::queries::annotation as q;
use rusqlite::Connection;

/// Multi-annotation filter. Valid combos: `source_fk` (+ optional
/// `project_fk`/`all_projects`), or `project_fk` alone.
#[derive(Debug, Clone, Default)]
pub struct Annotations {
    pub source_fk: Option<i64>,
    pub project_fk: Option<i64>,
    pub all_projects: bool,
}

/// Fetch a single annotation by id. `Ok(None)` if absent.
pub fn get(conn: &Connection, id: i64) -> Result<Option<AnnotationDetails>> {
    Ok(q::get_annotation(conn, id)?)
}

/// Every annotation, CREATED_AT ASC.
pub fn list_all(conn: &Connection) -> Result<Vec<AnnotationDetails>> {
    Ok(q::list_all_annotations(conn)?)
}

/// Fetch annotations by filter. Invalid combinations raise `Validation`.
pub fn get_many(conn: &Connection, anns: &Annotations) -> Result<Vec<AnnotationDetails>> {
    if anns.all_projects && anns.project_fk.is_some() {
        return Err(CoreError::Validation(
            "all_projects=True cannot be combined with a specific project_fk".into(),
        ));
    }
    if let Some(source_fk) = anns.source_fk {
        Ok(q::get_annotations(
            conn,
            source_fk,
            anns.project_fk,
            anns.all_projects,
        )?)
    } else if let Some(project_fk) = anns.project_fk {
        Ok(q::get_project_annotations(conn, project_fk)?)
    } else {
        Err(CoreError::Validation(
            "at least one of source_fk or project_fk must be set".into(),
        ))
    }
}

/// Annotations for an optional paper and/or project — same scoping rule as
/// [`crate::service::note::list_filtered`].
pub fn list_filtered(
    conn: &Connection,
    source_fk: Option<i64>,
    project_fk: Option<i64>,
) -> Result<Vec<AnnotationDetails>> {
    if source_fk.is_none() && project_fk.is_none() {
        return list_all(conn);
    }
    get_many(
        conn,
        &Annotations {
            source_fk,
            project_fk,
            all_projects: source_fk.is_some() && project_fk.is_none(),
        },
    )
}

/// Insert a new annotation. Returns ANNOTATION_SK. `Validation` if the anchor
/// is invalid, `ProjectNotFound` if `project_fk` is set and does not exist.
pub fn create(conn: &Connection, ann: &AnnotationIn) -> Result<i64> {
    validate_anchor(&ann.anchor).map_err(|m| CoreError::Validation(m.into()))?;
    if let Some(pid) = ann.project_fk {
        if crate::storage::queries::project::get_project(conn, pid, false)?.is_none() {
            return Err(CoreError::ProjectNotFound);
        }
    }
    let uuid: Option<String> = match &ann.uuid {
        Some(u) => crate::models::resolve_uuid(u, |n| q::uuid_taken(conn, n).map_err(Into::into))?,
        None => None,
    };
    Ok(q::create_annotation(
        conn,
        ann.source_fk,
        ann.project_fk,
        &ann.anchor,
        &ann.comment,
        uuid.as_deref(),
    )?)
}

/// Whether an annotation with this uuid already exists.
pub fn uuid_taken(conn: &Connection, uuid: &str) -> Result<bool> {
    Ok(q::uuid_taken(conn, uuid)?)
}

/// Delete an annotation by id. `false` if absent.
pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    Ok(q::delete_annotation(conn, id)?)
}

/// Update the written comment. `false` if no row matched. The anchor is immutable.
pub fn update(conn: &Connection, ann: &AnnotationUpdateIn) -> Result<bool> {
    Ok(q::patch_annotation(conn, ann.annotation_id, &ann.comment)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::open_in_memory;
    use crate::storage::init_db;

    const ANCHOR: &str = r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##;

    fn setup() -> Connection {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1')",
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
            &AnnotationIn {
                source_fk: 1,
                anchor: ANCHOR.into(),
                comment: String::new(),
                project_fk: Some(10),
                uuid: None,
            },
        )
        .unwrap();

        let got = get(&conn, id).unwrap().unwrap();
        assert_eq!(got.comment, "");
        assert_eq!(got.project_id, Some(10));

        assert!(update(
            &conn,
            &AnnotationUpdateIn {
                annotation_id: id,
                comment: "hello".into(),
            },
        )
        .unwrap());
        assert_eq!(get(&conn, id).unwrap().unwrap().comment, "hello");

        assert!(delete(&conn, id).unwrap());
        assert!(get(&conn, id).unwrap().is_none());
    }

    #[test]
    fn get_many_rejects_bad_combo_and_empty() {
        let conn = setup();
        assert!(matches!(
            get_many(
                &conn,
                &Annotations {
                    all_projects: true,
                    project_fk: Some(10),
                    ..Default::default()
                }
            )
            .unwrap_err(),
            CoreError::Validation(_)
        ));
        assert!(matches!(
            get_many(&conn, &Annotations::default()).unwrap_err(),
            CoreError::Validation(_)
        ));
    }

    #[test]
    fn uuid_preserved_on_first_create_and_fallback_on_duplicate() {
        let conn = setup();
        let fixed_uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

        let id1 = create(
            &conn,
            &AnnotationIn {
                source_fk: 1,
                anchor: ANCHOR.into(),
                comment: "first".into(),
                project_fk: Some(10),
                uuid: Some(fixed_uuid.into()),
            },
        )
        .unwrap();

        let got1 = get(&conn, id1).unwrap().unwrap();
        assert_eq!(got1.uuid, fixed_uuid);

        let id2 = create(
            &conn,
            &AnnotationIn {
                source_fk: 1,
                anchor: ANCHOR.into(),
                comment: "second".into(),
                project_fk: Some(10),
                uuid: Some(fixed_uuid.into()),
            },
        )
        .unwrap();

        let got2 = get(&conn, id2).unwrap().unwrap();
        assert_ne!(got2.uuid, fixed_uuid);
        assert!(!got2.uuid.is_empty());
    }
}
