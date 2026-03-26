import sqlite3
import json
import re
import arxiv
from typing import Optional

DB_PATH = "papers.db"

def _connect() -> sqlite3.Connection:
    conn = sqlite3.connect(DB_PATH)
    conn.row_factory = sqlite3.Row
    return conn

def init_db() -> None:
    with _connect() as conn:
        conn.executescript("""
            CREATE TABLE IF NOT EXISTS papers (
                paper_id    TEXT    NOT NULL,
                version     INTEGER NOT NULL,
                title       TEXT    NOT NULL,
                url         TEXT,
                published   TEXT,
                updated     TEXT,
                category    TEXT,
                categories  TEXT,
                doi         TEXT,
                journal_ref TEXT,
                comment     TEXT,
                summary     TEXT,
                authors     TEXT,
                PRIMARY KEY (paper_id, version)
            );

            CREATE VIEW IF NOT EXISTS latest_papers AS
            SELECT * FROM papers p
            WHERE version = (
                SELECT MAX(version) FROM papers WHERE paper_id = p.paper_id
            );
        """)
        # Migrate existing DBs that are missing the new columns
        existing = {row[1] for row in conn.execute("PRAGMA table_info(papers)")}
        for col, typedef in [
            ("updated",     "TEXT"),
            ("categories",  "TEXT"),
            ("journal_ref", "TEXT"),
            ("comment",     "TEXT"),
        ]:
            if col not in existing:
                conn.execute(f"ALTER TABLE papers ADD COLUMN {col} {typedef}")

def _parse_entry_id(entry_id: str) -> tuple[str, int]:
    """Split 'http://arxiv.org/abs/2204.12985v4' into ('2204.12985', 4)."""
    raw = entry_id.split('/')[-1]          # '2204.12985v4'
    match = re.match(r'^(.+?)(?:v(\d+))?$', raw)
    paper_id = match.group(1)
    version = int(match.group(2)) if match.group(2) else 1
    return paper_id, version

def _insert(conn: sqlite3.Connection, paper: arxiv.Result) -> tuple[str, int]:
    paper_id, version = _parse_entry_id(paper.entry_id)
    conn.execute("""
        INSERT OR REPLACE INTO papers
            (paper_id, version, title, url, published, updated,
             category, categories, doi, journal_ref, comment, summary, authors)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    """, (
        paper_id,
        version,
        paper.title,
        paper.pdf_url,
        paper.published.strftime('%Y-%m-%d'),
        paper.updated.strftime('%Y-%m-%d'),
        paper.primary_category,
        json.dumps(paper.categories),
        paper.doi,
        paper.journal_ref,
        paper.comment,
        paper.summary,
        json.dumps([a.name for a in paper.authors]),
    ))
    return paper_id, version

def save_paper(paper: arxiv.Result) -> tuple[str, int]:
    """Insert or replace a single paper. Returns (paper_id, version)."""
    with _connect() as conn:
        return _insert(conn, paper)

def save_papers(papers: list[arxiv.Result]) -> list[tuple[str, int]]:
    """Batch insert/replace papers in a single transaction. Returns list of (paper_id, version)."""
    with _connect() as conn:
        return [_insert(conn, paper) for paper in papers]

def get_paper(paper_id: str, version: Optional[int] = None) -> Optional[sqlite3.Row]:
    """Fetch a specific version, or the latest if version is None."""
    with _connect() as conn:
        if version is not None:
            return conn.execute(
                "SELECT * FROM papers WHERE paper_id = ? AND version = ?",
                (paper_id, version)
            ).fetchone()
        else:
            return conn.execute(
                "SELECT * FROM latest_papers WHERE paper_id = ?",
                (paper_id,)
            ).fetchone()

def get_all_versions(paper_id: str) -> list[sqlite3.Row]:
    """Fetch all stored versions of a paper, ordered oldest to newest."""
    with _connect() as conn:
        return conn.execute(
            "SELECT * FROM papers WHERE paper_id = ? ORDER BY version ASC",
            (paper_id,)
        ).fetchall()

def get_graph_data() -> tuple[list[dict], list[dict]]:
    """
    Returns (nodes, edges) ready to pass to the graph view.
    Paper nodes and author nodes are typed separately.
    Edges connect each paper to its authors via a json_each expansion.
    """
    with _connect() as conn:
        paper_nodes = [
            {"id": row["paper_id"], "label": row["title"], "type": "paper"}
            for row in conn.execute("SELECT paper_id, title FROM latest_papers")
        ]
        # Deduplicated author nodes and paper→author edges in one pass
        author_rows = conn.execute("""
            SELECT p.paper_id, je.value AS author_name
            FROM latest_papers p, json_each(p.authors) je
        """).fetchall()

    seen_authors: set[str] = set()
    author_nodes: list[dict] = []
    edges: list[dict] = []
    for row in author_rows:
        name = row["author_name"]
        author_id = f"author::{name}"
        if author_id not in seen_authors:
            author_nodes.append({"id": author_id, "label": name, "type": "author"})
            seen_authors.add(author_id)
        edges.append({"source": row["paper_id"], "target": author_id})

    return paper_nodes + author_nodes, edges

def list_papers(latest_only: bool = True) -> list[sqlite3.Row]:
    """List all stored papers (latest version per paper by default)."""
    with _connect() as conn:
        table = "latest_papers" if latest_only else "papers"
        return conn.execute(f"SELECT * FROM {table} ORDER BY published DESC").fetchall()
