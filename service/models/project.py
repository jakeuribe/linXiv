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

    def to_dict(self) -> dict:
        return {
            "id":           self.id,
            "name":         self.name,
            "description":  self.description,
            "color":        self.color,
            "project_tags": self.project_tags,
            "source_fks":   self.source_fks,
            "paper_count":  len(self.source_fks),
            "status":       self.status.value,
            "created_at":   self.created_at.isoformat() if self.created_at else None,
            "updated_at":   self.updated_at.isoformat() if self.updated_at else None,
            "archived_at":  self.archived_at.isoformat() if self.archived_at else None,
        }

