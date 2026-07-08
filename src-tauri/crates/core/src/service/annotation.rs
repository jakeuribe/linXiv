//! annotation service — PDF highlight CRUD, mirroring `service::note`.
//!
//! Thin delegation over `storage::queries::annotation`. DB-touching fns take
//! `conn: &Connection` first (DI seam — never open from config). The
//! `Annotation`/`Annotations` query objects are the one lookup seam.

use crate::error::{CoreError, Result};
use crate::models::{validate_anchor, AnnotationDetails, AnnotationIn, AnnotationUpdateIn};
use crate::storage::queries::annotation as q;
use rusqlite::Connection;

/// Single-annotation lookup key (ANNOTATION_SK). `None` short-circuits to a
/// None/false result.
#[derive(Debug, Clone, Default)]
pub struct Annotation {
    pub annotation_id: Option<i64>,
}

/// Multi-annotation filter. Valid combos: `source_fk` (+ optional
/// `project_fk`/`all_projects`), or `project_fk` alone.
#[derive(Debug, Clone, Default)]
pub struct Annotations {
    pub source_fk: Option<i64>,
    pub project_fk: Option<i64>,
    pub all_projects: bool,
}

/// Fetch a single annotation by id. `Ok(None)` if absent or id unset.
pub fn get(conn: &Connection, ann: &Annotation) -> Result<Option<AnnotationDetails>> {
    match ann.annotation_id {
        Some(id) => Ok(q::get_annotation(conn, id)?),
        None => Ok(None),
    }
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

/// Insert a new annotation. Returns ANNOTATION_SK. `Validation` if the anchor
/// is invalid, `ProjectNotFound` if `project_fk` is set and does not exist.
pub fn create(conn: &Connection, ann: &AnnotationIn) -> Result<i64> {
    validate_anchor(&ann.anchor).map_err(|m| CoreError::Validation(m.into()))?;
    if let Some(pid) = ann.project_fk {
        if crate::storage::queries::project::get_project(conn, pid, false)?.is_none() {
            return Err(CoreError::ProjectNotFound);
        }
    }
    Ok(q::create_annotation(
        conn,
        ann.source_fk,
        ann.project_fk,
        &ann.anchor,
        &ann.comment,
    )?)
}

/// Delete an annotation by id. `false` if absent or id unset.
pub fn delete(conn: &Connection, ann: &Annotation) -> Result<bool> {
    match ann.annotation_id {
        Some(id) => Ok(q::delete_annotation(conn, id)?),
        None => Ok(false),
    }
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
            },
        )
        .unwrap();

        let got = get(
            &conn,
            &Annotation {
                annotation_id: Some(id),
            },
        )
        .unwrap()
        .unwrap();
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
        assert_eq!(
            get(
                &conn,
                &Annotation {
                    annotation_id: Some(id)
                }
            )
            .unwrap()
            .unwrap()
            .comment,
            "hello"
        );

        assert!(delete(
            &conn,
            &Annotation {
                annotation_id: Some(id)
            }
        )
        .unwrap());
        assert!(get(
            &conn,
            &Annotation {
                annotation_id: Some(id)
            }
        )
        .unwrap()
        .is_none());
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
    fn unset_id_short_circuits() {
        let conn = setup();
        assert!(get(
            &conn,
            &Annotation {
                annotation_id: None
            }
        )
        .unwrap()
        .is_none());
        assert!(!delete(
            &conn,
            &Annotation {
                annotation_id: None
            }
        )
        .unwrap());
    }
}
