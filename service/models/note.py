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