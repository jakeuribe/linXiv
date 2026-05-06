from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional

from service.models.project import ProjectDetails, Status
from storage.projects import (
    Q,
    get_project as _get_project,
    filter_projects as _filter_projects,
)


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
        note_counts   = [0] * len(projects), # TODO:Fine for now but need to update
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