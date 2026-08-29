//! Graph nodes/edges for the GUI graph view — Rust port of `api/graph_payload.py`
//! (`get_augmented_graph_data` + `project_filter_options`) over `storage/db.py`'s
//! `get_graph_data`. Faithful to the CURRENT output (the graph subsystem is
//! mid-refactor; re-port if the shape changes).
//!
//! Author nodes come from PAPER_TO_AUTHOR and are keyed by AUTHOR_FK
//! (`author::<fk>`) — the relational half of the dual author storage every other
//! author read in the app already goes through. `PAPER_META.AUTHORS` is only a
//! free-text cache: it spells one person several ways and goes stale when an
//! author is renamed or merged, so keying on it split one person across nodes and
//! left them without the `author_id` their click handler navigates to.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::error::Result;
use crate::models::Status;
use crate::service::project::{self, Projects};
use crate::storage::db::list_from_sql;

const DEFAULT_PROJECT_COLOR: &str = "#5b8dee";

/// Paper nodes: the latest version of each active root (`get_graph_data`).
const PAPER_NODES_SQL: &str = "\
    SELECT r.SOURCE_FK AS source_fk, r.SOURCE_ID AS source_id, \
           p.TITLE AS title, p.CATEGORY AS category, \
           m.TAGS AS tags, p.HAS_PDF AS has_pdf, m.PUBLISHED AS published, \
           m.URL AS url, m.DOI AS doi, m.SUMMARY AS summary \
    FROM PAPER_ROOTS r \
    JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK \
    JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
    WHERE p.VERSION = (SELECT MAX(VERSION) FROM PAPER WHERE SOURCE_FK = r.SOURCE_FK) \
      AND r.STATUS = 'active'";

/// One row per (latest active paper, author linked to it), in the paper's own
/// author order. Reads the PAPER_TO_AUTHOR links, so it follows the renames and
/// merges that `PAPER_META.AUTHORS` does not see. `exclude_single_authors` joins
/// `author_paper_counts`, the same view the Authors page filters on.
fn author_rows_sql(exclude_single_authors: bool) -> String {
    let count_join = if exclude_single_authors {
        "JOIN author_paper_counts apc \
             ON apc.author_fk = a.AUTHOR_FK AND apc.paper_count > 1 "
    } else {
        ""
    };
    format!(
        "SELECT r.SOURCE_FK AS source_fk, a.AUTHOR_FK AS author_fk, \
                a.AUTHOR_FULL_NAME AS author_name \
         FROM PAPER_ROOTS r \
         JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK \
         JOIN PAPER_TO_AUTHOR pta ON pta.PAPER_ID = p.PAPER_ID \
         JOIN AUTHOR a ON a.AUTHOR_FK = pta.AUTHOR_FK \
         {count_join}\
         WHERE p.VERSION = (SELECT MAX(VERSION) FROM PAPER WHERE SOURCE_FK = r.SOURCE_FK) \
           AND r.STATUS = 'active' \
           AND a.AUTHOR_FULL_NAME IS NOT NULL \
         ORDER BY r.SOURCE_FK, pta.AUTHOR_INDEX"
    )
}

/// `GET /api/graph` payload — `get_augmented_graph_data`. Paper nodes carry their
/// active-project ids; tag nodes/edges are derived from each paper's tags.
pub fn augmented_graph_data(conn: &Connection, exclude_single_authors: bool) -> Result<Value> {
    let (paper_nodes, author_nodes, mut edges) = graph_data(conn, exclude_single_authors)?;

    // paper SOURCE_FK -> sorted active project ids.
    let active = project::get_many(
        conn,
        &Projects {
            project_fks: None,
            status: Some(Status::Active),
        },
    )?;
    let mut paper_to_projects: HashMap<i64, Vec<i64>> = HashMap::new();
    for proj in &active {
        if let Some(pid) = proj.id {
            for &sfk in &proj.source_fks {
                paper_to_projects.entry(sfk).or_default().push(pid);
            }
        }
    }

    // tag_node_id -> display label; insertion guards dedup edges.
    let mut tag_labels: BTreeMap<String, String> = BTreeMap::new();
    let mut seen_tag_edges: HashSet<(i64, String)> = HashSet::new();
    let mut tag_edges: Vec<Value> = Vec::new();
    let mut out_nodes: Vec<Value> = Vec::with_capacity(paper_nodes.len() + author_nodes.len());

    for (id, mut node, tags) in paper_nodes {
        let mut projects: Vec<i64> = paper_to_projects.get(&id).cloned().unwrap_or_default();
        projects.sort_unstable();
        projects.dedup();
        node["project_ids"] = json!(projects);
        out_nodes.push(node);
        for raw in &tags {
            let tag = raw.trim();
            if tag.is_empty() {
                continue;
            }
            let tag_node_id = format!("tag::{}", tag.to_lowercase());
            tag_labels
                .entry(tag_node_id.clone())
                .or_insert_with(|| tag.to_string());
            if seen_tag_edges.insert((id, tag_node_id.clone())) {
                tag_edges.push(json!({ "source": id, "target": tag_node_id }));
            }
        }
    }
    out_nodes.extend(author_nodes);
    // tag nodes, sorted by id (BTreeMap key order == Python `sorted(tag_labels)`).
    for (tid, label) in &tag_labels {
        out_nodes.push(json!({ "id": tid, "label": label, "type": "tag" }));
    }
    edges.extend(tag_edges);
    Ok(json!({ "nodes": out_nodes, "edges": edges }))
}

/// Paper nodes as `(source_fk, node, tags)` — tags ride alongside the node JSON
/// so tag edges can be built after the nodes are emitted.
type PaperNodesWithTags = Vec<(i64, Value, Vec<String>)>;

/// `(paper_nodes [(source_fk, node, tags)], author_nodes, edges)` — `get_graph_data`.
fn graph_data(
    conn: &Connection,
    exclude_single_authors: bool,
) -> Result<(PaperNodesWithTags, Vec<Value>, Vec<Value>)> {
    let mut paper_stmt = conn.prepare(PAPER_NODES_SQL)?;
    let mut rows = paper_stmt.query([])?;
    let mut paper_nodes = Vec::new();
    while let Some(row) = rows.next()? {
        let source_fk: i64 = row.get("source_fk")?;
        let tags: Vec<String> = row
            .get::<_, Option<String>>("tags")?
            .as_deref()
            .map(list_from_sql)
            .transpose()?
            .unwrap_or_default();
        let node = json!({
            "id": source_fk,
            "source_id": row.get::<_, String>("source_id")?,
            "label": row.get::<_, Option<String>>("title")?,
            "type": "paper",
            "category": row.get::<_, Option<String>>("category")?,
            "tags": tags,
            "has_pdf": row.get::<_, i64>("has_pdf")? != 0,
            "published": row.get::<_, Option<String>>("published")?,
            "url": row.get::<_, Option<String>>("url")?,
            "doi": row.get::<_, Option<String>>("doi")?,
            "summary": row.get::<_, Option<String>>("summary")?,
        });
        paper_nodes.push((source_fk, node, tags));
    }

    let mut author_stmt = conn.prepare(&author_rows_sql(exclude_single_authors))?;
    let mut rows = author_stmt.query([])?;
    let mut seen: HashSet<i64> = HashSet::new();
    let mut seen_edges: HashSet<(i64, i64)> = HashSet::new();
    let mut author_nodes = Vec::new();
    let mut edges = Vec::new();
    while let Some(row) = rows.next()? {
        let source_fk: i64 = row.get("source_fk")?;
        let author_fk: i64 = row.get("author_fk")?;
        let node_id = format!("author::{author_fk}");
        if seen.insert(author_fk) {
            let label: String = row.get("author_name")?;
            let node = json!({
                "id": node_id.clone(),
                "label": label,
                "type": "author",
                "author_id": author_fk,
            });
            author_nodes.push(node);
        }
        // A paper can list the same person twice (repeat or case variant); one edge.
        if seen_edges.insert((source_fk, author_fk)) {
            edges.push(json!({ "source": source_fk, "target": node_id }));
        }
    }

    Ok((paper_nodes, author_nodes, edges))
}

/// `project_filter_options` — active project chips, sorted by id. Returns the list
/// (the route wraps it in `{projects: …}`).
pub fn project_filter_options(conn: &Connection) -> Result<Vec<Value>> {
    let mut active = project::get_many(
        conn,
        &Projects {
            project_fks: None,
            status: Some(Status::Active),
        },
    )?;
    active.retain(|p| p.id.is_some());
    active.sort_by_key(|p| p.id.unwrap());
    Ok(active
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id.unwrap(),
                "name": p.name,
                "color": p.color.map(project::color_to_hex).unwrap_or_else(|| DEFAULT_PROJECT_COLOR.to_string()),
                "tags": p.project_tags,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::author as svc_author;
    use crate::storage::queries::author as store_author;
    use crate::storage::{self, db};

    /// AUTHOR_FK for `name`, matched COLLATE NOCASE like `author_fk_for_name`.
    fn find_author_fk(conn: &Connection, name: &str) -> Option<i64> {
        let sql = "SELECT AUTHOR_FK FROM AUTHOR WHERE AUTHOR_FULL_NAME = ? COLLATE NOCASE";
        conn.query_row(sql, [name], |r| r.get(0)).ok()
    }

    /// `find_author_fk`, inserting the AUTHOR row when there is no match.
    fn author_fk(conn: &Connection, name: &str) -> i64 {
        if let Some(fk) = find_author_fk(conn, name) {
            return fk;
        }
        let ins = "INSERT INTO AUTHOR (AUTHOR_FULL_NAME) VALUES (?)";
        conn.execute(ins, [name]).unwrap();
        conn.last_insert_rowid()
    }

    /// The node id the graph emits for the author currently named `name`.
    fn author_id(conn: &Connection, name: &str) -> String {
        format!("author::{}", find_author_fk(conn, name).unwrap())
    }

    /// Seed one active paper, writing its authors both ways the paper writer does:
    /// the `PAPER_META.AUTHORS` free-text cache AND the PAPER_TO_AUTHOR links the
    /// graph reads (reusing an AUTHOR row COLLATE NOCASE, as `sync_paper_authors`).
    fn seed_paper(conn: &Connection, source_id: &str, authors_json: &str, tags_json: &str) -> i64 {
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?1)",
            [source_id],
        )
        .unwrap();
        let sfk: i64 = conn
            .query_row(
                "SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID = ?1",
                [source_id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) \
             VALUES (?1, 1, 'T', 'cs.LG', 1, ?2)",
            rusqlite::params![source_id, sfk],
        )
        .unwrap();
        let pid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO PAPER_META (PAPER_ID, PUBLISHED, AUTHORS, TAGS, SUMMARY) \
             VALUES (?1, '2024-01-01', ?2, ?3, 'S')",
            rusqlite::params![pid, authors_json, tags_json],
        )
        .unwrap();
        let names = list_from_sql(authors_json).unwrap();
        for (i, name) in names.iter().enumerate() {
            let fk = author_fk(conn, name);
            let link = "INSERT INTO PAPER_TO_AUTHOR (PAPER_ID, AUTHOR_FK, AUTHOR_INDEX) \
                        VALUES (?1, ?2, ?3)";
            conn.execute(link, [pid, fk, i as i64]).unwrap();
        }
        sfk
    }

    /// Every author node in the payload, in emission order.
    fn author_nodes(g: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        for n in g["nodes"].as_array().unwrap() {
            if n["type"] == "author" {
                out.push(n.clone());
            }
        }
        out
    }

    /// The node with `id`; panics if the payload has none.
    fn node_by_id(g: &Value, id: &str) -> Value {
        for n in g["nodes"].as_array().unwrap() {
            if n["id"] == id {
                return n.clone();
            }
        }
        panic!("no node {id}");
    }

    /// How many `sfk -> target` edges the payload carries.
    fn edge_count(g: &Value, sfk: i64, target: &str) -> usize {
        let mut n = 0;
        for e in g["edges"].as_array().unwrap() {
            if e["source"] == json!(sfk) && e["target"] == target {
                n += 1;
            }
        }
        n
    }

    #[test]
    fn augmented_graph_has_paper_author_tag_nodes_and_edges() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let sfk = seed_paper(
            &conn,
            "arxiv:2204.1",
            r#"["Ada Lovelace","Alan Turing"]"#,
            r#"["ML","nlp"]"#,
        );

        let g = augmented_graph_data(&conn, false).unwrap();
        let nodes = g["nodes"].as_array().unwrap();
        let edges = g["edges"].as_array().unwrap();

        let by_type = |t: &str| nodes.iter().filter(|n| n["type"] == t).count();
        assert_eq!(by_type("paper"), 1);
        assert_eq!(by_type("author"), 2);
        assert_eq!(by_type("tag"), 2); // ML, nlp -> tag::ml, tag::nlp

        let paper = nodes.iter().find(|n| n["type"] == "paper").unwrap();
        assert_eq!(paper["id"], json!(sfk));
        assert_eq!(paper["project_ids"], json!([]));
        assert_eq!(paper["has_pdf"], json!(true));

        // Author nodes are keyed by AUTHOR_FK and always carry the author_id the
        // node's click handler navigates to.
        let ada_fk = find_author_fk(&conn, "Ada Lovelace").unwrap();
        let ada = node_by_id(&g, &format!("author::{ada_fk}"));
        assert_eq!(ada["label"], "Ada Lovelace");
        assert_eq!(ada["author_id"], json!(ada_fk));
        assert_eq!(edge_count(&g, sfk, &format!("author::{ada_fk}")), 1);

        assert!(edges.iter().any(|e| e["target"] == json!("tag::ml")));
        // tag node id is lowercased, label preserved.
        assert!(nodes
            .iter()
            .any(|n| n["id"] == json!("tag::ml") && n["label"] == json!("ML")));
    }

    #[test]
    fn author_spelled_differently_across_papers_is_one_node() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        // Same person, two spellings — PAPER_META.AUTHORS is a free-text cache, and
        // the AUTHOR row it reconciles against is matched COLLATE NOCASE.
        let a = seed_paper(&conn, "arxiv:1", r#"["Ada Lovelace"]"#, "[]");
        let b = seed_paper(&conn, "arxiv:2", r#"["ada lovelace"]"#, "[]");

        let g = augmented_graph_data(&conn, false).unwrap();
        let authors = author_nodes(&g);
        assert_eq!(authors.len(), 1, "one person, one node: {authors:?}");
        let id = author_id(&conn, "Ada Lovelace");
        assert_eq!(authors[0]["id"], id);
        // Both papers still reach it, so the co-authorship link is not severed.
        assert_eq!(edge_count(&g, a, &id), 1);
        assert_eq!(edge_count(&g, b, &id), 1);
    }

    #[test]
    fn author_listed_twice_on_one_paper_yields_one_edge() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let authors = r#"["Ada Lovelace","ada lovelace"]"#;
        let sfk = seed_paper(&conn, "arxiv:1", authors, "[]");

        let g = augmented_graph_data(&conn, false).unwrap();
        assert_eq!(author_nodes(&g).len(), 1);
        let id = author_id(&conn, "Ada Lovelace");
        assert_eq!(edge_count(&g, sfk, &id), 1);
    }

    #[test]
    fn author_node_label_follows_the_author_table_spelling() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let sql = "INSERT INTO AUTHOR (AUTHOR_FULL_NAME) VALUES ('Ada Lovelace')";
        conn.execute(sql, []).unwrap();
        let fk = conn.last_insert_rowid();
        // The paper caches a lowercased spelling; the node must still show the
        // canonical one, since clicking it opens that AUTHOR's page.
        seed_paper(&conn, "arxiv:1", r#"["ada lovelace"]"#, "[]");

        let g = augmented_graph_data(&conn, false).unwrap();
        let ada = node_by_id(&g, &format!("author::{fk}"));
        assert_eq!(ada["label"], "Ada Lovelace");
        assert_eq!(ada["author_id"], json!(fk));
    }

    #[test]
    fn renaming_an_author_relabels_its_graph_node() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let sfk = seed_paper(&conn, "arxiv:1", r#"["A Lovelace"]"#, "[]");
        let fk = find_author_fk(&conn, "A Lovelace").unwrap();
        // PATCH /api/authors/{id} rewrites the AUTHOR row only — PAPER_META.AUTHORS
        // keeps caching the old spelling, which a name-keyed graph rendered instead.
        let new_name = Some("Ada Lovelace");
        store_author::update_author(&conn, fk, new_name, None, None, None).unwrap();

        let g = augmented_graph_data(&conn, false).unwrap();
        let authors = author_nodes(&g);
        assert_eq!(authors.len(), 1, "still one person: {authors:?}");
        assert_eq!(authors[0]["label"], "Ada Lovelace");
        assert_eq!(authors[0]["author_id"], json!(fk));
        assert_eq!(edge_count(&g, sfk, &format!("author::{fk}")), 1);
    }

    #[test]
    fn merged_authors_collapse_into_one_graph_node() {
        let mut conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let a = seed_paper(&conn, "arxiv:1", r#"["Ada Lovelace"]"#, "[]");
        let b = seed_paper(&conn, "arxiv:2", r#"["A. Lovelace"]"#, "[]");
        let keep = find_author_fk(&conn, "Ada Lovelace").unwrap();
        let dup = find_author_fk(&conn, "A. Lovelace").unwrap();
        svc_author::merge(&mut conn, keep, &[dup]).unwrap();

        let g = augmented_graph_data(&conn, false).unwrap();
        let authors = author_nodes(&g);
        assert_eq!(authors.len(), 1, "merged into one: {authors:?}");
        let id = format!("author::{keep}");
        assert_eq!(authors[0]["id"], id);
        assert_eq!(edge_count(&g, a, &id), 1);
        assert_eq!(edge_count(&g, b, &id), 1);
    }

    #[test]
    fn exclude_single_authors_keeps_only_multi_paper_people() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        seed_paper(&conn, "arxiv:1", r#"["Ada Lovelace","Solo Dev"]"#, "[]");
        seed_paper(&conn, "arxiv:2", r#"["ada lovelace"]"#, "[]");

        // Ada is one person on two papers despite the spelling; Solo Dev is on one.
        let g = augmented_graph_data(&conn, true).unwrap();
        let authors = author_nodes(&g);
        assert_eq!(authors.len(), 1, "only Ada survives: {authors:?}");
        assert_eq!(authors[0]["label"], "Ada Lovelace");
    }

    #[test]
    fn empty_db_is_empty_graph_and_options() {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        assert_eq!(
            augmented_graph_data(&conn, false).unwrap(),
            json!({ "nodes": [], "edges": [] })
        );
        assert_eq!(project_filter_options(&conn).unwrap(), Vec::<Value>::new());
    }
}
