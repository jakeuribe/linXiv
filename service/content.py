from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

from service.models.content import ContentDetails
from storage.db import _connect


# ---------------------------------------------------------------------------
# GUI-facing models
# ---------------------------------------------------------------------------

@dataclass
class Content:
    content_fk: int | None = None


@dataclass
class ContentIn:
    content_text: str | None   = None
    content_file: bytes | None = None


# ---------------------------------------------------------------------------
# Master functions
# ---------------------------------------------------------------------------

def get(content: Content) -> Optional[ContentDetails]:
    """Fetch a single CONTENT row by content_fk."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT CONTENT_FK, CONTENT_TEXT, CONTENT_FILE FROM CONTENT WHERE CONTENT_FK = ?",
            (content.content_fk,),
        ).fetchone()
    if row is None:
        return None
    return ContentDetails(
        content_fk=row["CONTENT_FK"],
        content_text=row["CONTENT_TEXT"],
        content_file=row["CONTENT_FILE"],
    )


def upsert(content_fk: int, data: ContentIn) -> None:
    """Insert or replace a CONTENT row."""
    with _connect() as conn:
        conn.execute(
            "INSERT OR REPLACE INTO CONTENT (CONTENT_FK, CONTENT_TEXT, CONTENT_FILE) VALUES (?, ?, ?)",
            (content_fk, data.content_text, data.content_file),
        )


def delete(content: Content) -> None:
    if content.content_fk is None:
        return
    with _connect() as conn:
        conn.execute(
            "DELETE FROM CONTENT WHERE CONTENT_FK = ?",
            (content.content_fk,),
        )