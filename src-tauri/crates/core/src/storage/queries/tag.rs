use rusqlite::Connection;

use crate::error::Result;
use crate::models::TagDetails;

/// `storage/tags.py::list_tags` (no paper/project/label filter) — every tag,
/// ordered by label. TAG.TAG is UNIQUE NOCASE, so the rows are already distinct.
/// TAG.TAG is nullable, so `label` maps to `Option<String>`.
pub fn list_tags(conn: &Connection) -> Result<Vec<TagDetails>> {
    let mut stmt = conn.prepare("SELECT TAG_FK, TAG FROM TAG ORDER BY TAG")?;
    let rows = stmt.query_map([], |row| {
        Ok(TagDetails {
            tag_id: row.get(0)?,
            label: row.get(1)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};

    #[test]
    fn list_tags_returns_all_ordered_by_label() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn.execute("INSERT INTO TAG (TAG) VALUES ('zeta')", []).unwrap();
        conn.execute("INSERT INTO TAG (TAG) VALUES ('alpha')", []).unwrap();

        let tags = list_tags(&conn).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].label.as_deref(), Some("alpha"));
        assert_eq!(tags[1].label.as_deref(), Some("zeta"));
        assert!(tags[0].tag_id > 0);
    }
}
