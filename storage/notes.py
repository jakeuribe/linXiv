from __future__ import annotations

import datetime
from dataclasses import dataclass
from typing import Optional

from .db import _connect
from service.models.note import NoteDetails


# ── DB schema ─────────────────────────────────────────────────────────────────

def _notes_table_exists() -> bool:
    with _connect() as conn:
        row = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='LIBRARY_NOTE'"
        ).fetchone()
    return row is not None


def ensure_notes_db() -> None:
    if not _notes_table_exists():
        raise RuntimeError("LIBRARY_NOTE table not found — run apply_sql_schema first")


# ── Data model ────────────────────────────────────────────────────────────────

@dataclass
class Note:
    source_fk:   int
    project_id:  Optional[int]               = None
    paper_id_fk: Optional[int]               = None  # pins note to a specific paper version
    title:       str                          = ""
    content:     str                          = ""
    id:          Optional[int]               = None
    created_at:  Optional[datetime.datetime] = None
    updated_at:  Optional[datetime.datetime] = None

    @classmethod
    def from_row(cls, row) -> Note:
        return cls(
            id          = row["NOTE_SK"],
            source_fk   = row["SOURCE_FK"],
            paper_id_fk = row["PAPER_ID_FK"],
            project_id  = row["PROJECT_FK"],
            title       = row["TITLE"] or "",
            content     = row["NOTE"] or "",
            created_at  = row["CREATED_AT"],
            updated_at  = row["UPDATED_AT"],
        )

    def to_details(self) -> NoteDetails:
        return NoteDetails(
            note_id     = self.id,
            source_fk   = self.source_fk,
            paper_id_fk = self.paper_id_fk,
            project_id  = self.project_id,
            title       = self.title,
            content     = self.content,
            created_at  = self.created_at,
            updated_at  = self.updated_at,
        )

    def save(self) -> None:
        now = datetime.datetime.now()
        self.updated_at = now
        if self.id is None:
            self.created_at = now
            with _connect() as conn:
                cur = conn.execute(
                    """
                    INSERT INTO LIBRARY_NOTE
                        (SOURCE_FK, PAPER_ID_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT)
                    VALUES (?, ?, ?, ?, ?, ?, ?)
                    """,
                    (self.source_fk, self.paper_id_fk, self.project_id,
                     self.title, self.content, self.created_at, self.updated_at),
                )
                self.id = cur.lastrowid
        else:
            with _connect() as conn:
                conn.execute(
                    """
                    UPDATE LIBRARY_NOTE
                    SET SOURCE_FK = ?, PAPER_ID_FK = ?, PROJECT_FK = ?,
                        TITLE = ?, NOTE = ?, UPDATED_AT = ?
                    WHERE NOTE_SK = ?
                    """,
                    (self.source_fk, self.paper_id_fk, self.project_id,
                     self.title, self.content, self.updated_at, self.id),
                )

    def delete(self) -> None:
        if self.id is None:
            return
        with _connect() as conn:
            conn.execute("DELETE FROM LIBRARY_NOTE WHERE NOTE_SK = ?", (self.id,))
        self.id = None


# ── Queries ───────────────────────────────────────────────────────────────────

def get_note(note_id: int) -> Optional[NoteDetails]:
    with _connect() as conn:
        row = conn.execute("SELECT * FROM LIBRARY_NOTE WHERE NOTE_SK = ?", (note_id,)).fetchone()
    return Note.from_row(row).to_details() if row is not None else None


def get_notes(
    source_fk: int,
    project_id: Optional[int] = None,
    *,
    all_projects: bool = False,
) -> list[NoteDetails]:
    with _connect() as conn:
        if all_projects:
            rows = conn.execute(
                "SELECT * FROM LIBRARY_NOTE WHERE SOURCE_FK = ? ORDER BY CREATED_AT ASC",
                (source_fk,),
            ).fetchall()
        elif project_id is None:
            rows = conn.execute(
                "SELECT * FROM LIBRARY_NOTE WHERE SOURCE_FK = ? AND PROJECT_FK IS NULL ORDER BY CREATED_AT ASC",
                (source_fk,),
            ).fetchall()
        else:
            rows = conn.execute(
                "SELECT * FROM LIBRARY_NOTE WHERE SOURCE_FK = ? AND PROJECT_FK = ? ORDER BY CREATED_AT ASC",
                (source_fk, project_id),
            ).fetchall()
    return [Note.from_row(row).to_details() for row in rows]


def get_project_notes(project_id: int) -> list[NoteDetails]:
    with _connect() as conn:
        rows = conn.execute(
            "SELECT * FROM LIBRARY_NOTE WHERE PROJECT_FK = ? ORDER BY SOURCE_FK ASC, CREATED_AT ASC",
            (project_id,),
        ).fetchall()
    return [Note.from_row(row).to_details() for row in rows]


def count_project_notes(project_id: int) -> int:
    with _connect() as conn:
        row = conn.execute(
            "SELECT COUNT(*) FROM LIBRARY_NOTE WHERE PROJECT_FK = ?", (project_id,)
        ).fetchone()
    return row[0] if row else 0


def count_paper_notes(source_fk: int, project_id: Optional[int] = None) -> int:
    with _connect() as conn:
        if project_id is None:
            row = conn.execute(
                "SELECT COUNT(*) FROM LIBRARY_NOTE WHERE SOURCE_FK = ?", (source_fk,)
            ).fetchone()
        else:
            row = conn.execute(
                "SELECT COUNT(*) FROM LIBRARY_NOTE WHERE SOURCE_FK = ? AND PROJECT_FK = ?",
                (source_fk, project_id),
            ).fetchone()
    return row[0] if row else 0


def get_notes_by_paper_id(paper_id: int) -> list[NoteDetails]:
    with _connect() as conn:
        rows = conn.execute(
            "SELECT * FROM LIBRARY_NOTE WHERE PAPER_ID_FK = ? ORDER BY CREATED_AT ASC",
            (paper_id,),
        ).fetchall()
    return [Note.from_row(row).to_details() for row in rows]


def note_counts_by_paper_for_project(project_id: int) -> dict[int, int]:
    """
    Return note counts keyed by SOURCE_FK for each paper in the project.
    Papers with no notes appear with count 0. Missing project_id yields empty dict.
    """
    with _connect() as conn:
        rows = conn.execute(
            """
            SELECT pp.paper_id AS source_fk, COALESCE(n.cnt, 0) AS note_count
            FROM project_papers pp
            LEFT JOIN (
                SELECT SOURCE_FK, COUNT(*) AS cnt
                FROM LIBRARY_NOTE
                WHERE PROJECT_FK = ?
                GROUP BY SOURCE_FK
            ) AS n ON n.SOURCE_FK = pp.paper_id
            WHERE pp.project_id = ?
            ORDER BY pp.position
            """,
            (project_id, project_id),
        ).fetchall()
    return {int(row["source_fk"]): int(row["note_count"]) for row in rows}
