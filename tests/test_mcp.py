"""Tests for the MCP tool layer (linxiv_mcp.py) — membership and import guards.

The real `mcp` package is not a test dependency, so a minimal FastMCP
stand-in is registered before import; its tool() decorator returns the
function unchanged, so the tools are callable as plain functions.
"""
from __future__ import annotations

import importlib
import sys
import types

import pytest

import storage.db as _db
import service.project as _svc_project


def _ensure_mcp_importable() -> None:
    # The stubs stay registered in sys.modules for the whole session, like
    # linxiv_mcp itself once imported. Nothing else in the repo imports the
    # `mcp` package, so no other test observes them.
    # importlib.import_module rather than a bare `import mcp...` so pyright
    # doesn't try (and fail) to statically resolve the optional dependency.
    # Succeeds both for a real install and for the stub already in sys.modules.
    try:
        importlib.import_module("mcp.server.fastmcp")
        return
    except ModuleNotFoundError:
        pass

    class _FastMCP:
        def __init__(self, *args, **kwargs):
            pass

        def tool(self, *args, **kwargs):
            def deco(fn):
                return fn
            return deco

    root = types.ModuleType("mcp")
    server = types.ModuleType("mcp.server")
    fastmcp = types.ModuleType("mcp.server.fastmcp")
    setattr(fastmcp, "FastMCP", _FastMCP)
    setattr(root, "server", server)
    setattr(server, "fastmcp", fastmcp)
    sys.modules.setdefault("mcp", root)
    sys.modules.setdefault("mcp.server", server)
    sys.modules.setdefault("mcp.server.fastmcp", fastmcp)


@pytest.fixture()
def mcp_mod(tmp_db):
    # Imported lazily (and only once per session) so linxiv_mcp's import-time
    # DB init runs against the tmp_db-patched connections, not the real data dir.
    _ensure_mcp_importable()
    import linxiv_mcp
    return linxiv_mcp


def _make_project(name: str = "MCP P") -> int:
    return _svc_project.create(_svc_project.ProjectIn(name=name))


def _root(source_id: str = "2204.12985") -> None:
    with _db._connect() as conn:
        conn.execute(
            "INSERT OR IGNORE INTO PAPER_ROOTS (SOURCE_ID) VALUES (?)", (source_id,)
        )


class TestMcpProjectMembership:
    def test_add_paper_happy_path(self, mcp_mod):
        pid = _make_project()
        _root("2204.12985")
        out = mcp_mod.add_paper_to_project(pid, "2204.12985")
        assert out == {"project_id": pid, "paper_id": "2204.12985", "paper_count": 1}

    def test_remove_paper_happy_path(self, mcp_mod):
        pid = _make_project()
        _root("2204.12985")
        mcp_mod.add_paper_to_project(pid, "2204.12985")
        out = mcp_mod.remove_paper_from_project(pid, "2204.12985")
        assert out["paper_count"] == 0

    def test_add_paper_unknown_project_raises(self, mcp_mod):
        _root("2204.12985")
        with pytest.raises(ValueError, match="not found"):
            mcp_mod.add_paper_to_project(9999, "2204.12985")

    def test_add_paper_unknown_paper_raises(self, mcp_mod):
        pid = _make_project()
        with pytest.raises(ValueError, match="not found in database"):
            mcp_mod.add_paper_to_project(pid, "no.such.id")

    def test_remove_paper_unknown_paper_raises(self, mcp_mod):
        pid = _make_project()
        with pytest.raises(ValueError, match="not found in database"):
            mcp_mod.remove_paper_from_project(pid, "no.such.id")

    def test_add_paper_deleted_project_raises(self, mcp_mod):
        pid = _make_project()
        _svc_project.delete(_svc_project.Project(project_fk=pid))
        _root("2204.12985")
        with pytest.raises(ValueError, match="deleted"):
            mcp_mod.add_paper_to_project(pid, "2204.12985")


class TestMcpImportGuards:
    def test_import_bibtex_unknown_project_saves_nothing(self, mcp_mod, tmp_path, minimal_bib):
        bib = tmp_path / "refs.bib"
        bib.write_text(minimal_bib)
        with pytest.raises(ValueError, match="not found"):
            mcp_mod.import_bibtex(str(bib), project_id=9999)
        assert _db.get_paper_root("smith2020") is None

    def test_import_bibtex_deleted_project_saves_nothing(self, mcp_mod, tmp_path, minimal_bib):
        pid = _make_project("Bin")
        _svc_project.delete(_svc_project.Project(project_fk=pid))
        bib = tmp_path / "refs.bib"
        bib.write_text(minimal_bib)
        with pytest.raises(ValueError, match="deleted"):
            mcp_mod.import_bibtex(str(bib), project_id=pid)
        assert _db.get_paper_root("smith2020") is None

    def test_import_bibtex_links_to_project(self, mcp_mod, tmp_path, minimal_bib):
        pid = _make_project("Bib")
        bib = tmp_path / "refs.bib"
        bib.write_text(minimal_bib)
        out = mcp_mod.import_bibtex(str(bib), project_id=pid)
        assert out["imported"] == 1
        details = _svc_project.get(_svc_project.Project(project_fk=pid))
        assert details is not None
        assert len(details.source_fks) == 1

    def test_import_pdf_guard_runs_before_file_read(self, mcp_mod, tmp_path):
        # The membership guard fires before the file is opened, so a missing
        # file with a bad project id reports the project, not the file.
        missing = tmp_path / "missing.pdf"
        with pytest.raises(ValueError, match="not found"):
            mcp_mod.import_pdf(str(missing), project_id=9999)
