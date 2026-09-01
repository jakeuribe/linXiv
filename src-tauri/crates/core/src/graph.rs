//! The Knowledge Graph payload — one typed answer for `GET /api/graph`.
//!
//! The graph used to be drawn by a standalone browser script (`public/graph/
//! graph.js`) loaded into an iframe, which meant every derivation the canvas
//! needed had to be redone in JavaScript against an untyped `serde_json::Value`
//! wire shape: the tag spellings, the "no publication date" sentinel, the paper→
//! author name index the Author filter matches on, the degree each author/tag
//! node stands for, and the narrowing that keeps a filter dropdown from offering
//! values that can only empty the canvas. The graph is a React page now, so all
//! of that moved HERE — to the side that has the database, one canonical struct
//! per wire shape, and `#[derive(TS)]` bindings the frontend consumes directly.
//!
//! What the frontend is still left to do is the part only the DOM knows: which
//! nodes a filter MATCHES (the non-matching ones stay drawn as ghosts, so this
//! cannot be a WHERE clause), the force layout, and painting.
//!
//! Author nodes come from PAPER_TO_AUTHOR and are keyed by AUTHOR_FK
//! (`author::<fk>`) — the relational half of the dual author storage every other
//! author read in the app already goes through. `PAPER_META.AUTHORS` is only a
//! free-text cache: it spells one person several ways and goes stale when an
//! author is renamed or merged, so keying on it split one person across nodes and
//! left them without the `author_id` their click handler navigates to.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::Connection;
use serde::Serialize;
use ts_rs::TS;

use crate::error::Result;
use crate::models::{Status, NO_PUBLISHED_DATE};
use crate::service::paper as svc_paper;
use crate::service::project::{self, Projects};
use crate::service::tag as svc_tag;
use crate::storage::db::list_from_sql;
use crate::storage::queries::tag::READING_LIST_TAG;

const DEFAULT_PROJECT_COLOR: &str = "#5b8dee";

/// The one normalization every tag comparison in the graph goes through: TRIM,
/// then fold to lower case. Both halves are the storage layer's own rule.
///
///   - lower case, because TAG.TAG is UNIQUE COLLATE NOCASE, so a paper carries
///     the raw casing from its own metadata while the TAG table holds the
///     canonical one: "ML" must match "ml".
///   - trim, because the tag node id is built from a trimmed label while
///     `PAPER_META.TAGS` is stored verbatim on some paths (`export_import`
///     hands the archive's own strings straight to `add_paper_tags`), and
///     nothing upstream guarantees the two agree. Folding only the case left
///     "ml " and "ml" as two different tags in one library.
///
/// Public because the frontend's tag rows are free text typed by the user and
/// must fold the same way; [`GraphPaper::tag_keys`] is the set they compare to.
pub fn norm_tag(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// The node id for a tag, from its normalized key.
fn tag_node_id(key: &str) -> String {
    format!("tag::{key}")
}

/// One paper on the canvas: the latest version of an active root.
#[derive(Debug, Clone, Serialize, TS)]
pub struct GraphPaper {
    /// Node id — `PAPER_ROOTS.SOURCE_FK` as a string, so every id in this
    /// payload (papers, authors, tags) is one comparable type. The numeric
    /// value is [`Self::source_fk`], which is the `/library/:sfk` route param.
    pub id: String,
    pub source_fk: i64,
    /// `PAPER_ROOTS.SOURCE_ID` — the vocabulary the project picker speaks.
    pub source_id: String,
    /// `PAPER.TITLE`. The column is NOT NULL; an empty string is the honest
    /// answer for a row that somehow holds one, and the title filter can
    /// compare it unguarded.
    pub label: String,
    pub category: Option<String>,
    /// Display spellings of this paper's tags, deduped on [`norm_tag`] and
    /// resolved to the TAG table's casing — i.e. exactly the tag CHIPS the
    /// canvas draws for it, in the same order as [`Self::tag_keys`].
    pub tags: Vec<String>,
    /// [`norm_tag`] of each entry in [`Self::tags`]. What a tag filter row is
    /// compared against, so the comparison needs no normalization pass on the
    /// client and cannot disagree with the chips.
    pub tag_keys: Vec<String>,
    pub has_pdf: bool,
    /// `PAPER_META.PUBLISHED`, with the [`NO_PUBLISHED_DATE`] sentinel folded to
    /// `None`. Forwarding it raw made an undated paper read as a real date in
    /// year 1, so a Date-range `From` filter silently dropped every undated
    /// paper off the canvas as "too old".
    pub published: Option<String>,
    pub url: Option<String>,
    pub doi: Option<String>,
    pub summary: Option<String>,
    /// Ids of the ACTIVE projects holding this paper, ascending.
    pub project_ids: Vec<i64>,
    /// Lowercased names of this paper's authors — what the Author highlight
    /// filter substring-matches. Author names only reach the canvas as separate
    /// author NODES, so the client used to rebuild this index by walking the
    /// edge list on every load.
    pub author_keys: Vec<String>,
}

/// One AUTHOR row linked to at least one paper on the canvas.
#[derive(Debug, Clone, Serialize, TS)]
pub struct GraphAuthor {
    /// `author::<AUTHOR_FK>`.
    pub id: String,
    /// AUTHOR_FK, the `/authors/:id` route param.
    pub author_id: i64,
    /// `AUTHOR.AUTHOR_FULL_NAME` — the canonical spelling, so it follows renames
    /// and merges that `PAPER_META.AUTHORS` does not see.
    pub label: String,
    /// Papers on THIS canvas joined to this author. The hover inspector reports
    /// it; the client has no other source for a degree.
    pub paper_count: usize,
}

/// One tag carried by at least one paper on the canvas.
#[derive(Debug, Clone, Serialize, TS)]
pub struct GraphTag {
    /// `tag::<key>`.
    pub id: String,
    /// [`norm_tag`] of the label — the key a tag filter row matches on.
    pub key: String,
    /// `TAG.TAG`, the spelling the Tags index and TagPage show, falling back to
    /// the paper's own casing for a tag the TAG table cannot answer for (the
    /// reserved reading-list marker, which `list_all_tags` filters out).
    /// Resolving it here is what stops one tag being drawn "ML" on the canvas
    /// and offered as "ml" in the dropdown two panels away.
    pub label: String,
    pub paper_count: usize,
}

/// Always paper → author or paper → tag; both ends are node ids.
#[derive(Debug, Clone, Serialize, TS)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
}

/// An active project, as the Projects / Project Tags filters see it.
#[derive(Debug, Clone, Serialize, TS)]
pub struct GraphProject {
    pub id: i64,
    pub name: String,
    /// Always set: falls back to the app's default accent for a project with
    /// no colour of its own.
    pub color: String,
    /// `PROJECT_TO_TAG` labels, ordered by label, with the reserved
    /// reading-list marker removed — it is bookkeeping nobody typed, and every
    /// other surface that draws a project's tags filters it out too.
    pub tags: Vec<String>,
    /// Whether any paper on THIS canvas belongs to the project. Both filter
    /// boxes match a paper through [`GraphPaper::project_ids`], so a project
    /// that is active but holds no drawn paper can only empty the canvas —
    /// the frontend narrows what it OFFERS to the ones flagged here, and marks
    /// a hand-typed row that names only the others.
    pub on_graph: bool,
}

/// Everything the Knowledge Graph page needs, in one answer.
///
/// It used to be four requests (`/api/graph`, `/api/graph/project-options`,
/// `/api/categories`, `/api/tags`) fired in parallel from inside the iframe,
/// three of them optional so a dropdown endpoint being down could not fail a
/// load the graph request had already succeeded at. One query against one
/// connection cannot half-fail, so that whole partial-failure protocol goes
/// away with it.
#[derive(Debug, Clone, Serialize, TS)]
pub struct GraphView {
    pub papers: Vec<GraphPaper>,
    pub authors: Vec<GraphAuthor>,
    pub tags: Vec<GraphTag>,
    pub edges: Vec<GraphEdge>,
    /// Distinct `PAPER.CATEGORY` values in the library, for the Category box.
    pub categories: Vec<String>,
    pub projects: Vec<GraphProject>,
}

impl GraphView {
    /// Papers + authors + tags. Zero means the LIBRARY is empty, which is the
    /// page's empty state — never "filtered down to nothing".
    pub fn node_count(&self) -> usize {
        self.papers.len() + self.authors.len() + self.tags.len()
    }
}

/// One paper row, before its tags and projects are resolved.
struct PaperRow {
    source_fk: i64,
    source_id: String,
    label: String,
    category: Option<String>,
    raw_tags: Vec<String>,
    has_pdf: bool,
    published: Option<String>,
    url: Option<String>,
    doi: Option<String>,
    summary: Option<String>,
}

/// Paper rows: the latest version of each active root.
const PAPER_NODES_SQL: &str = "\
    SELECT r.SOURCE_FK AS source_fk, r.SOURCE_ID AS source_id, \
           p.TITLE AS title, p.CATEGORY AS category, \
           m.TAGS AS tags, p.HAS_PDF AS has_pdf, m.PUBLISHED AS published, \
           m.URL AS url, m.DOI AS doi, m.SUMMARY AS summary \
    FROM PAPER_ROOTS r \
    JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK \
    JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID \
    WHERE p.VERSION = (SELECT MAX(VERSION) FROM PAPER WHERE SOURCE_FK = r.SOURCE_FK) \
      AND r.STATUS = 'active' \
    ORDER BY r.SOURCE_FK";

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

/// Build the whole `GET /api/graph` answer.
pub fn graph_view(conn: &Connection, exclude_single_authors: bool) -> Result<GraphView> {
    let rows = paper_rows(conn)?;

    // Canonical tag spellings, indexed by key. `list_all_tags` walks the whole
    // TAG table, so this answers for tags no paper carries too — the narrowing
    // below is what keeps those out of the payload.
    let canonical: HashMap<String, String> = svc_tag::list_all_tags(conn)?
        .into_iter()
        .map(|l| (norm_tag(&l), l.trim().to_string()))
        .collect();

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

    let (authors_by_paper, mut authors) = author_rows(conn, exclude_single_authors)?;

    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut papers: Vec<GraphPaper> = Vec::with_capacity(rows.len());
    // key -> (display label, paper count). BTreeMap so tag nodes come out in a
    // stable, sorted order rather than SQLite's scan order.
    let mut tag_acc: BTreeMap<String, (String, usize)> = BTreeMap::new();
    // Author node id -> papers joined to it, counted from the edges actually
    // emitted (so it is a degree on THIS canvas, not a library-wide total).
    let mut author_degree: HashMap<String, usize> = HashMap::new();
    let mut on_graph_projects: HashSet<i64> = HashSet::new();

    for row in rows {
        let node_id = row.source_fk.to_string();

        let mut projects: Vec<i64> = paper_to_projects
            .get(&row.source_fk)
            .cloned()
            .unwrap_or_default();
        projects.sort_unstable();
        projects.dedup();
        on_graph_projects.extend(projects.iter().copied());

        // Tags: normalize, drop blanks, dedup on the key, and resolve the
        // display spelling — the paper's own casing is not what the canvas draws.
        let mut tags = Vec::new();
        let mut tag_keys = Vec::new();
        for raw in &row.raw_tags {
            let key = norm_tag(raw);
            if key.is_empty() || tag_keys.contains(&key) {
                continue;
            }
            let label = canonical
                .get(&key)
                .cloned()
                .unwrap_or_else(|| raw.trim().to_string());
            let entry = tag_acc.entry(key.clone()).or_insert((label.clone(), 0));
            entry.1 += 1;
            edges.push(GraphEdge {
                source: node_id.clone(),
                target: tag_node_id(&key),
            });
            tags.push(label);
            tag_keys.push(key);
        }

        let mut author_keys = Vec::new();
        for author in authors_by_paper.get(&row.source_fk).into_iter().flatten() {
            author_keys.push(author.label.to_lowercase());
            *author_degree.entry(author.id.clone()).or_insert(0) += 1;
            edges.push(GraphEdge {
                source: node_id.clone(),
                target: author.id.clone(),
            });
        }

        papers.push(GraphPaper {
            id: node_id,
            source_fk: row.source_fk,
            source_id: row.source_id,
            label: row.label,
            category: row.category,
            tags,
            tag_keys,
            has_pdf: row.has_pdf,
            published: row.published,
            url: row.url,
            doi: row.doi,
            summary: row.summary,
            project_ids: projects,
            author_keys,
        });
    }

    for author in &mut authors {
        author.paper_count = author_degree.get(&author.id).copied().unwrap_or(0);
    }
    // An author whose every paper left the canvas has no edge to stand on; the
    // SQL cannot produce one, but the invariant is worth holding explicitly.
    authors.retain(|a| a.paper_count > 0);

    let tags = tag_acc
        .into_iter()
        .map(|(key, (label, paper_count))| GraphTag {
            id: tag_node_id(&key),
            key,
            label,
            paper_count,
        })
        .collect();

    let mut projects: Vec<GraphProject> = active
        .into_iter()
        .filter_map(|p| {
            let id = p.id?;
            Some(GraphProject {
                id,
                name: p.name,
                color: p
                    .color
                    .map(project::color_to_hex)
                    .unwrap_or_else(|| DEFAULT_PROJECT_COLOR.to_string()),
                tags: p
                    .project_tags
                    .into_iter()
                    .filter(|t| !t.eq_ignore_ascii_case(READING_LIST_TAG))
                    .collect(),
                on_graph: on_graph_projects.contains(&id),
            })
        })
        .collect();
    projects.sort_by_key(|p| p.id);

    Ok(GraphView {
        papers,
        authors,
        tags,
        edges,
        categories: svc_paper::get_categories(conn)?,
        projects,
    })
}

fn paper_rows(conn: &Connection) -> Result<Vec<PaperRow>> {
    let mut stmt = conn.prepare(PAPER_NODES_SQL)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let raw_tags: Vec<String> = row
            .get::<_, Option<String>>("tags")?
            .as_deref()
            .map(list_from_sql)
            .transpose()?
            .unwrap_or_default();
        let published = row
            .get::<_, Option<String>>("published")?
            .filter(|p| !p.is_empty() && p != NO_PUBLISHED_DATE);
        out.push(PaperRow {
            source_fk: row.get("source_fk")?,
            source_id: row.get("source_id")?,
            label: row.get::<_, Option<String>>("title")?.unwrap_or_default(),
            category: row.get("category")?,
            raw_tags,
            has_pdf: row.get::<_, i64>("has_pdf")? != 0,
            published,
            url: row.get("url")?,
            doi: row.get("doi")?,
            summary: row.get("summary")?,
        });
    }
    Ok(out)
}

/// `(paper SOURCE_FK -> its authors in author order, every distinct author)`.
/// A paper can list the same person twice (a repeat or a case variant); they
/// appear once per paper, so exactly one edge is emitted for the pair.
type AuthorsByPaper = HashMap<i64, Vec<GraphAuthor>>;
fn author_rows(
    conn: &Connection,
    exclude_single_authors: bool,
) -> Result<(AuthorsByPaper, Vec<GraphAuthor>)> {
    let mut stmt = conn.prepare(&author_rows_sql(exclude_single_authors))?;
    let mut rows = stmt.query([])?;
    let mut by_paper: AuthorsByPaper = HashMap::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let mut all = Vec::new();
    while let Some(row) = rows.next()? {
        let source_fk: i64 = row.get("source_fk")?;
        let author_id: i64 = row.get("author_fk")?;
        let label: String = row.get("author_name")?;
        let author = GraphAuthor {
            id: format!("author::{author_id}"),
            author_id,
            label,
            paper_count: 0,
        };
        if seen.insert(author_id) {
            all.push(author.clone());
        }
        let list = by_paper.entry(source_fk).or_default();
        if !list.iter().any(|a| a.author_id == author_id) {
            list.push(author);
        }
    }
    Ok((by_paper, all))
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
        seed_paper_dated(conn, source_id, authors_json, tags_json, "2024-01-01")
    }

    fn seed_paper_dated(
        conn: &Connection,
        source_id: &str,
        authors_json: &str,
        tags_json: &str,
        published: &str,
    ) -> i64 {
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
             VALUES (?1, ?4, ?2, ?3, 'S')",
            rusqlite::params![pid, authors_json, tags_json, published],
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

    fn conn() -> Connection {
        let conn = db::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        conn
    }

    /// How many `source -> target` edges the payload carries.
    fn edge_count(v: &GraphView, source: &str, target: &str) -> usize {
        v.edges
            .iter()
            .filter(|e| e.source == source && e.target == target)
            .count()
    }

    #[test]
    fn view_has_paper_author_tag_nodes_and_edges() {
        let conn = conn();
        let sfk = seed_paper(
            &conn,
            "arxiv:2204.1",
            r#"["Ada Lovelace","Alan Turing"]"#,
            r#"["ML","nlp"]"#,
        );

        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.papers.len(), 1);
        assert_eq!(v.authors.len(), 2);
        assert_eq!(v.tags.len(), 2);
        assert_eq!(v.node_count(), 5);

        let paper = &v.papers[0];
        assert_eq!(paper.id, sfk.to_string());
        assert_eq!(paper.source_fk, sfk);
        assert_eq!(paper.project_ids, Vec::<i64>::new());
        assert!(paper.has_pdf);
        // Author names ride on the paper, lowercased, in the paper's own order —
        // the Author filter matches this without walking a single edge.
        assert_eq!(paper.author_keys, ["ada lovelace", "alan turing"]);

        let ada_fk = find_author_fk(&conn, "Ada Lovelace").unwrap();
        let ada = v.authors.iter().find(|a| a.author_id == ada_fk).unwrap();
        assert_eq!(ada.label, "Ada Lovelace");
        assert_eq!(ada.id, format!("author::{ada_fk}"));
        assert_eq!(ada.paper_count, 1);
        assert_eq!(edge_count(&v, &paper.id, &ada.id), 1);

        // Tag nodes are keyed on the normalized label; both keys and display
        // spellings ride on the paper in the same order.
        assert_eq!(paper.tag_keys, ["ml", "nlp"]);
        let ml = v.tags.iter().find(|t| t.key == "ml").unwrap();
        assert_eq!(ml.id, "tag::ml");
        assert_eq!(ml.paper_count, 1);
        assert_eq!(edge_count(&v, &paper.id, "tag::ml"), 1);
    }

    #[test]
    fn tag_label_follows_the_tag_table_not_the_papers_casing() {
        let conn = conn();
        // The TAG table holds "ML"; this paper's own metadata spells it "ml ".
        conn.execute("INSERT INTO TAG (TAG) VALUES ('ML')", [])
            .unwrap();
        seed_paper(&conn, "arxiv:1", "[]", r#"["ml "]"#);

        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.tags.len(), 1);
        assert_eq!(v.tags[0].key, "ml");
        // Canvas chip and filter dropdown now agree — both read TAG.TAG.
        assert_eq!(v.tags[0].label, "ML");
        assert_eq!(v.papers[0].tags, ["ML"]);
        assert_eq!(v.papers[0].tag_keys, ["ml"]);
    }

    #[test]
    fn tag_the_tag_table_cannot_answer_for_keeps_the_papers_spelling() {
        let conn = conn();
        // `list_all_tags` filters the reading-list marker out, so it is
        // the one tag on a paper that this map has no entry for.
        seed_paper(&conn, "arxiv:1", "[]", r#"["Reading-List"]"#);
        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.tags[0].label, "Reading-List");
        assert_eq!(v.tags[0].key, "reading-list");
    }

    #[test]
    fn a_papers_duplicate_tags_collapse_to_one_chip_and_one_edge() {
        let conn = conn();
        // Untrimmed and case-variant spellings of one tag, plus a blank.
        let sfk = seed_paper(&conn, "arxiv:1", "[]", r#"["ML"," ml ","",  "nlp"]"#);
        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.papers[0].tag_keys, ["ml", "nlp"]);
        assert_eq!(edge_count(&v, &sfk.to_string(), "tag::ml"), 1);
        assert_eq!(
            v.tags.iter().find(|t| t.key == "ml").unwrap().paper_count,
            1
        );
    }

    #[test]
    fn undated_papers_report_no_date_rather_than_year_one() {
        let conn = conn();
        seed_paper_dated(&conn, "arxiv:1", "[]", "[]", NO_PUBLISHED_DATE);
        seed_paper_dated(&conn, "arxiv:2", "[]", "[]", "2024-05-06");
        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.papers[0].published, None);
        assert_eq!(v.papers[1].published.as_deref(), Some("2024-05-06"));
    }

    #[test]
    fn author_spelled_differently_across_papers_is_one_node() {
        let conn = conn();
        // Same person, two spellings — PAPER_META.AUTHORS is a free-text cache, and
        // the AUTHOR row it reconciles against is matched COLLATE NOCASE.
        let a = seed_paper(&conn, "arxiv:1", r#"["Ada Lovelace"]"#, "[]");
        let b = seed_paper(&conn, "arxiv:2", r#"["ada lovelace"]"#, "[]");

        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.authors.len(), 1, "one person, one node: {:?}", v.authors);
        let id = author_id(&conn, "Ada Lovelace");
        assert_eq!(v.authors[0].id, id);
        assert_eq!(v.authors[0].paper_count, 2);
        assert_eq!(edge_count(&v, &a.to_string(), &id), 1);
        assert_eq!(edge_count(&v, &b.to_string(), &id), 1);
    }

    #[test]
    fn author_listed_twice_on_one_paper_yields_one_edge() {
        let conn = conn();
        let authors = r#"["Ada Lovelace","ada lovelace"]"#;
        let sfk = seed_paper(&conn, "arxiv:1", authors, "[]");

        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.authors.len(), 1);
        let id = author_id(&conn, "Ada Lovelace");
        assert_eq!(edge_count(&v, &sfk.to_string(), &id), 1);
        assert_eq!(v.authors[0].paper_count, 1);
        // …and one entry in the index the Author filter reads.
        assert_eq!(v.papers[0].author_keys, ["ada lovelace"]);
    }

    #[test]
    fn author_node_label_follows_the_author_table_spelling() {
        let conn = conn();
        let sql = "INSERT INTO AUTHOR (AUTHOR_FULL_NAME) VALUES ('Ada Lovelace')";
        conn.execute(sql, []).unwrap();
        let fk = conn.last_insert_rowid();
        // The paper caches a lowercased spelling; the node must still show the
        // canonical one, since clicking it opens that AUTHOR's page.
        seed_paper(&conn, "arxiv:1", r#"["ada lovelace"]"#, "[]");

        let v = graph_view(&conn, false).unwrap();
        let ada = v.authors.iter().find(|a| a.author_id == fk).unwrap();
        assert_eq!(ada.label, "Ada Lovelace");
        // The filter index follows the canonical spelling too, so typing the
        // name as the Authors page shows it matches.
        assert_eq!(v.papers[0].author_keys, ["ada lovelace"]);
    }

    #[test]
    fn renaming_an_author_relabels_its_graph_node() {
        let conn = conn();
        let sfk = seed_paper(&conn, "arxiv:1", r#"["A Lovelace"]"#, "[]");
        let fk = find_author_fk(&conn, "A Lovelace").unwrap();
        // PATCH /api/authors/{id} rewrites the AUTHOR row only — PAPER_META.AUTHORS
        // keeps caching the old spelling, which a name-keyed graph rendered instead.
        store_author::update_author(&conn, fk, Some("Ada Lovelace"), None, None, None).unwrap();

        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.authors.len(), 1, "still one person: {:?}", v.authors);
        assert_eq!(v.authors[0].label, "Ada Lovelace");
        assert_eq!(
            edge_count(&v, &sfk.to_string(), &format!("author::{fk}")),
            1
        );
    }

    #[test]
    fn merged_authors_collapse_into_one_graph_node() {
        let mut conn = conn();
        let a = seed_paper(&conn, "arxiv:1", r#"["Ada Lovelace"]"#, "[]");
        let b = seed_paper(&conn, "arxiv:2", r#"["A. Lovelace"]"#, "[]");
        let keep = find_author_fk(&conn, "Ada Lovelace").unwrap();
        let dup = find_author_fk(&conn, "A. Lovelace").unwrap();
        svc_author::merge(&mut conn, keep, &[dup]).unwrap();

        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.authors.len(), 1, "merged into one: {:?}", v.authors);
        let id = format!("author::{keep}");
        assert_eq!(v.authors[0].id, id);
        assert_eq!(v.authors[0].paper_count, 2);
        assert_eq!(edge_count(&v, &a.to_string(), &id), 1);
        assert_eq!(edge_count(&v, &b.to_string(), &id), 1);
    }

    #[test]
    fn exclude_single_authors_keeps_only_multi_paper_people() {
        let conn = conn();
        seed_paper(&conn, "arxiv:1", r#"["Ada Lovelace","Solo Dev"]"#, "[]");
        seed_paper(&conn, "arxiv:2", r#"["ada lovelace"]"#, "[]");

        // Ada is one person on two papers despite the spelling; Solo Dev is on one.
        let v = graph_view(&conn, true).unwrap();
        assert_eq!(v.authors.len(), 1, "only Ada survives: {:?}", v.authors);
        assert_eq!(v.authors[0].label, "Ada Lovelace");
        // The dropped author leaves the paper's filter index too — the Author
        // box genuinely cannot match them, which is what the page's notice says.
        assert_eq!(v.papers[0].author_keys, ["ada lovelace"]);
    }

    #[test]
    fn projects_carry_their_swatch_and_whether_they_touch_this_canvas() {
        let mut conn = conn();
        let sfk = seed_paper(&conn, "arxiv:1", "[]", "[]");
        let held = project::create(
            &mut conn,
            &crate::models::ProjectIn {
                name: "Held".into(),
                description: String::new(),
                color: None,
                tags: vec!["ml".into(), READING_LIST_TAG.into()],
                source_fks: vec![sfk],
            },
        )
        .unwrap();
        let empty = project::create(
            &mut conn,
            &crate::models::ProjectIn {
                name: "Empty".into(),
                description: String::new(),
                color: None,
                tags: vec![],
                source_fks: vec![],
            },
        )
        .unwrap();

        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.papers[0].project_ids, vec![held]);

        let held_opt = v.projects.iter().find(|p| p.id == held).unwrap();
        assert!(held_opt.on_graph);
        assert_eq!(held_opt.color, DEFAULT_PROJECT_COLOR);
        // The reserved reading-list marker is bookkeeping nobody typed; it must
        // not be offered as a Project Tags filter value.
        assert_eq!(held_opt.tags, ["ml"]);

        // Active, resolvable, and yet it can only empty the canvas.
        let empty_opt = v.projects.iter().find(|p| p.id == empty).unwrap();
        assert!(!empty_opt.on_graph);
    }

    #[test]
    fn categories_ride_along_with_the_graph() {
        let conn = conn();
        seed_paper(&conn, "arxiv:1", "[]", "[]");
        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.categories, ["cs.LG"]);
    }

    #[test]
    fn empty_db_is_an_empty_view() {
        let conn = conn();
        let v = graph_view(&conn, false).unwrap();
        assert_eq!(v.node_count(), 0);
        assert!(v.papers.is_empty());
        assert!(v.authors.is_empty());
        assert!(v.tags.is_empty());
        assert!(v.edges.is_empty());
        assert!(v.categories.is_empty());
        assert!(v.projects.is_empty());
    }
}
