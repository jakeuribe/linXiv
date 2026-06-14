from __future__ import annotations
# from collections.abc import Callable
import datetime
import json
from pathlib import Path
import re
import sqlite3
from typing import Optional, TYPE_CHECKING

import arxiv

from storage.config.core import apply_sql_schema
from storage.config.queries import (
    _GRAPH_AUTHORS_SQL,
    _GRAPH_AUTHORS_WITH_COUNT_SQL,
    _TAG_FK_BY_LABEL_SQL,
    list_papers as _queries_list_papers,
)
from storage.paths import old_pdf_dir, pdf_dir

if TYPE_CHECKING:
    from sources.base import PaperMetadata

from storage.paths import db_path as _db_path_fn
DB_PATH = str(_db_path_fn())
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


def ensure_paper_root(source_id: str) -> int:
    """Insert PAPER_ROOTS row if absent. Returns SOURCE_FK."""
    with _connect() as conn:
        source_fk, _ = _ensure_paper_root_row(conn, source_id)
        return source_fk


def get_source_id(source_fk: int) -> str | None:
    """Return SOURCE_ID for a given SOURCE_FK, or None."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_FK = ?", (source_fk,)
        ).fetchone()
    return str(row["SOURCE_ID"]) if row else None


def _ensure_paper_root_row(conn: sqlite3.Connection, source_id: str) -> tuple[int, bool]:
    conn.execute("INSERT OR IGNORE INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)", (source_id,))
    row = conn.execute(
        "SELECT SOURCE_FK, STATUS FROM PAPER_ROOTS WHERE SOURCE_ID = ?", (source_id,)
    ).fetchone()
    assert row
    was_restored = False
    if str(row["STATUS"]) == "deleted":
        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'active', DELETED_AT = NULL, UPDATED_AT = datetime('now') WHERE SOURCE_ID = ?",
            (source_id,),
        )
        was_restored = True
    return int(row[0]), was_restored


def _author_fk_for_name(conn: sqlite3.Connection, full_name: str) -> int | None:
    row = conn.execute(
        "SELECT AUTHOR_FK FROM AUTHOR WHERE AUTHOR_FULL_NAME = ? COLLATE NOCASE LIMIT 1",
        (full_name,),
    ).fetchone()
    if row:
        return int(row[0])
    cur = conn.execute(
        "INSERT INTO AUTHOR (AUTHOR_FULL_NAME) VALUES (?)",
        (full_name,),
    )
    return cur.lastrowid


def _tag_fk_for_label(conn: sqlite3.Connection, label: str) -> int|None:
    row = conn.execute(_TAG_FK_BY_LABEL_SQL, (label,)).fetchone()
    if row:
        return int(row[0])
    cur = conn.execute("INSERT INTO TAG (TAG) VALUES (?)", (label,))
    if cur.lastrowid:
        return int(cur.lastrowid)
    else:
        return


def _sync_paper_authors(
    conn: sqlite3.Connection,
    paper_id: int,
    authors: list[str] | None,
) -> None:
    old_fks = {
        int(r["AUTHOR_FK"]) for r in conn.execute(
            "SELECT AUTHOR_FK FROM PAPER_TO_AUTHOR WHERE PAPER_ID = ?", (paper_id,)
        ).fetchall()
    }
    conn.execute("DELETE FROM PAPER_TO_AUTHOR WHERE PAPER_ID = ?", (paper_id,))
    if authors:
        for i, name in enumerate(authors):
            aid = _author_fk_for_name(conn, name)
            conn.execute(
                "INSERT INTO PAPER_TO_AUTHOR (PAPER_ID, AUTHOR_FK, AUTHOR_INDEX) VALUES (?, ?, ?)",
                (paper_id, aid, i),
            )
    # Clean up AUTHOR rows that are no longer referenced by any paper after this sync.
    # hard_delete_paper relies on schema CASCADE for structural cleanup and does not
    # call this — AUTHOR orphan accumulation on hard delete is an accepted trade-off
    # (see docs/adr/0009-orphan-row-policy.md).
    for fk in old_fks:
        if not conn.execute(
            "SELECT 1 FROM PAPER_TO_AUTHOR WHERE AUTHOR_FK = ? LIMIT 1", (fk,)
        ).fetchone():
            conn.execute("DELETE FROM AUTHOR WHERE AUTHOR_FK = ?", (fk,))


def _sync_paper_tags(
    conn: sqlite3.Connection,
    paper_id: int,
    source_id: str,
    version: int,
    tags: list[str] | None,
) -> None:
    conn.execute("DELETE FROM PAPER_TO_TAG WHERE PAPER_ID = ?", (paper_id,))
    if not tags:
        return
    for label in tags:
        if label:
            tid = _tag_fk_for_label(conn, label)
            row = conn.execute(
                "INSERT INTO PAPER_TO_TAG (PAPER_ID, SOURCE_ID, VERSION, TAG_FK) VALUES (?, ?, ?, ?)",
                (paper_id, source_id, version, tid),
            )
            if not row:
                print(f"[db] Label [{label}] failed to be added")


def _write_paper_version(
    conn: sqlite3.Connection,
    source_id: str,
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
    source_fk, _ = _ensure_paper_root_row(conn, source_id)
    cur = conn.execute(
        "INSERT OR IGNORE INTO PAPER (SOURCE_ID, VERSION, TITLE, CATEGORY, HAS_PDF, SOURCE_FK) VALUES (?, ?, ?, ?, ?, ?)",
        (source_id, version, title, category, has_pdf, source_fk),
    )
    if cur.rowcount == 0 or cur.lastrowid is None:
        return
    paper_id = int(cur.lastrowid)
    conn.execute(
        """
        INSERT INTO PAPER_META (
            PAPER_ID, URL, PUBLISHED, UPDATED, CATEGORIES, DOI, JOURNAL_REF,
            COMMENT, SUMMARY, PROVIDER, PDF_PATH, FULL_TEXT, DOWNLOADED_SOURCE, AUTHORS, TAGS
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (
            paper_id, url, published, updated, categories, doi, journal_ref,
            comment, summary, source, pdf_path, full_text, downloaded_source,
            authors, tags,
        ),
    )
    conn.execute("UPDATE PAPER SET UPDATED_AT = date('now') WHERE PAPER_ID = ?", (paper_id,))
    _sync_paper_authors(conn, paper_id, authors)
    _sync_paper_tags(conn, paper_id, source_id, version, tags)


def init_db() -> None:
    with _connect() as conn:
        apply_sql_schema(conn)

    if old_pdf_dir().is_dir():
        wrong_path_rows = _get_deprecated_path_rows()
        if wrong_path_rows:
            for rows in wrong_path_rows:
                try:
                    curr_path = rows["PDF_PATH"]
                    if Path(curr_path).is_file() and Path(curr_path).rename(curr_path.replace(str(old_pdf_dir()), str(pdf_dir()))).exists():
                        set_pdf_path(rows["source_id"], curr_path.replace(str(old_pdf_dir()), str(pdf_dir())))
                        print(f"File [ {curr_path} ] moved and verified!")
                    else:
                        print(f"File [ {curr_path} ] could not be moved")
                except Exception as e:
                    print(f"An error occured while trying to parse file {rows['PDF_PATH']}:\n{e}")
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
    assert match
    source_id = match.group(1)
    version = int(match.group(2)) if match.group(2) else 1
    return source_id, version


def _insert_arxiv(conn: sqlite3.Connection, paper: arxiv.Result, tags: list[str] | None = None) -> tuple[str, int]:
    source_id, version = parse_entry_id(paper.entry_id)
    _write_paper_version(
        conn,
        source_id,
        version,
        paper.title,
        paper.primary_category,
        has_pdf = False,
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
    return source_id, version


def _insert_metadata(conn: sqlite3.Connection, meta: PaperMetadata, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a source-agnostic PaperMetadata row."""
    merged_tags = meta.tags
    if tags:
        merged_tags = list(set((merged_tags or []) + tags))
    _write_paper_version(
        conn,
        meta.source_id,
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
        source=meta.source or "",
        pdf_path=None,
        full_text=None,
        downloaded_source=None,
    )
    return meta.source_id, meta.version


def save_paper(paper: arxiv.Result, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a single arxiv paper. Returns (source_id, version)."""
    with _connect() as conn:
        return _insert_arxiv(conn, paper, tags)


def save_papers(papers: list[arxiv.Result], tags: list[str] | None = None) -> list[tuple[str, int]]:
    """Batch insert arxiv papers in a single transaction. Returns list of (source_id, version)."""
    with _connect() as conn:
        return [_insert_arxiv(conn, paper, tags) for paper in papers]


def save_paper_metadata(meta: PaperMetadata, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a paper from any source via PaperMetadata. Returns (source_id, version)."""
    with _connect() as conn:
        return _insert_metadata(conn, meta, tags)


def save_papers_metadata(papers: list[PaperMetadata], tags: list[str] | None = None) -> list[tuple[str, int]]:
    """Batch insert papers from any source. Returns list of (source_id, version)."""
    with _connect() as conn:
        return [_insert_metadata(conn, meta, tags) for meta in papers]


def repair_paper(source_fk: int, meta: PaperMetadata) -> None:
    """
    In-place repair of a paper's metadata, migrating SOURCE_ID if full ID changes.

    Keyed by SOURCE_FK (stable integer) so the caller never needs to track the old
    string ID.  Version history, pdf_path, has_pdf, full_text, and source are preserved.
    """
    new_id = meta.source_id
    with _connect() as conn:
        root = conn.execute(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_FK = ?", (source_fk,)
        ).fetchone()
        if root is None:
            return
        old_id = str(root["SOURCE_ID"])
        if new_id != old_id:
            # PAPER must be updated before PAPER_TO_TAG: PAPER_TO_TAG has a FK on
            # (SOURCE_ID, VERSION) referencing PAPER, so the new SOURCE_ID must
            # exist in PAPER before PAPER_TO_TAG rows are renamed.
            # PAPER_ROOTS order relative to PAPER is unconstrained — the FK between
            # them is the integer SOURCE_FK, which is not being changed here.
            conn.execute(
                "UPDATE PAPER SET SOURCE_ID = ? WHERE SOURCE_FK = ?",
                (new_id, source_fk),
            )
            conn.execute(
                "UPDATE PAPER_ROOTS SET SOURCE_ID = ? WHERE SOURCE_FK = ?",
                (new_id, source_fk),
            )
            conn.execute(
                "UPDATE PAPER_TO_TAG SET SOURCE_ID = ? WHERE SOURCE_ID = ?",
                (new_id, old_id),
            )

        row = conn.execute(
            "SELECT PAPER_ID, VERSION FROM PAPER WHERE SOURCE_FK = ? ORDER BY VERSION DESC LIMIT 1",
            (source_fk,),
        ).fetchone()
        if row:
            pid = int(row["PAPER_ID"])
            ver = int(row["VERSION"])

            # FTS5 virtual tables do not support UPDATE; use DELETE + INSERT instead.
            # PROJECT_TO_PAPER uses SOURCE_FK (integer) so no rename needed there.
            # Note: papers_fts.paper_id stores source_id strings (e.g. "2204.12985"),
            # not integer PAPER_IDs — the column name is a historical misnomer.
            if new_id != old_id and conn.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='papers_fts'"
            ).fetchone():
                full_text_row = conn.execute(
                    "SELECT FULL_TEXT FROM PAPER_META WHERE PAPER_ID = ?", (pid,)
                ).fetchone()
                conn.execute("DELETE FROM papers_fts WHERE paper_id = ?", (old_id,))
                if full_text_row and full_text_row["FULL_TEXT"]:
                    conn.execute(
                        "INSERT INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
                        (new_id, full_text_row["FULL_TEXT"]),
                    )

            conn.execute(
                "UPDATE PAPER SET TITLE = ?, CATEGORY = ? WHERE PAPER_ID = ?",
                (meta.title, meta.category, pid),
            )
            conn.execute(
                """
                UPDATE PAPER_META SET
                    AUTHORS = ?, PUBLISHED = ?, DOI = ?, URL = ?,
                    SUMMARY = ?, TAGS = ?, UPDATED_AT = datetime('now')
                WHERE PAPER_ID = ?
                """,
                (meta.authors, meta.published, meta.doi, meta.url,
                 meta.summary, meta.tags, pid),
            )
            _sync_paper_authors(conn, pid, meta.authors)
            _sync_paper_tags(conn, pid, new_id, ver, meta.tags)


def set_has_pdf(source_id: str, version: int, has: bool) -> None:
    """Set the has_pdf flag for a specific paper version."""
    with _connect() as conn:
        conn.execute(
            "UPDATE PAPER SET HAS_PDF = ? WHERE SOURCE_ID = ? AND VERSION = ?",
            (has, source_id, version),
        )


def set_has_pdf_all_versions(source_id: str, has: bool) -> None:
    """Set the has_pdf flag for every stored version of a paper."""
    with _connect() as conn:
        conn.execute("UPDATE PAPER SET HAS_PDF = ? WHERE SOURCE_ID = ?", (has, source_id))


#TODO: FIX TO WORK EXPECTED WAY 
def set_pdf_path(source_id: str, path: str, version: int | None = None) -> None:
    """Set the pdf_path for a paper (all versions)."""
    with _connect() as conn:
        if version:
            conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? AND VERSION = ?)",
                (path, source_id, version),
            )
        else:
            lastrow = conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                (path, source_id),
            )
            if lastrow:
                print("Warning: you are updating the paths of all pdfs associated with this paper. This will cause the pdfs of other version to be deleted, those other version.")
                print("This may incorrectly update the database, consider using different methodology")


def mark_pdf_saved(source_id: str, path: str, version: int) -> None:
    """Atomically write PDF_PATH and set HAS_PDF=True for a specific paper version.

    Both columns are updated inside one explicit BEGIN/COMMIT block so a crash
    between the two writes cannot leave them in a disagreeing state.
    Raises RuntimeError if either PAPER or PAPER_META has no matching row.
    """
    with _connect() as conn:
        conn.execute("BEGIN IMMEDIATE")
        meta_cur = conn.execute(
            "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN "
            "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? AND VERSION = ?)",
            (path, source_id, version),
        )
        if meta_cur.rowcount == 0:
            raise RuntimeError(
                f"mark_pdf_saved: no PAPER or PAPER_META row for source_id={source_id!r} version={version}"
            )
        paper_cur = conn.execute(
            "UPDATE PAPER SET HAS_PDF = ? WHERE SOURCE_ID = ? AND VERSION = ?",
            (True, source_id, version),
        )
        if paper_cur.rowcount == 0:
            raise RuntimeError(
                f"mark_pdf_saved: no PAPER row for source_id={source_id!r} version={version}"
            )


def soft_delete_paper(source_id: str) -> str | None:
    """Soft-delete a paper: set STATUS='deleted', remove PDF from linxiv dir if present.

    Returns the pdf_path that was stored (for caller reference), or None.
    """
    stored_path: str | None = None
    with _connect() as conn:
        row = conn.execute(
            "SELECT PDF_PATH FROM PAPER_META WHERE PAPER_ID IN "
            "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? ORDER BY VERSION DESC LIMIT 1)",
            (source_id,),
        ).fetchone()
        if row:
            stored_path = row["PDF_PATH"]

        if conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='papers_fts'"
        ).fetchone():
            conn.execute("DELETE FROM papers_fts WHERE paper_id = ?", (source_id,))

        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'deleted', DELETED_AT = ?, UPDATED_AT = datetime('now') WHERE SOURCE_ID = ?",
            (datetime.datetime.now(), source_id),
        )

    if stored_path:
        p = Path(stored_path)
        try:
            linxiv_dir = pdf_dir()
            if p.is_file() and p.is_relative_to(linxiv_dir):
                p.unlink()
                with _connect() as conn:
                    conn.execute(
                        "UPDATE PAPER SET HAS_PDF = 0 WHERE SOURCE_ID = ?",
                        (source_id,),
                    )
        except Exception as e:
            print(f"[db] Could not remove PDF for {source_id}: {e}")

    return stored_path


def restore_paper(source_id: str) -> str | None:
    """Restore a soft-deleted paper. Returns the stored pdf_path (may no longer exist)."""
    stored_path: str | None = None
    full_text: str | None = None
    with _connect() as conn:
        row = conn.execute(
            "SELECT PDF_PATH, FULL_TEXT FROM PAPER_META WHERE PAPER_ID IN "
            "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? ORDER BY VERSION DESC LIMIT 1)",
            (source_id,),
        ).fetchone()
        if row:
            stored_path = row["PDF_PATH"]
            full_text = row["FULL_TEXT"]

        conn.execute(
            "UPDATE PAPER_ROOTS SET STATUS = 'active', DELETED_AT = NULL, UPDATED_AT = datetime('now') WHERE SOURCE_ID = ?",
            (source_id,),
        )

        if row and row["FULL_TEXT"]:
            if conn.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='papers_fts'"
            ).fetchone():
                conn.execute("DELETE FROM papers_fts WHERE paper_id = ?", (source_id,))
                conn.execute(
                    "INSERT INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
                    (source_id, full_text),
                )

    return stored_path


def hard_delete_paper(source_id: str) -> None:
    """Permanently delete a paper and all associated data."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT PDF_PATH FROM PAPER_META WHERE PAPER_ID IN "
            "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? ORDER BY VERSION DESC LIMIT 1)",
            (source_id,),
        ).fetchone()
        if conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='papers_fts'"
        ).fetchone():
            conn.execute("DELETE FROM papers_fts WHERE paper_id = ?", (source_id,))
        conn.execute("DELETE FROM PAPER_ROOTS WHERE SOURCE_ID = ?", (source_id,))

    if row and row["PDF_PATH"]:
        p = Path(row["PDF_PATH"])
        try:
            linxiv_dir = pdf_dir()
            if p.is_file() and p.is_relative_to(linxiv_dir):
                p.unlink()
        except Exception as e:
            print(f"[db] Could not remove PDF for {source_id} during hard delete: {e}")


def list_deleted_papers() -> list[sqlite3.Row]:
    """Return all soft-deleted papers from the deleted_papers view."""
    with _connect() as conn:
        return conn.execute(
            "SELECT * FROM deleted_papers ORDER BY deleted_at DESC"
        ).fetchall()


def is_paper_deleted(source_id: str) -> bool:
    """Return True if a PAPER_ROOTS row exists with STATUS='deleted'."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT 1 FROM PAPER_ROOTS WHERE SOURCE_ID = ? AND STATUS = 'deleted'",
            (source_id,),
        ).fetchone()
    return row is not None


def get_paper(source_id: str, version: Optional[int] = None) -> Optional[sqlite3.Row]:
    """Fetch a specific version, or the latest if version is None."""
    with _connect() as conn:
        if version:
            return conn.execute(
                "SELECT * FROM papers WHERE source_id = ? AND version = ?",
                (source_id, version),
            ).fetchone()
        return conn.execute(
            "SELECT * FROM latest_papers WHERE source_id = ?",
            (source_id,),
        ).fetchone()


def get_paper_by_id(paper_id: int) -> Optional[sqlite3.Row]:
    """Fetch a paper version by its PAPER primary key."""
    with _connect() as conn:
        return conn.execute(
            "SELECT * FROM papers WHERE paper_id = ?",
            (paper_id,),
        ).fetchone()


def get_paper_by_source_fk(source_fk: int) -> Optional[sqlite3.Row]:
    """Fetch the latest version for a PAPER_ROOTS row by SOURCE_FK."""
    with _connect() as conn:
        return conn.execute(
            "SELECT * FROM latest_papers WHERE source_id = ("
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_FK = ?)",
            (source_fk,),
        ).fetchone()


def get_papers_by_source_fks(source_fks: list[int]) -> list[sqlite3.Row]:
    """Batch-fetch latest paper versions for a list of SOURCE_FKs in one query.

    The empty-list guard is intentional: _queries_list_papers(source_fks=None)
    returns the full library, which is the wrong behaviour here.
    """
    if not source_fks:
        return []
    return _queries_list_papers(source_fks=source_fks)


#TODO: FIX TO CONTAIN TOTAL FUNCTIONALITY, get paper_root from paper_id needs to be a fetching param or name can change 
def get_paper_root(source_id: str) -> Optional[sqlite3.Row]:
    """Return the PAPER_ROOTS row for a given source_id."""
    with _connect() as conn:
        return conn.execute(
            "SELECT * FROM PAPER_ROOTS WHERE SOURCE_ID = ?",
            (source_id,),
        ).fetchone()


def get_paper_roots_bulk(source_ids: list[str]) -> dict[str, int]:
    """Resolve SOURCE_IDs to SOURCE_FKs in bulk: {source_id: source_fk}.

    Unknown ids are simply absent from the result. Queries are chunked to
    stay under SQLite's IN-list variable cap (~999).
    """
    result: dict[str, int] = {}
    if not source_ids:
        return result
    chunk_size = 500
    with _connect() as conn:
        for i in range(0, len(source_ids), chunk_size):
            chunk = source_ids[i : i + chunk_size]
            placeholders = ",".join("?" * len(chunk))
            rows = conn.execute(
                f"SELECT SOURCE_ID, SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID IN ({placeholders})",
                chunk,
            ).fetchall()
            for row in rows:
                result[str(row["SOURCE_ID"])] = int(row["SOURCE_FK"])
    return result


#TODO: FIX TO WORK EXPECTED WAY 
def get_all_versions(source_id: str) -> list[sqlite3.Row]:
    """Fetch all stored versions of a paper, ordered oldest to newest."""
    with _connect() as conn:
        return conn.execute(
            "SELECT * FROM papers WHERE source_id = ? ORDER BY version ASC",
            (source_id,),
        ).fetchall()


def get_graph_data(exclude_single_authors: bool = False) -> tuple[list[dict], list[dict]]:
    """Returns (nodes, edges) ready to pass to the graph view.

    With ``exclude_single_authors`` True, an author node is dropped unless some
    AUTHOR_FK matching its name (COLLATE NOCASE) has two or more active papers.
    """
    with _connect() as conn:
        paper_nodes = [
            {
                "id":        row["source_fk"],
                "source_id": row["source_id"],
                "label":     row["title"],
                "type":      "paper",
                "category":  row["category"],
                "tags":      row["tags"] if row["tags"] else [],
                "has_pdf":   bool(row["has_pdf"]),
                "published": row["published"].isoformat() if row["published"] else None,
                "url":       row["url"],
                "doi":       row["doi"],
                "summary":   row["summary"],
            }
            for row in conn.execute("""
                SELECT r.SOURCE_FK AS source_fk, r.SOURCE_ID AS source_id,
                       p.TITLE AS title, p.CATEGORY AS category,
                       m.TAGS AS tags, p.HAS_PDF AS has_pdf, m.PUBLISHED AS published,
                       m.URL AS url, m.DOI AS doi, m.SUMMARY AS summary
                FROM PAPER_ROOTS r
                JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK
                JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID
                WHERE p.VERSION = (SELECT MAX(VERSION) FROM PAPER WHERE SOURCE_FK = r.SOURCE_FK)
                  AND r.STATUS = 'active'
            """)
        ]
        # _GRAPH_AUTHORS_WITH_COUNT_SQL adds the per-name paper_count column
        # (see queries.py); the default constant omits it and scans unchanged.
        sql = (
            _GRAPH_AUTHORS_WITH_COUNT_SQL
            if exclude_single_authors
            else _GRAPH_AUTHORS_SQL
        )
        author_rows = conn.execute(sql).fetchall()

    seen_authors: set[str] = set()
    author_nodes: list[dict] = []
    edges: list[dict] = []
    for row in author_rows:
        name = row["author_name"]
        # Drop an author when its paper_count is below two. A NULL count (no
        # matching AUTHOR row) fails the `is not None` guard, so it is kept.
        if (
            exclude_single_authors
            and row["paper_count"] is not None
            and row["paper_count"] < 2
        ):
            continue
        author_id = f"author::{name}"
        if author_id not in seen_authors:
            author_nodes.append({"id": author_id, "label": name, "type": "author"})
            seen_authors.add(author_id)
        edges.append({"source": row["source_fk"], "target": author_id})

    return paper_nodes + author_nodes, edges


def list_papers(latest_only: bool = True, limit: int | None = None, offset: int = 0) -> list[sqlite3.Row]:
    """List all stored papers (latest version per paper by default)."""
    with _connect() as conn:
        table = "latest_papers" if latest_only else "papers"
        sql = f"SELECT * FROM {table} ORDER BY published DESC"
        params: list[int] = []
        if limit:
            sql += " LIMIT ? OFFSET ?"
            params = [limit, offset]
        elif offset:
            sql += " LIMIT -1 OFFSET ?"
            params = [offset]
        return conn.execute(sql, params).fetchall()


def get_categories() -> list[str]:
    """Return a sorted list of all distinct primary categories in the DB."""
    with _connect() as conn:
        rows = conn.execute(
            "SELECT DISTINCT category FROM latest_papers WHERE category IS NOT NULL ORDER BY category"
        ).fetchall()
    return [row["category"] for row in rows]


def get_tags() -> list[str]:
    """Return a sorted list of all distinct tag labels across papers and projects."""
    with _connect() as conn:
        rows = conn.execute("""
            SELECT DISTINCT LOWER(je.value) AS tag
            FROM latest_papers p, json_each(p.tags) je
            WHERE p.tags IS NOT NULL
            UNION
            SELECT DISTINCT LOWER(t.TAG) AS tag
            FROM TAG t
            JOIN PROJECT_TO_TAG ptt ON ptt.TAG_FK = t.TAG_FK
            JOIN PROJECT pr ON pr.PROJECT_FK = ptt.PROJECT_FK
            WHERE pr.STATUS = 'active'
            ORDER BY tag
        """).fetchall()
    return [row["tag"] for row in rows]


def get_papers_by_json_tag(label: str) -> list[sqlite3.Row]:
    """Return latest_papers rows whose JSON tags array contains the given label (case-insensitive)."""
    with _connect() as conn:
        return conn.execute("""
            SELECT DISTINCT lp.*
            FROM latest_papers lp, json_each(lp.tags) je
            WHERE lp.tags IS NOT NULL
              AND je.value = ? COLLATE NOCASE
            ORDER BY lp.published DESC, lp.paper_id DESC
        """, (label,)).fetchall()


#TODO: FIX TO CONTAIN TOTAL FUNCTIONALITY 
def add_paper_tags(source_id: str, tags: list[str]) -> list[str]:
    """Add tags to a paper, deduplicating. Returns the updated tag list."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT tags FROM latest_papers WHERE source_id = ?", (source_id,)
        ).fetchone()
        if row is None:
            raise KeyError(source_id)
        current: list[str] = row["tags"] or []
        merged = list(dict.fromkeys(current + tags))
        conn.execute(
            "UPDATE PAPER_META SET TAGS = ? WHERE PAPER_ID IN (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
            (merged, source_id),
        )
        for vr in conn.execute(
            "SELECT PAPER_ID, VERSION FROM PAPER WHERE SOURCE_ID = ?", (source_id,)
        ):
            _sync_paper_tags(conn, int(vr["PAPER_ID"]), source_id, int(vr["VERSION"]), merged)
    return merged


#TODO: FIX TO CONTAIN TOTAL FUNCTIONALITY 
def remove_paper_tags(source_id: str, tags: list[str]) -> list[str]:
    """Remove tags from a paper. Returns the updated tag list."""
    remove = set(tags)
    with _connect() as conn:
        row = conn.execute(
            "SELECT tags FROM latest_papers WHERE source_id = ?", (source_id,)
        ).fetchone()
        if row is None:
            raise KeyError(source_id)
        current: list[str] = row["tags"] or []
        updated = [t for t in current if t not in remove]
        conn.execute(
            "UPDATE PAPER_META SET TAGS = ? WHERE PAPER_ID IN (SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
            (updated, source_id),
        )
        for vr in conn.execute(
            "SELECT PAPER_ID, VERSION FROM PAPER WHERE SOURCE_ID = ?", (source_id,)
        ):
            _sync_paper_tags(conn, int(vr["PAPER_ID"]), source_id, int(vr["VERSION"]), updated)
    return updated

#TODO: SHOULD USE PAPER_ID FIRST. DOCSTRING
def set_full_text(full_text: str|None, paper_id: int|None, source_id: str|None, version: int|None) -> None:
    """Store extracted TeX full text and update the FTS index."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ? AND VERSION = ?",
            (source_id, version),
        ).fetchone()
        if row is None:
            return
        paper_id = int(row["PAPER_ID"])
        conn.execute(
            "UPDATE PAPER_META SET FULL_TEXT = ?, DOWNLOADED_SOURCE = 1 WHERE PAPER_ID = ?",
            (full_text, paper_id),
        )
        conn.execute("DELETE FROM papers_fts WHERE paper_id = ?", (source_id,))
        conn.execute(
            "INSERT INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
            (source_id, full_text),
        )


def search_full_text(query: str, limit: int = 20) -> list[sqlite3.Row]:
    """Full-text search over TeX source content. Returns matching papers."""
    with _connect() as conn:
        if not conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='papers_fts'"
        ).fetchone():
            return []
        return conn.execute("""
            SELECT p.* FROM latest_papers p
            JOIN papers_fts fts ON p.source_id = fts.paper_id
            WHERE papers_fts MATCH ?
            ORDER BY rank
            LIMIT ?
        """, (query, limit)).fetchall()
