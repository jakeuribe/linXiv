"""Tests for projects.py — pure functions, Q predicates, and DB round-trips."""
import sqlite3
import sys
import os

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import storage.db as _db
import storage.tags as _tags
from storage.notes import Note as _StorageNote

from storage.projects import (
    color_to_hex,
    color_from_hex,
    Q,
    Project,
    Status,
    filter_projects,
    get_project,
)
import service.project as _svc_project
import service.tag as _svc_tag


def _sfk(label: str) -> int:
    """Insert or get a PAPER_ROOTS row, return its SOURCE_FK integer."""
    with _db._connect() as conn:
        conn.execute("INSERT OR IGNORE INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)", (label,))
        row = conn.execute("SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_ID = ?", (label,)).fetchone()
    return int(row[0])


# ---------------------------------------------------------------------------
# color_to_hex / color_from_hex — pure functions
# ---------------------------------------------------------------------------

class TestColorHelpers:
    def test_color_to_hex_known_value(self):
        assert color_to_hex(0x5b8dee) == "#5b8dee"

    def test_color_to_hex_zero(self):
        assert color_to_hex(0x000000) == "#000000"

    def test_color_to_hex_white(self):
        assert color_to_hex(0xFFFFFF) == "#ffffff"

    def test_color_from_hex_known_value(self):
        assert color_from_hex("#5b8dee") == 0x5b8dee

    def test_color_from_hex_no_hash(self):
        assert color_from_hex("5b8dee") == 0x5b8dee

    def test_color_from_hex_uppercase(self):
        assert color_from_hex("#AABBCC") == 0xAABBCC

    def test_round_trip_int_to_hex_to_int(self):
        original = 0x9b59b6
        assert color_from_hex(color_to_hex(original)) == original

    def test_round_trip_hex_to_int_to_hex(self):
        original = "#ff6347"
        assert color_to_hex(color_from_hex(original)) == original


# ---------------------------------------------------------------------------
# Q — composable predicates
# ---------------------------------------------------------------------------

class TestQ:
    def test_simple_q_has_sql_and_params(self):
        q = Q("status = ?", "active")
        assert q.sql == "status = ?"
        assert q.params == ("active",)

    def test_and_combines_sql(self):
        q = Q("a = ?", 1) & Q("b = ?", 2)
        assert q.sql == "(a = ? AND b = ?)"
        assert q.params == (1, 2)

    def test_or_combines_sql(self):
        q = Q("a = ?", 1) | Q("b = ?", 2)
        assert q.sql == "(a = ? OR b = ?)"
        assert q.params == (1, 2)

    def test_invert_wraps_with_not(self):
        q = ~Q("status = ?", "deleted")
        assert q.sql == "(NOT status = ?)"
        assert q.params == ("deleted",)

    def test_chained_and_or(self):
        q = Q("a = ?", 1) & (Q("b = ?", 2) | Q("c = ?", 3))
        assert "(b = ? OR c = ?)" in q.sql
        assert q.params == (1, 2, 3)

    def test_multiple_and_params_ordered(self):
        q = Q("x = ?", "foo") & Q("y = ?", "bar") & Q("z = ?", "baz")
        # Chaining left-to-right: first & gives (x AND y), second & gives ((x AND y) AND z)
        assert q.params == ("foo", "bar", "baz")

    def test_and_preserves_all_params(self):
        q = Q("a = ?", 10) & Q("b = ?", 20)
        assert 10 in q.params
        assert 20 in q.params

    def test_invert_preserves_params(self):
        q = ~Q("status = ?", "archived")
        assert "archived" in q.params


# ---------------------------------------------------------------------------
# Project.save / filter_projects
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestProjectSaveAndFilter:
    def test_save_assigns_id(self):
        p = Project(name="My Project")
        assert p.id is None
        p.save()
        assert p.id

    def test_save_sets_created_at(self):
        p = Project(name="Timestamped")
        p.save()
        assert p.created_at

    def test_saved_project_returned_by_filter(self):
        p = Project(name="Findable")
        p.save()
        results = filter_projects()
        names = [proj.name for proj in results]
        assert "Findable" in names

    def test_filter_no_condition_returns_all(self):
        Project(name="Alpha").save()
        Project(name="Beta").save()
        results = filter_projects()
        names = {proj.name for proj in results}
        assert {"Alpha", "Beta"}.issubset(names)

    def test_filter_by_status_active(self):
        active = Project(name="Active One", status=Status.ACTIVE)
        active.save()
        archived = Project(name="Archived One", status=Status.ARCHIVED)
        archived.save()
        results = filter_projects(Q("status = ?", Status.ACTIVE))
        names = [proj.name for proj in results]
        assert "Active One" in names
        assert "Archived One" not in names

    def test_filter_not_deleted(self):
        live = Project(name="Live")
        live.save()
        dead = Project(name="Dead")
        dead.save()
        dead.delete()
        results = filter_projects(~Q("status = ?", Status.DELETED))
        names = [proj.name for proj in results]
        assert "Live" in names
        assert "Dead" not in names

    def test_update_existing_project(self):
        p = Project(name="Original Name")
        p.save()
        first_id = p.id
        p.name = "Updated Name"
        p.save()
        assert p.id == first_id
        results = filter_projects()
        names = [proj.name for proj in results]
        assert "Updated Name" in names
        assert "Original Name" not in names

    def test_project_color_round_trip(self):
        p = Project(name="Colorful", color=0x5b8dee)
        p.save()
        results = filter_projects(Q("name = ?", "Colorful"))
        assert len(results) == 1
        assert results[0].color == 0x5b8dee

    def test_project_status_default_is_active(self):
        p = Project(name="Default Status")
        p.save()
        results = filter_projects(Q("name = ?", "Default Status"))
        assert results[0].status == Status.ACTIVE

    def test_project_description_persisted(self):
        p = Project(name="Described", description="A useful project")
        p.save()
        results = filter_projects(Q("name = ?", "Described"))
        assert results[0].description == "A useful project"


# ---------------------------------------------------------------------------
# Project.add_papers
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestProjectAddPaper:
    def test_add_paper_appears_in_source_ids(self):
        p = Project(name="Paper Collector")
        p.save()
        p.add_papers([_sfk("2204.12985")])
        assert _sfk("2204.12985") in p.source_fks

    def test_add_paper_persisted_to_db(self):
        p = Project(name="Persisted Papers")
        p.save()
        p.add_papers([_sfk("2301.00001")])
        results = filter_projects(Q("name = ?", "Persisted Papers"))
        assert _sfk("2301.00001") in results[0].source_fks

    def test_add_paper_no_duplicate(self):
        p = Project(name="No Dupes")
        p.save()
        p.add_papers([_sfk("2204.12985")])
        p.add_papers([_sfk("2204.12985")])
        assert p.source_fks.count(_sfk("2204.12985")) == 1

    def test_add_multiple_papers(self):
        p = Project(name="Multi Papers")
        p.save()
        p.add_papers([_sfk("2204.12985")])
        p.add_papers([_sfk("2301.00001")])
        assert _sfk("2204.12985") in p.source_fks
        assert _sfk("2301.00001") in p.source_fks
        assert len(p.source_fks) == 2

    def test_add_paper_before_save_raises(self):
        p = Project(name="Unsaved")
        with pytest.raises(ValueError, match="saved"):
            p.add_papers([1])

    def test_add_papers_preserves_input_order(self):
        p = Project(name="Ordered Papers")
        p.save()
        a, b, c = _sfk("2204.12985"), _sfk("2301.00001"), _sfk("1905.00001")
        p.add_papers([c, a, b])
        assert p.source_fks == [c, a, b]
        assert p.id is not None
        fresh = get_project(p.id)
        assert fresh is not None
        assert fresh.source_fks == [c, a, b]

    def test_paper_count_property(self):
        p = Project(name="Counter")
        p.save()
        assert p.paper_count == 0
        p.add_papers([_sfk("2204.12985")])
        assert p.paper_count == 1
        p.add_papers([_sfk("2301.00001")])
        assert p.paper_count == 2

    def test_concurrent_adds_with_stale_snapshots_both_persist(self):
        """Regression: two Project instances loaded before either writes must
        not clobber each other's adds (read-modify-write race)."""
        p = Project(name="Race")
        p.save()
        assert p.id is not None
        p1 = get_project(p.id)
        p2 = get_project(p.id)
        assert p1 is not None and p2 is not None
        p1.add_papers([_sfk("2204.12985")])
        p2.add_papers([_sfk("2301.00001")])
        fresh = get_project(p.id)
        assert fresh is not None
        assert _sfk("2204.12985") in fresh.source_fks
        assert _sfk("2301.00001") in fresh.source_fks

    def test_concurrent_removes_with_stale_snapshots_both_persist(self):
        p = Project(name="Race Remove")
        p.save()
        assert p.id is not None
        p.add_papers([_sfk("2204.12985"), _sfk("2301.00001")])
        p1 = get_project(p.id)
        p2 = get_project(p.id)
        assert p1 is not None and p2 is not None
        p1.remove_papers([_sfk("2204.12985")])
        p2.remove_papers([_sfk("2301.00001")])
        fresh = get_project(p.id)
        assert fresh is not None
        assert fresh.source_fks == []

    def test_add_papers_bulk_dedupes_input_and_existing(self):
        p = Project(name="Bulk")
        p.save()
        p.add_papers([_sfk("2204.12985")])
        p.add_papers([_sfk("2204.12985"), _sfk("2301.00001"), _sfk("2301.00001")])
        assert p.source_fks == [_sfk("2204.12985"), _sfk("2301.00001")]
        assert p.id is not None
        fresh = get_project(p.id)
        assert fresh is not None
        assert fresh.source_fks == [_sfk("2204.12985"), _sfk("2301.00001")]

    def test_save_does_not_clobber_concurrent_membership_writes(self):
        """Regression: a field-only save() from a stale snapshot must not
        discard membership rows written since the snapshot was loaded."""
        p = Project(name="Meta Edit")
        p.save()
        assert p.id is not None
        stale = get_project(p.id)
        assert stale is not None
        other = get_project(p.id)
        assert other is not None
        other.add_papers([_sfk("2204.12985")])
        stale.name = "Renamed"
        stale.save()
        fresh = get_project(p.id)
        assert fresh is not None
        assert fresh.name == "Renamed"
        assert _sfk("2204.12985") in fresh.source_fks

    def test_incremental_writes_keep_trashed_members(self):
        """Regression: incremental add/remove must not drop membership rows of
        trashed papers (the active-filtered snapshot omits them; a full rewrite
        from that snapshot would delete them, losing membership on restore)."""
        p = Project(name="Trash Safe")
        p.save()
        assert p.id is not None
        a, b = _sfk("2204.12985"), _sfk("2301.00001")
        p.add_papers([a, b])
        with _db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_ROOTS SET STATUS = 'deleted' WHERE SOURCE_FK = ?", (a,)
            )
        stale = get_project(p.id)  # sees only b
        assert stale is not None
        stale.add_papers([_sfk("1905.00001")])
        stale.remove_papers([b])
        with _db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_ROOTS SET STATUS = 'active' WHERE SOURCE_FK = ?", (a,)
            )
        fresh = get_project(p.id)
        assert fresh is not None
        assert a in fresh.source_fks  # membership survived the trash window

    def test_replace_papers_sets_exact_membership(self):
        p = Project(name="Replace")
        p.save()
        assert p.id is not None
        p.add_papers([_sfk("2204.12985"), _sfk("2301.00001")])
        p.replace_papers([_sfk("1905.00001"), _sfk("2301.00001"), _sfk("1905.00001")])
        fresh = get_project(p.id)
        assert fresh is not None
        assert fresh.source_fks == [_sfk("1905.00001"), _sfk("2301.00001")]


# ---------------------------------------------------------------------------
# storage.db.get_paper_roots_bulk
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGetPaperRootsBulk:
    def test_resolves_across_chunk_boundary(self):
        # 501 ids forces a second 500-id chunk.
        ids = [f"9000.{i:05d}" for i in range(501)]
        with _db._connect() as conn:
            conn.executemany(
                "INSERT OR IGNORE INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)",
                [(sid,) for sid in ids],
            )
        resolved = _db.get_paper_roots_bulk(ids + ["no.such.id"])
        assert len(resolved) == 501
        assert "no.such.id" not in resolved
        assert resolved[ids[0]] == _sfk(ids[0])
        assert resolved[ids[-1]] == _sfk(ids[-1])

    def test_empty_input_returns_empty(self):
        assert _db.get_paper_roots_bulk([]) == {}


# ---------------------------------------------------------------------------
# storage.tags — project tag join-table functions
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestStorageProjectTags:
    def _project(self, name: str = "Tag Test Project") -> int:
        p = Project(name=name)
        p.save()
        assert p.id
        return p.id

    def test_get_project_tags_empty(self):
        pid = self._project()
        assert _tags.get_project_tags(pid) == []

    def test_add_project_tags_returns_labels(self):
        pid = self._project()
        result = _tags.add_project_tags(pid, ["ml", "vision"])
        assert set(result) == {"ml", "vision"}

    def test_add_project_tags_persisted(self):
        pid = self._project()
        _tags.add_project_tags(pid, ["nlp", "graphs"])
        assert set(_tags.get_project_tags(pid)) == {"nlp", "graphs"}

    def test_add_project_tags_no_duplicates(self):
        pid = self._project()
        _tags.add_project_tags(pid, ["ml"])
        result = _tags.add_project_tags(pid, ["ml"])
        assert result.count("ml") == 1

    def test_add_project_tags_upserts_into_tag_table(self):
        pid = self._project()
        _tags.add_project_tags(pid, ["brand-new-label"])
        # Tag must now exist in TAG table
        with _tags._connect() as conn:
            row = conn.execute("SELECT TAG FROM TAG WHERE TAG = 'brand-new-label'").fetchone()
        assert row

    def test_remove_project_tags_removes_specific_label(self):
        pid = self._project()
        _tags.add_project_tags(pid, ["keep", "remove"])
        remaining = _tags.remove_project_tags(pid, ["remove"])
        assert "keep" in remaining
        assert "remove" not in remaining

    def test_remove_project_tags_returns_remaining(self):
        pid = self._project()
        _tags.add_project_tags(pid, ["a", "b", "c"])
        remaining = _tags.remove_project_tags(pid, ["b"])
        assert set(remaining) == {"a", "c"}

    def test_remove_nonexistent_tag_is_noop(self):
        pid = self._project()
        _tags.add_project_tags(pid, ["real"])
        remaining = _tags.remove_project_tags(pid, ["ghost"])
        assert remaining == ["real"]

    def test_remove_project_tags_case_insensitive(self):
        pid = self._project()
        _tags.add_project_tags(pid, ["ML"])
        remaining = _tags.remove_project_tags(pid, ["ml"])
        assert remaining == []

    def test_tags_isolated_between_projects(self):
        pid1 = self._project("Project A")
        pid2 = self._project("Project B")
        _tags.add_project_tags(pid1, ["exclusive"])
        assert _tags.get_project_tags(pid2) == []


# ---------------------------------------------------------------------------
# service.project.upsert — tag round-trips (insert + update branches)
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestServiceProjectUpsertTags:
    def test_insert_branch_persists_tags(self):
        fk = _svc_project.upsert(
            _svc_project.ProjectIn(name="Tagged", description="", tags=["research", "ml"])
        )
        assert set(_tags.get_project_tags(fk)) == {"research", "ml"}

    def test_insert_branch_no_tags(self):
        fk = _svc_project.upsert(
            _svc_project.ProjectIn(name="No Tags", description="", tags=[])
        )
        assert _tags.get_project_tags(fk) == []

    def test_update_branch_replaces_tags(self):
        fk = _svc_project.upsert(
            _svc_project.ProjectIn(name="Evolving", description="", tags=["old"])
        )
        _svc_project.upsert(
            _svc_project.ProjectIn(name="Evolving", description="", tags=["new"]),
            project_fk=fk,
        )
        remaining = _tags.get_project_tags(fk)
        assert "new" in remaining
        assert "old" not in remaining

    def test_update_branch_clears_tags_when_empty(self):
        fk = _svc_project.upsert(
            _svc_project.ProjectIn(name="Clearable", description="", tags=["gone"])
        )
        _svc_project.upsert(
            _svc_project.ProjectIn(name="Clearable", description="", tags=[]),
            project_fk=fk,
        )
        assert _tags.get_project_tags(fk) == []

    def test_update_branch_idempotent(self):
        fk = _svc_project.upsert(
            _svc_project.ProjectIn(name="Stable", description="", tags=["x", "y"])
        )
        _svc_project.upsert(
            _svc_project.ProjectIn(name="Stable", description="", tags=["x", "y"]),
            project_fk=fk,
        )
        assert set(_tags.get_project_tags(fk)) == {"x", "y"}

    def test_get_via_service_reflects_join_table(self):
        fk = _svc_project.upsert(
            _svc_project.ProjectIn(name="Readable", description="", tags=["svc-tag"])
        )
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details
        assert "svc-tag" in details.project_tags

    def test_upsert_update_nonexistent_raises(self):
        with pytest.raises(LookupError):
            _svc_project.upsert(
                _svc_project.ProjectIn(name="Ghost", description=""),
                project_fk=9999,
            )


# ---------------------------------------------------------------------------
# service.project — incremental membership
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestServiceProjectMembership:
    def test_add_papers_appends(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Inc", description=""))
        sfk = _sfk("2204.12985")
        failed = _svc_project.add_papers(fk, ["2204.12985"])
        assert failed == []
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details is not None
        assert sfk in details.source_fks

    def test_remove_papers_removes(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Inc Rm", description=""))
        _sfk("2204.12985")
        _svc_project.add_papers(fk, ["2204.12985"])
        failed = _svc_project.remove_papers(fk, ["2204.12985"])
        assert failed == []
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details is not None
        assert details.source_fks == []

    def test_add_papers_reports_unknown_ids_verbatim(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Partial", description=""))
        sfk = _sfk("2204.12985")
        failed = _svc_project.add_papers(
            fk, ["  2204.12985  ", "no.such.id", "no.such.id"]
        )
        # Unknown id comes back verbatim, once; the known id (stripped before
        # lookup) is still added.
        assert failed == ["no.such.id"]
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details is not None
        assert details.source_fks == [sfk]

    def test_remove_papers_reports_unknown_ids_and_removes_rest(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Partial Rm", description=""))
        _sfk("2204.12985")
        _svc_project.add_papers(fk, ["2204.12985"])
        failed = _svc_project.remove_papers(fk, ["2204.12985", "no.such.id"])
        assert failed == ["no.such.id"]
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details is not None
        assert details.source_fks == []

    def test_add_papers_to_archived_project_succeeds(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Arch", description=""))
        sfk = _sfk("2204.12985")
        _svc_project.archive(_svc_project.Project(project_fk=fk))
        failed = _svc_project.add_papers(fk, ["2204.12985"])
        assert failed == []
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details is not None
        assert sfk in details.source_fks

    def test_add_papers_unknown_project_raises(self):
        _sfk("2204.12985")
        with pytest.raises(LookupError):
            _svc_project.add_papers(9999, ["2204.12985"])

    def test_ensure_membership_writable_unknown_project_raises(self):
        with pytest.raises(LookupError):
            _svc_project.ensure_membership_writable(9999)

    def test_ensure_membership_writable_deleted_project_raises(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Gone", description=""))
        _svc_project.delete(_svc_project.Project(project_fk=fk))
        with pytest.raises(ValueError, match="deleted"):
            _svc_project.ensure_membership_writable(fk)

    def test_ensure_membership_writable_active_and_archived_pass(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Fine", description=""))
        _svc_project.ensure_membership_writable(fk)
        _svc_project.archive(_svc_project.Project(project_fk=fk))
        _svc_project.ensure_membership_writable(fk)

    def test_link_imported_logs_unresolved_ids(self, caplog):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Link", description=""))
        sfk = _sfk("2204.12985")
        with caplog.at_level("WARNING", logger="service.project"):
            _svc_project.link_imported(fk, ["2204.12985", "no.such.id"])
        assert any("did not resolve" in r.message for r in caplog.records)
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details is not None
        assert details.source_fks == [sfk]

    def test_add_papers_to_deleted_project_raises(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Trashed", description=""))
        _svc_project.delete(_svc_project.Project(project_fk=fk))
        with pytest.raises(ValueError, match="deleted"):
            _svc_project.add_papers(fk, ["2204.12985"])

    def test_remove_papers_from_deleted_project_raises(self):
        fk = _svc_project.upsert(_svc_project.ProjectIn(name="Trashed Rm", description=""))
        _svc_project.delete(_svc_project.Project(project_fk=fk))
        with pytest.raises(ValueError, match="deleted"):
            _svc_project.remove_papers(fk, ["2204.12985"])


# ---------------------------------------------------------------------------
# service.tag — project tag wrappers
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestServiceTagProjectWrappers:
    def _project_fk(self, name: str = "Wrapper Test") -> int:
        p = Project(name=name)
        p.save()
        assert p.id
        return p.id

    def test_get_project_tags_empty(self):
        pid = self._project_fk()
        assert _svc_tag.get_project_tags(pid) == []

    def test_add_project_tags_returns_list(self):
        pid = self._project_fk()
        result = _svc_tag.add_project_tags(pid, ["a", "b"])
        assert set(result) == {"a", "b"}

    def test_remove_project_tags_returns_remaining(self):
        pid = self._project_fk()
        _svc_tag.add_project_tags(pid, ["keep", "drop"])
        remaining = _svc_tag.remove_project_tags(pid, ["drop"])
        assert remaining == ["keep"]


# ---------------------------------------------------------------------------
# Project membership source-of-truth test (cross-cutting)
# ---------------------------------------------------------------------------

class TestProjectMembershipSourceOfTruth:
    def test_add_paper_in_memory_and_db(self, tmp_db):
        """After add_papers(), the source_fk must appear both in-memory and in PROJECT_TO_PAPER."""
        p = Project(name="Source of Truth Test")
        p.save()
        sfk = _sfk("2204.12985")
        p.add_papers([sfk])

        assert sfk in p.source_fks

        conn = sqlite3.connect(tmp_db)
        conn.row_factory = sqlite3.Row
        row = conn.execute(
            "SELECT SOURCE_FK FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ? AND SOURCE_FK = ?",
            (p.id, sfk),
        ).fetchone()
        conn.close()

        assert row
        assert row["SOURCE_FK"] == sfk


# ---------------------------------------------------------------------------
# storage.projects — Project.archive() and Project.restore()
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestStorageProjectArchiveAndRestore:
    def test_archive_sets_status_in_memory(self):
        p = Project(name="To Archive")
        p.save()
        p.archive()
        assert p.status == Status.ARCHIVED

    def test_archive_sets_archived_at_in_memory(self):
        p = Project(name="Archive Timestamp")
        p.save()
        p.archive()
        assert p.archived_at

    def test_archive_persists_status(self):
        p = Project(name="Persist Archive Status")
        p.save()
        p.archive()
        results = filter_projects(Q("name = ?", "Persist Archive Status"))
        assert len(results) == 1
        assert results[0].status == Status.ARCHIVED

    def test_archive_persists_archived_at(self):
        p = Project(name="Persist Archive At")
        p.save()
        p.archive()
        results = filter_projects(Q("name = ?", "Persist Archive At"))
        assert results[0].archived_at

    def test_restore_from_archived_sets_status_active(self):
        p = Project(name="Restore From Archive")
        p.save()
        p.archive()
        p.restore()
        assert p.status == Status.ACTIVE

    def test_restore_from_archived_clears_archived_at(self):
        p = Project(name="Restore Clears Timestamp")
        p.save()
        p.archive()
        p.restore()
        assert p.archived_at is None

    def test_restore_from_archived_persists(self):
        p = Project(name="Persist Restore")
        p.save()
        p.archive()
        p.restore()
        results = filter_projects(Q("name = ?", "Persist Restore"))
        assert results[0].status == Status.ACTIVE
        assert results[0].archived_at is None

    def test_restore_from_deleted_sets_status_active(self):
        p = Project(name="Restore From Deleted")
        p.save()
        p.delete()
        p.restore()
        assert p.status == Status.ACTIVE

    def test_restore_from_deleted_clears_archived_at(self):
        p = Project(name="Delete Then Restore")
        p.save()
        p.delete()
        p.restore()
        assert p.archived_at is None

    # Gap 3: papers and tags must survive archive() and restore() calls
    # (save() rewrites PROJECT_TO_PAPER from self.source_fks; a regression
    # that cleared source_fks before save would silently drop all papers)

    def test_archive_preserves_papers(self):
        p = Project(name="Archive Keeps Papers")
        p.save()
        assert p.id
        sfk = _sfk("archive-paper-1")
        p.add_papers([sfk])
        p.archive()
        reloaded = filter_projects(Q("name = ?", "Archive Keeps Papers"))
        assert len(reloaded) == 1
        assert sfk in reloaded[0].source_fks

    def test_restore_preserves_papers(self):
        p = Project(name="Restore Keeps Papers")
        p.save()
        assert p.id
        sfk = _sfk("restore-paper-1")
        p.add_papers([sfk])
        p.archive()
        p.restore()
        reloaded = filter_projects(Q("name = ?", "Restore Keeps Papers"))
        assert len(reloaded) == 1
        assert sfk in reloaded[0].source_fks

    def test_archive_preserves_project_tags(self):
        p = Project(name="Archive Keeps Tags")
        p.save()
        assert p.id
        _tags.add_project_tags(p.id, ["persist-tag"])
        p.archive()
        assert _tags.get_project_tags(p.id) == ["persist-tag"]

    def test_restore_preserves_project_tags(self):
        p = Project(name="Restore Keeps Tags")
        p.save()
        assert p.id
        _tags.add_project_tags(p.id, ["restore-tag"])
        p.archive()
        p.restore()
        assert _tags.get_project_tags(p.id) == ["restore-tag"]

    # Gap A: updated_at must be bumped by archive() and restore() (via save())

    def test_archive_updates_updated_at(self):
        p = Project(name="Archive Updates TS")
        p.save()
        ts_before = p.updated_at
        p.archive()
        reloaded = filter_projects(Q("name = ?", "Archive Updates TS"))
        assert reloaded[0].updated_at
        assert reloaded[0].updated_at >= ts_before  # type: ignore[operator]

    def test_restore_updates_updated_at(self):
        p = Project(name="Restore Updates TS")
        p.save()
        p.archive()
        ts_after_archive = p.updated_at
        p.restore()
        reloaded = filter_projects(Q("name = ?", "Restore Updates TS"))
        assert reloaded[0].updated_at
        assert reloaded[0].updated_at >= ts_after_archive  # type: ignore[operator]


# ---------------------------------------------------------------------------
# service.project — archive(), restore(), hard_delete()
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestServiceProjectLifecycle:
    def _fk(self, name: str = "Lifecycle Test") -> int:
        return _svc_project.upsert(_svc_project.ProjectIn(name=name, description=""))

    # ── archive ──────────────────────────────────────────────────────────────

    def test_service_archive_sets_status(self):
        fk = self._fk("Archive Me")
        _svc_project.archive(_svc_project.Project(project_fk=fk))
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details
        assert details.status == Status.ARCHIVED

    def test_service_archive_sets_archived_at(self):
        fk = self._fk("Archive Timestamp Svc")
        _svc_project.archive(_svc_project.Project(project_fk=fk))
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details
        assert details.archived_at

    def test_service_archive_none_fk_is_noop(self):
        _svc_project.archive(_svc_project.Project(project_fk=None))

    def test_service_archive_nonexistent_fk_is_noop(self):
        _svc_project.archive(_svc_project.Project(project_fk=9999))

    # ── restore ──────────────────────────────────────────────────────────────

    def test_service_restore_from_archived(self):
        fk = self._fk("Restore Archived Svc")
        _svc_project.archive(_svc_project.Project(project_fk=fk))
        _svc_project.restore(_svc_project.Project(project_fk=fk))
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details
        assert details.status == Status.ACTIVE
        assert details.archived_at is None

    def test_service_restore_from_deleted(self):
        fk = self._fk("Restore Deleted Svc")
        _svc_project.delete(_svc_project.Project(project_fk=fk))
        _svc_project.restore(_svc_project.Project(project_fk=fk))
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details
        assert details.status == Status.ACTIVE

    def test_service_restore_archived_at_is_none(self):
        fk = self._fk("Restore Clears AT Svc")
        _svc_project.archive(_svc_project.Project(project_fk=fk))
        _svc_project.restore(_svc_project.Project(project_fk=fk))
        details = _svc_project.get(_svc_project.Project(project_fk=fk))
        assert details
        assert details.archived_at is None

    def test_service_restore_none_fk_is_noop(self):
        _svc_project.restore(_svc_project.Project(project_fk=None))

    def test_service_restore_nonexistent_fk_is_noop(self):
        _svc_project.restore(_svc_project.Project(project_fk=9999))

    # ── hard_delete ──────────────────────────────────────────────────────────

    def test_hard_delete_removes_project_row(self):
        fk = self._fk("Hard Delete Me")
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk))
        assert _svc_project.get(_svc_project.Project(project_fk=fk)) is None

    def test_hard_delete_differs_from_soft_delete(self):
        """soft_delete leaves a row with status=DELETED; hard_delete removes the row entirely."""
        fk_soft = self._fk("Soft Deleted")
        fk_hard = self._fk("Hard Deleted")
        _svc_project.delete(_svc_project.Project(project_fk=fk_soft))
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk_hard))
        # Soft-deleted row still exists (status == DELETED)
        soft_results = filter_projects(Q("status = ?", Status.DELETED))
        soft_ids = [p.id for p in soft_results]
        assert fk_soft in soft_ids
        # Hard-deleted row is completely gone
        assert _svc_project.get(_svc_project.Project(project_fk=fk_hard)) is None

    def test_hard_delete_removes_project_to_paper_rows(self):
        p = Project(name="HD Papers")
        p.save()
        assert p.id
        p.add_papers([_sfk("hd-paper-1")])
        fk = p.id
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk))
        with _db._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?", (fk,)
            ).fetchall()
        assert rows == []

    def test_hard_delete_removes_project_to_tag_rows(self):
        fk = self._fk("HD Tags")
        _tags.add_project_tags(fk, ["science", "ml"])
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk))
        with _tags._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM PROJECT_TO_TAG WHERE PROJECT_FK = ?", (fk,)
            ).fetchall()
        assert rows == []

    def test_hard_delete_nullifies_note_project_fk(self, tmp_db):
        fk = self._fk("HD Notes")
        sfk = _sfk("hd-note-paper")
        note = _StorageNote(source_fk=sfk, project_id=fk, title="Keep Me", content="Body")
        note.save()
        note_sk = note.id
        assert note_sk
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk))
        # Row must survive (not be deleted)
        conn = sqlite3.connect(tmp_db)
        conn.row_factory = sqlite3.Row
        row = conn.execute("SELECT * FROM NOTE WHERE NOTE_SK = ?", (note_sk,)).fetchone()
        conn.close()
        assert row, "Note row must not be deleted by hard_delete"
        assert row["PROJECT_FK"] is None, "PROJECT_FK must be nullified, not deleted"

    def test_hard_delete_none_fk_is_noop(self):
        _svc_project.hard_delete(_svc_project.Project(project_fk=None))

    def test_hard_delete_nonexistent_fk_is_noop(self):
        # Goes straight to SQL (no _get_project guard); zero rows matched = silent no-op
        _svc_project.hard_delete(_svc_project.Project(project_fk=9999))

    # Gap 1: cross-project isolation — WHERE clause must be scoped to target project

    def test_hard_delete_does_not_affect_other_project_papers(self):
        pa = Project(name="HD Iso A Papers")
        pa.save()
        pb = Project(name="HD Iso B Papers")
        pb.save()
        assert pa.id and pb.id
        sfk_b = _sfk("iso-paper-b")
        pb.add_papers([sfk_b])
        _svc_project.hard_delete(_svc_project.Project(project_fk=pa.id))
        with _db._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM PROJECT_TO_PAPER WHERE PROJECT_FK = ?", (pb.id,)
            ).fetchall()
        assert len(rows) == 1

    def test_hard_delete_does_not_affect_other_project_tags(self):
        fk_a = self._fk("HD Iso A Tags")
        fk_b = self._fk("HD Iso B Tags")
        _tags.add_project_tags(fk_b, ["survive-tag"])
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk_a))
        with _tags._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM PROJECT_TO_TAG WHERE PROJECT_FK = ?", (fk_b,)
            ).fetchall()
        assert len(rows) == 1

    def test_hard_delete_does_not_remove_other_project_row(self):
        fk_a = self._fk("HD Iso A Row")
        fk_b = self._fk("HD Iso B Row")
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk_a))
        assert _svc_project.get(_svc_project.Project(project_fk=fk_b))

    def test_hard_delete_does_not_nullify_other_project_note(self, tmp_db):
        fk_a = self._fk("HD Iso A Note")
        fk_b = self._fk("HD Iso B Note")
        sfk = _sfk("iso-note-paper")
        note = _StorageNote(source_fk=sfk, project_id=fk_b, title="B Note", content="")
        note.save()
        note_sk = note.id
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk_a))
        conn = sqlite3.connect(tmp_db)
        conn.row_factory = sqlite3.Row
        row = conn.execute("SELECT PROJECT_FK FROM NOTE WHERE NOTE_SK = ?", (note_sk,)).fetchone()
        conn.close()
        assert row
        assert row["PROJECT_FK"] == fk_b

    # Gap 2: underlying TAG and PAPER_ROOTS rows must not be deleted by hard_delete

    def test_hard_delete_does_not_remove_tag_records(self):
        fk = self._fk("HD Keep Tag Records")
        _tags.add_project_tags(fk, ["keep-this-tag"])
        _svc_project.hard_delete(_svc_project.Project(project_fk=fk))
        with _tags._connect() as conn:
            row = conn.execute("SELECT TAG FROM TAG WHERE TAG = ?", ("keep-this-tag",)).fetchone()
        assert row

    def test_hard_delete_does_not_remove_paper_roots_records(self):
        p = Project(name="HD Keep Paper Roots")
        p.save()
        assert p.id
        sfk = _sfk("hd-keep-paper-root")
        p.add_papers([sfk])
        _svc_project.hard_delete(_svc_project.Project(project_fk=p.id))
        with _db._connect() as conn:
            row = conn.execute(
                "SELECT SOURCE_FK FROM PAPER_ROOTS WHERE SOURCE_FK = ?", (sfk,)
            ).fetchone()
        assert row
