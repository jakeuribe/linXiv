from __future__ import annotations

import datetime
from dataclasses import dataclass
from typing import Optional

from .db import _connect, init_table
from .projects import ensure_projects_db


# ── DB schema ─────────────────────────────────────────────────────────────────

def _notes_table_exists() -> bool:
    with _connect() as conn:
        row = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='notes'"
        ).fetchone()
    return row is not None


def ensure_notes_db() -> None:
    if not _notes_table_exists():
        init_notes_db()
    _ensure_notes_indices()


def _ensure_notes_indices() -> None:
    with _connect() as conn:
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_notes_project_paper "
            "ON notes(project_id, paper_id)"
        )


def init_notes_db() -> None:
    init_table(
        "notes",
        [
            ("id",         int, "PRIMARY KEY AUTOINCREMENT"),
            ("paper_id",   str, "NOT NULL"),
            ("project_id", int),
            ("title",      str),
            ("content",    str),
            ("created_at", datetime.datetime),
            ("updated_at", datetime.datetime),
        ],
    )


# ── Data model ────────────────────────────────────────────────────────────────

@dataclass
class Note:
    paper_id:   str
    project_id: Optional[int]               = None
    title:      str                          = ""
    content:    str                          = ""
    id:         Optional[int]               = None
    created_at: Optional[datetime.datetime] = None
    updated_at: Optional[datetime.datetime] = None

    @classmethod
    def from_row(cls, row) -> Note:
        return cls(
            id         = row["id"],
            paper_id   = row["paper_id"],
            project_id = row["project_id"],
            title      = row["title"] or "",
            content    = row["content"] or "",
            created_at = row["created_at"],
            updated_at = row["updated_at"],
        )

    def save(self) -> None:
        now = datetime.datetime.now()
        self.updated_at = now
        if self.id is None:
            self.created_at = now
            with _connect() as conn:
                cur = conn.execute(
                    """
                    INSERT INTO notes (paper_id, project_id, title, content, created_at, updated_at)
                    VALUES (?, ?, ?, ?, ?, ?)
                    """,
                    (self.paper_id, self.project_id, self.title, self.content,
                     self.created_at, self.updated_at),
                )
                self.id = cur.lastrowid
        else:
            with _connect() as conn:
                conn.execute(
                    """
                    UPDATE notes
                    SET paper_id = ?, project_id = ?, title = ?, content = ?, updated_at = ?
                    WHERE id = ?
                    """,
                    (self.paper_id, self.project_id, self.title, self.content,
                     self.updated_at, self.id),
                )

    def delete(self) -> None:
        if self.id is None:
            return
        with _connect() as conn:
            conn.execute("DELETE FROM notes WHERE id = ?", (self.id,))
        self.id = None


# ── Queries ───────────────────────────────────────────────────────────────────

def get_note(note_id: int) -> Optional[Note]:
    with _connect() as conn:
        row = conn.execute("SELECT * FROM notes WHERE id = ?", (note_id,)).fetchone()
    return Note.from_row(row) if row is not None else None


def get_notes(
    paper_id: str,
    project_id: Optional[int] = None,
    *,
    all_projects: bool = False,
) -> list[Note]:
    with _connect() as conn:
        if all_projects:
            rows = conn.execute(
                "SELECT * FROM notes WHERE paper_id = ? ORDER BY created_at ASC",
                (paper_id,),
            ).fetchall()
        elif project_id is None:
            rows = conn.execute(
                "SELECT * FROM notes WHERE paper_id = ? AND project_id IS NULL ORDER BY created_at ASC",
                (paper_id,),
            ).fetchall()
        else:
            rows = conn.execute(
                "SELECT * FROM notes WHERE paper_id = ? AND project_id = ? ORDER BY created_at ASC",
                (paper_id, project_id),
            ).fetchall()
    return [Note.from_row(row) for row in rows]


def get_project_notes(project_id: int) -> list[Note]:
    with _connect() as conn:
        rows = conn.execute(
            "SELECT * FROM notes WHERE project_id = ? ORDER BY paper_id ASC, created_at ASC",
            (project_id,),
        ).fetchall()
    return [Note.from_row(row) for row in rows]


def count_project_notes(project_id: int) -> int:
    with _connect() as conn:
        row = conn.execute(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?", (project_id,)
        ).fetchone()
    return row[0] if row else 0


def count_paper_notes(paper_id: str, project_id: Optional[int] = None) -> int:
    with _connect() as conn:
        if project_id is None:
            row = conn.execute(
                "SELECT COUNT(*) FROM notes WHERE paper_id = ?", (paper_id,)
            ).fetchone()
        else:
            row = conn.execute(
                "SELECT COUNT(*) FROM notes WHERE paper_id = ? AND project_id = ?",
                (paper_id, project_id),
            ).fetchone()
    return row[0] if row else 0


def note_counts_by_paper_for_project(project_id: int) -> dict[str, int]:
    """
    Return note counts for each paper listed on the project (``projects.paper_ids``).

    Papers with no notes appear with count ``0``. Order of keys follows
    ``paper_ids`` order. Missing or unknown ``project_id`` yields an empty dict.
    """
    ensure_notes_db()
    ensure_projects_db()
    with _connect() as conn:
        rows = conn.execute(
            """
            SELECT m.paper_id AS paper_id, COALESCE(n.cnt, 0) AS note_count
            FROM (
                SELECT je.value AS paper_id, CAST(je.key AS INTEGER) AS ord
                FROM projects p, json_each(p.paper_ids) AS je
                WHERE p.id = ?
            ) AS m
            LEFT JOIN (
                SELECT paper_id, COUNT(*) AS cnt
                FROM notes
                WHERE project_id = ?
                GROUP BY paper_id
            ) AS n ON n.paper_id = m.paper_id
            ORDER BY m.ord
            """,
            (project_id, project_id),
        ).fetchall()
    return {str(row["paper_id"]): int(row["note_count"]) for row in rows}
