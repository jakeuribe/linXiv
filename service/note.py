from dataclasses import dataclass
from typing import Optional

from service.models.note import NoteDetails
from storage.notes import (
    get_note as _get_note,
    get_notes as _get_notes,
    get_notes_by_paper_id as _get_notes_by_paper_id,
    get_project_notes as _get_project_notes,
    Note as _StorageNote,
)


@dataclass
class Note:
    note_id:    int | None = None  # NOTE_SK — exact PK lookup
    source_fk:  int | None = None  # PAPER_ROOTS.SOURCE_FK — all notes for a paper root
    paper_id:   int | None = None  # PAPER.PAPER_ID — pinned to a specific version
    project_fk: int | None = None  # PROJECT.PROJECT_FK — scoped to a project


@dataclass
class Notes:
    source_fk:  int | None = None
    paper_id:   int | None = None
    project_fk: int | None = None


@dataclass
class NoteIn:
    source_fk:  int
    title:       str
    content:     str
    paper_id:    int | None = None
    project_fk:  int | None = None


# ---------------------------------------------------------------------------
# Master functions
# ---------------------------------------------------------------------------

def get(note: Note) -> Optional[NoteDetails]:
    """Fetch a single note.

    Resolution order: note_id → source_fk + project_fk → paper_id + project_fk
    """
    if note.note_id is not None:
        return get_note_details(note)

    if note.source_fk is not None:
        results = _get_notes(
            note.source_fk,
            project_id=note.project_fk,
            all_projects=note.project_fk is None,
        )
        return results[0] if results else None

    if note.paper_id is not None and note.project_fk is not None:
        results = _get_project_notes(note.project_fk)
        matches = [n for n in results if n.paper_id_fk == note.paper_id]
        return matches[0] if matches else None

    return None


def get_many(notes: Notes) -> list[NoteDetails]:
    """Fetch notes matching any combination of Notes filter fields."""
    return get_notes(notes)


def upsert(note: NoteIn) -> int | None:
    """Insert or update a note. Returns NOTE_SK."""
    storage_note = _StorageNote(
        source_fk=note.source_fk,
        paper_id_fk=note.paper_id,
        project_id=note.project_fk,
        title=note.title,
        content=note.content,
    )
    storage_note.save()
    return storage_note.id


def delete(note: Note) -> None:
    details = get(note)
    if details is None:
        return
    storage_note = _StorageNote(
        id=details.note_id,
        source_fk=details.source_fk,
        paper_id_fk=details.paper_id_fk,
        project_id=details.project_id,
        title=details.title,
        content=details.content,
    )
    storage_note.delete()


# ---------------------------------------------------------------------------
# Low-level reads
# ---------------------------------------------------------------------------

def get_note_details(note: Note) -> Optional[NoteDetails]:
    if note.note_id is None:
        return None
    return _get_note(note.note_id)


def get_notes(notes: Notes) -> list[NoteDetails]:
    if notes.source_fk is not None:
        return _get_notes(
            notes.source_fk,
            project_id   = notes.project_fk,
            all_projects = notes.project_fk is None,
        )
    elif notes.paper_id is not None:
        return _get_notes_by_paper_id(notes.paper_id)
    elif notes.project_fk is not None:
        return _get_project_notes(notes.project_fk)
    return []