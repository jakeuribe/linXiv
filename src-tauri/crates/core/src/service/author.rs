//! Author service — Rust port of `service/author.py`. Plan §5.2.
//!
//! Thin orchestration over `storage::queries::author`. Every DB-touching fn
//! takes `conn: &Connection` first (DI seam — no config-opened connections),
//! except transactional writers like `merge`, which take `conn: &mut Connection`.
//!
//! D17 dual-lookup seam: `get(Author)` / `get_many(Authors)` are the ONE lookup
//! seam. The Python `get_author_details` / `get_full_author_details` /
//! `get_authors` wrappers that just re-shaped the same query are dropped — they
//! forwarded to the same storage reads with a narrower signature.

use crate::error::{CoreError, Result};
use crate::models::{
    AuthorIn, AuthorPaperPreview, AuthorWithCount, AuthorWithPapers, BasicAuthorDetails,
};
use crate::storage::queries::author as store;
use rusqlite::Connection;

// ── query objects (defined in service/author.py, not models.py) ─────────────

/// Single-author lookup key. Resolution order: `author_id` → `orcid`.
#[derive(Debug, Default, Clone)]
pub struct Author {
    pub author_id: Option<i64>,
    pub orcid: Option<String>,
}

/// Multi-author filter. Any combination of fields narrows the result.
#[derive(Debug, Default, Clone)]
pub struct Authors {
    pub paper_id: Option<i64>,
    pub name: Option<Vec<String>>,
    pub author_ids: Option<Vec<i64>>,
}

// `list_authors(paper_id, name)` priority: paper_id wins (name ignored), else
// name exact-match (NOCASE in storage), else every author. Ported faithfully.
fn list_authors(
    conn: &Connection,
    paper_id: Option<i64>,
    name: Option<&str>,
) -> Result<Vec<BasicAuthorDetails>> {
    match paper_id {
        Some(pid) => store::get_paper_authors(conn, pid),
        None => store::get_many(conn, name),
    }
}

// ── lookup seam ─────────────────────────────────────────────────────────────

/// Fetch a single author. Resolution order: `author_id` → `orcid` (scan).
pub fn get(conn: &Connection, author: &Author) -> Result<Option<BasicAuthorDetails>> {
    if let Some(id) = author.author_id {
        return store::get_author(conn, id);
    }
    if let Some(orcid) = author.orcid.as_deref() {
        return Ok(store::get_many(conn, None)?
            .into_iter()
            .find(|r| r.orcid.as_deref() == Some(orcid)));
    }
    Ok(None)
}

/// Fetch authors matching any combination of `Authors` filter fields.
///
/// The filter seam behind the same-name half of `GET /merge-candidates`
/// (route/authors.rs): a single `name` pushes down to the storage exact-match
/// (NOCASE), which is what makes a shared full name a merge candidate.
pub fn get_many(conn: &Connection, authors: &Authors) -> Result<Vec<BasicAuthorDetails>> {
    // A single name pushes down to the SQL exact-match; multiple names are
    // post-filtered (storage takes one name only).
    let single_name = match authors.name.as_deref() {
        Some([only]) => Some(only.as_str()),
        _ => None,
    };
    let mut rows = list_authors(conn, authors.paper_id, single_name)?;

    if let Some(names) = authors.name.as_deref() {
        if names.len() > 1 {
            let set: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
            rows.retain(|r| r.full_name.as_deref().is_some_and(|f| set.contains(f)));
        }
    }
    // Empty id list is a no-op filter (Python's truthy `if authors.author_ids:`),
    // mirroring the names>1 guard above — an empty Some(vec![]) must not drop all.
    if let Some(ids) = authors.author_ids.as_deref() {
        if !ids.is_empty() {
            let set: std::collections::HashSet<i64> = ids.iter().copied().collect();
            rows.retain(|r| set.contains(&r.author_id));
        }
    }
    Ok(rows)
}

// ── writes ──────────────────────────────────────────────────────────────────

/// Create an author row, returning the new AUTHOR_FK. (Python `upsert` /
/// `create_author` were byte-identical INSERTs — one fn here.)
pub fn create(conn: &Connection, author: &AuthorIn) -> Result<i64> {
    store::create_author(
        conn,
        &author.full_name,
        author.first_name.as_deref(),
        author.last_name.as_deref(),
        author.orcid.as_deref(),
    )
}

/// `service/author.py::update_fields` — the load-bearing partial-update primitive
/// (the CLI/MCP/API callers use it): `None` leaves a field unchanged. Storage also
/// skips empty strings. Errors if the author is absent or every field is `None`,
/// so all consumers get the same answer.
pub fn update_fields(
    conn: &Connection,
    author_id: i64,
    full_name: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
    orcid: Option<&str>,
) -> Result<()> {
    if store::get_author(conn, author_id)?.is_none() {
        return Err(CoreError::NotFound(format!("Author {author_id} not found")));
    }
    if full_name.is_none() && first_name.is_none() && last_name.is_none() && orcid.is_none() {
        return Err(CoreError::Validation(
            "at least one of full_name, first_name, last_name, or orcid must be provided".into(),
        ));
    }
    store::update_author(conn, author_id, full_name, first_name, last_name, orcid)
}

/// `service/author.py::update_author` — full-record wrapper over `update_fields`
/// (always sets full_name, which is required on the DTO).
pub fn update(conn: &Connection, author_id: i64, author: &AuthorIn) -> Result<()> {
    update_fields(
        conn,
        author_id,
        Some(&author.full_name),
        author.first_name.as_deref(),
        author.last_name.as_deref(),
        author.orcid.as_deref(),
    )
}

/// Merge one or more duplicate authors into `canonical_id`, re-pointing their
/// papers (deduped) and deleting the duplicate rows. Returns the ids actually
/// merged (excludes `canonical_id` itself). See `store::merge_authors`.
pub fn merge(conn: &mut Connection, canonical_id: i64, duplicate_ids: &[i64]) -> Result<Vec<i64>> {
    store::merge_authors(conn, canonical_id, duplicate_ids)
}

/// Delete by lookup key. Errors if the key resolves to no author (404) or if the
/// author is still linked to a paper (409) — callers must unlink (or merge) first.
/// The link count is the whole safety check: `PAPER_META.AUTHORS` is a free-text
/// read cache that holds names, not AUTHOR_FKs, so it cannot dangle on a delete.
pub fn delete(conn: &Connection, author: &Author) -> Result<()> {
    let id = get(conn, author)?
        .ok_or_else(|| CoreError::NotFound("Author not found".into()))?
        .author_id;
    let links = store::count_paper_links(conn, id)?;
    if links > 0 {
        return Err(CoreError::Conflict(format!(
            "Author is linked to {links} paper(s); unlink before deleting."
        )));
    }
    store::delete_author(conn, id)
}

// ── PAPER_TO_AUTHOR links ───────────────────────────────────────────────────

/// Attach one paper↔author link (idempotent — storage INSERT OR IGNORE). The
/// light-touch alternative to `merge` when only a single paper is misfiled.
pub fn link_author_to_paper(
    conn: &Connection,
    author_id: i64,
    paper_id: i64,
    author_index: Option<i64>,
) -> Result<()> {
    store::link_author_to_paper(conn, author_id, paper_id, author_index)
}

/// Drop one paper↔author link (per PAPER version row — the route unlinks every
/// version of a root). No-op if the pair is not linked.
pub fn unlink_author_from_paper(conn: &Connection, author_id: i64, paper_id: i64) -> Result<()> {
    store::unlink_author_from_paper(conn, author_id, paper_id)
}

// ── derived reads ───────────────────────────────────────────────────────────

/// Authors with their active-paper count, `>= min_papers`.
pub fn list_with_paper_count(conn: &Connection, min_papers: i64) -> Result<Vec<AuthorWithCount>> {
    store::list_with_paper_count(conn, min_papers)
}

/// Authors of a paper version, ordered by AUTHOR_INDEX.
pub fn get_paper_authors(conn: &Connection, paper_id: i64) -> Result<Vec<BasicAuthorDetails>> {
    store::get_paper_authors(conn, paper_id)
}

/// Batched `get_paper_authors` projected to ORCIDs: each paper's author ORCIDs
/// in AUTHOR_INDEX order, keyed by PAPER_ID (papers without author links absent).
pub fn paper_author_orcids(
    conn: &Connection,
    paper_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<Option<String>>>> {
    store::paper_author_orcids(conn, paper_ids)
}

/// Other authors sharing this author's ORCID — likely-duplicate suggestions for
/// the merge UI. Empty if the author has no ORCID.
pub fn orcid_merge_candidates(
    conn: &Connection,
    author_id: i64,
) -> Result<Vec<BasicAuthorDetails>> {
    store::orcid_merge_candidates(conn, author_id)
}

/// Latest-version display rows for active papers linked to an author.
pub fn get_paper_previews(conn: &Connection, author_id: i64) -> Result<Vec<AuthorPaperPreview>> {
    store::get_paper_previews(conn, author_id)
}

/// The author-detail composite (`AuthorWithPapers`) all three surfaces emit:
/// base fields + `paper_count` + `papers` previews. `Ok(None)` if absent.
pub fn get_with_papers(conn: &Connection, author_id: i64) -> Result<Option<AuthorWithPapers>> {
    let Some(base) = store::get_author(conn, author_id)? else {
        return Ok(None);
    };
    let papers = store::get_paper_previews(conn, author_id)?;
    Ok(Some(AuthorWithPapers {
        base,
        paper_count: papers.len(),
        papers,
    }))
}

/// Total distinct paper roots linked to this author, regardless of status.
pub fn count_paper_links(conn: &Connection, author_id: i64) -> Result<i64> {
    store::count_paper_links(conn, author_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;
    use rusqlite::params;

    // One active paper root, two linked authors. Returns (paper_id, bob, alice).
    fn seed(conn: &Connection) -> (i64, i64, i64) {
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:1')", [])
            .unwrap();
        let fk = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('arxiv:1', 1, 'T1', ?)",
            params![fk],
        )
        .unwrap();
        let pid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?, '2024-01-01')",
            params![pid],
        )
        .unwrap();
        let bob = create(
            conn,
            &AuthorIn {
                full_name: "Bob Stone".into(),
                first_name: Some("Bob".into()),
                last_name: Some("Stone".into()),
                orcid: None,
            },
        )
        .unwrap();
        let alice = create(
            conn,
            &AuthorIn {
                full_name: "Alice Cole".into(),
                first_name: Some("Alice".into()),
                last_name: Some("Cole".into()),
                orcid: Some("0000-1".into()),
            },
        )
        .unwrap();
        link_author_to_paper(conn, bob, pid, Some(0)).unwrap();
        link_author_to_paper(conn, alice, pid, Some(1)).unwrap();
        (pid, bob, alice)
    }

    #[test]
    fn get_resolves_by_id_then_orcid() {
        let conn = db();
        let (_pid, bob, alice) = seed(&conn);

        // by id
        let a = get(
            &conn,
            &Author {
                author_id: Some(bob),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(a.full_name.as_deref(), Some("Bob Stone"));

        // by orcid (scan path)
        let a = get(
            &conn,
            &Author {
                author_id: None,
                orcid: Some("0000-1".into()),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(a.author_id, alice);

        // id wins over orcid when both present
        let a = get(
            &conn,
            &Author {
                author_id: Some(bob),
                orcid: Some("0000-1".into()),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(a.author_id, bob);

        // misses
        assert!(get(
            &conn,
            &Author {
                orcid: Some("nope".into()),
                ..Default::default()
            }
        )
        .unwrap()
        .is_none());
        assert!(get(&conn, &Author::default()).unwrap().is_none());
    }

    #[test]
    fn get_many_filters() {
        let conn = db();
        let (pid, bob, _alice) = seed(&conn);

        // all, ordered by full name -> Alice, Bob
        let all = get_many(&conn, &Authors::default()).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].full_name.as_deref(), Some("Alice Cole"));

        // single name pushes to SQL exact match (NOCASE)
        let one = get_many(
            &conn,
            &Authors {
                name: Some(vec!["bob stone".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].full_name.as_deref(), Some("Bob Stone"));

        // multiple names post-filtered
        let multi = get_many(
            &conn,
            &Authors {
                name: Some(vec!["Bob Stone".into(), "Ghost".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(multi.len(), 1);
        assert_eq!(multi[0].full_name.as_deref(), Some("Bob Stone"));

        // author_ids filter
        let byid = get_many(
            &conn,
            &Authors {
                author_ids: Some(vec![bob]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(byid.len(), 1);
        assert_eq!(byid[0].author_id, bob);

        // empty author_ids is a no-op filter (returns all), not "match nothing"
        let empty = get_many(
            &conn,
            &Authors {
                author_ids: Some(vec![]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(empty.len(), 2);

        // paper_id path -> ordered by AUTHOR_INDEX (Bob 0, Alice 1)
        let bypaper = get_many(
            &conn,
            &Authors {
                paper_id: Some(pid),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(bypaper.len(), 2);
        assert_eq!(bypaper[0].full_name.as_deref(), Some("Bob Stone"));
    }

    #[test]
    fn create_update_delete() {
        let conn = db();
        let id = create(
            &conn,
            &AuthorIn {
                full_name: "Jane Doe".into(),
                first_name: None,
                last_name: None,
                orcid: None,
            },
        )
        .unwrap();
        assert!(id > 0);

        update(
            &conn,
            id,
            &AuthorIn {
                full_name: "Jane Q Doe".into(),
                first_name: None,
                last_name: Some("Doe".into()),
                orcid: Some("0000-2".into()),
            },
        )
        .unwrap();
        let a = get(
            &conn,
            &Author {
                author_id: Some(id),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(a.full_name.as_deref(), Some("Jane Q Doe"));
        assert_eq!(a.last_name.as_deref(), Some("Doe"));
        assert_eq!(a.orcid.as_deref(), Some("0000-2"));

        // partial update_fields: change only orcid, leave full_name/last untouched
        update_fields(&conn, id, None, None, None, Some("0000-9")).unwrap();
        let a = get(
            &conn,
            &Author {
                author_id: Some(id),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            a.full_name.as_deref(),
            Some("Jane Q Doe"),
            "full_name unchanged by partial update"
        );
        assert_eq!(a.orcid.as_deref(), Some("0000-9"));

        // a key that resolves to no author errors; with id it removes the row
        assert!(delete(&conn, &Author::default()).is_err());
        assert!(get(
            &conn,
            &Author {
                author_id: Some(id),
                ..Default::default()
            }
        )
        .unwrap()
        .is_some());
        delete(
            &conn,
            &Author {
                author_id: Some(id),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(get(
            &conn,
            &Author {
                author_id: Some(id),
                ..Default::default()
            }
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn merge_repoints_papers_and_removes_duplicate() {
        let mut conn = db();
        // Shared paper (both authors) + one paper each, to exercise dedupe.
        let (shared, bob, alice) = seed(&conn);
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES ('arxiv:2')", [])
            .unwrap();
        let fk = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, SOURCE_FK) VALUES ('arxiv:2', 1, 'T2', ?)",
            params![fk],
        )
        .unwrap();
        let solo = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED) VALUES (?, '2024-01-02')",
            params![solo],
        )
        .unwrap();
        link_author_to_paper(&conn, alice, solo, Some(0)).unwrap();

        merge(&mut conn, bob, &[alice]).unwrap();

        // Duplicate author is gone; canonical remains.
        assert!(get(
            &conn,
            &Author {
                author_id: Some(alice),
                ..Default::default()
            }
        )
        .unwrap()
        .is_none());
        assert!(get(
            &conn,
            &Author {
                author_id: Some(bob),
                ..Default::default()
            }
        )
        .unwrap()
        .is_some());

        // Bob now covers both paper roots, with exactly one link row per paper
        // (the shared paper did not double-link).
        assert_eq!(count_paper_links(&conn, bob).unwrap(), 2);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM PAPER_TO_AUTHOR WHERE AUTHOR_FK = ?",
                params![bob],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(get_paper_authors(&conn, shared).unwrap().len(), 1);
    }

    #[test]
    fn links_previews_and_counts() {
        let conn = db();
        let (pid, bob, _alice) = seed(&conn);

        assert_eq!(count_paper_links(&conn, bob).unwrap(), 1);

        let prev = get_paper_previews(&conn, bob).unwrap();
        assert_eq!(prev.len(), 1);
        assert_eq!(prev[0].source_id, "arxiv:1");
        assert_eq!(prev[0].title.as_deref(), Some("T1"));

        let withc = list_with_paper_count(&conn, 1).unwrap();
        assert_eq!(withc.len(), 2);
        assert_eq!(withc[0].base.last_name.as_deref(), Some("Cole")); // ordered by last name

        // unlink drops the link + one paper author
        unlink_author_from_paper(&conn, bob, pid).unwrap();
        assert_eq!(count_paper_links(&conn, bob).unwrap(), 0);
        assert_eq!(get_paper_authors(&conn, pid).unwrap().len(), 1);
    }

    /// Wire-shape pin: the composite is the flattened author + paper_count + papers.
    #[test]
    fn author_with_papers_wire_shape() {
        let conn = db();
        let (_pid, bob, _alice) = seed(&conn);
        let v = serde_json::to_value(get_with_papers(&conn, bob).unwrap().unwrap()).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "author_id",
                "orcid",
                "full_name",
                "first_name",
                "last_name",
                "paper_count",
                "papers"
            ]
        );
        assert_eq!(v["paper_count"], 1);
        assert_eq!(v["papers"][0]["source_id"], "arxiv:1");
        assert!(get_with_papers(&conn, 99_999).unwrap().is_none());
    }

    fn by_id(id: i64) -> Author {
        Author {
            author_id: Some(id),
            ..Default::default()
        }
    }

    #[test]
    fn delete_rejects_missing_and_still_linked_authors() {
        let conn = db();
        let (pid, bob, _alice) = seed(&conn);

        assert_eq!(
            delete(&conn, &by_id(99_999)).unwrap_err().http_status(),
            404
        );
        assert_eq!(delete(&conn, &by_id(bob)).unwrap_err().http_status(), 409);
        assert!(get(&conn, &by_id(bob)).unwrap().is_some());

        unlink_author_from_paper(&conn, bob, pid).unwrap();
        delete(&conn, &by_id(bob)).unwrap();
        assert!(get(&conn, &by_id(bob)).unwrap().is_none());
    }

    #[test]
    fn update_fields_rejects_missing_author_and_empty_patch() {
        let conn = db();
        let (_pid, bob, _alice) = seed(&conn);

        let e = update_fields(&conn, 99_999, Some("X"), None, None, None).unwrap_err();
        assert_eq!(e.http_status(), 404);
        let e = update_fields(&conn, bob, None, None, None, None).unwrap_err();
        assert_eq!(e.http_status(), 422);
    }
}
