//! Graph nodes/edges for the GUI graph view — Rust port of `api/graph_payload.py`
//! (`get_augmented_graph_data` + `project_filter_options`) over `storage/db.py`'s
//! `get_graph_data`. Faithful to the CURRENT output (the graph subsystem is
//! mid-refactor; re-port if the shape changes).
//!
//! Author nodes are keyed by the JSON author NAME (`author::<name>`), not the
//! AUTHOR_FK — the FK is attached only for navigation when a name resolves to one.

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

/// Per author name (COLLATE NOCASE): the AUTHOR_FK with the most papers, ties by
/// lowest FK. Shared CTE for both author queries.
const NAME_FK_CTE: &str = "\
    name_fk AS ( \
        SELECT author_name, author_fk FROM ( \
            SELECT a.AUTHOR_FULL_NAME AS author_name, a.AUTHOR_FK AS author_fk, \
                   ROW_NUMBER() OVER ( \
                       PARTITION BY a.AUTHOR_FULL_NAME COLLATE NOCASE \
                       ORDER BY COALESCE(apc.paper_count, 0) DESC, a.AUTHOR_FK ASC \
                   ) AS rn \
            FROM AUTHOR a \
            LEFT JOIN author_paper_counts apc ON apc.author_fk = a.AUTHOR_FK \
            WHERE a.AUTHOR_FULL_NAME IS NOT NULL \
        ) WHERE rn = 1 \
    )";

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

/// `(paper_nodes [(source_fk, node, tags)], author_nodes, edges)` — `get_graph_data`.
fn graph_data(
    conn: &Connection,
    exclude_single_authors: bool,
) -> Result<(Vec<(i64, Value, Vec<String>)>, Vec<Value>, Vec<Value>)> {
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

    let (count_cte, count_col, count_join) = if exclude_single_authors {
        (
            ", name_counts AS ( \
             SELECT a.AUTHOR_FULL_NAME AS author_name, MAX(apc.paper_count) AS paper_count \
             FROM AUTHOR a JOIN author_paper_counts apc ON apc.author_fk = a.AUTHOR_FK \
             GROUP BY a.AUTHOR_FULL_NAME COLLATE NOCASE \
         )",
            "nc.paper_count AS paper_count, ",
            "LEFT JOIN name_counts nc ON nc.author_name = je.value COLLATE NOCASE ",
        )
    } else {
        ("", "", "")
    };
    let authors_sql = format!(
        "WITH {NAME_FK_CTE}{count_cte} \
         SELECT r.SOURCE_FK AS source_fk, je.value AS author_name, {count_col}nf.author_fk AS author_fk \
         FROM PAPER_ROOTS r \
         JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK \
         JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID, json_each(m.AUTHORS) je \
         {count_join}LEFT JOIN name_fk nf ON nf.author_name = je.value COLLATE NOCASE \
         WHERE p.VERSION = (SELECT MAX(VERSION) FROM PAPER WHERE SOURCE_FK = r.SOURCE_FK) \
           AND r.STATUS = 'active'"
    );

    let mut author_stmt = conn.prepare(&authors_sql)?;
    let mut rows = author_stmt.query([])?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut author_nodes = Vec::new();
    let mut edges = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get("author_name")?;
        // Drop authors with a known count < 2; a NULL count (no AUTHOR match) is kept.
        if exclude_single_authors {
            if let Some(count) = row.get::<_, Option<i64>>("paper_count")? {
                if count < 2 {
                    continue;
                }
            }
        }
        let source_fk: i64 = row.get("source_fk")?;
        let node_id = format!("author::{name}");
        if seen.insert(node_id.clone()) {
            let mut node = json!({ "id": node_id, "label": name, "type": "author" });
            if let Some(fk) = row.get::<_, Option<i64>>("author_fk")? {
                node["author_id"] = json!(fk);
            }
            author_nodes.push(node);
        }
        edges.push(json!({ "source": source_fk, "target": node_id }));
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
    use crate::storage::{self, db};

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
        sfk
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

        // author edge keys author::<name>; one author + one tag edge per paper.
        assert!(edges
            .iter()
            .any(|e| e["source"] == json!(sfk) && e["target"] == json!("author::Ada Lovelace")));
        assert!(edges.iter().any(|e| e["target"] == json!("tag::ml")));
        // tag node id is lowercased, label preserved.
        assert!(nodes
            .iter()
            .any(|n| n["id"] == json!("tag::ml") && n["label"] == json!("ML")));
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
