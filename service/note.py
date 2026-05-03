from dataclasses import dataclass
from datetime import datetime
from typing import Optional

from storage.notes import (
    get_note as _get_note,
    get_notes as _get_notes,
    get_project_notes as _get_project_notes,
)


@dataclass
class Note:
    note_id: int


@dataclass
class Notes:
    paper_id:   int | None = None
    project_id: int | None = None


@dataclass
class NoteDetails:
    note_id:    int | None
    paper_id:   int
    project_id: int | None
    title:      str
    content:    str
    created_at: datetime | None
    updated_at: datetime | None


def _to_details(n) -> NoteDetails:
    return NoteDetails(
        note_id    = n.id,
        paper_id   = n.paper_id,
        project_id = n.project_id,
        title      = n.title,
        content    = n.content,
        created_at = n.created_at,
        updated_at = n.updated_at,
    )


def get_note_details(note: Note) -> Optional[NoteDetails]:
    n = _get_note(note.note_id)
    return _to_details(n) if n is not None else None


def get_notes(notes: Notes) -> list[NoteDetails]:
    if notes.paper_id is not None:
        ns = _get_notes(
            notes.paper_id,
            project_id   = notes.project_id,
            all_projects = notes.project_id is None,
        )
    elif notes.project_id is not None:
        ns = _get_project_notes(notes.project_id)
    else:
        return []
    return [_to_details(n) for n in ns]
