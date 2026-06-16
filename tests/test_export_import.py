"""Tests for service/export_import.py — round-trip export/import coverage."""
from __future__ import annotations

import datetime
import json
import sys
import os
import zipfile

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import storage.db as _db
from storage.notes import Note as _StorageNote
import service.export_import as ei
import service.project as _project
import service.paper as _paper
import service.note as _note
from service.models.project import Status


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _save_paper(source_id: str, title: str, tags: list[str] | None = None) -> int:
    """Insert a minimal paper and return its SOURCE_FK."""
    from sources.base import PaperMetadata
    meta = PaperMetadata(
        source_id = source_id,
        version   = 1,
        title     = title,
        authors   = ["Test Author"],
        published = datetime.date(2023, 1, 1),
        summary   = "A test paper.",
        source    = "arxiv",
    )
    _paper.save_paper_metadata(meta, tags)
    return _paper.ensure_paper_root(source_id)


def _make_project(name: str, source_fks: list[int], color: int = 0x5b8dee) -> int:
    return _project.upsert(
        _project.ProjectIn(
            name        = name,
            description = "Test project",
            color       = color,
            tags        = ["research", "ml"],
            source_fks  = source_fks,
        )
    )


def _make_archive(tmp_path, manifest: dict):
    """Write a raw manifest dict into a .lxproj file and return its path."""
    p = tmp_path / "raw.lxproj"
    with zipfile.ZipFile(p, "w") as zf:
        zf.writestr("manifest.json", json.dumps(manifest))
    return p


def _minimal_paper_dict(source_id: str = "2204.99999", title: str = "T") -> dict:
    return {
        "source_id": source_id,
        "version": 1,
        "title": title,
        "authors": ["A"],
        "published": "2023-01-01",
        "updated": None,
        "summary": "",
        "category": None,
        "categories": None,
        "doi": None,
        "journal_ref": None,
        "comment": None,
        "url": None,
        "tags": [],
        "source": "arxiv",
    }


# ---------------------------------------------------------------------------
# preview_import
# ---------------------------------------------------------------------------

class TestPreviewImport:

    def test_reads_manifest_fields(self, tmp_path):
        sfk = _save_paper("2204.00001", "Alpha Paper")
        proj_fk = _make_project("My Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        preview = ei.preview_import(archive)

        assert preview.project_name   == "My Project"
        assert preview.paper_count    == 1
        assert preview.note_count     == 0
        assert preview.has_pdfs       is False
        assert preview.format_version == 1

    def test_has_pdfs_true_when_pdfs_bundled(self, tmp_path):
        sfk = _save_paper("2204.00001", "Alpha Paper")
        proj_fk = _make_project("PDF Project", [sfk])

        fake_pdf = tmp_path / "src_pdfs" / "alpha.pdf"
        fake_pdf.parent.mkdir(parents=True, exist_ok=True)
        fake_pdf.write_bytes(b"%PDF-fake")
        _paper.set_pdf_path("2204.00001", str(fake_pdf))
        _paper.set_has_pdf("2204.00001", 1, True)

        archive = ei.export_project(proj_fk, tmp_path / "export", include_pdfs=True)
        preview = ei.preview_import(archive)

        assert preview.has_pdfs is True

    def test_falls_back_when_summary_and_format_version_absent(self, tmp_path):
        # Both "summary" and "format_version" are absent to exercise the fallback paths
        manifest = {
            "project": {"name": "Bare", "description": ""},
            "papers": [_minimal_paper_dict()],
            "notes":  [],
        }
        archive = _make_archive(tmp_path, manifest)
        preview = ei.preview_import(archive)

        assert preview.paper_count    == 1   # counted from papers list
        assert preview.note_count     == 0   # counted from notes list
        assert preview.format_version == 1   # default when key absent

    def test_missing_manifest_raises(self, tmp_path):
        bad = tmp_path / "bad.lxproj"
        with zipfile.ZipFile(bad, "w") as zf:
            zf.writestr("not_manifest.json", "{}")
        with pytest.raises(ValueError, match="manifest.json"):
            ei.preview_import(bad)


# ---------------------------------------------------------------------------
# commit_import — project creation
# ---------------------------------------------------------------------------

class TestCommitImportProject:

    def test_creates_new_project_with_correct_fields(self, tmp_path):
        sfk = _save_paper("2204.00001", "Alpha Paper")
        proj_fk = _make_project("My Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")
        new_fk  = ei.commit_import(archive)

        assert new_fk != proj_fk
        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        assert proj.name        == "My Project"
        assert proj.description == "Test project"
        assert proj.color       == 0x5b8dee
        assert set(proj.project_tags) == {"research", "ml"}

    def test_imports_colorless_project(self, tmp_path):
        sfk = _save_paper("2204.00001", "Alpha Paper")
        proj_fk = _project.upsert(_project.ProjectIn(name="No Color", description="", source_fks=[sfk]))
        archive = ei.export_project(proj_fk, tmp_path / "export")
        new_fk  = ei.commit_import(archive)

        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        assert proj.color is None


# ---------------------------------------------------------------------------
# commit_import — paper import paths
# ---------------------------------------------------------------------------

class TestCommitImportPapers:

    def test_new_paper_branch_saves_metadata(self, tmp_path):
        sfk = _save_paper("2204.00007", "New Paper")
        proj_fk = _make_project("New Paper Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        _paper.delete(_paper.Paper(source_id="2204.00007"))

        new_fk = ei.commit_import(archive)

        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        assert len(proj.source_fks) == 1
        paper = _paper.get(_paper.Paper(source_fk=proj.source_fks[0]))
        assert paper
        assert paper.source_id == "2204.00007"
        assert paper.title     == "New Paper"

    def test_merge_preserves_existing_metadata(self, tmp_path):
        sfk = _save_paper("2204.00003", "Gamma Paper")
        proj_fk = _make_project("Gamma Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        with _db._connect() as conn:
            conn.execute("UPDATE PAPER SET TITLE = 'Changed' WHERE SOURCE_ID = '2204.00003'")

        ei.commit_import(archive, on_conflict="merge")

        paper = _paper.get(_paper.Paper(source_id="2204.00003"))
        assert paper
        assert paper.title == "Changed"  # merge skips overwrite

    def test_overwrite_restores_metadata(self, tmp_path):
        sfk = _save_paper("2204.00003", "Gamma Paper")
        proj_fk = _make_project("Gamma Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        with _db._connect() as conn:
            conn.execute("UPDATE PAPER SET TITLE = 'Changed' WHERE SOURCE_ID = '2204.00003'")

        ei.commit_import(archive, on_conflict="overwrite")

        paper = _paper.get(_paper.Paper(source_id="2204.00003"))
        assert paper
        assert paper.title == "Gamma Paper"

    def test_merge_unions_archive_tag_onto_existing_paper(self, tmp_path):
        # Paper has "archive-only" at export time; tag is removed before import so
        # the merge branch must add it back from the archive.
        sfk = _save_paper("2204.00004", "Delta Paper", tags=["archive-only"])
        proj_fk = _make_project("Delta Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        _paper.remove_paper_tags("2204.00004", ["archive-only"])
        _paper.add_paper_tags("2204.00004", ["db-only"])

        ei.commit_import(archive, on_conflict="merge")

        paper = _paper.get(_paper.Paper(source_id="2204.00004"))
        assert paper
        assert "archive-only" in (paper.tags or [])  # added from archive
        assert "db-only"      in (paper.tags or [])  # preserved from DB

    def test_overwrite_applies_archive_tags_additively(self, tmp_path):
        sfk = _save_paper("2204.00010", "Overwrite Tags Paper", tags=["archive-tag"])
        proj_fk = _make_project("Overwrite Tags Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        # Delete and re-import so the paper exists in DB for the overwrite branch
        _paper.delete(_paper.Paper(source_id="2204.00010"))
        ei.commit_import(archive, on_conflict="merge")
        _paper.add_paper_tags("2204.00010", ["db-tag"])

        # Overwrite: repair_paper sets TAGS to the archive list, replacing DB tags
        ei.commit_import(archive, on_conflict="overwrite")

        paper = _paper.get(_paper.Paper(source_id="2204.00010"))
        assert paper
        assert "archive-tag" in  (paper.tags or [])  # archive tag applied
        assert "db-tag"      not in (paper.tags or [])  # overwrite replaces, not unions

    def test_zero_paper_import(self, tmp_path):
        proj_fk = _project.upsert(_project.ProjectIn(name="Empty", description="", source_fks=[]))
        archive = ei.export_project(proj_fk, tmp_path / "export")
        new_fk  = ei.commit_import(archive)

        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        assert proj.source_fks == []

    def test_deserialize_paper_with_updated_date(self, tmp_path):
        # Verify the updated_raw truthy branch in _deserialize_paper
        manifest = {
            "format_version": 1,
            "project": {"name": "Updated", "description": ""},
            "papers": [{
                **_minimal_paper_dict("2204.99998"),
                "updated": "2023-06-15",
            }],
            "notes": [],
        }
        archive = _make_archive(tmp_path, manifest)
        new_fk  = ei.commit_import(archive)

        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        paper = _paper.get(_paper.Paper(source_fk=proj.source_fks[0]))
        assert paper
        assert paper.updated == datetime.date(2023, 6, 15)

    def test_deserialize_paper_published_absent_uses_today(self, tmp_path):
        # Verify the published_raw falsy fallback in _deserialize_paper
        pd = _minimal_paper_dict("2204.99997")
        pd["published"] = None
        manifest = {
            "format_version": 1,
            "project": {"name": "NoPub", "description": ""},
            "papers": [pd],
            "notes": [],
        }
        archive = _make_archive(tmp_path, manifest)
        new_fk  = ei.commit_import(archive)

        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        paper = _paper.get(_paper.Paper(source_fk=proj.source_fks[0]))
        assert paper
        assert paper.published == datetime.date.today()


# ---------------------------------------------------------------------------
# commit_import — note import
# ---------------------------------------------------------------------------

class TestCommitImportNotes:

    def test_notes_recreated_with_title_and_content(self, tmp_path):
        sfk = _save_paper("2204.00002", "Beta Paper")
        proj_fk = _make_project("Note Project", [sfk])
        _StorageNote(source_fk=sfk, project_id=proj_fk, title="My Note", content="Insight.").save()

        archive = ei.export_project(proj_fk, tmp_path / "export")
        new_fk  = ei.commit_import(archive)

        notes = _note.get_many(_note.Notes(project_fk=new_fk))
        assert len(notes) == 1
        assert notes[0].title   == "My Note"
        assert notes[0].content == "Insight."

    def test_note_pinned_to_paper_version(self, tmp_path):
        sfk = _save_paper("2204.00008", "Pinned Paper")
        proj_fk = _make_project("Pinned Project", [sfk])

        paper = _paper.get(_paper.Paper(source_fk=sfk))
        assert paper
        _StorageNote(
            source_fk   = sfk,
            project_id  = proj_fk,
            paper_id_fk = paper.paper_id,
            title       = "Version Note",
            content     = "Pinned to v1.",
        ).save()

        archive = ei.export_project(proj_fk, tmp_path / "export")
        new_fk  = ei.commit_import(archive)

        notes = _note.get_many(_note.Notes(project_fk=new_fk))
        assert len(notes) == 1
        assert notes[0].paper_id_fk

    def test_note_with_missing_paper_source_id_skipped(self, tmp_path):
        # A note dict with no paper_source_id should be silently skipped
        _save_paper("2204.00002", "Beta Paper")
        manifest = {
            "format_version": 1,
            "project": {"name": "Skip Note Project", "description": ""},
            "papers": [_minimal_paper_dict("2204.00002", "Beta Paper")],
            "notes": [
                {"paper_source_id": "", "title": "Empty ID", "content": "X"},
                {"title": "No ID key", "content": "Y"},
            ],
        }
        archive = _make_archive(tmp_path, manifest)
        new_fk  = ei.commit_import(archive)

        notes = _note.get_many(_note.Notes(project_fk=new_fk))
        assert len(notes) == 0

    def test_note_referencing_paper_not_in_project_skipped(self, tmp_path):
        # A note whose paper_source_id is not among the imported papers is skipped
        _save_paper("2204.00002", "Beta Paper")
        manifest = {
            "format_version": 1,
            "project": {"name": "Orphan Note Project", "description": ""},
            "papers": [_minimal_paper_dict("2204.00002", "Beta Paper")],
            "notes": [
                {"paper_source_id": "9999.XXXXX", "title": "Orphan", "content": "Z"},
            ],
        }
        archive = _make_archive(tmp_path, manifest)
        new_fk  = ei.commit_import(archive)

        notes = _note.get_many(_note.Notes(project_fk=new_fk))
        assert len(notes) == 0

    def test_note_pinned_version_absent_from_db_falls_back(self, tmp_path, monkeypatch):
        # When a pinned version is listed but that PAPER row is gone, paper_id stays None
        sfk = _save_paper("2204.00009", "Version Gone Paper")
        proj_fk = _make_project("Version Gone Project", [sfk])

        paper = _paper.get(_paper.Paper(source_fk=sfk))
        assert paper
        _StorageNote(
            source_fk   = sfk,
            project_id  = proj_fk,
            paper_id_fk = paper.paper_id,
            title       = "Pinned",
            content     = "Was pinned.",
        ).save()

        archive = ei.export_project(proj_fk, tmp_path / "export")

        # Simulate the pinned version being missing at import time
        monkeypatch.setattr(_paper, "get_paper", lambda *_: None)

        new_fk = ei.commit_import(archive)

        notes = _note.get_many(_note.Notes(project_fk=new_fk))
        assert len(notes) == 1
        assert notes[0].paper_id_fk is None   # graceful fallback, not an error
        assert notes[0].title == "Pinned"

    def test_export_skips_note_with_unresolvable_source_fk(self, tmp_path, monkeypatch):
        # If get_source_id returns None for a note's source_fk, it is excluded from manifest
        sfk = _save_paper("2204.00002", "Beta Paper")
        proj_fk = _make_project("Dangle Project", [sfk])
        _StorageNote(source_fk=sfk, project_id=proj_fk, title="Keep", content="A").save()
        _StorageNote(source_fk=sfk, project_id=proj_fk, title="Drop", content="B").save()

        original_get_source_id = _paper.get_source_id
        call_count = [0]
        def _fake_get_source_id(fk):
            call_count[0] += 1
            return None if call_count[0] == 2 else original_get_source_id(fk)

        monkeypatch.setattr(_paper, "get_source_id", _fake_get_source_id)

        archive = ei.export_project(proj_fk, tmp_path / "export")

        with zipfile.ZipFile(archive) as zf:
            manifest = json.loads(zf.read("manifest.json"))

        assert len(manifest["notes"]) == 1
        assert manifest["notes"][0]["title"] == "Keep"


# ---------------------------------------------------------------------------
# commit_import — PDF import
# ---------------------------------------------------------------------------

class TestCommitImportPdfs:

    def test_pdf_extracted_to_disk_and_db_updated(self, tmp_path, monkeypatch):
        sfk = _save_paper("2204.00005", "Epsilon Paper")
        proj_fk = _make_project("PDF Project", [sfk])

        fake_pdf = tmp_path / "src_pdfs" / "epsilon.pdf"
        fake_pdf.parent.mkdir(parents=True, exist_ok=True)
        fake_pdf.write_bytes(b"%PDF-fake")
        _paper.set_pdf_path("2204.00005", str(fake_pdf))
        _paper.set_has_pdf("2204.00005", 1, True)

        archive = ei.export_project(proj_fk, tmp_path / "export", include_pdfs=True)

        dest_dir = tmp_path / "imported_pdfs"
        monkeypatch.setattr(ei, "pdf_dir", lambda: dest_dir)

        _paper.delete(_paper.Paper(source_id="2204.00005"))
        new_fk = ei.commit_import(archive)

        assert any(dest_dir.iterdir())

        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        paper = _paper.get(_paper.Paper(source_fk=proj.source_fks[0]))
        assert paper
        assert paper.has_pdf  is True
        assert paper.pdf_path
        assert paper.pdf_path.startswith(str(dest_dir))  # landed in the right directory

    def test_pdf_with_no_v_separator_in_name_is_skipped(self, tmp_path, monkeypatch):
        sfk = _save_paper("2204.00005", "Epsilon Paper")
        proj_fk = _make_project("PDF Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        with zipfile.ZipFile(archive, "a") as zf:
            zf.writestr("pdfs/badname.pdf", b"%PDF-bad")

        dest_dir = tmp_path / "imported_pdfs"
        monkeypatch.setattr(ei, "pdf_dir", lambda: dest_dir)

        ei.commit_import(archive)
        assert not any(dest_dir.iterdir())

    def test_pdf_unknown_source_id_is_skipped(self, tmp_path, monkeypatch):
        sfk = _save_paper("2204.00005", "Epsilon Paper")
        proj_fk = _make_project("PDF Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        with zipfile.ZipFile(archive, "a") as zf:
            zf.writestr("pdfs/9999.99999_v1.pdf", b"%PDF-unknown")

        dest_dir = tmp_path / "imported_pdfs"
        monkeypatch.setattr(ei, "pdf_dir", lambda: dest_dir)

        ei.commit_import(archive)
        assert not any(dest_dir.iterdir())

    def test_pdf_non_integer_version_falls_back_to_1_and_updates_db(self, tmp_path, monkeypatch):
        sfk = _save_paper("2204.00005", "Epsilon Paper")
        proj_fk = _make_project("PDF Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        with zipfile.ZipFile(archive, "a") as zf:
            zf.writestr("pdfs/2204.00005_vabc.pdf", b"%PDF-bad-version")

        dest_dir = tmp_path / "imported_pdfs"
        monkeypatch.setattr(ei, "pdf_dir", lambda: dest_dir)

        ei.commit_import(archive)

        # Fallback to version=1 means mark_pdf_saved("2204.00005", ..., 1) was called
        paper = _paper.get(_paper.Paper(source_id="2204.00005"))
        assert paper
        assert paper.has_pdf is True
        assert paper.pdf_path

    def test_pdf_for_missing_version_is_skipped_and_file_removed(self, tmp_path, monkeypatch):
        sfk = _save_paper("2204.00005", "Epsilon Paper")  # version 1 only
        proj_fk = _make_project("PDF Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        with zipfile.ZipFile(archive, "a") as zf:
            zf.writestr("pdfs/2204.00005_v99.pdf", b"%PDF-missing-version")

        dest_dir = tmp_path / "imported_pdfs"
        monkeypatch.setattr(ei, "pdf_dir", lambda: dest_dir)

        ei.commit_import(archive)  # must not raise ProjectImportError

        assert not (dest_dir / "2204.00005_v99.pdf").exists()
        paper = _paper.get(_paper.Paper(source_id="2204.00005"))
        assert paper
        assert paper.has_pdf is False


# ---------------------------------------------------------------------------
# commit_import — rollback on failure
# ---------------------------------------------------------------------------

class TestCommitImportRollback:

    def test_failure_rolls_back_project(self, tmp_path, monkeypatch):
        sfk = _save_paper("2204.00006", "Zeta Paper")
        proj_fk = _make_project("Zeta Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        monkeypatch.setattr(_paper, "get_paper_root", lambda *_: (_ for _ in ()).throw(RuntimeError("boom")))

        active = _project.Projects(status=Status.ACTIVE)
        before_ids = {p.id for p in _project.get_many(active)}

        with pytest.raises(ei.ProjectImportError, match="boom"):
            ei.commit_import(archive)

        after_ids = {p.id for p in _project.get_many(active)}
        assert after_ids == before_ids  # no new ACTIVE project left

    def test_unresolved_link_ids_fail_import_and_roll_back(self, tmp_path, monkeypatch):
        sfk = _save_paper("2204.00006", "Zeta Paper")
        proj_fk = _make_project("Zeta Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        # Force the membership seam to report every id as unresolved.
        monkeypatch.setattr(_project, "add_papers", lambda fk, ids: list(ids))

        active = _project.Projects(status=Status.ACTIVE)
        before_ids = {p.id for p in _project.get_many(active)}

        with pytest.raises(ei.ProjectImportError, match="could not be linked"):
            ei.commit_import(archive)

        after_ids = {p.id for p in _project.get_many(active)}
        assert after_ids == before_ids  # imported project rolled back

    def test_raises_project_import_error_not_bare_exception(self, tmp_path, monkeypatch):
        sfk = _save_paper("2204.00006", "Zeta Paper")
        proj_fk = _make_project("Zeta Project", [sfk])
        archive = ei.export_project(proj_fk, tmp_path / "export")

        monkeypatch.setattr(_paper, "get_paper_root", lambda *_: (_ for _ in ()).throw(RuntimeError("inner")))

        with pytest.raises(ei.ProjectImportError) as exc_info:
            ei.commit_import(archive)
        assert isinstance(exc_info.value.__cause__, RuntimeError)


# ---------------------------------------------------------------------------
# export_project — edge cases
# ---------------------------------------------------------------------------

class TestExportProject:

    def test_nonexistent_project_raises(self, tmp_path):
        with pytest.raises(ValueError, match="not found"):
            ei.export_project(99999, tmp_path / "export")

    def test_missing_pdf_silently_excluded(self, tmp_path):
        sfk = _save_paper("2204.00009", "Ghost PDF Paper")
        proj_fk = _make_project("Ghost Project", [sfk])
        _paper.set_pdf_path("2204.00009", "/nonexistent/path/paper.pdf")
        _paper.set_has_pdf("2204.00009", 1, True)

        archive = ei.export_project(proj_fk, tmp_path / "export", include_pdfs=True)

        with zipfile.ZipFile(archive, "r") as zf:
            assert not any(n.startswith("pdfs/") for n in zf.namelist())

    def test_appends_lxproj_extension(self, tmp_path):
        sfk = _save_paper("2204.00001", "Alpha Paper")
        proj_fk = _make_project("My Project", [sfk])
        path = ei.export_project(proj_fk, tmp_path / "export_no_ext")
        assert path.suffix == ".lxproj"

    def test_serialize_paper_with_updated_date(self, tmp_path):
        # Exercises the truthy arm of `p.updated.isoformat() if p.updated else None`
        from sources.base import PaperMetadata
        meta = PaperMetadata(
            source_id = "2204.00020",
            version   = 1,
            title     = "Updated Paper",
            authors   = ["Author"],
            published = datetime.date(2023, 1, 1),
            updated   = datetime.date(2023, 6, 15),
            summary   = "Has an updated date.",
            source    = "arxiv",
        )
        _paper.save_paper_metadata(meta, None)
        sfk = _paper.ensure_paper_root("2204.00020")
        proj_fk = _make_project("Updated Project", [sfk])

        archive = ei.export_project(proj_fk, tmp_path / "export")

        with zipfile.ZipFile(archive) as zf:
            manifest = json.loads(zf.read("manifest.json"))
        assert manifest["papers"][0]["updated"] == "2023-06-15"

    def test_pdf_path_none_skipped_inside_include_pdfs(self, tmp_path):
        # Paper with no pdf_path should be silently skipped in the pdfs loop
        sfk = _save_paper("2204.00021", "No PDF Paper")
        proj_fk = _make_project("No PDF Project", [sfk])
        # pdf_path is None by default — do not call set_pdf_path

        archive = ei.export_project(proj_fk, tmp_path / "export", include_pdfs=True)

        with zipfile.ZipFile(archive) as zf:
            assert not any(n.startswith("pdfs/") for n in zf.namelist())

    def test_relative_pdf_path_resolved_against_pdf_dir(self, tmp_path, monkeypatch):
        # When pdf_path is stored relative, export should resolve it against pdf_dir()
        sfk = _save_paper("2204.00022", "Relative PDF Paper")
        proj_fk = _make_project("Relative PDF Project", [sfk])

        src_dir = tmp_path / "src_pdfs"
        src_dir.mkdir()
        (src_dir / "relative.pdf").write_bytes(b"%PDF-relative")

        monkeypatch.setattr(ei, "pdf_dir", lambda: src_dir)
        _paper.set_pdf_path("2204.00022", "relative.pdf")  # store as relative
        _paper.set_has_pdf("2204.00022", 1, True)

        archive = ei.export_project(proj_fk, tmp_path / "export", include_pdfs=True)

        with zipfile.ZipFile(archive) as zf:
            assert any(n.startswith("pdfs/") for n in zf.namelist())

    def test_deserialize_paper_missing_optional_keys_use_defaults(self, tmp_path):
        # Keys absent entirely (not present-but-None) should fall back to defaults
        manifest = {
            "format_version": 1,
            "project": {"name": "Sparse", "description": ""},
            "papers": [{"source_id": "2204.00023", "title": "Sparse Paper", "published": "2023-01-01"}],
            "notes": [],
        }
        archive = _make_archive(tmp_path, manifest)
        new_fk  = ei.commit_import(archive)

        proj = _project.get(_project.Project(project_fk=new_fk))
        assert proj
        paper = _paper.get(_paper.Paper(source_fk=proj.source_fks[0]))
        assert paper
        assert paper.version == 1         # pd.get("version", 1) default
        assert paper.authors == []        # pd.get("authors", []) default
        assert (paper.summary or "") == ""  # pd.get("summary", "") default
