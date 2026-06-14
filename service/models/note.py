from dataclasses import dataclass
from datetime import datetime


@dataclass
class NoteDetails:
    note_id:     int | None
    source_fk:   int
    paper_id_fk: int | None
    project_id:  int | None
    title:       str
    content:     str
    created_at:  datetime | None
    updated_at:  datetime | None

    def to_dict(self) -> dict:
        return {
            "id":          self.note_id,
            "source_fk":   self.source_fk,
            "paper_id_fk": self.paper_id_fk,
            "project_id":  self.project_id,
            "title":       self.title,
            "content":     self.content,
            "created_at":  self.created_at.isoformat() if self.created_at else None,
            "updated_at":  self.updated_at.isoformat() if self.updated_at else None,
        }
