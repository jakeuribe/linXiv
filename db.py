import sqlite3
import json
import re
import datetime
from pathlib import Path
import arxiv
from typing import Optional

DB_PATH = str(Path(__file__).parent / "papers.db")

# --- adapters: Python → SQLite storage ---
sqlite3.register_adapter(list,              lambda v: json.dumps(v))
sqlite3.register_adapter(datetime.date,     lambda v: v.isoformat())
sqlite3.register_adapter(datetime.datetime, lambda v: v.isoformat())

# --- converters: SQLite storage → Python (triggered by declared column type) ---
sqlite3.register_converter("LIST",      lambda v: json.loads(v))
sqlite3.register_converter("DATE",      lambda v: datetime.date.fromisoformat(v.decode()))
sqlite3.register_converter("TIMESTAMP", lambda v: datetime.datetime.fromisoformat(v.decode()))
sqlite3.register_converter("BOOL",      lambda v: bool(int(v)))

# Python type → SQLite type name used in DDL
_PY_TO_SQL: dict[type, str] = {
    int:               "INTEGER",
    str:               "TEXT",
    float:             "REAL",
    bytes:             "BLOB",
    bool:              "BOOL",
    list:              "LIST",
    datetime.date:     "DATE",
    datetime.datetime: "TIMESTAMP",
}


def init_table(
    table_name: str,
    columns: list[tuple],
    primary_key: list[str] | None = None,
    db_path: str = DB_PATH,
) -> None:
    """Create a table if it doesn't exist.

    Each column is a tuple: (name, python_type[, constraints])
    where constraints is an optional SQL string appended after the type.

    Examples
    --------
    init_table("users", [
        ("id",    int,   "PRIMARY KEY AUTOINCREMENT"),
        ("name",  str,   "NOT NULL"),
        ("score", float),
        ("tags",  list),
    ])

    init_table("paper_tags", [
        ("paper_id", str, "NOT NULL"),
        ("tag",      str, "NOT NULL"),
    ], primary_key=["paper_id", "tag"])
    """
    col_defs: list[str] = []
    for col in columns:
        name  = col[0]
        sql_t = _PY_TO_SQL.get(col[1], "TEXT")
        extra = col[2] if len(col) > 2 else ""
        col_defs.append(f"    {name} {sql_t} {extra}".rstrip())
    if primary_key:
        col_defs.append(f"    PRIMARY KEY ({', '.join(primary_key)})")
    ddl = (
        f"CREATE TABLE IF NOT EXISTS {table_name} (\n"
        + ",\n".join(col_defs)
        + "\n);"
    )
    with _connect(db_path) as conn:
        conn.execute(ddl)


def _connect(db_path: str = DB_PATH) -> sqlite3.Connection:
    conn = sqlite3.connect(db_path, detect_types=sqlite3.PARSE_DECLTYPES)
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
                published   DATE,
                updated     DATE,
                category    TEXT,
                categories  LIST,
                doi         TEXT,
                journal_ref TEXT,
                comment     TEXT,
                summary     TEXT,
                authors     LIST,
                tags        LIST,
                has_pdf     BOOL NOT NULL DEFAULT 0,
                PRIMARY KEY (paper_id, version)
            );

            CREATE VIEW IF NOT EXISTS latest_papers AS
            SELECT * FROM papers p
            WHERE version = (
                SELECT MAX(version) FROM papers WHERE paper_id = p.paper_id
            );
        """)
        # Migrate existing DBs that are missing columns
        existing = {row[1] for row in conn.execute("PRAGMA table_info(papers)")}
        for col, typedef in [
            ("updated",     "DATE"),
            ("categories",  "LIST"),
            ("journal_ref", "TEXT"),
            ("comment",     "TEXT"),
            ("tags",        "LIST"),
            ("has_pdf",     "BOOL NOT NULL DEFAULT 0"),
            ("source",      "TEXT DEFAULT 'arxiv'"),
        ]:
            if col not in existing:
                conn.execute(f"ALTER TABLE papers ADD COLUMN {col} {typedef}")


def parse_entry_id(entry_id: str) -> tuple[str, int]:
    """Split 'http://arxiv.org/abs/2204.12985v4' into ('2204.12985', 4)."""
    raw = entry_id.split('/')[-1]
    match = re.match(r'^(.+?)(?:v(\d+))?$', raw)
    assert match is not None
    paper_id = match.group(1)
    version = int(match.group(2)) if match.group(2) else 1
    return paper_id, version


def _insert(conn: sqlite3.Connection, paper: arxiv.Result, tags: list[str] | None = None) -> tuple[str, int]:
    paper_id, version = parse_entry_id(paper.entry_id)
    conn.execute("""
        INSERT OR REPLACE INTO papers
            (paper_id, version, title, url, published, updated,
             category, categories, doi, journal_ref, comment, summary, authors, tags, source)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    """, (
        paper_id,
        version,
        paper.title,
        paper.pdf_url,
        paper.published.date(),
        paper.updated.date(),
        paper.primary_category,
        paper.categories,
        paper.doi,
        paper.journal_ref,
        paper.comment,
        paper.summary,
        [a.name for a in paper.authors],
        tags,
        "arxiv",
    ))
    return paper_id, version


def _insert_metadata(conn: sqlite3.Connection, meta: "PaperMetadata", tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a source-agnostic PaperMetadata into the papers table."""
    from sources.base import PaperMetadata as _PM  # noqa: F811 — deferred to avoid circular import
    merged_tags = meta.tags
    if tags:
        merged_tags = list(set((merged_tags or []) + tags))
    conn.execute("""
        INSERT OR REPLACE INTO papers
            (paper_id, version, title, url, published, updated,
             category, categories, doi, journal_ref, comment, summary, authors, tags, source)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    """, (
        meta.paper_id,
        meta.version,
        meta.title,
        meta.url,
        meta.published,
        meta.updated,
        meta.category,
        meta.categories,
        meta.doi,
        meta.journal_ref,
        meta.comment,
        meta.summary,
        meta.authors,
        merged_tags,
        meta.source,
    ))
    return meta.paper_id, meta.version


def save_paper(paper: arxiv.Result, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert or replace a single arxiv paper. Returns (paper_id, version)."""
    with _connect() as conn:
        return _insert(conn, paper, tags)


def save_papers(papers: list[arxiv.Result], tags: list[str] | None = None) -> list[tuple[str, int]]:
    """Batch insert/replace arxiv papers in a single transaction. Returns list of (paper_id, version)."""
    with _connect() as conn:
        return [_insert(conn, paper, tags) for paper in papers]


def save_paper_metadata(meta: "PaperMetadata", tags: list[str] | None = None) -> tuple[str, int]:
    """Insert or replace a paper from any source via PaperMetadata. Returns (paper_id, version)."""
    with _connect() as conn:
        return _insert_metadata(conn, meta, tags)


def save_papers_metadata(papers: list["PaperMetadata"], tags: list[str] | None = None) -> list[tuple[str, int]]:
    """Batch insert/replace papers from any source. Returns list of (paper_id, version)."""
    with _connect() as conn:
        return [_insert_metadata(conn, meta, tags) for meta in papers]


def set_has_pdf(paper_id: str, version: int, has: bool) -> None:
    """Set the has_pdf flag for a specific paper version."""
    with _connect() as conn:
        conn.execute(
            "UPDATE papers SET has_pdf = ? WHERE paper_id = ? AND version = ?",
            (has, paper_id, version)
        )

def delete_paper(paper_id: str) -> None:
    """Delete all versions of a paper."""
    with _connect() as conn:
        conn.execute("DELETE FROM papers WHERE paper_id = ?", (paper_id,))


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
    """Returns (nodes, edges) ready to pass to the graph view."""
    with _connect() as conn:
        paper_nodes = [
            {
                "id":        row["paper_id"],
                "label":     row["title"],
                "type":      "paper",
                "category":  row["category"],
                "tags":      row["tags"] if row["tags"] is not None else [],
                "has_pdf":   bool(row["has_pdf"]),
                "published": row["published"].isoformat() if row["published"] else None,
            }
            for row in conn.execute(
                "SELECT paper_id, title, category, tags, has_pdf, published FROM latest_papers"
            )
        ]
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


def get_categories() -> list[str]:
    """Return a sorted list of all distinct primary categories in the DB."""
    with _connect() as conn:
        rows = conn.execute(
            "SELECT DISTINCT category FROM latest_papers WHERE category IS NOT NULL ORDER BY category"
        ).fetchall()
    return [row["category"] for row in rows]


def get_tags() -> list[str]:
    """Return a sorted list of all distinct tags across all papers."""
    with _connect() as conn:
        rows = conn.execute("""
            SELECT DISTINCT je.value AS tag
            FROM latest_papers p, json_each(p.tags) je
            WHERE p.tags IS NOT NULL
            ORDER BY je.value
        """).fetchall()
    return [row["tag"] for row in rows]