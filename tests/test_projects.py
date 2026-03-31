"""Tests for projects.py — pure functions, Q predicates, and DB round-trips."""
import sys
import os

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from projects import (
    color_to_hex,
    color_from_hex,
    Q,
    Project,
    Status,
    filter_projects,
)


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

class TestProjectSaveAndFilter:
    def test_save_assigns_id(self, tmp_db):
        p = Project(name="My Project")
        assert p.id is None
        p.save()
        assert p.id is not None

    def test_save_sets_created_at(self, tmp_db):
        p = Project(name="Timestamped")
        p.save()
        assert p.created_at is not None

    def test_saved_project_returned_by_filter(self, tmp_db):
        p = Project(name="Findable")
        p.save()
        results = filter_projects()
        names = [proj.name for proj in results]
        assert "Findable" in names

    def test_filter_no_condition_returns_all(self, tmp_db):
        Project(name="Alpha").save()
        Project(name="Beta").save()
        results = filter_projects()
        names = {proj.name for proj in results}
        assert {"Alpha", "Beta"}.issubset(names)

    def test_filter_by_status_active(self, tmp_db):
        active = Project(name="Active One", status=Status.ACTIVE)
        active.save()
        archived = Project(name="Archived One", status=Status.ARCHIVED)
        archived.save()
        results = filter_projects(Q("status = ?", Status.ACTIVE))
        names = [proj.name for proj in results]
        assert "Active One" in names
        assert "Archived One" not in names

    def test_filter_not_deleted(self, tmp_db):
        live = Project(name="Live")
        live.save()
        dead = Project(name="Dead")
        dead.save()
        dead.delete()
        results = filter_projects(~Q("status = ?", Status.DELETED))
        names = [proj.name for proj in results]
        assert "Live" in names
        assert "Dead" not in names

    def test_update_existing_project(self, tmp_db):
        p = Project(name="Original Name")
        p.save()
        first_id = p.id
        p.name = "Updated Name"
        p.save()
        # id should stay the same
        assert p.id == first_id
        results = filter_projects()
        names = [proj.name for proj in results]
        assert "Updated Name" in names
        assert "Original Name" not in names

    def test_project_color_round_trip(self, tmp_db):
        p = Project(name="Colorful", color=0x5b8dee)
        p.save()
        results = filter_projects(Q("name = ?", "Colorful"))
        assert len(results) == 1
        assert results[0].color == 0x5b8dee

    def test_project_status_default_is_active(self, tmp_db):
        p = Project(name="Default Status")
        p.save()
        results = filter_projects(Q("name = ?", "Default Status"))
        assert results[0].status == Status.ACTIVE

    def test_project_description_persisted(self, tmp_db):
        p = Project(name="Described", description="A useful project")
        p.save()
        results = filter_projects(Q("name = ?", "Described"))
        assert results[0].description == "A useful project"


# ---------------------------------------------------------------------------
# Project.add_paper
# ---------------------------------------------------------------------------

class TestProjectAddPaper:
    def test_add_paper_appears_in_paper_ids(self, tmp_db):
        p = Project(name="Paper Collector")
        p.save()
        p.add_paper("2204.12985")
        assert "2204.12985" in p.paper_ids

    def test_add_paper_persisted_to_db(self, tmp_db):
        p = Project(name="Persisted Papers")
        p.save()
        p.add_paper("2301.00001")
        results = filter_projects(Q("name = ?", "Persisted Papers"))
        assert "2301.00001" in results[0].paper_ids

    def test_add_paper_no_duplicate(self, tmp_db):
        p = Project(name="No Dupes")
        p.save()
        p.add_paper("2204.12985")
        p.add_paper("2204.12985")
        assert p.paper_ids.count("2204.12985") == 1

    def test_add_multiple_papers(self, tmp_db):
        p = Project(name="Multi Papers")
        p.save()
        p.add_paper("2204.12985")
        p.add_paper("2301.00001")
        assert "2204.12985" in p.paper_ids
        assert "2301.00001" in p.paper_ids
        assert len(p.paper_ids) == 2

    def test_add_paper_before_save_raises(self):
        p = Project(name="Unsaved")
        with pytest.raises(ValueError, match="saved"):
            p.add_paper("2204.12985")

    def test_add_paper_at_position(self, tmp_db):
        p = Project(name="Ordered Papers")
        p.save()
        p.add_paper("2204.12985")
        p.add_paper("2301.00001")
        p.add_paper("1905.00001", position=0)
        assert p.paper_ids[0] == "1905.00001"

    def test_paper_count_property(self, tmp_db):
        p = Project(name="Counter")
        p.save()
        assert p.paper_count == 0
        p.add_paper("2204.12985")
        assert p.paper_count == 1
        p.add_paper("2301.00001")
        assert p.paper_count == 2
