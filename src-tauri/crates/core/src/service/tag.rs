//! tag service — Phase 2 port of `service/tag.py`.
//!
//! Lookup seam (D17): the `Tag` query object is the ONE lookup form.
//! Python's redundant `get_tag_details` (a 1-line forward to `get`) is dropped.
//!
//! These query structs live in `service/tag.py` itself (not `service/models/`),
//! so they stay local here too. All DB access delegates to
//! `storage::queries::tag`; the service issues no raw SQL.

use rusqlite::Connection;

use crate::error::Result;
use crate::models::{TagDetails, TagIn, TagWithCount};
use crate::storage::queries::tag as q;

/// `service/tag.py::Tag` — single-tag lookup. Resolution order: tag_id -> label.
#[derive(Debug, Default, Clone)]
pub struct Tag {
    pub tag_id: Option<i64>,
    pub label: Option<String>,
}

/// `service/tag.py::get` — resolve a single tag. tag_id wins; else a
/// case-insensitive label match returns a sentinel `tag_id = -1` row (Python
/// has no TAG_FK for the label-only path). `None` when nothing matches.
pub fn get(conn: &Connection, tag: &Tag) -> Result<Option<TagDetails>> {
    if let Some(id) = tag.tag_id {
        return q::get_tag(conn, id);
    }
    if let Some(label) = &tag.label {
        for existing in list_all_tags(conn)? {
            // NOCASE is ASCII in sqlite default collation — match it with ASCII fold.
            if existing.eq_ignore_ascii_case(label) {
                return Ok(Some(TagDetails {
                    tag_id: -1,
                    label: Some(existing),
                }));
            }
        }
    }
    Ok(None)
}

/// `service/tag.py::upsert` — case-insensitive get-or-create. Returns the TAG_FK.
/// `storage::tag::create_tag` already does the NOCASE get-or-create (UNIQUE
/// NOCASE index, select+insert in one tx), so the Python manual scan collapses
/// to a direct delegation.
pub fn upsert(conn: &mut Connection, tag: &TagIn) -> Result<i64> {
    q::create_tag(conn, &tag.label)
}

/// `service/tag.py::delete` — delete by tag_id; no-op when tag_id is absent.
pub fn delete(conn: &Connection, tag: &Tag) -> Result<()> {
    if let Some(id) = tag.tag_id {
        q::delete_tag(conn, id)?;
    }
    Ok(())
}

/// `service/tag.py::list_all_tags` — every tag label, ordered by label
/// (storage orders the rows). Null labels are dropped.
pub fn list_all_tags(conn: &Connection) -> Result<Vec<String>> {
    Ok(q::list_tags(conn)?
        .into_iter()
        .filter_map(|t| t.label)
        .collect())
}

/// Every named tag with its active-paper count, for the Tags index table.
pub fn list_tags_with_count(conn: &Connection) -> Result<Vec<TagWithCount>> {
    q::list_tags_with_count(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};

    fn seeded() -> Connection {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        q::create_tag(&mut conn, "Neural").unwrap();
        q::create_tag(&mut conn, "RL").unwrap();
        q::create_tag(&mut conn, "Vision").unwrap();
        conn
    }

    #[test]
    fn get_by_id_returns_real_row() {
        let mut conn = seeded();
        let id = q::create_tag(&mut conn, "Graphs").unwrap();
        let got = get(
            &conn,
            &Tag {
                tag_id: Some(id),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.tag_id, id);
        assert_eq!(got.label.as_deref(), Some("Graphs"));
    }

    #[test]
    fn get_by_label_is_case_insensitive_sentinel() {
        let conn = seeded();
        let got = get(
            &conn,
            &Tag {
                label: Some("neural".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(got.tag_id, -1, "label-only path has no TAG_FK");
        assert_eq!(
            got.label.as_deref(),
            Some("Neural"),
            "returns the stored casing"
        );
        // missing label -> None
        assert!(get(
            &conn,
            &Tag {
                label: Some("nope".into()),
                ..Default::default()
            }
        )
        .unwrap()
        .is_none());
        // empty Tag -> None
        assert!(get(&conn, &Tag::default()).unwrap().is_none());
    }

    #[test]
    fn list_all_tags_ordered() {
        let conn = seeded();
        assert_eq!(
            list_all_tags(&conn).unwrap(),
            vec!["Neural", "RL", "Vision"]
        );
    }

    #[test]
    fn upsert_dedups_case_insensitively() {
        let mut conn = seeded();
        let id = upsert(
            &mut conn,
            &TagIn {
                label: "neural".into(),
            },
        )
        .unwrap();
        // returns the existing Neural row, no new TAG inserted
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM TAG", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
        let neural = get(
            &conn,
            &Tag {
                tag_id: Some(id),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(neural.label.as_deref(), Some("Neural"));
        // a genuinely new label inserts
        let new_id = upsert(
            &mut conn,
            &TagIn {
                label: "Diffusion".into(),
            },
        )
        .unwrap();
        assert_ne!(new_id, id);
        let n2: i64 = conn
            .query_row("SELECT COUNT(*) FROM TAG", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 4);
    }

    #[test]
    fn delete_removes_then_noops() {
        let mut conn = seeded();
        let id = q::create_tag(&mut conn, "Doomed").unwrap();
        delete(
            &conn,
            &Tag {
                tag_id: Some(id),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(q::get_tag(&conn, id).unwrap().is_none());
        // no tag_id -> no-op, no error
        delete(&conn, &Tag::default()).unwrap();
    }
}
