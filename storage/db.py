from __future__ import annotations
import datetime
import json
from pathlib import Path
import re
import sqlite3
from typing import Optional, TYPE_CHECKING

import arxiv

if TYPE_CHECKING:
    from sources.base import PaperMetadata

DB_PATH = str(Path(__file__).parent.parent / "papers.db")

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
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def _ensure_paper_root_row(conn: sqlite3.Connection, paper_id: str) -> None:
    conn.execute("INSERT OR IGNORE INTO paper_roots (paper_id) VALUES (?)", (paper_id,))


def _author_fk_for_name(conn: sqlite3.Connection, full_name: str) -> int:
    row = conn.execute(
        "SELECT AUTHOR_FK FROM AUTHOR WHERE AUTHOR_FULL_NAME = ? COLLATE NOCASE LIMIT 1",
        (full_name,),
    ).fetchone()
    if row is not None:
        return int(row[0])
    cur = conn.execute(
        "INSERT INTO AUTHOR (AUTHOR_FULL_NAME) VALUES (?)",
        (full_name,),
    )
    return int(cur.lastrowid)


def _tag_fk_for_label(conn: sqlite3.Connection, label: str) -> int:
    row = conn.execute(
        "SELECT TAG_FK FROM TAG WHERE TAG = ? COLLATE NOCASE LIMIT 1",
        (label,),
    ).fetchone()
    if row is not None:
        return int(row[0])
    cur = conn.execute("INSERT INTO TAG (TAG) VALUES (?)", (label,))
    return int(cur.lastrowid)


def _sync_paper_authors(
    conn: sqlite3.Connection,
    paper_id: str,
    version: int,
    authors: list[str] | None,
) -> None:
    conn.execute(
        "DELETE FROM PAPER_TO_AUTHOR WHERE paper_id = ? AND version = ?",
        (paper_id, version),
    )
    if not authors:
        return
    for i, name in enumerate(authors):
        aid = _author_fk_for_name(conn, name)
        conn.execute(
            """
            INSERT INTO PAPER_TO_AUTHOR (paper_id, version, AUTHOR_FK, author_index)
            VALUES (?, ?, ?, ?)
            """,
            (paper_id, version, aid, i),
        )


def _sync_paper_tags(
    conn: sqlite3.Connection,
    paper_id: str,
    version: int,
    tags: list[str] | None,
) -> None:
    conn.execute(
        "DELETE FROM PAPER_TO_TAG WHERE paper_id = ? AND version = ?",
        (paper_id, version),
    )
    if not tags:
        return
    for label in tags:
        tid = _tag_fk_for_label(conn, label)
        conn.execute(
            "INSERT INTO PAPER_TO_TAG (paper_id, version, TAG_FK) VALUES (?, ?, ?)",
            (paper_id, version, tid),
        )


def _write_paper_version(
    conn: sqlite3.Connection,
    paper_id: str,
    version: int,
    title: str,
    category: str | None,
    has_pdf: bool,
    *,
    url: str | None,
    published: datetime.date | None,
    updated: datetime.date | None,
    categories: list[str] | None,
    doi: str | None,
    journal_ref: str | None,
    comment: str | None,
    summary: str | None,
    authors: list[str] | None,
    tags: list[str] | None,
    source: str,
    pdf_path: str | None,
    full_text: str | None,
    downloaded_source: bool | None,
) -> None:
    _ensure_paper_root_row(conn, paper_id)
    conn.execute(
        """
        INSERT INTO PAPER (paper_id, version, title, category, has_pdf)
        VALUES (?, ?, ?, ?, ?)
        """,
        (paper_id, version, title, category, has_pdf),
    )
    conn.execute(
        """
        INSERT INTO PAPER_META (
            paper_id, version, url, published, updated, categories, doi, journal_ref,
            comment, summary, source, pdf_path, full_text, downloaded_source, authors, tags
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            paper_id,
            version,
            url,
            published,
            updated,
            categories,
            doi,
            journal_ref,
            comment,
            summary,
            source,
            pdf_path,
            full_text,
            downloaded_source,
            authors,
            tags,
        ),
    )
    conn.execute(
        "UPDATE PAPER SET updated_at = date('now') WHERE paper_id = ? AND version = ?",
        (paper_id, version),
    )
    _sync_paper_authors(conn, paper_id, version, authors)
    _sync_paper_tags(conn, paper_id, version, tags)


def init_db() -> None:
    from storage.config.core import apply_sql_schema

    with _connect() as conn:
        apply_sql_schema(conn)


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
    _write_paper_version(
        conn,
        paper_id,
        version,
        paper.title,
        paper.primary_category,
        False,
        url=paper.pdf_url,
        published=paper.published.date(),
        updated=paper.updated.date(),
        categories=paper.categories,
        doi=paper.doi,
        journal_ref=paper.journal_ref,
        comment=paper.comment,
        summary=paper.summary,
        authors=[a.name for a in paper.authors],
        tags=tags,
        source="arxiv",
        pdf_path=None,
        full_text=None,
        downloaded_source=None,
    )
    return paper_id, version


def _insert_metadata(conn: sqlite3.Connection, meta: PaperMetadata, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a source-agnostic PaperMetadata row."""
    merged_tags = meta.tags
    if tags:
        merged_tags = list(set((merged_tags or []) + tags))
    _write_paper_version(
        conn,
        meta.paper_id,
        meta.version,
        meta.title,
        meta.category,
        False,
        url=meta.url,
        published=meta.published,
        updated=meta.updated,
        categories=meta.categories,
        doi=meta.doi,
        journal_ref=meta.journal_ref,
        comment=meta.comment,
        summary=meta.summary,
        authors=meta.authors,
        tags=merged_tags,
        source=meta.source,
        pdf_path=None,
        full_text=None,
        downloaded_source=None,
    )
    return meta.paper_id, meta.version


def save_paper(paper: arxiv.Result, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a single arxiv paper. Returns (paper_id, version)."""
    with _connect() as conn:
        return _insert(conn, paper, tags)


def save_papers(papers: list[arxiv.Result], tags: list[str] | None = None) -> list[tuple[str, int]]:
    """Batch insert arxiv papers in a single transaction. Returns list of (paper_id, version)."""
    with _connect() as conn:
        return [_insert(conn, paper, tags) for paper in papers]


def save_paper_metadata(meta: PaperMetadata, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a paper from any source via PaperMetadata. Returns (paper_id, version)."""
    with _connect() as conn:
        return _insert_metadata(conn, meta, tags)


def save_papers_metadata(papers: list[PaperMetadata], tags: list[str] | None = None) -> list[tuple[str, int]]:
    """Batch insert papers from any source. Returns list of (paper_id, version)."""
    with _connect() as conn:
        return [_insert_metadata(conn, meta, tags) for meta in papers]


def set_has_pdf(paper_id: str, version: int, has: bool) -> None:
    """Set the has_pdf flag for a specific paper version."""
    with _connect() as conn:
        conn.execute(
            "UPDATE PAPER SET has_pdf = ? WHERE paper_id = ? AND version = ?",
            (has, paper_id, version),
        )


def set_pdf_path(paper_id: str, path: str) -> None:
    """Set the pdf_path for a paper (all versions)."""
    with _connect() as conn:
        conn.execute(
            "UPDATE PAPER_META SET pdf_path = ? WHERE paper_id = ?",
            (path, paper_id),
        )


def delete_paper(paper_id: str) -> None:
    """Delete all versions of a paper and its root row."""
    with _connect() as conn:
        if conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='papers_fts'"
        ).fetchone():
            conn.execute("DELETE FROM papers_fts WHERE paper_id = ?", (paper_id,))
        conn.execute("DELETE FROM paper_roots WHERE paper_id = ?", (paper_id,))


def get_paper(paper_id: str, version: Optional[int] = None) -> Optional[sqlite3.Row]:
    """Fetch a specific version, or the latest if version is None."""
    with _connect() as conn:
        if version is not None:
            return conn.execute(
                "SELECT * FROM papers WHERE paper_id = ? AND version = ?",
                (paper_id, version),
            ).fetchone()
        return conn.execute(
            "SELECT * FROM latest_papers WHERE paper_id = ?",
            (paper_id,),
        ).fetchone()


def get_all_versions(paper_id: str) -> list[sqlite3.Row]:
    """Fetch all stored versions of a paper, ordered oldest to newest."""
    with _connect() as conn:
        return conn.execute(
            "SELECT * FROM papers WHERE paper_id = ? ORDER BY version ASC",
            (paper_id,),
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
                "url":       row["url"],
                "doi":       row["doi"],
                "summary":   row["summary"],
            }
            for row in conn.execute(
                "SELECT paper_id, title, category, tags, has_pdf, published, url, doi, summary FROM latest_papers"
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


def list_papers(latest_only: bool = True, limit: int | None = None, offset: int = 0) -> list[sqlite3.Row]:
    """List all stored papers (latest version per paper by default)."""
    with _connect() as conn:
        table = "latest_papers" if latest_only else "papers"
        sql = f"SELECT * FROM {table} ORDER BY published DESC"
        params: list[int] = []
        if limit is not None:
            sql += " LIMIT ? OFFSET ?"
            params = [limit, offset]
        return conn.execute(sql, params).fetchall()


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


def add_paper_tags(paper_id: str, tags: list[str]) -> list[str]:
    """Add tags to a paper, deduplicating. Returns the updated tag list."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT tags FROM latest_papers WHERE paper_id = ?", (paper_id,)
        ).fetchone()
        if row is None:
            raise KeyError(paper_id)
        current: list[str] = row["tags"] or []
        merged = list(dict.fromkeys(current + tags))
        conn.execute(
            "UPDATE PAPER_META SET tags = ? WHERE paper_id = ?",
            (merged, paper_id),
        )
        for vr in conn.execute(
            "SELECT DISTINCT version FROM PAPER_META WHERE paper_id = ?",
            (paper_id,),
        ):
            _sync_paper_tags(conn, paper_id, int(vr[0]), merged)
    return merged


def remove_paper_tags(paper_id: str, tags: list[str]) -> list[str]:
    """Remove tags from a paper. Returns the updated tag list."""
    remove = set(tags)
    with _connect() as conn:
        row = conn.execute(
            "SELECT tags FROM latest_papers WHERE paper_id = ?", (paper_id,)
        ).fetchone()
        if row is None:
            raise KeyError(paper_id)
        current: list[str] = row["tags"] or []
        updated = [t for t in current if t not in remove]
        conn.execute(
            "UPDATE PAPER_META SET tags = ? WHERE paper_id = ?",
            (updated, paper_id),
        )
        for vr in conn.execute(
            "SELECT DISTINCT version FROM PAPER_META WHERE paper_id = ?",
            (paper_id,),
        ):
            _sync_paper_tags(conn, paper_id, int(vr[0]), updated)
    return updated


def set_full_text(paper_id: str, version: int, full_text: str) -> None:
    """Store extracted TeX full text and update the FTS index."""
    with _connect() as conn:
        conn.execute(
            "UPDATE PAPER_META SET full_text = ?, downloaded_source = 1 "
            "WHERE paper_id = ? AND version = ?",
            (full_text, paper_id, version),
        )
        conn.execute("DELETE FROM papers_fts WHERE paper_id = ?", (paper_id,))
        conn.execute(
            "INSERT INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
            (paper_id, full_text),
        )


def search_full_text(query: str, limit: int = 20) -> list[sqlite3.Row]:
    """Full-text search over TeX source content. Returns matching papers."""
    with _connect() as conn:
        return conn.execute("""
            SELECT p.* FROM papers p
            JOIN papers_fts fts ON p.paper_id = fts.paper_id
            WHERE fts MATCH ?
            ORDER BY rank
            LIMIT ?
        """, (query, limit)).fetchall()
