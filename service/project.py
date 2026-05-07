from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional

from service.models.project import ProjectDetails, Status
from storage.notes import count_project_notes as _count_project_notes
from storage.projects import (
    Q,
    Project as _StorageProject,
    color_to_hex as _color_to_hex,
    color_from_hex as _color_from_hex,
    ensure_projects_db as _ensure_projects_db,
    get_project as _get_project,
    filter_projects as _filter_projects,
)


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
        project_tags = p.project_tags,
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
    return _to_details(p) if p is not None else None


def get_many(projects: Projects) -> list[ProjectDetails]:
    """Fetch projects matching any combination of Projects filter fields."""
    condition: Q | None = None

    if projects.project_fks is not None and len(projects.project_fks) > 0:
        placeholders = ",".join("?" * len(projects.project_fks))
        q = Q(f"id IN ({placeholders})", *projects.project_fks)
        condition = q if condition is None else condition & q

    if projects.status is not None:
        q = Q("status = ?", projects.status)
        condition = q if condition is None else condition & q

    return [_to_details(p) for p in _filter_projects(condition)]


def upsert(project: ProjectIn, project_fk: int | None = None) -> int:
    """Insert a new project or update an existing one. Returns PROJECT_FK."""
    if project_fk is None:
        p = _StorageProject(
            name         = project.name,
            description  = project.description,
            color        = project.color,
            project_tags = project.tags,
            source_fks   = project.source_fks,
        )
        p.save()
        assert p.id is not None
        return p.id
    else:
        p = _get_project(project_fk)
        if p is None:
            raise ValueError(f"Project with id={project_fk} not found")
        p.name         = project.name
        p.description  = project.description
        p.color        = project.color
        p.project_tags = project.tags
        p.source_fks   = project.source_fks
        p.save()
        assert p.id is not None
        return p.id


def delete(project: Project) -> None:
    if project.project_fk is None:
        return
    p = _get_project(project.project_fk)
    if p is None:
        return
    p.delete()


# ---------------------------------------------------------------------------
# Colour / DB helpers
# ---------------------------------------------------------------------------

def color_to_hex(color: int) -> str:
    return _color_to_hex(color)


def color_from_hex(hex_str: str) -> int:
    return _color_from_hex(hex_str)


def ensure_projects_db() -> None:
    _ensure_projects_db()


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


@dataclass
class ProjectClass:
    id: int


@dataclass
class ProjectUpdate(ProjectClass):
    """
    Data class for updating the data of a project, should never be passed with no
    values, besides project id number, ProjectClass should be used for calling
    for updates
    """
    name:         str|None
    description:  str|None           = ""
    color:        Optional[int]|None = None
    project_tags: list[str]|None     = field(default_factory=list)
    source_fks:   list[int]|None     = field(default_factory=list)
    status:       Status|None       = Status.ACTIVE
    id:           Optional[int]      = None
    created_at:   Optional[datetime] = None
    updated_at:   Optional[datetime] = None
    archived_at:  Optional[datetime] = None

def get_projects(status: Status = Status.ACTIVE) -> ProjectPage:
    projects = _filter_projects(Q("status = ?", status))
    return ProjectPage(
        num_projects  = len(projects),
        project_names = [p.name for p in projects],
        project_ids   = [p.id for p in projects],
        paper_counts  = [p.paper_count for p in projects],
        note_counts   = [_count_project_notes(p.id) if p.id is not None else 0 for p in projects],
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
            project_tags = project.project_tags,
            source_fks   = project.source_fks,
            status       = project.status,
            created_at   = project.created_at,
            updated_at   = project.updated_at,
            archived_at  = project.archived_at,
        )