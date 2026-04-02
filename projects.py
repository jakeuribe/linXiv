from __future__ import annotations

import datetime
import enum
from dataclasses import dataclass, field
from typing import Optional

from db import _connect, init_table


# ── Query builder

class Q:
    """A composable SQL predicate. Combine with &, |, ~."""
    def __init__(self, sql: str, *params) -> None:
        self.sql    = sql
        self.params = params

    def __and__(self, other: Q) -> Q:
        return Q(f"({self.sql} AND {other.sql})", *self.params, *other.params)

    def __or__(self, other: Q) -> Q:
        return Q(f"({self.sql} OR {other.sql})", *self.params, *other.params)

    def __invert__(self) -> Q:
        return Q(f"(NOT {self.sql})", *self.params)


# ── Status enum ───────────────────────────────────────────────────────────────

class Status(str, enum.Enum):
    ACTIVE   = "active"
    ARCHIVED = "archived"
    DELETED  = "deleted"
    # ACTIVE   — visible and in use
    # ARCHIVED — hidden from default views, data preserved
    # DELETED  — same behaviour as archived for now; distinct so intent is clear


# ── DB schema ─────────────────────────────────────────────────────────────────

def _projects_tables_exist() -> bool:
    """Return True if the projects table is already present in the DB."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='projects'"
        ).fetchone()
    return row is not None


def ensure_projects_db() -> None:
    """Initialise the projects tables only if they don't exist yet, then migrate."""
    if not _projects_tables_exist():
        init_projects_db()
    _migrate_projects_db()


def _migrate_projects_db() -> None:
    """Add missing columns and convert data from older schemas."""
    with _connect() as conn:
        existing = {row[1] for row in conn.execute("PRAGMA table_info(projects)")}
        for col, typedef in [
            ("paper_ids",    "LIST"),
            ("project_tags", "LIST"),
        ]:
            if col not in existing:
                conn.execute(f"ALTER TABLE projects ADD COLUMN {col} {typedef}")

        # Migrate color: TEXT hex strings like "#5b8dee" → INTEGER
        # Only rows where color is stored as a text hex string need converting.
        rows = conn.execute(
            "SELECT id, color FROM projects WHERE typeof(color) = 'text'"
        ).fetchall()
        for row in rows:
            raw = row["color"]
            if raw and raw.startswith("#"):
                conn.execute(
                    "UPDATE projects SET color = ? WHERE id = ?",
                    (int(raw.lstrip("#"), 16), row["id"]),
                )


def init_projects_db() -> None:
    """Create the projects table if it doesn't exist."""
    init_table(
        "projects",
        [
            ("id",           int,              "PRIMARY KEY AUTOINCREMENT"),
            ("name",         str,              "NOT NULL"),
            ("description",  str),
            ("color",        int),             # RGB packed as integer, e.g. 0x5b8dee
            ("created_at",   datetime.datetime),
            ("updated_at",   datetime.datetime),
            ("archived_at",  datetime.datetime),
            ("project_tags", list),
            ("paper_ids",    list),            # ordered; source of truth for membership & order
            ("status",       str,              "NOT NULL DEFAULT 'active'"),
        ],
    )


# ── Colour helpers ────────────────────────────────────────────────────────────

def color_to_hex(color: int) -> str:
    """Convert a packed RGB integer to a CSS hex string, e.g. 0x5b8dee → '#5b8dee'."""
    return f"#{color:06x}"


def color_from_hex(hex_str: str) -> int:
    """Convert a CSS hex string to a packed RGB integer, e.g. '#5b8dee' → 0x5b8dee."""
    return int(hex_str.lstrip("#"), 16)


# ── Data model 

@dataclass
class Project:
    name:         str
    description:  str                         = ""
    color:        Optional[int]               = None   # packed RGB, e.g. 0x5b8dee
    project_tags: list[str]                   = field(default_factory=list)
    paper_ids:    list[str]                   = field(default_factory=list)  # ordered; persisted in DB
    status:       Status                      = Status.ACTIVE
    id:           Optional[int]               = None
    created_at:   Optional[datetime.datetime] = None
    updated_at:   Optional[datetime.datetime] = None
    archived_at:  Optional[datetime.datetime] = None

    # ── Construction 

    @classmethod
    def from_row(cls, row) -> Project:
        """Construct a Project from a sqlite3.Row returned by a projects query."""
        return cls(
            id           = row["id"],
            name         = row["name"],
            description  = row["description"] or "",
            color        = int(row["color"]) if row["color"] is not None else None,
            project_tags = row["project_tags"] or [],
            paper_ids    = row["paper_ids"] or [],
            status       = Status(row["status"]),
            created_at   = row["created_at"],
            updated_at   = row["updated_at"],
            archived_at  = row["archived_at"],
        )

    # ── Persistence 

    def save(self) -> None:
        """Insert (if new) or update (if existing) the project row."""
        now = datetime.datetime.now()
        self.updated_at = now
        if self.id is None:
            self.created_at = now
            with _connect() as conn:
                cur = conn.execute(
                    """
                    INSERT INTO projects
                        (name, description, color, created_at, updated_at, archived_at,
                         project_tags, paper_ids, status)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (self.name, self.description, self.color,
                     self.created_at, self.updated_at, self.archived_at,
                     self.project_tags, self.paper_ids, self.status),
                )
                self.id = cur.lastrowid
        else:
            with _connect() as conn:
                conn.execute(
                    """
                    UPDATE projects
                    SET name = ?, description = ?, color = ?, updated_at = ?,
                        archived_at = ?, project_tags = ?, paper_ids = ?, status = ?
                    WHERE id = ?
                    """,
                    (self.name, self.description, self.color,
                     self.updated_at, self.archived_at,
                     self.project_tags, self.paper_ids, self.status, self.id),
                )

    def delete(self) -> None:
        """
        Soft-delete: sets status to DELETED and records archived_at.
        Same effect as archive() for now; kept separate so intent is preserved.
        """
        self.status      = Status.DELETED
        self.archived_at = datetime.datetime.now()
        self.save()
        # Considering updating the table updates on change at the tick intervals that
        # require it rather than each time, this is fine for now
    def archive(self) -> None:
        """Sets status to ARCHIVED and records archived_at."""
        self.status      = Status.ARCHIVED
        self.archived_at = datetime.datetime.now()
        self.save()

    def restore(self) -> None:
        """Returns status to ACTIVE and clears archived_at."""
        self.status      = Status.ACTIVE
        self.archived_at = None
        self.save()

    # ── Paper membership ──────────────────────────────────────────────────────

    def add_paper(self, paper_id: str, position: Optional[int] = None) -> None:
        """Associate a paper with this project. No-op if already a member."""
        if self.id is None:
            raise ValueError("Project must be saved before papers can be added.")
        if paper_id in self.paper_ids:
            return
        if position is None:
            self.paper_ids.append(paper_id)
        else:
            self.paper_ids.insert(position, paper_id)
        self.save()

    def add_papers(self, paper_ids: list[str]) -> None:
        """Bulk-add papers, appended in order. Skips duplicates."""
        if self.id is None:
            raise ValueError("Project must be saved before papers can be added.")
        new_ids = [pid for pid in paper_ids if pid not in self.paper_ids]
        if not new_ids:
            return
        self.paper_ids.extend(new_ids)
        self.save()

    def remove_paper(self, paper_id: str) -> None:
        """Remove a paper from this project."""
        if self.id is None:
            return
        if paper_id not in self.paper_ids:
            return
        self.paper_ids.remove(paper_id)
        self.save()

    def reorder_paper(self, paper_id: str, new_position: int) -> None:
        """Move a paper to a new index within the ordered list."""
        if paper_id not in self.paper_ids:
            return
        self.paper_ids.remove(paper_id)
        self.paper_ids.insert(new_position, paper_id)
        self.save()

    def load_papers(self) -> list[str]:
        """Return paper_ids (already loaded from the DB row on construction)."""
        return self.paper_ids

    @property
    def paper_count(self) -> int:
        return len(self.paper_ids)

    # ── Representation ────────────────────────────────────────────────────────

    def __repr__(self) -> str:
        return f"<Project id={self.id!r} name={self.name!r} status={self.status!r} papers={self.paper_count}>"


# ── Queries ───────────────────────────────────────────────────────────────────

def get_project(project_id: int) -> Optional[Project]:
    """Fetch a single project by id. Returns None if not found."""
    with _connect() as conn:
        row = conn.execute(
            "SELECT * FROM projects WHERE id = ?", (project_id,)
        ).fetchone()
    return Project.from_row(row) if row is not None else None


def filter_projects(condition: Q | None = None) -> list[Project]:
    """
    Return projects matching an optional Q predicate.
    Pass condition=None to return all projects.

    Examples
    --------
    filter_projects()                                      # all projects
    filter_projects(Q("status = ?", Status.ACTIVE))        # active only
    filter_projects(~Q("status = ?", Status.DELETED))      # anything not deleted
    filter_projects(
        Q("status = ?", Status.ACTIVE)
        & (Q("color = ?", 0x5b8dee) | Q("color = ?", 0x9b59b6))
        & Q("name LIKE ?", "%diffusion%")
    )
    """
    if condition is None:
        sql, params = "SELECT * FROM projects", ()
    else:
        sql    = f"SELECT * FROM projects WHERE {condition.sql}"
        params = condition.params
    with _connect() as conn:
        rows = conn.execute(sql, params).fetchall()
    return [Project.from_row(row) for row in rows]
