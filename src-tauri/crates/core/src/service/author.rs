//! Author service — Rust port of `service/author.py`. Plan §5.2.
//!
//! Thin orchestration over `storage::queries::author`. Every DB-touching fn
//! takes `conn: &Connection` first (DI seam — no config-opened connections).
//!
//! D17 dual-lookup seam: `get(Author)` / `get_many(Authors)` are the ONE lookup
//! seam. The Python `get_author_details` / `get_full_author_details` /
//! `get_authors` wrappers that just re-shaped the same query are dropped — they
//! forwarded to the same storage reads with a narrower signature.

use crate::error::Result;
use crate::models::{AuthorIn, AuthorPaperPreview, AuthorWithCount, BasicAuthorDetails};
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
        for row in list_authors(conn, None, None)? {
            if row.orcid.as_deref() == Some(orcid) {
                return Ok(Some(row));
            }
        }
    }
    Ok(None)
}

/// Fetch authors matching any combination of `Authors` filter fields.
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
/// skips empty strings.
pub fn update_fields(
    conn: &Connection,
    author_id: i64,
    full_name: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
    orcid: Option<&str>,
) -> Result<()> {
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

/// Delete by lookup key. No-op when the key carries no `author_id` (matching
/// Python's `if author.author_id:`).
pub fn delete(conn: &Connection, author: &Author) -> Result<()> {
    if let Some(id) = author.author_id {
        store::delete_author(conn, id)?;
    }
    Ok(())
}

// ── PAPER_TO_AUTHOR links ───────────────────────────────────────────────────

pub fn link_author_to_paper(
    conn: &Connection,
    author_id: i64,
    paper_id: i64,
    author_index: Option<i64>,
) -> Result<()> {
    store::link_author_to_paper(conn, author_id, paper_id, author_index)
}

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

/// Latest-version display rows for active papers linked to an author.
pub fn get_paper_previews(conn: &Connection, author_id: i64) -> Result<Vec<AuthorPaperPreview>> {
    store::get_paper_previews(conn, author_id)
}

/// Total distinct paper roots linked to this author, regardless of status.
pub fn count_paper_links(conn: &Connection, author_id: i64) -> Result<i64> {
    store::count_paper_links(conn, author_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{db::open_in_memory, init_db};
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

    fn mem() -> Connection {
        let conn = open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn get_resolves_by_id_then_orcid() {
        let conn = mem();
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
        let conn = mem();
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
        let conn = mem();
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

        // delete with no id is a no-op; with id removes the row
        delete(&conn, &Author::default()).unwrap();
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
    fn links_previews_and_counts() {
        let conn = mem();
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
}
