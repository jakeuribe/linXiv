from __future__ import annotations

import datetime
from dataclasses import dataclass, field
from typing import Optional

from storage.db import _connect
from storage.config.queries import Q
from service.models.project import Status


# ── DB helpers ────────────────────────────────────────────────────────────────

def ensure_projects_db() -> None:
    with _connect() as conn:
        row = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='PROJECT'"
        ).fetchone()
    if row is None:
        raise RuntimeError("PROJECT table not found — run apply_sql_schema first")


# ── Colour helpers ────────────────────────────────────────────────────────────

def color_to_hex(color: int) -> str:
    return f"#{color:06x}"


def color_from_hex(hex_str: str) -> int:
    return int(hex_str.lstrip("#"), 16)


# ── Internal helpers ──────────────────────────────────────────────────────────
#
# Membership writes come in two shapes:
#   * Incremental (add_paper / add_papers / remove_paper): single-statement
#     INSERT OR IGNORE / DELETE per row. These never touch other rows.
#   * Full replace (_save_source_fks, used by replace_papers and the initial
#     insert in save()): DELETE everything for the project, re-insert from the
#     caller's list. Rows written by anyone else since the caller loaded its
#     list are not preserved.
# save() persists project fields only; it does not write membership on update.

def _load_source_fks(project_fk: int) -> list[int]:
    with _connect() as conn:
        rows = conn.execute(
            """
            SELECT p2p.SOURCE_FK FROM PROJECT_TO_PAPER p2p
            JOIN PAPER_ROOTS r ON r.SOURCE_FK = p2p.SOURCE_FK
            WHERE p2p.PROJECT_FK = ? AND r.STATUS = 'active'
            ORDER BY p2p.PROJECT_TO_PAPER_FK
            """,
            (project_fk,),
        ).fetchall()
    return [int(row["SOURCE_FK"]) for row in rows]


# Backed by idx_project_to_paper_unique on (PROJECT_FK, SOURCE_FK); OR IGNORE
# makes the insert a no-op when the row already exists. Reads order by
# PROJECT_TO_PAPER_FK (rowid alias), so appended rows sort after existing ones.
_INSERT_MEMBERSHIP_SQL = (
    "INSERT OR IGNORE INTO PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK) VALUES (?, ?)"
)


def _save_source_fks(conn, project_fk: int, source_fks: list[int]) -> None:
    """Full replace: rewrite the project's membership to exactly source_fks."""
    conn.execute("DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?", (project_fk,))
    conn.executemany(
        _INSERT_MEMBERSHIP_SQL,
        [(project_fk, sfk) for sfk in source_fks],
    )


# ── Data model ────────────────────────────────────────────────────────────────

@dataclass
class Project:
    name:            str
    description:     str                         = ""
    color:           Optional[int]               = None
    project_tags:    list[str]                   = field(default_factory=list)
    source_fks:      list[int]                   = field(default_factory=list)
    status:          Status                      = Status.ACTIVE
    id:              Optional[int]               = None
    created_at:      Optional[datetime.datetime] = None
    updated_at:      Optional[datetime.datetime] = None
    archived_at:     Optional[datetime.datetime] = None
    _sources_loaded: bool                        = field(default=True, repr=False, compare=False)

    @classmethod
    def from_row(cls, row, load_sources: bool = True) -> Project:
        proj_fk = row["PROJECT_FK"]
        source_fks = _load_source_fks(proj_fk) if (proj_fk and load_sources) else []
        return cls(
            id              = proj_fk,
            name            = row["NAME"],
            description     = row["DESCRIPTION"] or "",
            color           = int(row["COLOR"]) if row["COLOR"] is not None else None,
            source_fks      = source_fks,
            status          = Status(row["STATUS"]),
            created_at      = row["CREATED_AT"],
            updated_at      = row["UPDATED_AT"],
            archived_at     = row["ARCHIVED_AT"],
            _sources_loaded = load_sources,
        )

    def save(self) -> None:
        now = datetime.datetime.now()
        self.updated_at = now
        if self.id is None:
            self.created_at = now
            with _connect() as conn:
                cur = conn.execute(
                    """
                    INSERT INTO PROJECT
                        (NAME, DESCRIPTION, COLOR, STATUS,
                         CREATED_AT, UPDATED_AT, ARCHIVED_AT)
                    VALUES (?, ?, ?, ?, ?, ?, ?)
                    """,
                    (self.name, self.description, self.color, self.status,
                     self.created_at, self.updated_at, self.archived_at),
                )
                self.id = cur.lastrowid
                assert self.id
                _save_source_fks(conn, self.id, self.source_fks)
        else:
            # Fields only. Membership is written by add_paper/add_papers/
            # remove_paper/replace_papers — rewriting it here from this
            # instance's (possibly stale) snapshot would discard rows written
            # by other requests since the snapshot was loaded.
            with _connect() as conn:
                conn.execute(
                    """
                    UPDATE PROJECT
                    SET NAME = ?, DESCRIPTION = ?, COLOR = ?, STATUS = ?,
                        UPDATED_AT = ?, ARCHIVED_AT = ?
                    WHERE PROJECT_FK = ?
                    """,
                    (self.name, self.description, self.color, self.status,
                     self.updated_at, self.archived_at, self.id),
                )

    def delete(self) -> None:
        self.status      = Status.DELETED
        self.archived_at = datetime.datetime.now()
        self.save()

    def archive(self) -> None:
        self.status      = Status.ARCHIVED
        self.archived_at = datetime.datetime.now()
        self.save()

    def restore(self) -> None:
        self.status      = Status.ACTIVE
        self.archived_at = None
        self.save()

    def _refresh_source_fks(self) -> None:
        """Reload membership through the usual read path after a write, so the
        in-memory list matches what any fresh load would see (active papers,
        PROJECT_TO_PAPER_FK order) rather than this instance's snapshot."""
        assert self.id is not None
        self.source_fks = _load_source_fks(self.id)
        self._sources_loaded = True

    def add_paper(self, source_fk: int) -> None:
        """Add one paper to the project (no-op if already a member)."""
        if self.id is None:
            raise ValueError("Project must be saved before papers can be added.")
        # No membership pre-check against self.source_fks (it may be stale);
        # the insert is OR IGNORE.
        with _connect() as conn:
            conn.execute(_INSERT_MEMBERSHIP_SQL, (self.id, source_fk))
        self._refresh_source_fks()

    def add_papers(self, source_fks: list[int]) -> None:
        """Add many papers; duplicates and existing members are skipped."""
        if self.id is None:
            raise ValueError("Project must be saved before papers can be added.")
        if not source_fks:
            return
        with _connect() as conn:
            conn.executemany(
                _INSERT_MEMBERSHIP_SQL,
                [(self.id, sfk) for sfk in source_fks],
            )
        self._refresh_source_fks()

    def remove_paper(self, source_fk: int) -> None:
        """Remove one paper from the project (no-op if not a member)."""
        if self.id is None:
            return
        with _connect() as conn:
            conn.execute(
                "DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ? AND SOURCE_FK = ?",
                (self.id, source_fk),
            )
        self._refresh_source_fks()

    def replace_papers(self, source_fks: list[int]) -> None:
        """Set the membership to exactly source_fks (full replace, in order).

        Any membership rows written since the caller loaded its snapshot are
        discarded. Use add/remove for incremental changes.
        """
        if self.id is None:
            raise ValueError("Project must be saved before papers can be replaced.")
        seen: set[int] = set()
        deduped: list[int] = []
        for sfk in source_fks:
            if sfk not in seen:
                seen.add(sfk)
                deduped.append(sfk)
        with _connect() as conn:
            _save_source_fks(conn, self.id, deduped)
        self._refresh_source_fks()

    def load_papers(self) -> list[int]:
        return self.source_fks

    @property
    def paper_count(self) -> int:
        # Returns 0 when built via from_row(load_sources=False); check
        # _sources_loaded before trusting this value for display.
        return len(self.source_fks)

    def __repr__(self) -> str:
        papers = len(self.source_fks) if self._sources_loaded else "?"
        return f"<Project id={self.id!r} name={self.name!r} status={self.status!r} papers={papers}>"


# ── Queries ───────────────────────────────────────────────────────────────────

def get_project(project_id: int) -> Optional[Project]:
    with _connect() as conn:
        row = conn.execute(
            "SELECT * FROM PROJECT WHERE PROJECT_FK = ?", (project_id,)
        ).fetchone()
    return Project.from_row(row) if row else None


def filter_projects(condition: Q | None = None, load_sources: bool = True) -> list[Project]:
    if condition is None:
        sql, params = "SELECT * FROM PROJECT", ()
    else:
        sql    = f"SELECT * FROM PROJECT WHERE {condition.sql}"
        params = condition.params
    with _connect() as conn:
        rows = conn.execute(sql, params).fetchall()
    return [Project.from_row(row, load_sources=load_sources) for row in rows]


def get_paper_project_fks(source_fk: int) -> list[int]:
    """Return PROJECT_FKs of all projects that contain this paper.

    Returns membership regardless of project status (active, archived, deleted).
    Callers that need only active projects must filter the result.
    """
    with _connect() as conn:
        rows = conn.execute(
            "SELECT PROJECT_FK FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?",
            (source_fk,),
        ).fetchall()
    return [int(r["PROJECT_FK"]) for r in rows]


def remove_paper_from_all_projects(source_fk: int) -> list[int]:
    """Remove a paper from every project. Returns the project FKs it was removed from."""
    with _connect() as conn:
        rows = conn.execute(
            "SELECT PROJECT_FK FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?", (source_fk,)
        ).fetchall()
        fks = [int(r["PROJECT_FK"]) for r in rows]
        if fks:
            conn.execute("DELETE FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?", (source_fk,))
    return fks


def hard_delete_project(project_fk: int) -> None:
    """Permanently remove a project and all its associations in a single transaction.

    Silently no-ops if project_fk does not exist — all four statements succeed as
    zero-row operations. Callers are responsible for existence checks.

    NOTE rows are not deleted: notes keep their content but lose their project scope.
    TAG rows are not cleaned up; orphan TAGs are an accepted trade-off.
    See docs/adr/0009-orphan-row-policy.md.
    """
    with _connect() as conn:
        conn.execute("DELETE FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?", (project_fk,))
        conn.execute("DELETE FROM PROJECT_TO_TAG WHERE PROJECT_FK = ?", (project_fk,))
        conn.execute("UPDATE NOTE SET PROJECT_FK = NULL WHERE PROJECT_FK = ?", (project_fk,))
        conn.execute("DELETE FROM PROJECT WHERE PROJECT_FK = ?", (project_fk,))
