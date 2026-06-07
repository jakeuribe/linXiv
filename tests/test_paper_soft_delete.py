"""Tests for paper soft/hard delete — db layer, service layer, and integration."""
from __future__ import annotations

import datetime
import os
import sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import arxiv
import storage.db as db
import service.paper as paper_svc
from service.paper import Paper


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_result(
    arxiv_id: str = "2204.12985v1",
    title: str = "Test Paper",
) -> arxiv.Result:
    now = datetime.datetime(2024, 1, 15, 12, 0, 0, tzinfo=datetime.timezone.utc)
    return arxiv.Result(
        entry_id=f"http://arxiv.org/abs/{arxiv_id}",
        title=title,
        summary="Abstract.",
        authors=[arxiv.Result.Author("Alice Smith")],
        published=now,
        updated=now,
        primary_category="cs.LG",
        categories=["cs.LG"],
        doi="",
        comment="",
        journal_ref="",
        links=[],
    )


def _save(arxiv_id: str = "2204.12985v1", title: str = "Test Paper") -> str:
    """Save a paper and return its source_id."""
    source_id, _ = db.save_paper(_make_result(arxiv_id, title))
    return source_id


# ---------------------------------------------------------------------------
# DB layer — soft_delete_paper
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestSoftDeletePaper:
    def test_paper_hidden_from_get_paper_after_soft_delete(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        assert db.get_paper("2204.12985") is None

    def test_paper_hidden_from_list_papers_after_soft_delete(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        ids = [r["source_id"] for r in db.list_papers()]
        assert "2204.12985" not in ids

    def test_paper_hidden_from_get_all_versions_after_soft_delete(self):
        _save("2204.12985v1")
        _save("2204.12985v2")
        db.soft_delete_paper("2204.12985")
        assert db.get_all_versions("2204.12985") == []

    def test_soft_delete_only_affects_target(self):
        _save("2204.12985v1")
        _save("2301.00001v1")
        db.soft_delete_paper("2204.12985")
        assert db.get_paper("2301.00001") is not None

    def test_soft_delete_sets_status_deleted(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        with db._connect() as conn:
            row = conn.execute(
                "SELECT STATUS FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is not None
        assert str(row["STATUS"]) == "deleted"

    def test_soft_delete_sets_deleted_at(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        with db._connect() as conn:
            row = conn.execute(
                "SELECT DELETED_AT FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is not None
        assert row["DELETED_AT"] is not None

    def test_soft_delete_noop_for_unknown_paper(self):
        # Should not raise
        db.soft_delete_paper("0000.00000")

    def test_soft_delete_preserves_paper_rows_in_db(self):
        """PAPER and PAPER_ROOTS rows must still exist after soft delete (for restore)."""
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        with db._connect() as conn:
            root = conn.execute(
                "SELECT * FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
            paper = conn.execute(
                "SELECT * FROM PAPER WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert root is not None
        assert paper is not None

    def test_soft_delete_removes_pdf_file_in_linxiv_dir(self, tmp_path, monkeypatch):
        import storage.paths as paths
        pdf_dir = tmp_path / "pdfs"
        pdf_dir.mkdir(exist_ok=True)
        monkeypatch.setattr(paths, "pdf_dir", lambda: pdf_dir)
        # Also patch db module's imported pdf_dir reference
        monkeypatch.setattr("storage.db.pdf_dir", lambda: pdf_dir)

        _save("2204.12985v1")
        pdf_file = pdf_dir / "test.pdf"
        pdf_file.write_bytes(b"%PDF")
        with db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                (str(pdf_file), "2204.12985"),
            )
        db.soft_delete_paper("2204.12985")
        assert not pdf_file.exists()

    def test_soft_delete_does_not_remove_external_pdf(self, tmp_path, monkeypatch):
        """PDFs outside the linxiv pdf_dir should not be deleted."""
        import storage.paths as paths
        pdf_dir = tmp_path / "pdfs"
        pdf_dir.mkdir(exist_ok=True)
        monkeypatch.setattr(paths, "pdf_dir", lambda: pdf_dir)
        monkeypatch.setattr("storage.db.pdf_dir", lambda: pdf_dir)

        external = tmp_path / "external.pdf"
        external.write_bytes(b"%PDF")

        _save("2204.12985v1")
        with db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                (str(external), "2204.12985"),
            )
        db.soft_delete_paper("2204.12985")
        assert external.exists()


# ---------------------------------------------------------------------------
# DB layer — is_paper_deleted
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestIsPaperDeleted:
    def test_false_for_active_paper(self):
        _save("2204.12985v1")
        assert db.is_paper_deleted("2204.12985") is False

    def test_true_after_soft_delete(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        assert db.is_paper_deleted("2204.12985") is True

    def test_false_after_restore(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.restore_paper("2204.12985")
        assert db.is_paper_deleted("2204.12985") is False

    def test_false_for_unknown_paper(self):
        assert db.is_paper_deleted("0000.00000") is False


# ---------------------------------------------------------------------------
# DB layer — restore_paper
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestRestorePaper:
    def test_restore_makes_paper_visible_again(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.restore_paper("2204.12985")
        assert db.get_paper("2204.12985") is not None

    def test_restore_clears_deleted_at(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.restore_paper("2204.12985")
        with db._connect() as conn:
            row = conn.execute(
                "SELECT DELETED_AT, STATUS FROM PAPER_ROOTS WHERE SOURCE_ID = ?",
                ("2204.12985",),
            ).fetchone()
        assert row is not None
        assert row["DELETED_AT"] is None
        assert str(row["STATUS"]) == "active"

    def test_restore_returns_stored_pdf_path(self):
        _save("2204.12985v1")
        with db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = '/some/path.pdf' WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                ("2204.12985",),
            )
        db.soft_delete_paper("2204.12985")
        pdf_path = db.restore_paper("2204.12985")
        assert pdf_path == "/some/path.pdf"

    def test_restore_returns_none_when_no_pdf(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        pdf_path = db.restore_paper("2204.12985")
        assert pdf_path is None

    def test_restore_paper_appears_in_list_papers(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.restore_paper("2204.12985")
        ids = [r["source_id"] for r in db.list_papers()]
        assert "2204.12985" in ids

    def test_restore_noop_for_active_paper(self):
        _save("2204.12985v1")
        db.restore_paper("2204.12985")
        assert db.get_paper("2204.12985") is not None


# ---------------------------------------------------------------------------
# DB layer — hard_delete_paper
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestHardDeletePaper:
    def test_hard_delete_removes_from_paper_roots(self):
        _save("2204.12985v1")
        db.hard_delete_paper("2204.12985")
        with db._connect() as conn:
            row = conn.execute(
                "SELECT * FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is None

    def test_hard_delete_removes_all_paper_versions(self):
        _save("2204.12985v1")
        _save("2204.12985v2")
        db.hard_delete_paper("2204.12985")
        with db._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM PAPER WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchall()
        assert rows == []

    def test_hard_delete_paper_not_found_by_get(self):
        _save("2204.12985v1")
        db.hard_delete_paper("2204.12985")
        assert db.get_paper("2204.12985") is None

    def test_hard_delete_only_affects_target(self):
        _save("2204.12985v1")
        _save("2301.00001v1")
        db.hard_delete_paper("2204.12985")
        assert db.get_paper("2301.00001") is not None

    def test_hard_delete_noop_for_unknown(self):
        db.hard_delete_paper("0000.00000")

    def test_hard_delete_soft_deleted_paper_removes_root(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.hard_delete_paper("2204.12985")
        with db._connect() as conn:
            row = conn.execute(
                "SELECT * FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is None


# ---------------------------------------------------------------------------
# DB layer — list_deleted_papers
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestListDeletedPapers:
    def test_empty_when_no_deletions(self):
        _save("2204.12985v1")
        assert db.list_deleted_papers() == []

    def test_returns_soft_deleted_paper(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        rows = db.list_deleted_papers()
        assert len(rows) == 1
        assert rows[0]["source_id"] == "2204.12985"

    def test_excludes_active_papers(self):
        _save("2204.12985v1")
        _save("2301.00001v1")
        db.soft_delete_paper("2204.12985")
        rows = db.list_deleted_papers()
        ids = [r["source_id"] for r in rows]
        assert "2301.00001" not in ids

    def test_multiple_deleted_papers(self):
        _save("2204.12985v1")
        _save("2301.00001v1")
        db.soft_delete_paper("2204.12985")
        db.soft_delete_paper("2301.00001")
        rows = db.list_deleted_papers()
        ids = {r["source_id"] for r in rows}
        assert ids == {"2204.12985", "2301.00001"}

    def test_restored_paper_not_in_list(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.restore_paper("2204.12985")
        assert db.list_deleted_papers() == []

    def test_view_fields_present(self):
        _save("2204.12985v1", title="Attention Paper")
        db.soft_delete_paper("2204.12985")
        rows = db.list_deleted_papers()
        assert rows[0]["title"] == "Attention Paper"
        assert rows[0]["deleted_at"] is not None


# ---------------------------------------------------------------------------
# DB layer — re-save restores a soft-deleted paper
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestResaveRestoresSoftDeleted:
    def test_resave_restores_status_to_active(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        assert db.is_paper_deleted("2204.12985")
        db.save_paper(_make_result("2204.12985v1"))
        assert not db.is_paper_deleted("2204.12985")

    def test_resave_makes_paper_visible_again(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.save_paper(_make_result("2204.12985v1"))
        assert db.get_paper("2204.12985") is not None

    def test_resave_does_not_duplicate_paper_roots(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.save_paper(_make_result("2204.12985v1"))
        with db._connect() as conn:
            count = conn.execute(
                "SELECT COUNT(*) FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()[0]
        assert count == 1


# ---------------------------------------------------------------------------
# DB layer — project membership survives soft delete
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestProjectMembershipSurvivesSoftDelete:
    def test_project_to_paper_rows_survive_soft_delete(self):
        from storage.projects import Project
        p = Project(name="My Project")
        p.save()
        source_fk = db.ensure_paper_root("2204.12985")
        db.save_paper(_make_result("2204.12985v1"))
        p.add_papers([source_fk])

        db.soft_delete_paper("2204.12985")

        with db._connect() as conn:
            row = conn.execute(
                "SELECT * FROM PROJECT_TO_PAPER WHERE SOURCE_FK = ?", (source_fk,)
            ).fetchone()
        assert row is not None

    def test_project_load_excludes_soft_deleted_paper(self):
        from storage.projects import Project
        p = Project(name="My Project")
        p.save()
        source_fk = db.ensure_paper_root("2204.12985")
        db.save_paper(_make_result("2204.12985v1"))
        p.add_papers([source_fk])

        db.soft_delete_paper("2204.12985")

        from storage.projects import get_project
        assert p.id is not None
        reloaded = get_project(p.id)
        assert reloaded is not None
        assert source_fk not in reloaded.source_fks

    def test_project_membership_returns_after_restore(self):
        from storage.projects import Project, get_project
        p = Project(name="My Project")
        p.save()
        source_fk = db.ensure_paper_root("2204.12985")
        db.save_paper(_make_result("2204.12985v1"))
        p.add_papers([source_fk])

        db.soft_delete_paper("2204.12985")
        db.restore_paper("2204.12985")

        assert p.id is not None
        reloaded = get_project(p.id)
        assert reloaded is not None
        assert source_fk in reloaded.source_fks

    def test_count_project_papers_excludes_deleted(self):
        from storage.projects import Project
        from storage.config.queries import count_project_papers
        p = Project(name="My Project")
        p.save()
        source_fk = db.ensure_paper_root("2204.12985")
        db.save_paper(_make_result("2204.12985v1"))
        p.add_papers([source_fk])
        assert p.id is not None
        assert count_project_papers(p.id) == 1

        db.soft_delete_paper("2204.12985")
        assert count_project_papers(p.id) == 0


# ---------------------------------------------------------------------------
# DB layer — graph data excludes deleted
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGraphDataExcludesDeleted:
    def test_get_graph_data_excludes_soft_deleted(self):
        _save("2204.12985v1", title="Active Paper")
        _save("2301.00001v1", title="Deleted Paper")
        db.soft_delete_paper("2301.00001")
        nodes, _ = db.get_graph_data()
        paper_nodes = [n for n in nodes if n.get("type") == "paper"]
        titles = [n["label"] for n in paper_nodes]
        assert "Active Paper" in titles
        assert "Deleted Paper" not in titles


# ---------------------------------------------------------------------------
# Service layer — paper_svc.delete / restore / hard_delete / list_deleted
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestPaperSvcDelete:
    def test_delete_by_source_id(self):
        _save("2204.12985v1")
        paper_svc.delete(Paper(source_id="2204.12985"))
        assert paper_svc.get(Paper(source_id="2204.12985")) is None

    def test_delete_by_source_fk(self):
        _save("2204.12985v1")
        sfk = db.ensure_paper_root("2204.12985")
        paper_svc.delete(Paper(source_fk=sfk))
        assert paper_svc.get(Paper(source_id="2204.12985")) is None

    def test_delete_returns_none_when_no_pdf(self):
        _save("2204.12985v1")
        result = paper_svc.delete(Paper(source_id="2204.12985"))
        assert result is None

    def test_delete_noop_for_unknown_paper(self):
        result = paper_svc.delete(Paper(source_id="0000.00000"))
        assert result is None


@pytest.mark.usefixtures("tmp_db")
class TestPaperSvcRestore:
    def test_restore_makes_paper_visible(self):
        _save("2204.12985v1")
        paper_svc.delete(Paper(source_id="2204.12985"))
        paper_svc.restore(Paper(source_id="2204.12985"))
        assert paper_svc.get(Paper(source_id="2204.12985")) is not None

    def test_restore_returns_project_fks(self):
        from storage.projects import Project
        p = Project(name="Proj")
        p.save()
        _save("2204.12985v1")
        sfk = db.ensure_paper_root("2204.12985")
        p.add_papers([sfk])

        paper_svc.delete(Paper(source_id="2204.12985"))
        _, project_fks = paper_svc.restore(Paper(source_id="2204.12985"))
        assert p.id in project_fks

    def test_restore_returns_pdf_path(self):
        _save("2204.12985v1")
        with db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = '/ext/paper.pdf' WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                ("2204.12985",),
            )
        paper_svc.delete(Paper(source_id="2204.12985"))
        pdf_path, _ = paper_svc.restore(Paper(source_id="2204.12985"))
        assert pdf_path == "/ext/paper.pdf"

    def test_restore_empty_project_fks_when_no_projects(self):
        _save("2204.12985v1")
        paper_svc.delete(Paper(source_id="2204.12985"))
        _, project_fks = paper_svc.restore(Paper(source_id="2204.12985"))
        assert project_fks == []


@pytest.mark.usefixtures("tmp_db")
class TestPaperSvcHardDelete:
    def test_hard_delete_removes_paper_root(self):
        _save("2204.12985v1")
        paper_svc.hard_delete(Paper(source_id="2204.12985"))
        with db._connect() as conn:
            row = conn.execute(
                "SELECT * FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is None

    def test_hard_delete_soft_deleted_paper(self):
        _save("2204.12985v1")
        paper_svc.delete(Paper(source_id="2204.12985"))
        paper_svc.hard_delete(Paper(source_id="2204.12985"))
        assert db.list_deleted_papers() == []


@pytest.mark.usefixtures("tmp_db")
class TestPaperSvcListDeleted:
    def test_returns_empty_when_none_deleted(self):
        _save("2204.12985v1")
        assert paper_svc.list_deleted() == []

    def test_returns_deleted_paper_details(self):
        _save("2204.12985v1", title="Deleted One")
        paper_svc.delete(Paper(source_id="2204.12985"))
        deleted = paper_svc.list_deleted()
        assert len(deleted) == 1
        assert deleted[0].source_id == "2204.12985"
        assert deleted[0].title == "Deleted One"

    def test_deleted_paper_details_include_project_fks(self):
        from storage.projects import Project
        p = Project(name="Proj")
        p.save()
        _save("2204.12985v1")
        sfk = db.ensure_paper_root("2204.12985")
        p.add_papers([sfk])

        paper_svc.delete(Paper(source_id="2204.12985"))
        deleted = paper_svc.list_deleted()
        assert len(deleted) == 1
        assert p.id in deleted[0].project_fks

    def test_restored_paper_absent_from_list_deleted(self):
        _save("2204.12985v1")
        paper_svc.delete(Paper(source_id="2204.12985"))
        paper_svc.restore(Paper(source_id="2204.12985"))
        assert paper_svc.list_deleted() == []


@pytest.mark.usefixtures("tmp_db")
class TestPaperSvcIsPaperDeleted:
    def test_false_for_active(self):
        _save("2204.12985v1")
        assert paper_svc.is_paper_deleted("2204.12985") is False

    def test_true_after_delete(self):
        _save("2204.12985v1")
        paper_svc.delete(Paper(source_id="2204.12985"))
        assert paper_svc.is_paper_deleted("2204.12985") is True

    def test_false_after_restore(self):
        _save("2204.12985v1")
        paper_svc.delete(Paper(source_id="2204.12985"))
        paper_svc.restore(Paper(source_id="2204.12985"))
        assert paper_svc.is_paper_deleted("2204.12985") is False


# ---------------------------------------------------------------------------
# Service helpers — set_has_pdf_by_source
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestPaperSvcHelpers:
    def test_set_has_pdf_by_source_sets_flag(self):
        _save("2204.12985v1")
        paper_svc.set_has_pdf_by_source("2204.12985", True)
        with db._connect() as conn:
            row = conn.execute(
                "SELECT HAS_PDF FROM PAPER WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is not None
        assert bool(row["HAS_PDF"]) is True

    def test_set_has_pdf_by_source_clears_flag(self):
        _save("2204.12985v1")
        paper_svc.set_has_pdf_by_source("2204.12985", True)
        paper_svc.set_has_pdf_by_source("2204.12985", False)
        with db._connect() as conn:
            row = conn.execute(
                "SELECT HAS_PDF FROM PAPER WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is not None
        assert bool(row["HAS_PDF"]) is False



# ---------------------------------------------------------------------------
# Migration — new columns added to existing DB
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestMigrationAddsSoftDeleteColumns:
    def test_status_column_exists(self):
        with db._connect() as conn:
            cols = {row[1] for row in conn.execute("PRAGMA table_info(PAPER_ROOTS)")}
        assert "STATUS" in cols

    def test_deleted_at_column_exists(self):
        with db._connect() as conn:
            cols = {row[1] for row in conn.execute("PRAGMA table_info(PAPER_ROOTS)")}
        assert "DELETED_AT" in cols

    def test_new_papers_default_to_active(self):
        _save("2204.12985v1")
        with db._connect() as conn:
            row = conn.execute(
                "SELECT STATUS FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is not None
        assert str(row["STATUS"]) == "active"

    def test_migration_is_idempotent(self):
        """Applying schema twice should not raise."""
        from storage.config.core import apply_sql_schema
        with db._connect() as conn:
            apply_sql_schema(conn)
        # If no exception, test passes


# ---------------------------------------------------------------------------
# Migration — real upgrade scenario (pre-existing rows get correct default)
# ---------------------------------------------------------------------------

class TestMigrationRealUpgradeScenario:
    def test_existing_rows_default_to_active_after_migration(self, tmp_path):
        """Simulate a pre-migration DB (no STATUS/DELETED_AT) being upgraded."""
        import sqlite3 as _sqlite3
        from storage.config.core import _migrate_paper_roots_soft_delete

        db_file = str(tmp_path / "legacy.db")
        conn = _sqlite3.connect(db_file)
        conn.row_factory = _sqlite3.Row
        # Create PAPER_ROOTS without STATUS / DELETED_AT
        conn.execute(
            "CREATE TABLE PAPER_ROOTS ("
            "  SOURCE_FK INTEGER PRIMARY KEY AUTOINCREMENT,"
            "  SOURCE_ID TEXT NOT NULL UNIQUE,"
            "  CREATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now')),"
            "  UPDATED_AT TIMESTAMP NOT NULL DEFAULT (datetime('now'))"
            ")"
        )
        conn.execute("INSERT INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)", ("2204.12985",))
        conn.commit()

        # Apply migration — must add STATUS with default 'active'
        _migrate_paper_roots_soft_delete(conn)
        conn.commit()

        row = conn.execute(
            "SELECT STATUS, DELETED_AT FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
        ).fetchone()
        conn.close()
        assert row is not None
        assert str(row["STATUS"]) == "active"
        assert row["DELETED_AT"] is None


# ---------------------------------------------------------------------------
# FTS state — soft_delete removes, restore re-indexes
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestFTSStateOnSoftDelete:
    def _assert_fts_exists(self, conn) -> None:
        assert conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='papers_fts'"
        ).fetchone(), "papers_fts table must exist (check apply_sql_schema)"

    def test_soft_delete_removes_fts_entry(self):
        _save("2204.12985v1")
        with db._connect() as conn:
            self._assert_fts_exists(conn)
            conn.execute(
                "INSERT OR REPLACE INTO papers_fts(paper_id, full_text) VALUES (?, ?)",
                ("2204.12985", "some full text"),
            )
        db.soft_delete_paper("2204.12985")
        with db._connect() as conn:
            row = conn.execute(
                "SELECT * FROM papers_fts WHERE paper_id = ?", ("2204.12985",)
            ).fetchone()
        assert row is None

    def test_restore_reindexes_fts_when_full_text_present(self):
        _save("2204.12985v1")
        with db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_META SET FULL_TEXT = 'deep learning stuff' WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                ("2204.12985",),
            )
        db.soft_delete_paper("2204.12985")
        db.restore_paper("2204.12985")
        with db._connect() as conn:
            self._assert_fts_exists(conn)
            row = conn.execute(
                "SELECT full_text FROM papers_fts WHERE paper_id = ?", ("2204.12985",)
            ).fetchone()
        assert row is not None
        assert row["full_text"] == "deep learning stuff"

    def test_restore_does_not_insert_fts_when_no_full_text(self):
        _save("2204.12985v1")
        db.soft_delete_paper("2204.12985")
        db.restore_paper("2204.12985")
        with db._connect() as conn:
            self._assert_fts_exists(conn)
            row = conn.execute(
                "SELECT * FROM papers_fts WHERE paper_id = ?", ("2204.12985",)
            ).fetchone()
        assert row is None


# ---------------------------------------------------------------------------
# note_counts_by_paper_for_project — excludes soft-deleted papers
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestNoteCountsExcludeDeleted:
    def test_deleted_paper_absent_from_note_count_dict(self):
        from storage.projects import Project
        from storage.notes import note_counts_by_paper_for_project

        proj = Project(name="P")
        proj.save()
        assert proj.id is not None

        _save("2204.12985v1")
        sfk = db.ensure_paper_root("2204.12985")
        proj.add_papers([sfk])

        db.soft_delete_paper("2204.12985")

        counts = note_counts_by_paper_for_project(proj.id)
        assert sfk not in counts

    def test_active_paper_present_in_note_count_dict(self):
        from storage.projects import Project
        from storage.notes import note_counts_by_paper_for_project

        proj = Project(name="P")
        proj.save()
        assert proj.id is not None

        _save("2204.12985v1")
        sfk = db.ensure_paper_root("2204.12985")
        proj.add_papers([sfk])

        counts = note_counts_by_paper_for_project(proj.id)
        assert sfk in counts
        assert counts[sfk] == 0


# ---------------------------------------------------------------------------
# HAS_PDF cleared after soft-delete (linxiv-dir PDF removed)
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestHasPdfClearedOnSoftDelete:
    def test_has_pdf_cleared_after_linxiv_pdf_deleted(self, tmp_path, monkeypatch):
        import storage.paths as paths
        pdf_dir = tmp_path / "pdfs"
        pdf_dir.mkdir(exist_ok=True)
        monkeypatch.setattr(paths, "pdf_dir", lambda: pdf_dir)
        monkeypatch.setattr("storage.db.pdf_dir", lambda: pdf_dir)

        _save("2204.12985v1")
        pdf_file = pdf_dir / "paper.pdf"
        pdf_file.write_bytes(b"%PDF")
        with db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                (str(pdf_file), "2204.12985"),
            )
            conn.execute("UPDATE PAPER SET HAS_PDF = 1 WHERE SOURCE_ID = ?", ("2204.12985",))

        db.soft_delete_paper("2204.12985")

        with db._connect() as conn:
            row = conn.execute(
                "SELECT HAS_PDF FROM PAPER WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is not None
        assert bool(row["HAS_PDF"]) is False


# ---------------------------------------------------------------------------
# Graph data — author nodes and edges for deleted papers are excluded
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGraphDataAuthorEdgesExcludeDeleted:
    def test_author_of_deleted_paper_absent_when_only_on_deleted(self):
        _save("2301.00001v1", title="Deleted Paper")  # author: "Alice Smith"
        db.soft_delete_paper("2301.00001")
        nodes, edges = db.get_graph_data()
        author_labels = {n["label"] for n in nodes if n.get("type") == "author"}
        # Alice Smith only appears on the deleted paper → should be absent
        assert "Alice Smith" not in author_labels
        assert edges == []

    def test_author_on_active_paper_still_present(self):
        _save("2204.12985v1", title="Active Paper")   # Alice Smith on active
        _save("2301.00001v1", title="Deleted Paper")   # Alice Smith on deleted
        db.soft_delete_paper("2301.00001")
        nodes, _ = db.get_graph_data()
        author_labels = {n["label"] for n in nodes if n.get("type") == "author"}
        # Alice Smith appears on active paper too → still present
        assert "Alice Smith" in author_labels


# ---------------------------------------------------------------------------
# hard_delete_paper — PDF in linxiv dir is also removed
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestHardDeletePaperCleansPdf:
    def test_hard_delete_removes_linxiv_pdf(self, tmp_path, monkeypatch):
        import storage.paths as paths
        pdf_dir = tmp_path / "pdfs"
        pdf_dir.mkdir(exist_ok=True)
        monkeypatch.setattr(paths, "pdf_dir", lambda: pdf_dir)
        monkeypatch.setattr("storage.db.pdf_dir", lambda: pdf_dir)

        _save("2204.12985v1")
        pdf_file = pdf_dir / "paper.pdf"
        pdf_file.write_bytes(b"%PDF")
        with db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                (str(pdf_file), "2204.12985"),
            )
        db.hard_delete_paper("2204.12985")
        assert not pdf_file.exists()

    def test_hard_delete_does_not_remove_external_pdf(self, tmp_path, monkeypatch):
        import storage.paths as paths
        pdf_dir = tmp_path / "pdfs"
        pdf_dir.mkdir(exist_ok=True)
        monkeypatch.setattr(paths, "pdf_dir", lambda: pdf_dir)
        monkeypatch.setattr("storage.db.pdf_dir", lambda: pdf_dir)

        external = tmp_path / "external.pdf"
        external.write_bytes(b"%PDF")
        _save("2204.12985v1")
        with db._connect() as conn:
            conn.execute(
                "UPDATE PAPER_META SET PDF_PATH = ? WHERE PAPER_ID IN "
                "(SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?)",
                (str(external), "2204.12985"),
            )
        db.hard_delete_paper("2204.12985")
        assert external.exists()


# ---------------------------------------------------------------------------
# _resolve_source_id — source_fk and paper_id branches
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestResolveBranches:
    def test_delete_via_paper_id(self):
        _save("2204.12985v1")
        with db._connect() as conn:
            pid = conn.execute(
                "SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()["PAPER_ID"]
        paper_svc.delete(Paper(paper_id=pid))
        assert paper_svc.is_paper_deleted("2204.12985")

    def test_restore_via_source_fk(self):
        _save("2204.12985v1")
        sfk = db.ensure_paper_root("2204.12985")
        paper_svc.delete(Paper(source_id="2204.12985"))
        paper_svc.restore(Paper(source_fk=sfk))
        assert not paper_svc.is_paper_deleted("2204.12985")

    def test_hard_delete_via_source_fk(self):
        _save("2204.12985v1")
        sfk = db.ensure_paper_root("2204.12985")
        paper_svc.hard_delete(Paper(source_fk=sfk))
        with db._connect() as conn:
            row = conn.execute(
                "SELECT * FROM PAPER_ROOTS WHERE SOURCE_ID = ?", ("2204.12985",)
            ).fetchone()
        assert row is None


# ---------------------------------------------------------------------------
# DeletedPaperDetails.had_pdf field
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestDeletedPaperDetailsHadPdf:
    def test_had_pdf_true_when_paper_had_pdf(self):
        _save("2204.12985v1")
        with db._connect() as conn:
            conn.execute("UPDATE PAPER SET HAS_PDF = 1 WHERE SOURCE_ID = ?", ("2204.12985",))
        paper_svc.delete(Paper(source_id="2204.12985"))
        deleted = paper_svc.list_deleted()
        assert len(deleted) == 1
        assert deleted[0].had_pdf is True

    def test_had_pdf_false_when_paper_had_no_pdf(self):
        _save("2204.12985v1")
        paper_svc.delete(Paper(source_id="2204.12985"))
        deleted = paper_svc.list_deleted()
        assert len(deleted) == 1
        assert deleted[0].had_pdf is False


# ---------------------------------------------------------------------------
# INSERT OR IGNORE metadata semantics — pinned behavior
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestResaveMetadataSemantics:
    def test_resave_of_restored_paper_does_not_update_metadata(self):
        """INSERT OR IGNORE means re-saving an existing (source_id, version) is a no-op
        for metadata. This pins the current behavior so any future change is explicit."""
        db.save_paper(_make_result("2204.12985v1", title="Original Title"))
        db.soft_delete_paper("2204.12985")
        # Re-save with different title — auto-restores but PAPER row already exists
        db.save_paper(_make_result("2204.12985v1", title="New Title"))
        row = db.get_paper("2204.12985")
        assert row is not None
        # Title is unchanged because INSERT OR IGNORE skips the re-insert
        assert row["title"] == "Original Title"


# ---------------------------------------------------------------------------
# export_import — overwrite branch restores a soft-deleted paper
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestExportImportRestoresSoftDeleted:
    def test_overwrite_branch_restores_soft_deleted_paper(self, tmp_path):
        """When on_conflict='overwrite' and the target paper is soft-deleted,
        commit_import must restore it (not leave it deleted)."""
        import service.export_import as ei
        import service.project as _project
        from sources.base import PaperMetadata
        import datetime as _dt

        meta = PaperMetadata(
            source_id="2204.77777",
            version=1,
            title="Overwrite Target",
            authors=["Bob Jones"],
            published=_dt.date(2024, 1, 1),
            summary="Abstract.",
        )
        sfk = db.ensure_paper_root("2204.77777")
        paper_svc.save_paper_metadata(meta)

        proj_fk = _project.upsert(
            _project.ProjectIn(name="OW Project", tags=[], source_fks=[sfk])
        )
        archive = ei.export_project(proj_fk, tmp_path / "export")

        # Soft-delete the paper
        paper_svc.delete(Paper(source_id="2204.77777"))
        assert paper_svc.is_paper_deleted("2204.77777")

        # Import with overwrite — should restore the paper
        new_fk = ei.commit_import(archive, on_conflict="overwrite")

        assert not paper_svc.is_paper_deleted("2204.77777")
        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        assert len(proj.source_fks) == 1

    def test_merge_branch_restores_soft_deleted_paper(self, tmp_path):
        """When on_conflict='merge' and the target paper is soft-deleted,
        commit_import must restore it (not leave it deleted)."""
        import service.export_import as ei
        import service.project as _project
        from sources.base import PaperMetadata
        import datetime as _dt

        meta = PaperMetadata(
            source_id="2204.88888",
            version=1,
            title="Merge Target",
            authors=["Carol White"],
            published=_dt.date(2024, 2, 1),
            summary="Abstract.",
        )
        sfk = db.ensure_paper_root("2204.88888")
        paper_svc.save_paper_metadata(meta)

        proj_fk = _project.upsert(
            _project.ProjectIn(name="Merge Project", tags=[], source_fks=[sfk])
        )
        archive = ei.export_project(proj_fk, tmp_path / "export")

        paper_svc.delete(Paper(source_id="2204.88888"))
        assert paper_svc.is_paper_deleted("2204.88888")

        new_fk = ei.commit_import(archive, on_conflict="merge")

        assert not paper_svc.is_paper_deleted("2204.88888")
        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        assert len(proj.source_fks) == 1
