from __future__ import annotations

import datetime
import enum
from dataclasses import dataclass, field
from typing import Optional

from storage.db import _connect, init_table


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


def _project_papers_table_exists() -> bool:
    with _connect() as conn:
        row = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='project_papers'"
        ).fetchone()
    return row is not None


def ensure_projects_db() -> None:
    """Initialise the projects tables only if they don't exist yet, then migrate."""
    if not _projects_tables_exist():
        init_projects_db()
    _migrate_projects_db()


def _migrate_projects_db() -> None:
    """Add missing columns, convert data from older schemas, and drop obsolete columns."""
    _ensure_project_membership_table()
    with _connect() as conn:
        existing = {row[1] for row in conn.execute("PRAGMA table_info(projects)")}

        # Add project_tags if missing (old schema without it)
        if "project_tags" not in existing:
            conn.execute("ALTER TABLE projects ADD COLUMN project_tags LIST")

        # Migrate color: TEXT hex strings like "#5b8dee" → INTEGER
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

        # If paper_ids JSON column still exists: migrate its data into project_papers,
        # then rebuild the projects table without that column.
        if "paper_ids" in existing:
            old_rows = conn.execute("SELECT id, paper_ids FROM projects").fetchall()
            for proj_row in old_rows:
                proj_id = proj_row["id"]
                paper_ids = proj_row["paper_ids"] or []
                for pos, pid in enumerate(paper_ids):
                    conn.execute(
                        "INSERT OR IGNORE INTO project_papers (project_id, paper_id, position) VALUES (?, ?, ?)",
                        (proj_id, pid, pos),
                    )
            conn.execute("""
                CREATE TABLE projects_new (
                    id           INTEGER   PRIMARY KEY AUTOINCREMENT,
                    name         TEXT      NOT NULL,
                    description  TEXT,
                    color        INTEGER,
                    created_at   TIMESTAMP,
                    updated_at   TIMESTAMP,
                    archived_at  TIMESTAMP,
                    project_tags LIST,
                    status       TEXT      NOT NULL DEFAULT 'active'
                )
            """)
            conn.execute("""
                INSERT INTO projects_new
                    (id, name, description, color, created_at, updated_at,
                     archived_at, project_tags, status)
                SELECT id, name, description, color, created_at, updated_at,
                       archived_at, project_tags, status
                FROM projects
            """)
            conn.execute("DROP TABLE projects")
            conn.execute("ALTER TABLE projects_new RENAME TO projects")
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status)"
            )


def init_projects_db() -> None:
    """Create the projects and project_papers tables if they don't exist."""
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
            ("status",       str,              "NOT NULL DEFAULT 'active'"),
        ],
    )
    _ensure_project_membership_table()


def _ensure_project_membership_table() -> None:
    with _connect() as conn:
        conn.execute("""
            CREATE TABLE IF NOT EXISTS project_papers (
                project_id INTEGER NOT NULL,
                paper_id   TEXT    NOT NULL,
                position   INTEGER NOT NULL,
                PRIMARY KEY (project_id, paper_id),
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
                FOREIGN KEY (paper_id) REFERENCES paper_roots(paper_id) ON DELETE CASCADE
            )
        """)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_project_papers_project_pos ON project_papers(project_id, position)"
        )
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status)"
        )
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_project_papers_paper_id ON project_papers(paper_id)"
        )


def _backfill_project_memberships() -> None:
    if not _project_papers_table_exists():
        return
    with _connect() as conn:
        has_data = conn.execute("SELECT 1 FROM project_papers LIMIT 1").fetchone() is not None
        if has_data:
            return
        rows = conn.execute("SELECT id, paper_ids FROM projects").fetchall()
        for row in rows:
            project_id = row["id"]
            paper_ids = row["paper_ids"] or []
            _sync_project_papers(conn, project_id, paper_ids)


def _load_paper_ids(project_id: int) -> list[str]:
    with _connect() as conn:
        rows = conn.execute(
            "SELECT paper_id FROM project_papers WHERE project_id = ? ORDER BY position ASC",
            (project_id,),
        ).fetchall()
    return [row["paper_id"] for row in rows]


def _save_paper_ids(conn, project_id: int, paper_ids: list[str]) -> None:
    conn.execute("DELETE FROM project_papers WHERE project_id = ?", (project_id,))
    conn.executemany(
        "INSERT INTO project_papers(project_id, paper_id, position) VALUES (?, ?, ?)",
        [(project_id, pid, idx) for idx, pid in enumerate(paper_ids)],
    )


def _sync_project_papers(conn, project_id: int, paper_ids: list[str]) -> None:
    deduped = list(dict.fromkeys(paper_ids))
    for pid in deduped:
        conn.execute("INSERT OR IGNORE INTO paper_roots(paper_id) VALUES (?)", (pid,))
    _save_paper_ids(conn, project_id, deduped)


# ── Colour helpers ────────────────────────────────────────────────────────────

def color_to_hex(color: int) -> str:
    """Convert a packed RGB integer to a CSS hex string, e.g. 0x5b8dee → '#5b8dee'."""
    return f"#{color:06x}"


def color_from_hex(hex_str: str) -> int:
    """Convert a CSS hex string to a packed RGB integer, e.g. '#5b8dee' → 0x5b8dee."""
    return int(hex_str.lstrip("#"), 16)


# ── Internal helpers ──────────────────────────────────────────────────────────

def _load_paper_ids(project_id: int) -> list[str]:
    """Return the ordered list of paper_ids for a project from the bridge table."""
    with _connect() as conn:
        rows = conn.execute(
            "SELECT paper_id FROM project_papers WHERE project_id = ? ORDER BY position",
            (project_id,),
        ).fetchall()
    return [row["paper_id"] for row in rows]


def _save_paper_ids(conn, project_id: int, paper_ids: list[str]) -> None:
    conn.execute("DELETE FROM project_papers WHERE project_id = ?", (project_id,))
    for pos, pid in enumerate(paper_ids):
        conn.execute("INSERT OR IGNORE INTO paper_roots(paper_id) VALUES (?)", (pid,))
        conn.execute(
            "INSERT INTO project_papers (project_id, paper_id, position) VALUES (?, ?, ?)",
            (project_id, pid, pos),
        )


# ── Data model

@dataclass
class Project:
    name:         str
    description:  str                         = ""
    color:        Optional[int]               = None   # packed RGB, e.g. 0x5b8dee
    project_tags: list[str]                   = field(default_factory=list)
    paper_ids:    list[str]                   = field(default_factory=list)  # in-memory; sourced from project_papers
    status:       Status                      = Status.ACTIVE
    id:           Optional[int]               = None
    created_at:   Optional[datetime.datetime] = None
    updated_at:   Optional[datetime.datetime] = None
    archived_at:  Optional[datetime.datetime] = None

    # ── Construction

    @classmethod
    def from_row(cls, row) -> Project:
        """Construct a Project from a sqlite3.Row returned by a projects query."""
        proj_id = row["id"]
        paper_ids = _load_paper_ids(proj_id) if proj_id is not None else []
        return cls(
            id           = proj_id,
            name         = row["name"],
            description  = row["description"] or "",
            color        = int(row["color"]) if row["color"] is not None else None,
            project_tags = row["project_tags"] or [],
            paper_ids    = paper_ids,
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
                         project_tags, status)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (self.name, self.description, self.color,
                     self.created_at, self.updated_at, self.archived_at,
                     self.project_tags, self.status),
                )
                self.id = cur.lastrowid
                assert self.id is not None
                _save_paper_ids(conn, self.id, self.paper_ids)
        else:
            with _connect() as conn:
                conn.execute(
                    """
                    UPDATE projects
                    SET name = ?, description = ?, color = ?, updated_at = ?,
                        archived_at = ?, project_tags = ?, status = ?
                    WHERE id = ?
                    """,
                    (self.name, self.description, self.color,
                     self.updated_at, self.archived_at,
                     self.project_tags, self.status, self.id),
                )
                assert self.id is not None
                _save_paper_ids(conn, self.id, self.paper_ids)

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
        with _connect() as conn:
            _save_paper_ids(conn, self.id, self.paper_ids)

    def add_papers(self, paper_ids: list[str]) -> None:
        """Bulk-add papers, appended in order. Skips duplicates."""
        if self.id is None:
            raise ValueError("Project must be saved before papers can be added.")
        new_ids = [pid for pid in paper_ids if pid not in self.paper_ids]
        if not new_ids:
            return
        self.paper_ids.extend(new_ids)
        with _connect() as conn:
            _save_paper_ids(conn, self.id, self.paper_ids)

    def remove_paper(self, paper_id: str) -> None:
        """Remove a paper from this project."""
        if self.id is None:
            return
        if paper_id not in self.paper_ids:
            return
        self.paper_ids.remove(paper_id)
        with _connect() as conn:
            _save_paper_ids(conn, self.id, self.paper_ids)

    def reorder_paper(self, paper_id: str, new_position: int) -> None:
        """Move a paper to a new index within the ordered list."""
        if paper_id not in self.paper_ids:
            return
        self.paper_ids.remove(paper_id)
        self.paper_ids.insert(new_position, paper_id)
        with _connect() as conn:
            _save_paper_ids(conn, self.id, self.paper_ids)

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
