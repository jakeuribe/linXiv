from __future__ import annotations

from typing import Optional

from .db import _connect
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
    with _connect() as conn:
        row = conn.execute(
            "SELECT * FROM AUTHOR WHERE AUTHOR_FK = ?", (author_id,)
        ).fetchone()
    return _row_to_details(row) if row is not None else None


def list_authors(
    paper_id: int | None = None,
    name:     str | None = None,
) -> list[BasicAuthorDetails]:
    with _connect() as conn:
        if paper_id is not None:
            rows = conn.execute(
                """
                SELECT a.*
                FROM AUTHOR a
                JOIN PAPER_TO_AUTHOR pta ON pta.AUTHOR_FK = a.AUTHOR_FK
                WHERE pta.PAPER_ID = ?
                ORDER BY pta.AUTHOR_INDEX
                """,
                (paper_id,),
            ).fetchall()
        elif name is not None:
            rows = conn.execute(
                "SELECT * FROM AUTHOR WHERE AUTHOR_FULL_NAME = ? COLLATE NOCASE",
                (name,),
            ).fetchall()
        else:
            rows = conn.execute(
                "SELECT * FROM AUTHOR ORDER BY AUTHOR_FULL_NAME"
            ).fetchall()
    return [_row_to_details(row) for row in rows]


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
    if full_name  is not None: fields.append("AUTHOR_FULL_NAME = ?"); params.append(full_name)
    if first_name is not None: fields.append("AUTHOR_FIRST = ?");     params.append(first_name)
    if last_name  is not None: fields.append("AUTHOR_LAST = ?");      params.append(last_name)
    if orcid      is not None: fields.append("AUTHOR_ORCID = ?");     params.append(orcid)
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