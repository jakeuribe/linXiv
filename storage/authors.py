from __future__ import annotations

from typing import Optional

from .db import _connect
from .config.queries import (
    get_author_from_key as _get_author_row,
    list_authors as _list_authors_rows,
)
from service.models.author import BasicAuthorDetails


def _row_to_details(row) -> BasicAuthorDetails:
    return BasicAuthorDetails(
        author_id  = row["AUTHOR_FK"],
        orcid      = row["AUTHOR_ORCID"],
        full_name  = row["AUTHOR_FULL_NAME"],
        first_name = row["AUTHOR_FIRST"],
        last_name  = row["AUTHOR_LAST"],
    )


def get_author(author_id: int) -> Optional[BasicAuthorDetails]:
    row = _get_author_row(author_id)
    return _row_to_details(row) if row else None


def list_authors(
    paper_id: int | None = None,
    name:     str | None = None,
) -> list[BasicAuthorDetails]:
    if paper_id:
        return [_row_to_details(r) for r in _list_authors_rows(paper_id=paper_id)]
    with _connect() as conn:
        if name:
            rows = conn.execute(
                "SELECT * FROM AUTHOR WHERE AUTHOR_FULL_NAME = ? COLLATE NOCASE",
                (name,),
            ).fetchall()
        else:
            rows = conn.execute(
                "SELECT * FROM AUTHOR ORDER BY AUTHOR_FULL_NAME"
            ).fetchall()
    return [_row_to_details(row) for row in rows]


def list_authors_with_paper_count(min_papers: int = 0) -> list[dict]:
    """Return authors ordered by last name with their distinct active-paper count.

    Counts are sourced from the ``author_paper_counts`` view so the aggregation
    lives in SQL. Pass ``min_papers=2`` to drop single-paper authors; the default
    of ``0`` keeps every author (including those with no active papers).
    """
    with _connect() as conn:
        rows = conn.execute(
            """
            SELECT a.AUTHOR_FK, a.AUTHOR_FULL_NAME, a.AUTHOR_FIRST, a.AUTHOR_LAST, a.AUTHOR_ORCID,
                   apc.paper_count AS paper_count
            FROM AUTHOR a
            JOIN author_paper_counts apc ON apc.author_fk = a.AUTHOR_FK
            WHERE apc.paper_count >= ?
            ORDER BY (a.AUTHOR_LAST IS NULL), a.AUTHOR_LAST,
                     (a.AUTHOR_FIRST IS NULL), a.AUTHOR_FIRST
            """,
            (min_papers,),
        ).fetchall()
    return [
        {
            "author_id":   row["AUTHOR_FK"],
            "full_name":   row["AUTHOR_FULL_NAME"],
            "first_name":  row["AUTHOR_FIRST"],
            "last_name":   row["AUTHOR_LAST"],
            "orcid":       row["AUTHOR_ORCID"],
            "paper_count": row["paper_count"],
        }
        for row in rows
    ]


def get_author_paper_previews(author_id: int) -> list[dict]:
    """Return latest-version paper fields for active papers linked to an author.

    Looks up via PAPER_ROOTS to avoid version-mismatch when PAPER_TO_AUTHOR stores
    a specific version's PAPER_ID while a newer version exists.
    """
    with _connect() as conn:
        rows = conn.execute(
            """
            SELECT lp.paper_id, lp.source_id, lp.source_fk, lp.version, lp.title
            FROM latest_papers lp
            WHERE lp.source_fk IN (
                SELECT DISTINCT p.SOURCE_FK
                FROM PAPER p
                JOIN PAPER_TO_AUTHOR pta ON pta.PAPER_ID = p.PAPER_ID
                WHERE pta.AUTHOR_FK = ?
            )
            ORDER BY (lp.title IS NULL), lp.title
            """,
            (author_id,),
        ).fetchall()
    return [
        {
            "paper_id":  row["paper_id"],
            "source_id": row["source_id"],
            "source_fk": row["source_fk"],
            "version":   row["version"],
            "title":     row["title"],
        }
        for row in rows
    ]


def count_author_paper_links(author_id: int) -> int:
    """Return distinct paper roots linked to this author, regardless of paper status."""
    with _connect() as conn:
        row = conn.execute(
            """
            SELECT COUNT(DISTINCT p.SOURCE_FK)
            FROM PAPER_TO_AUTHOR pta
            JOIN PAPER p ON p.PAPER_ID = pta.PAPER_ID
            WHERE pta.AUTHOR_FK = ?
            """,
            (author_id,),
        ).fetchone()
    return row[0]


def get_author_papers(author_id: int) -> list[dict]:
    with _connect() as conn:
        rows = conn.execute(
            """
            SELECT PAPER_ID, AUTHOR_INDEX
            FROM PAPER_TO_AUTHOR
            WHERE AUTHOR_FK = ?
            ORDER BY PAPER_ID
            """,
            (author_id,),
        ).fetchall()
    return [{"paper_id": row["PAPER_ID"], "author_index": row["AUTHOR_INDEX"]} for row in rows]


def create_author(
    full_name:  str,
    first_name: str | None = None,
    last_name:  str | None = None,
    orcid:      str | None = None,
) -> int | None:
    with _connect() as conn:
        cur = conn.execute(
            """
            INSERT INTO AUTHOR (AUTHOR_FULL_NAME, AUTHOR_FIRST, AUTHOR_LAST, AUTHOR_ORCID)
            VALUES (?, ?, ?, ?)
            """,
            (full_name, first_name, last_name, orcid),
        )
        return cur.lastrowid


def update_author(
    author_id:  int,
    full_name:  str | None = None,
    first_name: str | None = None,
    last_name:  str | None = None,
    orcid:      str | None = None,
) -> None:
    fields: list[str] = []
    params: list      = []
    if full_name : fields.append("AUTHOR_FULL_NAME = ?"); params.append(full_name)
    if first_name: fields.append("AUTHOR_FIRST = ?");     params.append(first_name)
    if last_name : fields.append("AUTHOR_LAST = ?");      params.append(last_name)
    if orcid     : fields.append("AUTHOR_ORCID = ?");     params.append(orcid)
    if not fields:
        return
    params.append(author_id)
    with _connect() as conn:
        conn.execute(
            f"UPDATE AUTHOR SET {', '.join(fields)} WHERE AUTHOR_FK = ?",
            params,
        )


def delete_author(author_id: int) -> None:
    with _connect() as conn:
        conn.execute("DELETE FROM AUTHOR WHERE AUTHOR_FK = ?", (author_id,))


def link_author_to_paper(
    author_fk:    int,
    paper_id:     int,
    author_index: int | None = None,
) -> None:
    with _connect() as conn:
        conn.execute(
            """
            INSERT OR IGNORE INTO PAPER_TO_AUTHOR (PAPER_ID, AUTHOR_FK, AUTHOR_INDEX)
            VALUES (?, ?, ?)
            """,
            (paper_id, author_fk, author_index),
        )


def unlink_author_from_paper(author_fk: int, paper_id: int) -> None:
    with _connect() as conn:
        conn.execute(
            "DELETE FROM PAPER_TO_AUTHOR WHERE AUTHOR_FK = ? AND PAPER_ID = ?",
            (author_fk, paper_id),
        )