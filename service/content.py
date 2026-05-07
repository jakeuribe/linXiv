from __future__ import annotations

import sqlite3
from dataclasses import dataclass
from typing import Optional

from service.models.content import ContentDetails
from storage.config.core import DB_PATH


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
    with sqlite3.connect(DB_PATH) as conn:
        conn.row_factory = sqlite3.Row
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
    with sqlite3.connect(DB_PATH) as conn:
        conn.row_factory = sqlite3.Row
        conn.execute(
            "INSERT OR REPLACE INTO CONTENT (CONTENT_FK, CONTENT_TEXT, CONTENT_FILE) VALUES (?, ?, ?)",
            (content_fk, data.content_text, data.content_file),
        )


def delete(content: Content) -> None:
    if content.content_fk is None:
        return
    with sqlite3.connect(DB_PATH) as conn:
        conn.row_factory = sqlite3.Row
        conn.execute(
            "DELETE FROM CONTENT WHERE CONTENT_FK = ?",
            (content.content_fk,),
        )