use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;
use crate::models::{TagDetails, TagWithCount};
use crate::storage::db;

/// `_TAG_FK_BY_LABEL_SQL` — label match is COLLATE NOCASE, mirroring the UNIQUE
/// index `idx_tag_label_unique`. Used by every get-or-create path below.
const TAG_FK_BY_LABEL_SQL: &str = "SELECT TAG_FK FROM TAG WHERE TAG = ? COLLATE NOCASE LIMIT 1";

/// Internal marker tag on projects that are reading lists (frontend
/// `src/lib/readingStatus.ts::READING_LIST_TAG`). Hidden from index listings only.
pub const READING_LIST_TAG: &str = "reading-list";

/// `storage/tags.py::list_tags` (no paper/project/label filter) — every tag,
/// ordered by label. TAG.TAG is UNIQUE NOCASE, so the rows are already distinct.
/// TAG.TAG is nullable, so `label` maps to `Option<String>`.
/// The internal `READING_LIST_TAG` marker is excluded (get/delete by id still work).
pub fn list_tags(conn: &Connection) -> Result<Vec<TagDetails>> {
    let mut stmt = conn.prepare(
        "SELECT TAG_FK, TAG FROM TAG \
         WHERE TAG IS NULL OR TAG <> ? COLLATE NOCASE ORDER BY TAG",
    )?;
    let rows = stmt.query_map([READING_LIST_TAG], |row| {
        Ok(TagDetails {
            tag_id: row.get(0)?,
            label: row.get(1)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Every named tag with its distinct *active*-paper count (trashed/deleted roots
/// excluded, matching `author_paper_counts`), ordered by label. Null-label rows
/// are dropped. Drives the Tags index table's count column / count sort.
pub fn list_tags_with_count(conn: &Connection) -> Result<Vec<TagWithCount>> {
    let mut stmt = conn.prepare(
        "SELECT t.TAG AS label, \
                COUNT(DISTINCT CASE WHEN pr.STATUS = 'active' THEN p.SOURCE_FK END) AS paper_count \
         FROM TAG t \
         LEFT JOIN PAPER_TO_TAG ptt ON ptt.TAG_FK = t.TAG_FK \
         LEFT JOIN PAPER p          ON p.PAPER_ID = ptt.PAPER_ID \
         LEFT JOIN PAPER_ROOTS pr   ON pr.SOURCE_FK = p.SOURCE_FK \
         WHERE t.TAG IS NOT NULL AND t.TAG <> ? COLLATE NOCASE \
         GROUP BY t.TAG_FK \
         ORDER BY t.TAG",
    )?;
    let rows = stmt.query_map([READING_LIST_TAG], |row| {
        Ok(TagWithCount {
            label: row.get("label")?,
            paper_count: row.get("paper_count")?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// `config/queries.py::list_tags_by_paper` — DISTINCT tags linked to a paper via
/// PAPER_TO_TAG (on PAPER_ID), ordered by label. Mirrors `_TAGS_BY_PAPER_BASE_SQL`.
pub fn list_tags_by_paper(conn: &Connection, paper_id: i64) -> Result<Vec<TagDetails>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT t.TAG_FK, t.TAG FROM TAG t \
         JOIN PAPER_TO_TAG ptt ON ptt.TAG_FK = t.TAG_FK \
         WHERE ptt.PAPER_ID = ? ORDER BY t.TAG",
    )?;
    let rows = stmt.query_map([paper_id], |r| {
        Ok(TagDetails {
            tag_id: r.get(0)?,
            label: r.get(1)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// `storage/tags.py::get_tag` — single tag by id, or `None` if absent.
pub fn get_tag(conn: &Connection, tag_id: i64) -> Result<Option<TagDetails>> {
    conn.query_row(
        "SELECT TAG_FK, TAG FROM TAG WHERE TAG_FK = ?",
        [tag_id],
        |row| {
            Ok(TagDetails {
                tag_id: row.get(0)?,
                label: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// `storage/tags.py::create_tag` — get-or-create. Returns the existing TAG_FK on
/// a COLLATE NOCASE label match (the UNIQUE index), else inserts and returns the
/// new id. Select+insert run in one transaction so a concurrent insert can't slip
/// a duplicate between the two statements.
pub fn create_tag(conn: &mut Connection, label: &str) -> Result<i64> {
    db::transaction(conn, |tx| {
        if let Some(id) = tx
            .query_row(TAG_FK_BY_LABEL_SQL, [label], |r| r.get::<_, i64>(0))
            .optional()?
        {
            return Ok(id);
        }
        tx.execute("INSERT INTO TAG (TAG) VALUES (?)", [label])?;
        Ok(tx.last_insert_rowid())
    })
}

/// `storage/tags.py::delete_tag` — hard delete by id. No-op if absent.
pub fn delete_tag(conn: &Connection, tag_id: i64) -> Result<()> {
    conn.execute("DELETE FROM TAG WHERE TAG_FK = ?", [tag_id])?;
    Ok(())
}

/// `storage/tags.py::get_project_tags` — labels of every tag linked to a project,
/// ordered by label. Mirrors `_TAGS_BY_PROJECT_BASE_SQL` (DISTINCT join).
pub fn get_project_tags(conn: &Connection, project_id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT t.TAG_FK, t.TAG FROM TAG t \
         JOIN PROJECT_TO_TAG ptt ON ptt.TAG_FK = t.TAG_FK \
         WHERE ptt.PROJECT_FK = ? ORDER BY t.TAG",
    )?;
    let rows = stmt.query_map([project_id], |r| r.get::<_, Option<String>>(1))?;
    // TAG is nullable; Python keeps None rows, but linked tags always carry a
    // label in practice — drop nulls rather than surface a None label.
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect())
}

/// `storage/tags.py::add_project_tags` — get-or-create each (trimmed, deduped
/// case-insensitively) label, then link it to the project. INSERT OR IGNORE +
/// re-SELECT is safe under the UNIQUE NOCASE index. Returns the project's tags
/// after the write. All statements run in one transaction.
pub fn add_project_tags(
    conn: &mut Connection,
    project_id: i64,
    tags: &[String],
) -> Result<Vec<String>> {
    db::transaction(conn, |tx| {
        let mut tag_fks: Vec<i64> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for raw in tags {
            let label = raw.trim();
            if label.is_empty() || !seen.insert(label.to_lowercase()) {
                continue;
            }
            tx.execute("INSERT OR IGNORE INTO TAG (TAG) VALUES (?)", [label])?;
            let fk = tx
                .query_row(TAG_FK_BY_LABEL_SQL, [label], |r| r.get::<_, i64>(0))
                .optional()?
                .ok_or_else(|| {
                    crate::error::CoreError::Internal(format!(
                        "Could not get or create TAG for label {label:?}"
                    ))
                })?;
            tag_fks.push(fk);
        }
        for fk in tag_fks {
            tx.execute(
                "INSERT OR IGNORE INTO PROJECT_TO_TAG (PROJECT_FK, TAG_FK) VALUES (?, ?)",
                [project_id, fk],
            )?;
        }
        Ok(())
    })?;
    get_project_tags(conn, project_id)
}

/// `storage/tags.py::remove_project_tags` — unlink each given label (COLLATE
/// NOCASE) from the project; the TAG row itself is left alone. Returns the
/// project's remaining tags.
pub fn remove_project_tags(
    conn: &mut Connection,
    project_id: i64,
    tags: &[String],
) -> Result<Vec<String>> {
    db::transaction(conn, |tx| {
        for label in tags {
            if let Some(fk) = tx
                .query_row(TAG_FK_BY_LABEL_SQL, [label], |r| r.get::<_, i64>(0))
                .optional()?
            {
                tx.execute(
                    "DELETE FROM PROJECT_TO_TAG WHERE PROJECT_FK = ? AND TAG_FK = ?",
                    [project_id, fk],
                )?;
            }
        }
        Ok(())
    })?;
    get_project_tags(conn, project_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{self, db};

    #[test]
    fn list_tags_returns_all_ordered_by_label() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn.execute("INSERT INTO TAG (TAG) VALUES ('zeta')", [])
            .unwrap();
        conn.execute("INSERT INTO TAG (TAG) VALUES ('alpha')", [])
            .unwrap();

        let tags = list_tags(&conn).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].label.as_deref(), Some("alpha"));
        assert_eq!(tags[1].label.as_deref(), Some("zeta"));
        assert!(tags[0].tag_id > 0);
    }

    #[test]
    fn list_tags_with_count_counts_active_papers_only() {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let used = create_tag(&mut conn, "Used").unwrap();
        create_tag(&mut conn, "Empty").unwrap();

        // one active + one deleted paper, both tagged "Used".
        for (sid, status) in [("p-active", "active"), ("p-trashed", "deleted")] {
            conn.execute(
                "INSERT INTO PAPER_ROOTS (SOURCE_ID, STATUS) VALUES (?, ?)",
                rusqlite::params![sid, status],
            )
            .unwrap();
            let fk = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES (?, 1, 'T', ?)",
                rusqlite::params![sid, fk],
            )
            .unwrap();
            let pid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO PAPER_TO_TAG (PAPER_ID, TAG_FK) VALUES (?, ?)",
                rusqlite::params![pid, used],
            )
            .unwrap();
        }

        let counts = list_tags_with_count(&conn).unwrap();
        // ordered by label: Empty, Used.
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0].label, "Empty");
        assert_eq!(counts[0].paper_count, 0);
        assert_eq!(counts[1].label, "Used");
        assert_eq!(counts[1].paper_count, 1, "deleted paper excluded");
    }

    #[test]
    fn create_tag_inserts_then_returns_existing_id_nocase() {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();

        let id = create_tag(&mut conn, "Neural").unwrap();
        assert!(id > 0);
        // re-read the row actually exists
        assert_eq!(
            get_tag(&conn, id).unwrap().unwrap().label.as_deref(),
            Some("Neural")
        );
        // dup (different case) returns the same id, no second row
        assert_eq!(create_tag(&mut conn, "neural").unwrap(), id);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM TAG", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn delete_tag_removes_row() {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let id = create_tag(&mut conn, "doomed").unwrap();
        delete_tag(&conn, id).unwrap();
        assert!(get_tag(&conn, id).unwrap().is_none());
    }

    #[test]
    fn add_and_remove_project_tags_roundtrip() {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn.execute("INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (1, 'p')", [])
            .unwrap();

        // trim + case-dedup: "  ML ", "ml", "" collapse to one tag
        let out = add_project_tags(
            &mut conn,
            1,
            &["  ML ".into(), "ml".into(), "".into(), "RL".into()],
        )
        .unwrap();
        assert_eq!(out, vec!["ML".to_string(), "RL".to_string()]); // ORDER BY label

        // tags exist relationally
        let links: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM PROJECT_TO_TAG WHERE PROJECT_FK = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(links, 2);

        // idempotent add does not duplicate links
        add_project_tags(&mut conn, 1, &["ML".into()]).unwrap();
        let links2: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM PROJECT_TO_TAG WHERE PROJECT_FK = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(links2, 2);

        // remove one (NOCASE) — link gone, TAG row stays
        let remaining = remove_project_tags(&mut conn, 1, &["ml".into()]).unwrap();
        assert_eq!(remaining, vec!["RL".to_string()]);
        let tag_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM TAG", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tag_rows, 2, "remove unlinks, never deletes the TAG row");
    }

    #[test]
    fn reading_list_tag_hidden_from_index_listings_only() {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (1, 'rl')",
            [],
        )
        .unwrap();
        add_project_tags(&mut conn, 1, &[READING_LIST_TAG.into(), "ml".into()]).unwrap();

        // hidden from both index listings (NOCASE-safe by the unique index)
        let labels: Vec<_> = list_tags(&conn)
            .unwrap()
            .into_iter()
            .map(|t| t.label)
            .collect();
        assert_eq!(labels, vec![Some("ml".to_string())]);
        let counts = list_tags_with_count(&conn).unwrap();
        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].label, "ml");

        // still directly addressable: project listing keeps it, CRUD by id works
        assert_eq!(
            get_project_tags(&conn, 1).unwrap(),
            vec!["ml".to_string(), READING_LIST_TAG.to_string()]
        );
        let id = create_tag(&mut conn, READING_LIST_TAG).unwrap();
        assert!(get_tag(&conn, id).unwrap().is_some());
    }
}
