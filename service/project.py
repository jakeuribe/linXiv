import logging
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Optional

from service.models.project import ProjectDetails, Status
from storage.db import get_paper_roots_bulk as _get_paper_roots_bulk
from storage.notes import count_project_notes as _count_project_notes
from storage.projects import (
    Q,
    Project as _StorageProject,
    color_to_hex as _color_to_hex,
    color_from_hex as _color_from_hex,
    ensure_projects_db as _ensure_projects_db,
    get_project as _get_project,
    filter_projects as _filter_projects,
    hard_delete_project as _hard_delete_project,
    remove_paper_from_all_projects as _remove_paper_from_all_projects,
)
import storage.tags as _tags_storage

_log = logging.getLogger(__name__)


class Unset:
    """Sentinel type for "caller did not supply this argument".

    Distinct from None, which explicitly clears a nullable field (e.g. color).
    """
    def __repr__(self) -> str:
        return "UNSET"


UNSET = Unset()


# ---------------------------------------------------------------------------
# Tag helpers (shared by create, update, and upsert)
# ---------------------------------------------------------------------------

def _normalize_tags(tags: list[str]) -> list[str]:
    """Strip and deduplicate tags (case-insensitive dedup, case-preserving)."""
    seen: set[str] = set()
    result: list[str] = []
    for t in tags:
        label = t.strip()
        if label and label.lower() not in seen:
            seen.add(label.lower())
            result.append(label)
    return result


def _sync_tags(project_id: int, new_tags: list[str]) -> None:
    """Diff-based tag sync: only add new tags and remove dropped ones."""
    normalized = _normalize_tags(new_tags)
    current = _tags_storage.get_project_tags(project_id)
    current_lower = {t.lower() for t in current}
    new_lower = {t.lower() for t in normalized}
    to_remove = [t for t in current if t.lower() not in new_lower]
    to_add = [t for t in normalized if t.lower() not in current_lower]
    if to_remove:
        _tags_storage.remove_project_tags(project_id, to_remove)
    if to_add:
        _tags_storage.add_project_tags(project_id, to_add)


@dataclass
class Project:
    project_fk: int | None = None


@dataclass
class Projects:
    project_fks: list[int] | None = None
    status:      Status | None    = None


@dataclass
class ProjectIn:
    name:        str
    description: str            = ""
    color:       int | None     = None
    tags:        list[str]      = field(default_factory=list)
    source_fks:  list[int]      = field(default_factory=list)


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

def _to_details(p: _StorageProject) -> ProjectDetails:
    return ProjectDetails(
        id           = p.id,
        name         = p.name,
        description  = p.description,
        color        = p.color,
        project_tags = _tags_storage.get_project_tags(p.id) if p.id else [],
        source_fks   = p.source_fks,
        status       = p.status,
        created_at   = p.created_at,
        updated_at   = p.updated_at,
        archived_at  = p.archived_at,
    )


# ---------------------------------------------------------------------------
# Master functions
# ---------------------------------------------------------------------------

def get(project: Project) -> Optional[ProjectDetails]:
    """Fetch a single project by project_fk."""
    if project.project_fk is None:
        return None
    p = _get_project(project.project_fk)
    return _to_details(p) if p else None


def get_many(projects: Projects) -> list[ProjectDetails]:
    """Fetch projects matching any combination of Projects filter fields."""
    condition: Q | None = None

    if projects.project_fks and len(projects.project_fks) > 0:
        placeholders = ",".join("?" * len(projects.project_fks))
        q = Q(f"PROJECT_FK IN ({placeholders})", *projects.project_fks)
        condition = q if condition is None else condition & q

    if projects.status:
        q = Q("STATUS = ?", projects.status)
        condition = q if condition is None else condition & q

    return [_to_details(p) for p in _filter_projects(condition)]


def upsert(project: ProjectIn, project_fk: int | None = None) -> int:
    """Insert a new project or update an existing one. Returns PROJECT_FK.

    The update branch (project_fk given) is a full replace: fields, membership
    (becomes exactly project.source_fks), and tags. As of 2026-06 no production
    code calls it — all production updates are field-only (update()) or
    incremental membership (add_papers()/remove_papers()); it is kept as the
    one sanctioned full-replace entry point rather than having callers reach
    for Project.replace_papers directly. Do not use it to express incremental
    membership changes: the rewrite discards rows written by other processes
    since project.source_fks was loaded.
    """
    if project_fk is None:
        name = project.name.strip()
        if not name:
            raise ValueError("name cannot be blank")
        p = _StorageProject(
            name         = name,
            description  = project.description,
            color        = project.color,
            source_fks   = project.source_fks,
        )
        p.save()
        if p.id is None:
            raise RuntimeError("Project.save() did not set an id")
        tags = _normalize_tags(project.tags)
        if tags:
            _tags_storage.add_project_tags(p.id, tags)
        return p.id
    else:
        p = _get_project(project_fk)
        if p is None:
            raise LookupError(f"Project with id={project_fk} not found")
        name = project.name.strip()
        if not name:
            raise ValueError("name cannot be blank")
        if p.status == Status.DELETED:
            raise ValueError("cannot update a deleted project")
        p.name         = name
        p.description  = project.description
        p.color        = project.color
        # Three separate transactions (fields, membership, tags), in that
        # order: on partial failure, earlier steps are committed and later
        # ones are not.
        p.save()
        if p.id is None:
            raise RuntimeError("Project.save() did not set an id")
        # Upsert semantics: membership becomes exactly project.source_fks.
        # save() itself no longer writes membership (see storage.projects).
        # Callers making incremental changes should use add_papers/remove_papers.
        p.replace_papers(project.source_fks)
        _sync_tags(p.id, project.tags)
        return p.id


def create(project: ProjectIn) -> int:
    """Insert a new project and return PROJECT_FK."""
    return upsert(project, project_fk=None)


def update(
    project_fk: int,
    name: str | None = None,
    description: str | None = None,
    color: int | None | Unset = UNSET,
    project_tags: list[str] | None = None,
    status: Status | None = None,
) -> None:
    """Partial update of a project. Raises LookupError if project not found.

    Pass ``color=None`` to explicitly clear an existing color.
    Omit ``color`` (or pass the default) to leave it unchanged.
    """
    p = _get_project(project_fk)
    if p is None:
        raise LookupError(f"Project {project_fk} not found")
    if p.status == Status.DELETED and status != Status.ACTIVE:
        raise ValueError("cannot update a deleted project")
    dirty = False
    if name is not None:
        stripped = name.strip()
        if not stripped:
            raise ValueError("name cannot be blank")
        p.name = stripped
        dirty = True
    if description is not None:
        p.description = description
        dirty = True
    if not isinstance(color, Unset):
        p.color = color
        dirty = True
    if project_tags is not None:
        if p.id is None:
            raise RuntimeError("Project has no id after fetch — data integrity error")
        # Tag sync and project save are separate transactions. A failure between
        # them leaves tags updated but project fields unchanged.
        # dirty=True here so UPDATED_AT is bumped to reflect the tag change.
        _sync_tags(p.id, project_tags)
        dirty = True
    if status is not None:
        if status == p.status:
            # Already in requested state; only save if there are field changes.
            if dirty:
                p.save()
        elif status == Status.ARCHIVED:
            # archive/restore/delete call p.save() internally, persisting any
            # name/description/color mutations applied above.
            p.archive()
        elif status == Status.ACTIVE:
            p.restore()
        elif status == Status.DELETED:
            p.delete()
        else:
            raise ValueError(f"Unhandled status: {status!r}")
        return
    if dirty:
        p.save()


# ---------------------------------------------------------------------------
# Membership seam
#
# add_papers / remove_papers / link_imported are the membership-write
# functions for consumers (API, CLI, MCP, import flows); they resolve paper
# ids, apply the shared guards, and perform only per-row writes. Full replace
# happens solely through upsert() -> replace_papers. Guards: missing project
# raises ProjectNotFoundError (a LookupError), deleted project raises
# ProjectDeletedError (a ValueError); archived projects accept membership
# writes. Boundaries that translate errors (HTTP statuses, MCP ValueError
# convention) catch the typed subclasses so unrelated LookupError/ValueError
# from deeper layers pass through unclaimed.
# ---------------------------------------------------------------------------

class ProjectNotFoundError(LookupError):
    """Membership guard: the project does not exist."""


class ProjectDeletedError(ValueError):
    """Membership guard: the project has been soft-deleted."""


def _get_for_membership(project_fk: int) -> _StorageProject:
    """Load a project for a membership write, applying the shared guards.

    Skips loading the membership list — the write paths refresh it after
    writing, and the guards only need existence and status.
    """
    p = _get_project(project_fk, load_sources=False)
    if p is None:
        raise ProjectNotFoundError(f"Project {project_fk} not found")
    if p.status == Status.DELETED:
        raise ProjectDeletedError("cannot update a deleted project")
    return p


def ensure_membership_writable(project_fk: int) -> None:
    """Apply the membership-write guards without writing anything.

    Import flows call this before saving papers, so a missing project
    (ProjectNotFoundError) or deleted project (ProjectDeletedError) fails
    the operation before the library is mutated.
    """
    _get_for_membership(project_fk)


def _resolve_source_ids(source_ids: list[str]) -> tuple[list[int], list[str]]:
    """Resolve paper ids to SOURCE_FKs in one bulk lookup.

    Ids are stripped before lookup. Returns (fks in first-seen input order,
    unresolved ids verbatim — each reported once however often it repeats).
    """
    resolved = _get_paper_roots_bulk([sid.strip() for sid in source_ids])
    fks: list[int] = []
    failed: list[str] = []
    seen: set[str] = set()
    for sid in source_ids:
        stripped = sid.strip()
        if stripped in seen:
            continue
        seen.add(stripped)
        fk = resolved.get(stripped)
        if fk is None:
            failed.append(sid)
        else:
            fks.append(fk)
    return fks, failed


def add_papers(project_fk: int, source_ids: list[str]) -> list[str]:
    """Add papers to a project by paper id (no-op for existing members).

    Resolves each id to its PAPER_ROOTS row and performs only per-row
    inserts — the membership list is never rewritten. Partial success: ids
    that don't match any known paper are returned (verbatim, each reported
    once) while the rest are still added. Trashed papers count as known —
    the membership row is written but hidden from project reads until the
    paper is restored.

    Raises ProjectNotFoundError if the project does not exist,
    ProjectDeletedError if it has been deleted.
    """
    p = _get_for_membership(project_fk)
    fks, failed = _resolve_source_ids(source_ids)
    if fks:
        p.add_papers(fks)
    return failed


def remove_papers(project_fk: int, source_ids: list[str]) -> list[str]:
    """Remove papers from a project by paper id (no-op for non-members).

    Same contract as add_papers: per-row deletes only, and unresolved ids
    are returned verbatim while the rest are still removed.
    """
    p = _get_for_membership(project_fk)
    fks, failed = _resolve_source_ids(source_ids)
    if fks:
        p.remove_papers(fks)
    return failed


def link_imported(project_fk: int, source_ids: list[str]) -> None:
    """Link just-imported papers to a project (same write path as add_papers).

    On import flows the ids come from the import itself, not from user
    input, so an unresolved id means the save step and the lookup disagree
    on id form — it is logged as a warning rather than returned, since the
    caller has no user to report it to.

    Raises ProjectNotFoundError if the project does not exist,
    ProjectDeletedError if it has been deleted (e.g. deleted between the
    caller's pre-import ensure_membership_writable() check and this call).
    """
    failed = add_papers(project_fk, source_ids)
    if failed:
        _log.warning(
            "link_imported: %d of %d imported id(s) did not resolve for project %d: %r",
            len(failed), len(source_ids), project_fk, failed[:5],
        )


def remove_paper_from_all_projects(source_fk: int) -> list[int]:
    """Delete this paper's membership rows across all projects (single
    transaction). Returns the PROJECT_FKs that contained it."""
    return _remove_paper_from_all_projects(source_fk)


def remove_paper_from_all_projects_by_id(source_id: str) -> list[int] | None:
    """Resolve a paper id (stripped before lookup) and delete its membership
    rows across all projects. Returns the PROJECT_FKs that contained it, or
    None when the id does not match any known paper."""
    fk = _get_paper_roots_bulk([source_id.strip()]).get(source_id.strip())
    if fk is None:
        return None
    return _remove_paper_from_all_projects(fk)


def delete(project: Project) -> None:
    if project.project_fk is None:
        return
    p = _get_project(project.project_fk)
    if p is None:
        return
    p.delete()


def restore(project: Project) -> None:
    if project.project_fk is None:
        return
    p = _get_project(project.project_fk)
    if p is None:
        return
    p.restore()


def archive(project: Project) -> None:
    if project.project_fk is None:
        return
    p = _get_project(project.project_fk)
    if p is None:
        return
    p.archive()


def hard_delete(project: Project) -> None:
    if project.project_fk is None:
        return
    _hard_delete_project(project.project_fk)


def list_deleted() -> list[ProjectDetails]:
    """Return all soft-deleted projects ordered by deletion time (newest first)."""
    projects = _filter_projects(Q("STATUS = ?", Status.DELETED.value))
    # archived_at is stored/read as naive datetimes; datetime.min is also naive, safe to compare.
    projects.sort(key=lambda p: p.archived_at or datetime.min, reverse=True)
    return [_to_details(p) for p in projects]


def purge_old(days: int = 30) -> int:
    """Hard-delete projects that have been in the trash for more than `days` days. Returns count."""
    cutoff = datetime.now() - timedelta(days=days)
    old = [
        p for p in _filter_projects(Q("STATUS = ?", Status.DELETED.value))
        if p.archived_at and p.archived_at < cutoff
    ]
    for p in old:
        hard_delete(Project(project_fk=p.id))
    return len(old)


# ---------------------------------------------------------------------------
# Colour / DB helpers
# ---------------------------------------------------------------------------

def color_to_hex(color: int) -> str:
    return _color_to_hex(color)


def color_from_hex(hex_str: str) -> int:
    return _color_from_hex(hex_str)


def ensure_projects_db() -> None:
    _ensure_projects_db()


def filter_projects(condition: Q | None = None) -> list[_StorageProject]:
    return _filter_projects(condition)


# ---------------------------------------------------------------------------
# Low-level / legacy
# ---------------------------------------------------------------------------

@dataclass
class ProjectPage:
    num_projects: int
    project_names: list[str]
    project_ids: list[int|None]
    paper_counts: list[int]
    note_counts: list[int]


def get_projects(status: Status = Status.ACTIVE) -> ProjectPage:
    projects = _filter_projects(Q("status = ?", status))
    return ProjectPage(
        num_projects  = len(projects),
        project_names = [p.name for p in projects],
        project_ids   = [p.id for p in projects],
        paper_counts  = [p.paper_count for p in projects],
        note_counts   = [_count_project_notes(p.id) if p.id else 0 for p in projects],
    )


def get_project_details(project_id: int) -> Optional[ProjectDetails]:
    project = _get_project(project_id)
    if project is None:
        return None
    else:
        if project.id is None or project.id != project_id:
            print("ID fetched doesn't match ID received, returning none")
            return None
        return ProjectDetails(
            name         = project.name,
            id           = project.id,
            description  = project.description,
            color        = project.color,
            project_tags = _tags_storage.get_project_tags(project_id),
            source_fks   = project.source_fks,
            status       = project.status,
            created_at   = project.created_at,
            updated_at   = project.updated_at,
            archived_at  = project.archived_at,
        )