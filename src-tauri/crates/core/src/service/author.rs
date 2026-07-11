//! Author service — Rust port of `service/author.py`. Plan §5.2.
//!
//! Thin orchestration over `storage::queries::author`. Every DB-touching fn
//! takes `conn: &Connection` first (DI seam — no config-opened connections),
//! except transactional writers like `merge`, which take `conn: &mut Connection`.
//!
//! D17 lookup seam: `get(Author)` is the ONE single-author lookup. The Python
//! `get_author_details` / `get_full_author_details` / `get_authors` wrappers
//! that just re-shaped the same query are dropped — they forwarded to the same
//! storage reads with a narrower signature.

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

/// Merge one or more duplicate authors into `canonical_id`, re-pointing their
/// papers (deduped) and deleting the duplicate rows. Returns the ids actually
/// merged (excludes `canonical_id` itself). See `store::merge_authors`.
pub fn merge(conn: &mut Connection, canonical_id: i64, duplicate_ids: &[i64]) -> Result<Vec<i64>> {
    store::merge_authors(conn, canonical_id, duplicate_ids)
}

/// Delete by lookup key. No-op when the key carries no `author_id` (matching
/// Python's `if author.author_id:`).
pub fn delete(conn: &Connection, author: &Author) -> Result<()> {
    if let Some(id) = author.author_id {
        store::delete_author(conn, id)?;
    }
    Ok(())
}

// ── derived reads ───────────────────────────────────────────────────────────

/// Authors with their active-paper count, `>= min_papers`.
pub fn list_with_paper_count(conn: &Connection, min_papers: i64) -> Result<Vec<AuthorWithCount>> {
    store::list_with_paper_count(conn, min_papers)
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
        store::link_author_to_paper(conn, bob, pid, Some(0)).unwrap();
        store::link_author_to_paper(conn, alice, pid, Some(1)).unwrap();
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

        update_fields(&conn, id, Some("Jane Q Doe"), None, Some("Doe"), Some("0000-2")).unwrap();
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
    fn merge_repoints_papers_and_removes_duplicate() {
        let mut conn = mem();
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
        store::link_author_to_paper(&conn, alice, solo, Some(0)).unwrap();

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
        assert_eq!(store::get_paper_authors(&conn, shared).unwrap().len(), 1);
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
        store::unlink_author_from_paper(&conn, bob, pid).unwrap();
        assert_eq!(count_paper_links(&conn, bob).unwrap(), 0);
        assert_eq!(store::get_paper_authors(&conn, pid).unwrap().len(), 1);
    }
}
