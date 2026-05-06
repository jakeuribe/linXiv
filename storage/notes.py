from __future__ import annotations

import datetime
from dataclasses import dataclass
from typing import Optional

from pathlib import Path

from .db import _connect, init_table
from service.models.note import NoteDetails

_MIGRATIONS_DIR = Path(__file__).parent / "migrations"


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
            ("paper_id",   str, "NOT NULL REFERENCES paper_roots(paper_id) ON DELETE CASCADE"),
            ("project_id", int, "REFERENCES projects(id) ON DELETE SET NULL"),
            ("title",      str),
            ("content",    str),
            ("created_at", datetime.datetime),
            ("updated_at", datetime.datetime),
        ],
    )
    with _connect() as conn:
        conn.executescript("""
            CREATE INDEX IF NOT EXISTS idx_notes_paper_id   ON notes(paper_id);
            CREATE INDEX IF NOT EXISTS idx_notes_project_id ON notes(project_id);
            CREATE INDEX IF NOT EXISTS idx_notes_created_at ON notes(created_at);
        """)


def _migrate_notes_db() -> None:
    """Add FK constraints and indexes to an existing notes table if absent."""
    with _connect() as conn:
        fk_rows = conn.execute("PRAGMA foreign_key_list(notes)").fetchall()
        has_project_fk = any(row["table"] == "projects"    and row["to"] == "id"       for row in fk_rows)
        has_paper_fk   = any(row["table"] == "paper_roots" and row["to"] == "paper_id" for row in fk_rows)
        if has_project_fk and has_paper_fk:
            conn.execute("CREATE INDEX IF NOT EXISTS idx_notes_paper_id   ON notes(paper_id)")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_notes_project_id ON notes(project_id)")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_notes_created_at ON notes(created_at)")
            return

        # Seed paper_roots for any notes whose paper_id isn't there yet
        # so the rebuild doesn't fail on orphaned rows.
        conn.execute("""
            INSERT OR IGNORE INTO paper_roots(paper_id)
            SELECT DISTINCT paper_id FROM notes WHERE paper_id IS NOT NULL
        """)
        conn.executescript(
            (_MIGRATIONS_DIR / "notes_add_project_fk.sql").read_text()
        )


def ensure_notes_db() -> None:
    if not _notes_table_exists():
        init_notes_db()
    _migrate_notes_db()


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
