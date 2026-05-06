from dataclasses import dataclass
from typing import Optional

from service.models.note import NoteDetails
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
    source_fk:  int | None = None  # filter by paper across all versions
    paper_id:   int | None = None  # filter by specific version
    project_id: int | None = None


def get_note_details(note: Note) -> Optional[NoteDetails]:
    return _get_note(note.note_id)


def get_notes(notes: Notes) -> list[NoteDetails]:
    if notes.source_fk is not None:
        return _get_notes(
            notes.source_fk,
            project_id   = notes.project_id,
            all_projects = notes.project_id is None,
        )
    elif notes.paper_id is not None:
        return []  # TODO: storage needs a get_notes_by_paper_id(paper_id: int) function
    elif notes.project_id is not None:
        return _get_project_notes(notes.project_id)
    return []