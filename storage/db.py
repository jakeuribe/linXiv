from __future__ import annotations
import datetime
import json
from pathlib import Path
import re
import sqlite3
from collections.abc import Callable
from typing import Optional, TYPE_CHECKING
from .paths import old_pdf_dir, pdf_dir
import arxiv

if TYPE_CHECKING:
    from sources.base import PaperMetadata

DB_PATH = str(Path(__file__).parent.parent / "papers.db")
_MIGRATIONS_DIR = Path(__file__).parent / "migrations"

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
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def _migration_v1(conn: sqlite3.Connection) -> None:
    existing = {row[1] for row in conn.execute("PRAGMA table_info(papers)")}
    sql = (_MIGRATIONS_DIR / "papers_v1.sql").read_text()
    for statement in sql.splitlines():
        statement = statement.strip()
        if not statement or statement.startswith("--"):
            continue
        col_name = statement.split()[5]
        if col_name not in existing:
            conn.execute(statement)


_MIGRATIONS: list[tuple[int, Callable[[sqlite3.Connection], None]]] = [
    (1, _migration_v1),
]


def _papers_has_root_fk(conn: sqlite3.Connection) -> bool:
    rows = conn.execute("PRAGMA foreign_key_list(papers)").fetchall()
    return any(row["table"] == "paper_roots" and row["from"] == "paper_id" for row in rows)


def _rebuild_papers_with_root_fk(conn: sqlite3.Connection) -> None:
    conn.execute((_MIGRATIONS_DIR / "papers_add_root_fk.sql").read_text())
    old_cols = {row[1] for row in conn.execute("PRAGMA table_info(papers)")}
    col_list = ", ".join(
        row[1] for row in conn.execute("PRAGMA table_info(papers_new)")
        if row[1] in old_cols
    )
    conn.execute(f"INSERT INTO papers_new ({col_list}) SELECT {col_list} FROM papers")
    conn.executescript("""
        DROP VIEW IF EXISTS latest_papers;
        DROP TABLE papers;
        ALTER TABLE papers_new RENAME TO papers;

        CREATE VIEW latest_papers AS
        SELECT * FROM papers p
        WHERE version = (
            SELECT MAX(version) FROM papers WHERE paper_id = p.paper_id
        );
    """)


def init_db() -> None:
    with _connect() as conn:
        conn.executescript("""
            CREATE TABLE IF NOT EXISTS paper_roots (
                paper_id    TEXT    PRIMARY KEY
            );

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
                PRIMARY KEY (paper_id, version),
                FOREIGN KEY (paper_id) REFERENCES paper_roots(paper_id) ON DELETE CASCADE
            );

            CREATE VIEW IF NOT EXISTS latest_papers AS
            SELECT * FROM papers p
            WHERE version = (
                SELECT MAX(version) FROM papers WHERE paper_id = p.paper_id
            );

            CREATE TABLE IF NOT EXISTS schema_migrations (
                version     INTEGER PRIMARY KEY,
                applied_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
        """)
        conn.execute("""
            INSERT OR IGNORE INTO paper_roots(paper_id)
            SELECT DISTINCT paper_id FROM papers
        """)
        current_version: int = conn.execute("PRAGMA user_version").fetchone()[0]
        for version, fn in _MIGRATIONS:
            if version > current_version:
                fn(conn)
                conn.execute(
                    "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?)",
                    (version,),
                )
                conn.execute(f"PRAGMA user_version = {version}")
                current_version = version
        if not _papers_has_root_fk(conn):
            _rebuild_papers_with_root_fk(conn)

        # FTS5 virtual table for full-text search of TeX source
        conn.execute("""
            CREATE VIRTUAL TABLE IF NOT EXISTS papers_fts
            USING fts5(paper_id, full_text)
        """)
        # Migrate databases that had the broken external-content declaration
        row = conn.execute(
            "SELECT sql FROM sqlite_master WHERE name='papers_fts'"
        ).fetchone()
        if row and row[0] and "content=''" in row[0]:
            conn.execute("DROP TABLE IF EXISTS papers_fts")
            conn.execute("CREATE VIRTUAL TABLE papers_fts USING fts5(paper_id, full_text)")

    wrong_path_rows = _get_deprecated_path_rows()
    if wrong_path_rows:
        for rows in wrong_path_rows:
            try:
                curr_path = rows["PDF_PATH"]
                if Path(curr_path).is_file() and Path(curr_path).rename(curr_path.replace(str(old_pdf_dir()), str(pdf_dir()))).exists():
                    print(f"File [ {curr_path} ] moved and verified!")
                else:
                    print(f"File [ {curr_path} ] could not be moved")
            except Exception as e:
                print(f"An error occured while trying to parse file {rows['PDF_PATH']}:\n{e}")
    if old_pdf_dir().is_dir():
        _remove_gui_pdf_dir(old_pdf_dir())

def _remove_gui_pdf_dir(path: Path):
    for child in path.iterdir():
        child.unlink()
    path.rmdir()


def _get_deprecated_path_rows() -> list[sqlite3.Row] | None:
    with _connect() as conn:
        rows = conn.execute(
            f"SELECT * FROM papers WHERE PDF_PATH LIKE '%{str(old_pdf_dir())}%';"
        ).fetchall()
    return rows


def parse_entry_id(entry_id: str) -> tuple[str, int]:
    """EX:Split 'http://arxiv.org/abs/2204.12985v4' into ('2204.12985', 4)."""
    raw = entry_id.split('/')[-1]
    match = re.match(r'^(.+?)(?:v(\d+))?$', raw)
    assert match is not None
    paper_id = match.group(1)
    version = int(match.group(2)) if match.group(2) else 1
    return paper_id, version


def _insert_arxiv(conn: sqlite3.Connection, paper: arxiv.Result, tags: list[str] | None = None) -> tuple[str, int]:
    paper_id, version = parse_entry_id(paper.entry_id)
    conn.execute("INSERT OR IGNORE INTO paper_roots(paper_id) VALUES (?)", (paper_id,))
    conn.execute("""
        INSERT OR REPLACE INTO papers
            (paper_id, version, title, url, published, updated,
             category, categories, doi, journal_ref, comment, summary, authors, tags, source)
        VALUES
            (:paper_id, :version, :title, :url, :published, :updated,
             :category, :categories, :doi, :journal_ref, :comment, :summary, :authors, :tags, :source)
    """, {
        "paper_id":    paper_id,
        "version":     version,
        "title":       paper.title,
        "url":         paper.pdf_url,
        "published":   paper.published.date(),
        "updated":     paper.updated.date(),
        "category":    paper.primary_category,
        "categories":  paper.categories,
        "doi":         paper.doi,
        "journal_ref": paper.journal_ref,
        "comment":     paper.comment,
        "summary":     paper.summary,
        "authors":     [a.name for a in paper.authors],
        "tags":        tags,
        "source":      "arxiv",
    })
    return paper_id, version


def _insert_metadata(conn: sqlite3.Connection, meta: PaperMetadata, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a source-agnostic PaperMetadata into the papers table."""
    merged_tags = meta.tags
    if tags:
        merged_tags = list(set((merged_tags or []) + tags))
    conn.execute("INSERT OR IGNORE INTO paper_roots(paper_id) VALUES (?)", (meta.paper_id,))
    conn.execute("""
        INSERT OR REPLACE INTO papers
            (paper_id, version, title, url, published, updated,
             category, categories, doi, journal_ref, comment, summary, authors, tags, source)
        VALUES
            (:paper_id, :version, :title, :url, :published, :updated,
             :category, :categories, :doi, :journal_ref, :comment, :summary, :authors, :tags, :source)
    """, {
        "paper_id":    meta.paper_id,
        "version":     meta.version,
        "title":       meta.title,
        "url":         meta.url,
        "published":   meta.published,
        "updated":     meta.updated,
        "category":    meta.category,
        "categories":  meta.categories,
        "doi":         meta.doi,
        "journal_ref": meta.journal_ref,
        "comment":     meta.comment,
        "summary":     meta.summary,
        "authors":     meta.authors,
        "tags":        merged_tags,
        "source":      meta.source,
    })
    return meta.paper_id, meta.version


def save_paper(paper: arxiv.Result, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert or replace a single arxiv paper. Returns (paper_id, version)."""
    with _connect() as conn:
        return _insert_arxiv(conn, paper, tags)


def save_papers(papers: list[arxiv.Result], tags: list[str] | None = None) -> list[tuple[str, int]]:
    """Batch insert/replace arxiv papers in a single transaction. Returns list of (paper_id, version)."""
    with _connect() as conn:
        return [_insert_arxiv(conn, paper, tags) for paper in papers]


def save_paper_metadata(meta: PaperMetadata, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert or replace a paper from any source via PaperMetadata. Returns (paper_id, version)."""
    with _connect() as conn:
        return _insert_metadata(conn, meta, tags)


def save_papers_metadata(papers: list[PaperMetadata], tags: list[str] | None = None) -> list[tuple[str, int]]:
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

def set_pdf_path(paper_id: str, path: str) -> None:
    """Set the pdf_path for a paper (all versions)."""
    with _connect() as conn:
        conn.execute("UPDATE papers SET pdf_path = ? WHERE paper_id = ?", (path, paper_id))


def delete_paper(paper_id: str) -> None:
    """Delete all versions of a paper."""
    with _connect() as conn:
        conn.execute("DELETE FROM paper_roots WHERE paper_id = ?", (paper_id,))


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
        conn.execute("UPDATE papers SET tags = ? WHERE paper_id = ?", (merged, paper_id))
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
        conn.execute("UPDATE papers SET tags = ? WHERE paper_id = ?", (updated, paper_id))
    return updated


def set_full_text(paper_id: str, version: int, full_text: str) -> None:
    """Store extracted TeX full text and update the FTS index."""
    with _connect() as conn:
        conn.execute(
            "UPDATE papers SET full_text = ?, downloaded_source = 1 "
            "WHERE paper_id = ? AND version = ?",
            (full_text, paper_id, version),
        )
        # Update FTS index — delete old entry then insert new one
        conn.execute(
            "INSERT OR REPLACE INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
            (paper_id, full_text),
        )


def search_full_text(query: str, limit: int = 20) -> list[sqlite3.Row]:
    """Full-text search over TeX source content. Returns matching papers."""
    with _connect() as conn:
        return conn.execute("""
            SELECT p.* FROM papers p
            JOIN papers_fts fts ON p.paper_id = fts.paper_id
            WHERE papers_fts MATCH ?
            ORDER BY rank
            LIMIT ?
        """, (query, limit)).fetchall()
