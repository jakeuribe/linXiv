from __future__ import annotations

from typing import Optional

from .db import _connect
from service.models.tag import TagDetails


def get_tag(tag_id: int) -> Optional[TagDetails]:
    with _connect() as conn:
        row = conn.execute(
            "SELECT TAG_FK, TAG FROM TAG WHERE TAG_FK = ?", (tag_id,)
        ).fetchone()
    if row is None:
        return None
    return TagDetails(tag_id=row["TAG_FK"], label=row["TAG"])


def list_tags(
    paper_id:   int | None = None,
    project_id: int | None = None,
    label:      str | None = None,
) -> list[TagDetails]:
    with _connect() as conn:
        if paper_id is not None:
            rows = conn.execute(
                """
                SELECT DISTINCT t.TAG_FK, t.TAG
                FROM TAG t
                JOIN PAPER_TO_TAG ptt ON ptt.TAG_FK = t.TAG_FK
                WHERE ptt.PAPER_ID = ?
                ORDER BY t.TAG
                """,
                (paper_id,),
            ).fetchall()
        elif project_id is not None:
            rows = conn.execute(
                """
                SELECT DISTINCT t.TAG_FK, t.TAG
                FROM TAG t
                JOIN PROJECT_TO_TAG pt ON pt.TAG_FK = t.TAG_FK
                WHERE pt.PROJECT_FK = ?
                ORDER BY t.TAG
                """,
                (project_id,),
            ).fetchall()
        elif label is not None:
            rows = conn.execute(
                "SELECT TAG_FK, TAG FROM TAG WHERE TAG = ? COLLATE NOCASE ORDER BY TAG",
                (label,),
            ).fetchall()
        else:
            rows = conn.execute(
                "SELECT TAG_FK, TAG FROM TAG ORDER BY TAG"
            ).fetchall()
    return [TagDetails(tag_id=row["TAG_FK"], label=row["TAG"]) for row in rows]


def create_tag(label: str) -> int | None:
    with _connect() as conn:
        existing = conn.execute(
            "SELECT TAG_FK FROM TAG WHERE TAG = ? COLLATE NOCASE LIMIT 1", (label,)
        ).fetchone()
        if existing is not None:
            return int(existing["TAG_FK"])
        cur = conn.execute("INSERT INTO TAG (TAG) VALUES (?)", (label,))
        return cur.lastrowid


def delete_tag(tag_id: int) -> None:
    with _connect() as conn:
        conn.execute("DELETE FROM TAG WHERE TAG_FK = ?", (tag_id,))