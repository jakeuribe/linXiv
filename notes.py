from __future__ import annotations

import datetime
from dataclasses import dataclass
from typing import Optional

from db import _connect, init_table


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


def init_notes_db() -> None:
    init_table(
        "notes",
        [
            ("id",         int, "PRIMARY KEY AUTOINCREMENT"),
            ("paper_id",   str, "NOT NULL"),
            ("project_id", int),              # nullable — note may belong to a project or stand alone
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
        """Insert (if new) or update (if existing) the note row."""
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
        """Permanently delete this note."""
        if self.id is None:
            return
        with _connect() as conn:
            conn.execute("DELETE FROM notes WHERE id = ?", (self.id,))
        self.id = None


# ── Queries ───────────────────────────────────────────────────────────────────

def get_note(note_id: int) -> Optional[Note]:
    """Fetch a single note by id."""
    with _connect() as conn:
        row = conn.execute("SELECT * FROM notes WHERE id = ?", (note_id,)).fetchone()
    return Note.from_row(row) if row is not None else None


def get_notes(
    paper_id: str,
    project_id: Optional[int] = None,
    *,
    all_projects: bool = False,
) -> list[Note]:
    """
    Fetch notes for a paper.

    - project_id=None, all_projects=False  → only standalone notes (project_id IS NULL)
    - project_id=<id>, all_projects=False  → notes for that specific project
    - all_projects=True                    → all notes for the paper regardless of project
    """
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
    """Fetch all notes belonging to a project, ordered by paper then creation time."""
    with _connect() as conn:
        rows = conn.execute(
            "SELECT * FROM notes WHERE project_id = ? ORDER BY paper_id ASC, created_at ASC",
            (project_id,),
        ).fetchall()
    return [Note.from_row(row) for row in rows]


# ── Counts (useful for sanity-checking project / paper note membership) ────────

def count_project_notes(project_id: int) -> int:
    """Total number of notes belonging to a project."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?", (project_id,)
        ).fetchone()
    return row[0] if row else 0


def count_paper_notes(paper_id: str, project_id: Optional[int] = None) -> int:
    """
    Notes for a paper, optionally scoped to a project.

    - project_id=None  → all notes for the paper across every project (and standalone)
    - project_id=<id>  → only notes tied to that specific project
    """
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
