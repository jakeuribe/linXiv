"""Unit tests for storage/notes.py — Note CRUD and query functions."""

from __future__ import annotations

import pytest

from storage.projects import Project, ensure_projects_db
from storage.notes import (
    Note,
    count_paper_notes,
    count_project_notes,
    get_note,
    get_notes,
    get_project_notes,
    note_counts_by_paper_for_project,
)


# ---------------------------------------------------------------------------
# Note.save() — create path
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestNoteSaveCreate:
    def test_save_assigns_id(self):
        n = Note(paper_id="2204.12985")
        assert n.id is None
        n.save()
        assert isinstance(n.id, int)

    def test_save_sets_created_at(self):
        n = Note(paper_id="2204.12985")
        assert n.created_at is None
        n.save()
        assert n.created_at is not None

    def test_save_sets_updated_at(self):
        n = Note(paper_id="2204.12985")
        n.save()
        assert n.updated_at is not None

    def test_save_persists_fields(self):
        n = Note(paper_id="2204.12985", title="My Title", content="Body text")
        n.save()
        assert n.id is not None
        fetched = get_note(n.id)
        assert fetched is not None
        assert fetched.paper_id == "2204.12985"
        assert fetched.title == "My Title"
        assert fetched.content == "Body text"

    def test_save_persists_project_id(self):
        n = Note(paper_id="2204.12985", project_id=42)
        n.save()
        assert n.id is not None
        fetched = get_note(n.id)
        assert fetched is not None
        assert fetched.project_id == 42

    def test_save_null_project_id(self):
        n = Note(paper_id="2204.12985", project_id=None)
        n.save()
        assert n.id is not None
        fetched = get_note(n.id)
        assert fetched is not None
        assert fetched.project_id is None

    def test_save_empty_title_and_content(self):
        n = Note(paper_id="2204.12985")
        n.save()
        assert n.id is not None
        fetched = get_note(n.id)
        assert fetched is not None
        assert fetched.title == ""
        assert fetched.content == ""

    def test_multiple_notes_get_distinct_ids(self):
        a = Note(paper_id="2204.12985")
        b = Note(paper_id="2204.12985")
        a.save()
        b.save()
        assert a.id != b.id


# ---------------------------------------------------------------------------
# Note.save() — update path
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestNoteSaveUpdate:
    def test_resave_does_not_create_new_row(self):
        n = Note(paper_id="2204.12985", title="Original")
        n.save()
        original_id = n.id
        n.title = "Updated"
        n.save()
        assert n.id == original_id

    def test_resave_updates_title(self):
        n = Note(paper_id="2204.12985", title="Before")
        n.save()
        n.title = "After"
        n.save()
        assert n.id is not None
        result = get_note(n.id)
        assert result is not None
        assert result.title == "After"

    def test_resave_updates_content(self):
        n = Note(paper_id="2204.12985", content="v1")
        n.save()
        n.content = "v2"
        n.save()
        assert n.id is not None
        result = get_note(n.id)
        assert result is not None
        assert result.content == "v2"

    def test_resave_updates_updated_at(self):
        import time
        n = Note(paper_id="2204.12985")
        n.save()
        first_updated = n.updated_at
        assert first_updated is not None
        time.sleep(0.01)
        n.save()
        assert n.updated_at is not None
        assert n.updated_at >= first_updated

    def test_resave_preserves_created_at(self):
        n = Note(paper_id="2204.12985")
        n.save()
        created = n.created_at
        n.title = "changed"
        n.save()
        assert n.created_at == created


# ---------------------------------------------------------------------------
# Note.delete()
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestNoteDelete:
    def test_delete_removes_from_db(self):
        n = Note(paper_id="2204.12985")
        n.save()
        assert n.id is not None
        nid = n.id
        n.delete()
        assert get_note(nid) is None

    def test_delete_clears_id(self):
        n = Note(paper_id="2204.12985")
        n.save()
        n.delete()
        assert n.id is None

    def test_delete_unsaved_note_is_noop(self):
        n = Note(paper_id="2204.12985")
        n.delete()  # should not raise
        assert n.id is None

    def test_delete_only_removes_target(self):
        a = Note(paper_id="2204.12985", title="A")
        b = Note(paper_id="2204.12985", title="B")
        a.save()
        b.save()
        a.delete()
        assert b.id is not None
        assert get_note(b.id) is not None


# ---------------------------------------------------------------------------
# get_note
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGetNote:
    def test_returns_none_for_missing_id(self):
        assert get_note(9999) is None

    def test_returns_note_for_valid_id(self):
        n = Note(paper_id="2204.12985", title="T")
        n.save()
        assert n.id is not None
        fetched = get_note(n.id)
        assert fetched is not None
        assert fetched.id == n.id
        assert fetched.title == "T"


# ---------------------------------------------------------------------------
# get_notes
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGetNotes:
    def test_empty_db_returns_empty_list(self):
        assert get_notes("2204.12985") == []

    def test_returns_notes_for_paper(self):
        Note(paper_id="2204.12985", title="A").save()
        Note(paper_id="2204.12985", title="B").save()
        assert len(get_notes("2204.12985")) == 2

    def test_isolates_by_paper_id(self):
        Note(paper_id="AAAA", title="A").save()
        Note(paper_id="BBBB", title="B").save()
        results = get_notes("AAAA")
        assert len(results) == 1
        assert results[0].title == "A"

    def test_default_returns_only_null_project_notes(self):
        Note(paper_id="X", project_id=None, title="global").save()
        Note(paper_id="X", project_id=1, title="project").save()
        results = get_notes("X")  # project_id=None, all_projects=False
        assert len(results) == 1
        assert results[0].title == "global"

    def test_project_id_filter(self):
        Note(paper_id="X", project_id=None, title="global").save()
        Note(paper_id="X", project_id=7, title="p7").save()
        Note(paper_id="X", project_id=8, title="p8").save()
        results = get_notes("X", project_id=7)
        assert len(results) == 1
        assert results[0].title == "p7"

    def test_all_projects_returns_everything(self):
        Note(paper_id="X", project_id=None).save()
        Note(paper_id="X", project_id=1).save()
        Note(paper_id="X", project_id=2).save()
        assert len(get_notes("X", all_projects=True)) == 3

    def test_ordered_by_created_at_asc(self):
        import time
        for title in ("first", "second", "third"):
            Note(paper_id="X", title=title).save()
            time.sleep(0.01)
        results = get_notes("X", all_projects=True)
        assert [n.title for n in results] == ["first", "second", "third"]


# ---------------------------------------------------------------------------
# get_project_notes
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGetProjectNotes:
    def test_empty_returns_empty_list(self):
        assert get_project_notes(99) == []

    def test_returns_notes_for_project(self):
        Note(paper_id="A", project_id=1).save()
        Note(paper_id="B", project_id=1).save()
        Note(paper_id="C", project_id=2).save()
        results = get_project_notes(1)
        assert len(results) == 2
        assert all(n.project_id == 1 for n in results)

    def test_ordered_by_paper_id_then_created_at(self):
        import time
        Note(paper_id="Z", project_id=5, title="z1").save()
        time.sleep(0.01)
        Note(paper_id="A", project_id=5, title="a1").save()
        results = get_project_notes(5)
        assert results[0].paper_id == "A"
        assert results[1].paper_id == "Z"


# ---------------------------------------------------------------------------
# count_project_notes
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestCountProjectNotes:
    def test_zero_for_empty(self):
        assert count_project_notes(1) == 0

    def test_counts_only_target_project(self):
        Note(paper_id="A", project_id=1).save()
        Note(paper_id="B", project_id=1).save()
        Note(paper_id="C", project_id=2).save()
        assert count_project_notes(1) == 2
        assert count_project_notes(2) == 1

    def test_reflects_deletions(self):
        n = Note(paper_id="A", project_id=3)
        n.save()
        assert count_project_notes(3) == 1
        n.delete()
        assert count_project_notes(3) == 0


# ---------------------------------------------------------------------------
# count_paper_notes
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestCountPaperNotes:
    def test_zero_for_empty(self):
        assert count_paper_notes("2204.12985") == 0

    def test_counts_all_notes_for_paper(self):
        Note(paper_id="X", project_id=None).save()
        Note(paper_id="X", project_id=1).save()
        Note(paper_id="X", project_id=2).save()
        assert count_paper_notes("X") == 3

    def test_counts_only_target_project(self):
        Note(paper_id="X", project_id=1).save()
        Note(paper_id="X", project_id=2).save()
        assert count_paper_notes("X", project_id=1) == 1

    def test_isolates_by_paper_id(self):
        Note(paper_id="AAA").save()
        Note(paper_id="BBB").save()
        assert count_paper_notes("AAA") == 1

    def test_reflects_deletions(self):
        n = Note(paper_id="Y")
        n.save()
        assert count_paper_notes("Y") == 1
        n.delete()
        assert count_paper_notes("Y") == 0


# ---------------------------------------------------------------------------
# note_counts_by_paper_for_project
# ---------------------------------------------------------------------------


@pytest.mark.usefixtures("tmp_db")
class TestNoteCountsByPaperForProject:
    def test_unknown_project_returns_empty(self):
        ensure_projects_db()
        assert note_counts_by_paper_for_project(99999) == {}

    def test_empty_paper_ids_returns_empty(self):
        ensure_projects_db()
        proj = Project(name="Empty", paper_ids=[])
        proj.save()
        assert note_counts_by_paper_for_project(proj.id) == {}

    def test_counts_include_zeros_and_order(self):
        ensure_projects_db()
        proj = Project(name="P", paper_ids=["a", "b", "c"])
        proj.save()
        Note(paper_id="a", project_id=proj.id, title="n1").save()
        Note(paper_id="a", project_id=proj.id, title="n2").save()
        Note(paper_id="c", project_id=proj.id, title="n3").save()
        counts = note_counts_by_paper_for_project(proj.id)
        assert list(counts.keys()) == ["a", "b", "c"]
        assert counts == {"a": 2, "b": 0, "c": 1}
