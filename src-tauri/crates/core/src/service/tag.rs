//! tag service — Phase 2 port of `service/tag.py`.
//!
//! Lookup seam (D17): the `Tag` / `Tags` query objects are the ONE lookup form.
//! Python's redundant `get_tag_details` (a 1-line forward to `get`) is dropped.
//!
//! These query structs live in `service/tag.py` itself (not `service/models/`),
//! so they stay local here too. All DB access delegates to
//! `storage::queries::tag`; the service issues no raw SQL.

use rusqlite::Connection;
use serde::Serialize;
use ts_rs::TS;

use crate::error::Result;
use crate::models::{PaperDetails, ProjectOut, Status, TagDetails, TagIn, TagWithCount};
use crate::storage::queries::tag as q;

#[derive(Debug, Clone, Serialize, TS)]
pub struct TagsResponse {
    pub tags: Vec<TagWithCount>,
}

/// `GET /api/tags/{label}` envelope (route/tags.rs) — see [`detail`]; `label` is the canonical stored casing, or the raw query label when unknown.
#[derive(Debug, Clone, Serialize, TS)]
pub struct TagDetail {
    pub label: String,
    pub papers: Vec<PaperDetails>,
    pub projects: Vec<ProjectOut>,
}

/// `POST /api/tags` / `linxiv tag create` envelope.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedTag {
    pub tag_id: i64,
    pub label: String,
}

/// `DELETE /api/tags/{id}` / `linxiv tag delete` envelope.
#[derive(Debug, Clone, Serialize)]
pub struct DeletedTag {
    pub deleted_tag_id: i64,
}

/// A paper's tag list after a tag mutation — `POST`/`DELETE /api/papers/{id}/tags`.
#[derive(Debug, Clone, Serialize)]
pub struct PaperTags {
    pub source_id: String,
    pub tags: Vec<String>,
}

/// `service/tag.py::Tag` — single-tag lookup. Resolution order: tag_id -> label.
#[derive(Debug, Default, Clone)]
pub struct Tag {
    pub tag_id: Option<i64>,
    pub label: Option<String>,
}

/// `service/tag.py::Tags` — multi-tag filter (any combination of fields).
#[derive(Debug, Default, Clone)]
pub struct Tags {
    pub paper_id: Option<i64>,
    pub project_id: Option<i64>,
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
        // NOCASE is ASCII in sqlite default collation — the same fold the old
        // in-Rust eq_ignore_ascii_case scan over list_all_tags used.
        return Ok(
            q::canonical_tag_label(conn, label)?.map(|existing| TagDetails {
                tag_id: -1,
                label: Some(existing),
            }),
        );
    }
    Ok(None)
}

/// `service/tag.py::get_tags` — tags matching the `Tags` filter.
///
/// Currently unwired above the service layer — kept as the pending
/// paper/project-scoped tag-filter seam (see `get_many`).
///
/// Mirrors Python `storage.tags.list_tags`'s priority: `paper_id` wins (the real
/// tags linked to that paper, via PAPER_TO_TAG), else `project_id`/`label` narrow
/// the full set in-service (keeping real TAG_FKs).
pub fn get_tags(conn: &Connection, tags: &Tags) -> Result<Vec<TagDetails>> {
    if let Some(pid) = tags.paper_id {
        // Python list_tags(paper_id) -> list_tags_by_paper: the paper's actual
        // tags (PAPER_TO_TAG join), and paper_id takes priority over the rest.
        return q::list_tags_by_paper(conn, pid);
    }
    let mut rows = q::list_tags(conn)?;
    if let Some(pid) = tags.project_id {
        let proj = q::get_project_tags(conn, pid)?;
        rows.retain(|t| {
            t.label
                .as_deref()
                .is_some_and(|l| proj.iter().any(|p| p.eq_ignore_ascii_case(l)))
        });
    }
    if let Some(label) = &tags.label {
        rows.retain(|t| {
            t.label
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(label))
        });
    }
    Ok(rows)
}

/// `service/tag.py::get_many` — filtered tags.
///
/// Python falls back to synthesising `tag_id = -1` rows when storage returns
/// nothing, but that path is unreachable once `storage::list_tags` is the
/// authoritative TAG-table read (same table the fallback scans).
pub fn get_many(conn: &Connection, tags: &Tags) -> Result<Vec<TagDetails>> {
    get_tags(conn, tags)
}

/// `service/tag.py::upsert` — case-insensitive get-or-create. Returns the TAG_FK.
/// `storage::tag::create_tag` already does the NOCASE get-or-create (UNIQUE
/// NOCASE index, select+insert in one tx), so the Python manual scan collapses
/// to a direct delegation.
pub fn upsert(conn: &mut Connection, tag: &TagIn) -> Result<i64> {
    q::create_tag(conn, &tag.label)
}

/// `service/tag.py::delete` — delete by tag_id; no-op when tag_id is absent.
pub fn delete(conn: &mut Connection, tag: &Tag) -> Result<()> {
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

/// PROJECT_FKs of every project carrying `label` (COLLATE NOCASE), any status.
pub fn project_fks_by_label(conn: &Connection, label: &str) -> Result<Vec<i64>> {
    q::project_fks_by_tag(conn, label)
}

/// Every named tag with its active-paper count, for the Tags index table.
pub fn list_tags_with_count(conn: &Connection) -> Result<Vec<TagWithCount>> {
    q::list_tags_with_count(conn)
}

/// `GET /api/tags/{label}` composite: canonical label, tagged papers, active tagged projects.
pub fn detail(conn: &Connection, label: &str) -> Result<TagDetail> {
    let canonical = get(
        conn,
        &Tag {
            label: Some(label.to_string()),
            ..Default::default()
        },
    )?
    .and_then(|t| t.label)
    .unwrap_or_else(|| label.to_string());

    let papers = crate::service::paper::get_papers_by_tag(conn, label)?;

    // Status::Active filter matches Python's `_LIST_PROJECTS_BY_TAG_SQL`
    // (`AND pr.STATUS = 'active'`): PROJECT_TO_TAG rows survive soft-delete, so
    // an unfiltered lookup would leak archived/deleted projects the API excludes.
    // Empty fks must short-circuit: get_many treats an empty project_fks
    // filter as "no filter" and would return every active project.
    let fks = project_fks_by_label(conn, label)?;
    let tagged = if fks.is_empty() {
        Vec::new()
    } else {
        let active = crate::service::project::Projects {
            project_fks: Some(fks),
            status: Some(Status::Active),
        };
        crate::service::project::get_many(conn, &active)?
    };
    let projects = crate::service::project::to_out_many(conn, tagged)?;

    Ok(TagDetail {
        label: canonical,
        papers,
        projects,
    })
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
            &mut conn,
            &Tag {
                tag_id: Some(id),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(q::get_tag(&conn, id).unwrap().is_none());
        // no tag_id -> no-op, no error
        delete(&mut conn, &Tag::default()).unwrap();
    }

    #[test]
    fn get_tags_filters_by_label_and_project() {
        let mut conn = seeded();
        conn.execute("INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (1, 'p')", [])
            .unwrap();
        q::add_project_tags(&mut conn, 1, &["RL".into(), "Vision".into()]).unwrap();

        // label filter (NOCASE) -> the single Neural row, real id
        let by_label = get_tags(
            &conn,
            &Tags {
                label: Some("NEURAL".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_label.len(), 1);
        assert_eq!(by_label[0].label.as_deref(), Some("Neural"));
        assert!(by_label[0].tag_id > 0);

        // project filter -> only the two linked tags, ordered by label
        let by_proj = get_tags(
            &conn,
            &Tags {
                project_id: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        let labels: Vec<_> = by_proj.iter().filter_map(|t| t.label.clone()).collect();
        assert_eq!(labels, vec!["RL", "Vision"]);

        // no filter -> all tags (get_many delegates here)
        assert_eq!(get_many(&conn, &Tags::default()).unwrap().len(), 3);

        // paper_id filter -> the paper's REAL tags via PAPER_TO_TAG (Python parity).
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1')", [])
            .unwrap();
        let src_fk = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('arxiv:1', 1, 'T', ?1)",
            [src_fk],
        )
        .unwrap();
        let paper_id = conn.last_insert_rowid();
        // link Neural + Vision (not RL) to the paper.
        let neural_fk: i64 = conn
            .query_row("SELECT TAG_FK FROM TAG WHERE TAG = 'Neural'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let vision_fk: i64 = conn
            .query_row("SELECT TAG_FK FROM TAG WHERE TAG = 'Vision'", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO PAPER_TO_TAG (PAPER_ID, TAG_FK) VALUES (?1, ?2), (?1, ?3)",
            [paper_id, neural_fk, vision_fk],
        )
        .unwrap();

        let by_paper = get_tags(
            &conn,
            &Tags {
                paper_id: Some(paper_id),
                ..Default::default()
            },
        )
        .unwrap();
        let labels: Vec<_> = by_paper.iter().filter_map(|t| t.label.clone()).collect();
        assert_eq!(labels, vec!["Neural", "Vision"]); // ORDER BY label, RL excluded
        assert!(
            by_paper.iter().all(|t| t.tag_id > 0),
            "real TAG_FKs, not sentinels"
        );
        // a paper with no links -> []
        assert!(get_tags(
            &conn,
            &Tags {
                paper_id: Some(999),
                ..Default::default()
            }
        )
        .unwrap()
        .is_empty());
    }
}
