import enum
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional


class Status(str, enum.Enum):
    ACTIVE   = "active"
    ARCHIVED = "archived"
    DELETED  = "deleted"


@dataclass
class ProjectDetails:
    id:           Optional[int]      = None
    name:         str                = ""
    description:  str                = ""
    color:        Optional[int]      = None
    project_tags: list[str]          = field(default_factory=list)
    source_fks:   list[int]          = field(default_factory=list)
    status:       Status             = Status.ACTIVE
    created_at:   Optional[datetime] = None
    updated_at:   Optional[datetime] = None
    archived_at:  Optional[datetime] = None



from storage.projects import Q, Project  # noqa: E402