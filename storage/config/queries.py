"""SQL queries for the service layer, matching the schema in storage/config/sql/tables/.

Conventions
-----------
- ``Q`` is a lightweight composable WHERE-clause builder (combine with ``&``, ``|``, ``~``).
- ``_fetch_one``, ``_fetch_all``, and ``_count`` are the catch-all runners; wrapper
  functions build a ``Q`` dynamically and call them — no hardcoded SQL strings for
  simple lookups.
- JOIN queries that span multiple tables are kept as named SQL constants below
  their section header.
- Paper queries use the ``papers`` / ``latest_papers`` views (PAPER JOIN PAPER_META).
- Surrogate-key column names follow the schema exactly (AUTHOR_FK, SOURCE_FK, ...).
"""
from __future__ import annotations

import sqlite3
from typing import Iterable, Optional

from .core import DB_PATH
from service.models.project import Status as _ProjectStatus


def _connect(db_path: str | None = None) -> sqlite3.Connection:
    conn = sqlite3.connect(db_path or DB_PATH, detect_types=sqlite3.PARSE_DECLTYPES)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


# ---------------------------------------------------------------------------
# Query builder
# ---------------------------------------------------------------------------

class Q:
    """Composable SQL WHERE predicate. Combine with &, |, ~."""
    def __init__(self, sql: str, *params) -> None:
        self.sql    = sql
        self.params = params

    def __and__(self, other: Q) -> Q:
        return Q(f"({self.sql} AND {other.sql})", *self.params, *other.params)

    def __or__(self, other: Q) -> Q:
        return Q(f"({self.sql} OR {other.sql})", *self.params, *other.params)

    def __invert__(self) -> Q:
        return Q(f"(NOT {self.sql})", *self.params)


def _in(col: str, vals: list) -> Q:
    """Return a Q for ``col IN (?, ?, …)``. Caller must ensure vals is non-empty."""
    return Q(f"{col} IN ({','.join('?' * len(vals))})", *vals)


# ---------------------------------------------------------------------------
# Catch-all runners
# ---------------------------------------------------------------------------

def _fetch_one(table: str, q: Q) -> Optional[sqlite3.Row]:
    with _connect() as conn:
        return conn.execute(
            f"SELECT * FROM {table} WHERE {q.sql}", q.params
        ).fetchone()


def _fetch_all(
    table: str,
    q: Q | None = None,
    order_by: str | None = None,
) -> list[sqlite3.Row]:
    sql = f"SELECT * FROM {table}"
    params: tuple = ()
    if q:
        sql += f" WHERE {q.sql}"
        params = q.params
    if order_by:
        sql += f" ORDER BY {order_by}"
    with _connect() as conn:
        return conn.execute(sql, params).fetchall()


def _count(table: str, q: Q) -> int:
    with _connect() as conn:
        return conn.execute(
            f"SELECT COUNT(*) AS n FROM {table} WHERE {q.sql}", q.params
        ).fetchone()["n"]


# ---------------------------------------------------------------------------
# AUTHOR
# ---------------------------------------------------------------------------

_LIST_AUTHORS_FROM_PAPER_SQL = """
SELECT a.AUTHOR_FK, a.AUTHOR_FULL_NAME, a.AUTHOR_FIRST, a.AUTHOR_LAST, a.AUTHOR_ORCID
FROM AUTHOR a
JOIN PAPER_TO_AUTHOR pta ON pta.AUTHOR_FK = a.AUTHOR_FK
WHERE pta.PAPER_ID = ?
ORDER BY pta.AUTHOR_INDEX
"""

_GET_AUTHOR_PAPERS_SQL = """
SELECT p.PAPER_ID, p.SOURCE_ID, p.VERSION, p.TITLE, p.CATEGORY, p.HAS_PDF, p.SOURCE_FK
FROM PAPER p
JOIN PAPER_TO_AUTHOR pta ON pta.PAPER_ID = p.PAPER_ID
WHERE pta.AUTHOR_FK = ?
ORDER BY pta.AUTHOR_INDEX
"""


def get_author_from_key(author_fk: int) -> Optional[sqlite3.Row]:
    return _fetch_one("AUTHOR", Q("AUTHOR_FK = ?", author_fk))


def list_authors(
    paper_id: int | None = None,
    name: str | None = None,
) -> list[sqlite3.Row]:
    if paper_id:
        with _connect() as conn:
            return conn.execute(_LIST_AUTHORS_FROM_PAPER_SQL, (paper_id,)).fetchall()
    q = Q("AUTHOR_FULL_NAME LIKE ?", f"%{name}%") if name else None
    return _fetch_all("AUTHOR", q, order_by="AUTHOR_LAST, AUTHOR_FIRST")


def get_papers_via_author_fk(author_fk: int) -> list[sqlite3.Row]:
    with _connect() as conn:
        return conn.execute(_GET_AUTHOR_PAPERS_SQL, (author_fk,)).fetchall()


# ---------------------------------------------------------------------------
# GRAPH
# ---------------------------------------------------------------------------

# Graph author rows: one per (latest-version active root, JSON author name).
# _WITH_COUNT adds the per-name active-paper count for the single-author filter.
_GRAPH_AUTHORS_SQL = """
    SELECT r.SOURCE_FK AS source_fk, je.value AS author_name
    FROM PAPER_ROOTS r
    JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK
    JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID,
         json_each(m.AUTHORS) je
    WHERE p.VERSION = (SELECT MAX(VERSION) FROM PAPER WHERE SOURCE_FK = r.SOURCE_FK)
      AND r.STATUS = 'active'
"""

# paper_count: per author name (grouped COLLATE NOCASE), the max single-AUTHOR_FK
# count from author_paper_counts; NULL when no AUTHOR row matches the JSON name.
_GRAPH_AUTHORS_WITH_COUNT_SQL = """
    WITH name_counts AS (
        SELECT a.AUTHOR_FULL_NAME AS author_name,
               MAX(apc.paper_count) AS paper_count
        FROM AUTHOR a
        JOIN author_paper_counts apc ON apc.author_fk = a.AUTHOR_FK
        GROUP BY a.AUTHOR_FULL_NAME COLLATE NOCASE
    )
    SELECT r.SOURCE_FK AS source_fk, je.value AS author_name,
           nc.paper_count AS paper_count
    FROM PAPER_ROOTS r
    JOIN PAPER p ON p.SOURCE_FK = r.SOURCE_FK
    JOIN PAPER_META m ON m.PAPER_ID = p.PAPER_ID,
         json_each(m.AUTHORS) je
    LEFT JOIN name_counts nc ON nc.author_name = je.value COLLATE NOCASE
    WHERE p.VERSION = (SELECT MAX(VERSION) FROM PAPER WHERE SOURCE_FK = r.SOURCE_FK)
      AND r.STATUS = 'active'
"""


# ---------------------------------------------------------------------------
# PAPER  (via `papers` / `latest_papers` views — PAPER JOIN PAPER_META)
# ---------------------------------------------------------------------------

def get_paper(paper_id: int) -> Optional[sqlite3.Row]:
    """Fetch a specific paper version by its PAPER_ID."""
    return _fetch_one("papers", Q("paper_id = ?", paper_id))


def get_paper_by_source(source_fk: int, version: int | None = None) -> Optional[sqlite3.Row]:
    """Fetch the latest version of a paper, or a specific version if given."""
    q = Q("source_fk = ?", source_fk)
    if version:
        return _fetch_one("papers", q & Q("version = ?", version))
    return _fetch_one("latest_papers", q)


def list_papers(source_fks: Iterable[int] | None = None) -> list[sqlite3.Row]:
    if not source_fks:
        return _fetch_all("latest_papers", order_by="paper_id")
    fks = list(source_fks)
    if not fks:
        return []
    return _fetch_all("latest_papers", _in("source_fk", fks), order_by="paper_id")


def list_paper_versions(source_fk: int) -> list[sqlite3.Row]:
    return _fetch_all("papers", Q("source_fk = ?", source_fk), order_by="version ASC")


def get_paper_meta(paper_id: int) -> Optional[sqlite3.Row]:
    return _fetch_one("PAPER_META", Q("PAPER_ID = ?", paper_id))



# ---------------------------------------------------------------------------
# TAG
# ---------------------------------------------------------------------------

_TAG_FK_BY_LABEL_SQL = "SELECT TAG_FK FROM TAG WHERE TAG = ? COLLATE NOCASE LIMIT 1"


def get_tag_by_label(label: str) -> Optional[sqlite3.Row]:
    """Return the TAG row (TAG_FK, TAG) for a label (case-insensitive), or None."""
    with _connect() as conn:
        return conn.execute(
            "SELECT TAG_FK, TAG FROM TAG WHERE TAG = ? COLLATE NOCASE ORDER BY TAG_FK ASC LIMIT 1",
            (label,),
        ).fetchone()

_TAGS_BY_PAPER_BASE_SQL = """
SELECT DISTINCT t.TAG_FK, t.TAG
FROM TAG t
JOIN PAPER_TO_TAG ptt ON ptt.TAG_FK = t.TAG_FK
"""

_TAGS_BY_PROJECT_BASE_SQL = """
SELECT DISTINCT t.TAG_FK, t.TAG
FROM TAG t
JOIN PROJECT_TO_TAG ptt ON ptt.TAG_FK = t.TAG_FK
"""

# Relational paper-by-tag query (via PAPER_TO_TAG join). Returns the latest
# version of every paper that has the tag on any version.
# NOTE: The public API uses get_papers_by_json_tag() (storage/db.py) instead,
# which queries the JSON tags column directly — consistent with what paper cards
# display. This relational path is kept for completeness and potential future use.
_LIST_PAPERS_BY_TAG_SQL = """
SELECT DISTINCT lp.*
FROM latest_papers lp
JOIN PAPER p ON p.SOURCE_FK = lp.source_fk
JOIN PAPER_TO_TAG ptt ON ptt.PAPER_ID = p.PAPER_ID
WHERE ptt.TAG_FK = ?
ORDER BY lp.paper_id
"""

# Only returns active projects; archived/deleted projects are intentionally excluded.
_LIST_PROJECTS_BY_TAG_SQL = """
SELECT DISTINCT pr.*
FROM PROJECT pr
JOIN PROJECT_TO_TAG ptt ON ptt.PROJECT_FK = pr.PROJECT_FK
WHERE ptt.TAG_FK = ?
  AND pr.STATUS = 'active'
ORDER BY pr.PROJECT_FK
"""


def tags_in_rows(rows: list[sqlite3.Row]) -> Q:
    """Return a Q predicate for TAG_FK IN (…). Compose with & / | as needed."""
    return _in("TAG_FK", [row["TAG_FK"] for row in rows if row["TAG_FK"]])


def get_tag(tag_fk: int) -> Optional[sqlite3.Row]:
    return _fetch_one("TAG", Q("TAG_FK = ?", tag_fk))


def list_tags_by_paper(q: Q | None = None) -> list[sqlite3.Row]:
    where = f"WHERE {q.sql} " if q else ""
    with _connect() as conn:
        return conn.execute(
            _TAGS_BY_PAPER_BASE_SQL + where + "ORDER BY t.TAG",
            q.params if q else (),
        ).fetchall()


def list_tags_by_project(q: Q | None = None) -> list[sqlite3.Row]:
    where = f"WHERE {q.sql} " if q else ""
    with _connect() as conn:
        return conn.execute(
            _TAGS_BY_PROJECT_BASE_SQL + where + "ORDER BY t.TAG",
            q.params if q else (),
        ).fetchall()


def list_papers_by_tag(tag_fk: int) -> list[sqlite3.Row]:
    with _connect() as conn:
        return conn.execute(_LIST_PAPERS_BY_TAG_SQL, (tag_fk,)).fetchall()


def list_projects_by_tag(tag_fk: int) -> list[sqlite3.Row]:
    """Return active projects tagged with tag_fk. Archived/deleted projects are excluded."""
    with _connect() as conn:
        return conn.execute(_LIST_PROJECTS_BY_TAG_SQL, (tag_fk,)).fetchall()

# ---------------------------------------------------------------------------
# PROJECT
# ---------------------------------------------------------------------------

_LIST_PROJECT_PAPERS_SQL = """
SELECT lp.*
FROM latest_papers lp
JOIN PROJECT_TO_PAPER ptp ON ptp.SOURCE_FK = lp.source_fk
WHERE ptp.PROJECT_FK = ?
ORDER BY lp.paper_id
"""

_LIST_PROJECTS_FOR_PAPER_SQL = """
SELECT pr.PROJECT_FK, pr.NAME, pr.DESCRIPTION, pr.COLOR, pr.STATUS,
       pr.PROJECT_TAGS, pr.CREATED_AT, pr.UPDATED_AT, pr.ARCHIVED_AT
FROM PROJECT pr
JOIN PROJECT_TO_PAPER ptp ON ptp.PROJECT_FK = pr.PROJECT_FK
WHERE ptp.SOURCE_FK = ?
ORDER BY pr.PROJECT_FK
"""


def get_project(project_fk: int) -> Optional[sqlite3.Row]:
    return _fetch_one("PROJECT", Q("PROJECT_FK = ?", project_fk))


def list_projects() -> list[sqlite3.Row]:
    return _fetch_all("PROJECT", order_by="PROJECT_FK")


def list_project_papers(project_fk: int) -> list[sqlite3.Row]:
    with _connect() as conn:
        return conn.execute(_LIST_PROJECT_PAPERS_SQL, (project_fk,)).fetchall()


_COUNT_ACTIVE_PROJECT_PAPERS_SQL = """
SELECT COUNT(*) FROM PROJECT_TO_PAPER p2p
JOIN PAPER_ROOTS r ON r.SOURCE_FK = p2p.SOURCE_FK
WHERE p2p.PROJECT_FK = ? AND r.STATUS = 'active'
"""


def count_project_papers(project_fk: int) -> int:
    with _connect() as conn:
        row = conn.execute(_COUNT_ACTIVE_PROJECT_PAPERS_SQL, (project_fk,)).fetchone()
    return int(row[0]) if row else 0


def list_projects_for_paper(source_fk: int) -> list[sqlite3.Row]:
    with _connect() as conn:
        return conn.execute(_LIST_PROJECTS_FOR_PAPER_SQL, (source_fk,)).fetchall()


_PROJECT_TAGS_BULK_BASE_SQL = """
SELECT DISTINCT ptt.PROJECT_FK, t.TAG
FROM TAG t
JOIN PROJECT_TO_TAG ptt ON ptt.TAG_FK = t.TAG_FK
"""

_PROJECT_SOURCE_IDS_BULK_BASE_SQL = """
SELECT p2p.PROJECT_FK, pr.SOURCE_ID
FROM PROJECT_TO_PAPER p2p
JOIN PAPER_ROOTS pr ON pr.SOURCE_FK = p2p.SOURCE_FK
"""

def list_project_tags_bulk(
    project_fks: list[int] | None = None,
) -> dict[int, list[str]]:
    """Return {project_fk: [tag, ...]} for all (or the given) projects in one query.

    Pass project_fks=None to fetch for all projects. Pass an empty list to get {}.
    SQLite's SQLITE_LIMIT_VARIABLE_NUMBER caps IN-list size at ~999; callers are
    expected to stay well below that (project counts in the hundreds at most).
    """
    if project_fks is not None and not project_fks:
        return {}
    q = _in("ptt.PROJECT_FK", project_fks) if project_fks is not None else None
    where = f"WHERE {q.sql} " if q else ""
    params: tuple = q.params if q else ()
    with _connect() as conn:
        rows = conn.execute(
            _PROJECT_TAGS_BULK_BASE_SQL + where + "ORDER BY ptt.PROJECT_FK, t.TAG",
            params,
        ).fetchall()
    result: dict[int, list[str]] = {}
    for row in rows:
        result.setdefault(int(row["PROJECT_FK"]), []).append(str(row["TAG"]))
    return result


def list_project_source_ids_bulk(
    project_fks: list[int] | None = None,
) -> dict[int, list[str]]:
    """Return {project_fk: [source_id, ...]} for all (or the given) projects in one query.

    Only active papers (PAPER_ROOTS.STATUS = 'active') are included, consistent
    with the per-project filter in storage.projects._load_source_fks.
    Ordering within each project follows PROJECT_TO_PAPER_FK (approximates insertion
    order; no AUTOINCREMENT so strict monotonicity is not engine-guaranteed).

    Pass project_fks=None to fetch for all projects. Pass an empty list to get {}.
    SQLite's SQLITE_LIMIT_VARIABLE_NUMBER caps IN-list size at ~999; callers are
    expected to stay well below that (project counts in the hundreds at most).
    """
    if project_fks is not None and not project_fks:
        return {}
    q: Q = Q("pr.STATUS = ?", _ProjectStatus.ACTIVE.value)
    if project_fks is not None:
        q = q & _in("p2p.PROJECT_FK", project_fks)
    with _connect() as conn:
        rows = conn.execute(
            _PROJECT_SOURCE_IDS_BULK_BASE_SQL
            + f"WHERE {q.sql} ORDER BY p2p.PROJECT_FK, p2p.PROJECT_TO_PAPER_FK",
            q.params,
        ).fetchall()
    result: dict[int, list[str]] = {}
    for row in rows:
        result.setdefault(int(row["PROJECT_FK"]), []).append(str(row["SOURCE_ID"]))
    return result


# ---------------------------------------------------------------------------
# NOTE
# ---------------------------------------------------------------------------

def get_note(note_sk: int) -> Optional[sqlite3.Row]:
    return _fetch_one("NOTE", Q("NOTE_SK = ?", note_sk))


def list_notes(
    source_fk: int | None = None,
    paper_id_fk: int | None = None,
    project_fk: int | None = None,
    project_unscoped: bool = False,
) -> list[sqlite3.Row]:
    clauses: list[Q] = []
    if source_fk:
        clauses.append(Q("SOURCE_FK = ?", source_fk))
    if paper_id_fk:
        clauses.append(Q("PAPER_ID_FK = ?", paper_id_fk))
    if project_fk:
        clauses.append(Q("PROJECT_FK = ?", project_fk))
    elif project_unscoped:
        clauses.append(Q("PROJECT_FK IS NULL"))
    if not clauses:
        return []
    q = clauses[0]
    for c in clauses[1:]:
        q = q & c
    return _fetch_all("NOTE", q, order_by="CREATED_AT ASC")


def count_notes(source_fk: int, project_fk: int | None = None) -> int:
    q = Q("SOURCE_FK = ?", source_fk)
    if project_fk is not None:
        q = q & Q("PROJECT_FK = ?", project_fk)
    return _count("NOTE", q)


def count_project_notes(project_fk: int) -> int:
    return _count("NOTE", Q("PROJECT_FK = ?", project_fk))
