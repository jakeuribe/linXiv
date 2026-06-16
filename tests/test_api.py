"""Integration tests for the FastAPI API endpoints (api/app.py).

Uses FastAPI TestClient with a patched in-memory SQLite DB so no real
database or external network calls are made (external calls are mocked).
"""

from __future__ import annotations

import datetime
import os
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from sources.base import PaperMetadata


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture()
def client(tmp_path, monkeypatch):
    """TestClient wired to a fresh temp SQLite DB for each test."""
    import storage.db as db
    import storage.projects as projects
    import storage.notes as notes

    db_file = str(tmp_path / "test.db")
    real_connect = db._connect

    def patched_connect(db_path=None):
        del db_path
        return real_connect(db_file)

    monkeypatch.setattr(db, "_connect", patched_connect)
    monkeypatch.setattr(projects, "_connect", patched_connect)
    monkeypatch.setattr(notes, "_connect", patched_connect)

    from api.app import app

    with TestClient(app) as c:
        yield c


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _meta(**kwargs) -> PaperMetadata:
    return PaperMetadata(
        source_id=kwargs.get("source_id", "2204.12985"),
        version=kwargs.get("version", 1),
        title=kwargs.get("title", "Test Paper"),
        authors=kwargs.get("authors", ["Author One"]),
        published=kwargs.get("published", datetime.date(2022, 4, 1)),
        summary=kwargs.get("summary", "Abstract text."),
        category=kwargs.get("category", "cs.AI"),
        doi=kwargs.get("doi", None),
        url=kwargs.get("url", None),
        source=kwargs.get("source", "arxiv"),
    )


# ---------------------------------------------------------------------------
# Root / Health
# ---------------------------------------------------------------------------

class TestRootAndHealth:
    def test_root_returns_service_info(self, client):
        r = client.get("/")
        assert r.status_code == 200
        data = r.json()
        assert data["service"] == "linXiv"
        assert "/docs" in data["docs"]

    def test_health(self, client):
        r = client.get("/api/health")
        assert r.status_code == 200
        data = r.json()
        assert data["ok"] is True
        assert data["service"] == "linxiv-api"


# ---------------------------------------------------------------------------
# Stats
# ---------------------------------------------------------------------------

class TestStats:
    def test_stats_empty_db(self, client):
        r = client.get("/api/stats")
        assert r.status_code == 200
        data = r.json()
        assert data["paper_count"] == 0
        assert data["tag_count"] == 0
        assert data["recent_papers"] == []

    def test_stats_counts_saved_paper(self, client):
        import storage.db as db
        db.save_paper_metadata(_meta())
        r = client.get("/api/stats")
        assert r.status_code == 200
        data = r.json()
        assert data["paper_count"] == 1
        assert len(data["recent_papers"]) == 1


# ---------------------------------------------------------------------------
# Papers
# ---------------------------------------------------------------------------

class TestPapers:
    def test_list_empty(self, client):
        r = client.get("/api/papers")
        assert r.status_code == 200
        assert r.json() == {"papers": []}

    def test_get_nonexistent_returns_404(self, client):
        r = client.get("/api/papers/9999.00001")
        assert r.status_code == 404

    def test_delete_nonexistent_returns_404(self, client):
        r = client.delete("/api/papers/9999.00001")
        assert r.status_code == 404

    def test_list_and_get_saved_paper(self, client):
        import storage.db as db
        db.save_paper_metadata(_meta(source_id="2204.12985"))

        r = client.get("/api/papers")
        assert r.status_code == 200
        papers = r.json()["papers"]
        assert len(papers) == 1
        assert papers[0]["source_id"] == "2204.12985"

        r = client.get("/api/papers/2204.12985")
        assert r.status_code == 200
        assert r.json()["title"] == "Test Paper"

    def test_delete_removes_paper(self, client):
        import storage.db as db
        db.save_paper_metadata(_meta(source_id="2204.12985"))

        r = client.delete("/api/papers/2204.12985")
        assert r.status_code == 200
        assert r.json()["deleted"] == "2204.12985"

        assert client.get("/api/papers/2204.12985").status_code == 404

    def test_list_pagination(self, client):
        import storage.db as db
        for i in range(5):
            db.save_paper_metadata(_meta(source_id=f"2204.{10000 + i}"))
        r = client.get("/api/papers?limit=2&offset=0")
        assert r.status_code == 200
        assert len(r.json()["papers"]) == 2

    def test_list_offset(self, client):
        import storage.db as db
        for i in range(3):
            db.save_paper_metadata(_meta(source_id=f"2204.{10000 + i}"))
        r = client.get("/api/papers?limit=10&offset=2")
        assert r.status_code == 200
        assert len(r.json()["papers"]) == 1


# ---------------------------------------------------------------------------
# Categories / Tags
# ---------------------------------------------------------------------------

class TestCategoriesAndTags:
    def test_categories_returns_list(self, client):
        r = client.get("/api/categories")
        assert r.status_code == 200
        assert "categories" in r.json()

    def test_tags_empty(self, client):
        r = client.get("/api/tags")
        assert r.status_code == 200
        assert r.json() == {"tags": []}


# ---------------------------------------------------------------------------
# Graph
# ---------------------------------------------------------------------------

class TestGraph:
    def test_graph_returns_nodes_and_edges(self, client):
        r = client.get("/api/graph")
        assert r.status_code == 200
        data = r.json()
        assert "nodes" in data
        assert "edges" in data

    def test_graph_project_options(self, client):
        r = client.get("/api/graph/project-options")
        assert r.status_code == 200
        assert "projects" in r.json()

    def test_graph_project_options_includes_tags_from_join_table(self, client):
        import storage.tags as _tags
        pid = client.post("/api/projects", json={"name": "Graph Tagged"}).json()["project"]["id"]
        _tags.add_project_tags(pid, ["graph-tag"])
        r = client.get("/api/graph/project-options")
        assert r.status_code == 200
        projects = r.json()["projects"]
        tagged = next((p for p in projects if p["id"] == pid), None)
        assert tagged
        assert "graph-tag" in tagged["tags"]


# ---------------------------------------------------------------------------
# Projects CRUD
# ---------------------------------------------------------------------------

class TestProjects:
    def test_list_empty(self, client):
        r = client.get("/api/projects")
        assert r.status_code == 200
        assert r.json() == {"projects": []}

    def test_create_project(self, client):
        r = client.post("/api/projects", json={"name": "My Project"})
        assert r.status_code == 200
        data = r.json()["project"]
        assert data["name"] == "My Project"
        assert isinstance(data["id"], int)

    def test_create_project_with_color(self, client):
        r = client.post("/api/projects", json={"name": "Colorful", "color_hex": "#ff5733"})
        assert r.status_code == 200

    def test_create_project_empty_name_fails(self, client):
        r = client.post("/api/projects", json={"name": ""})
        assert r.status_code == 422

    def test_get_nonexistent_project_returns_404(self, client):
        r = client.get("/api/projects/999")
        assert r.status_code == 404

    def test_get_project(self, client):
        pid = client.post("/api/projects", json={"name": "Alpha"}).json()["project"]["id"]
        r = client.get(f"/api/projects/{pid}")
        assert r.status_code == 200
        data = r.json()
        assert data["name"] == "Alpha"
        assert data["id"] == pid

    def test_list_includes_created_project(self, client):
        client.post("/api/projects", json={"name": "Listed"})
        r = client.get("/api/projects")
        assert r.status_code == 200
        names = [p["name"] for p in r.json()["projects"]]
        assert "Listed" in names

    def test_patch_name(self, client):
        pid = client.post("/api/projects", json={"name": "Old"}).json()["project"]["id"]
        assert client.patch(f"/api/projects/{pid}", json={"name": "New"}).status_code == 200
        assert client.get(f"/api/projects/{pid}").json()["name"] == "New"

    def test_patch_description(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        client.patch(f"/api/projects/{pid}", json={"description": "A description"})
        assert client.get(f"/api/projects/{pid}").json()["description"] == "A description"

    def test_patch_invalid_status_returns_400(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        r = client.patch(f"/api/projects/{pid}", json={"status": "not_a_status"})
        assert r.status_code == 400

    def test_get_project_returns_project_tags_from_join_table(self, client):
        import storage.tags as _tags
        pid = client.post("/api/projects", json={"name": "Tagged"}).json()["project"]["id"]
        _tags.add_project_tags(pid, ["science", "ml"])
        r = client.get(f"/api/projects/{pid}")
        assert r.status_code == 200
        assert set(r.json()["project_tags"]) == {"science", "ml"}

    def test_list_projects_returns_project_tags_from_join_table(self, client):
        import storage.tags as _tags
        pid = client.post("/api/projects", json={"name": "Listed Tagged"}).json()["project"]["id"]
        _tags.add_project_tags(pid, ["graph"])
        r = client.get("/api/projects")
        assert r.status_code == 200
        projects = r.json()["projects"]
        tagged = next(p for p in projects if p["id"] == pid)
        assert tagged["project_tags"] == ["graph"]

    def test_patch_project_tags_persists(self, client):
        pid = client.post("/api/projects", json={"name": "Tagged"}).json()["project"]["id"]
        r = client.patch(f"/api/projects/{pid}", json={"project_tags": ["ml", "nlp"]})
        assert r.status_code == 200
        assert set(client.get(f"/api/projects/{pid}").json()["project_tags"]) == {"ml", "nlp"}

    def test_patch_project_tags_replaces_existing(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        r1 = client.patch(f"/api/projects/{pid}", json={"project_tags": ["old"]})
        assert r1.status_code == 200
        client.patch(f"/api/projects/{pid}", json={"project_tags": ["new"]})
        assert set(client.get(f"/api/projects/{pid}").json()["project_tags"]) == {"new"}

    def test_patch_project_tags_empty_clears_all(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        client.patch(f"/api/projects/{pid}", json={"project_tags": ["ml"]})
        assert set(client.get(f"/api/projects/{pid}").json()["project_tags"]) == {"ml"}
        client.patch(f"/api/projects/{pid}", json={"project_tags": []})
        assert client.get(f"/api/projects/{pid}").json()["project_tags"] == []

    def test_patch_project_tags_preserves_case(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        client.patch(f"/api/projects/{pid}", json={"project_tags": ["ML", "NLP"]})
        assert set(client.get(f"/api/projects/{pid}").json()["project_tags"]) == {"ML", "NLP"}

    def test_patch_project_tags_api_normalizes_and_deduplicates(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        client.patch(f"/api/projects/{pid}", json={"project_tags": ["ml", "ml", "ML"]})
        assert set(client.get(f"/api/projects/{pid}").json()["project_tags"]) == {"ml"}

    def test_patch_nonexistent_returns_404(self, client):
        r = client.patch("/api/projects/999", json={"name": "X"})
        assert r.status_code == 404

    def test_delete_project(self, client):
        pid = client.post("/api/projects", json={"name": "Temp"}).json()["project"]["id"]
        assert client.delete(f"/api/projects/{pid}").status_code == 200
        # delete() is a soft-delete; project is still fetchable with status="deleted"
        r = client.get(f"/api/projects/{pid}")
        assert r.status_code == 200
        assert r.json()["status"] == "deleted"

    def test_delete_nonexistent_returns_404(self, client):
        r = client.delete("/api/projects/999")
        assert r.status_code == 404


# ---------------------------------------------------------------------------
# Project–Paper relationships
# ---------------------------------------------------------------------------

class TestProjectPapers:
    def _setup(self, client):
        import storage.db as db
        db.save_paper_metadata(_meta(source_id="2204.12985"))
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        return pid

    def test_add_paper_to_project(self, client):
        pid = self._setup(client)
        r = client.post(f"/api/projects/{pid}/papers", json={"source_id": "2204.12985"})
        assert r.status_code == 200
        assert r.json() == {"ok": True}
        assert "2204.12985" in client.get(f"/api/projects/{pid}").json()["source_ids"]

    def test_remove_paper_from_project(self, client):
        pid = self._setup(client)
        client.post(f"/api/projects/{pid}/papers", json={"source_id": "2204.12985"})
        r = client.delete(f"/api/projects/{pid}/papers/2204.12985")
        assert r.status_code == 200
        assert "2204.12985" not in client.get(f"/api/projects/{pid}").json()["source_ids"]

    def test_add_to_nonexistent_project_returns_404(self, client):
        r = client.post("/api/projects/999/papers", json={"source_id": "2204.12985"})
        assert r.status_code == 404

    def test_bulk_add_papers(self, client):
        import storage.db as db
        pid = self._setup(client)
        db.save_paper_metadata(_meta(source_id="2301.00001"))
        r = client.post(
            f"/api/projects/{pid}/papers/bulk",
            json={"source_ids": ["2204.12985", "2301.00001"]},
        )
        assert r.status_code == 200
        assert r.json() == {"ok": True, "failed": []}
        source_ids = client.get(f"/api/projects/{pid}").json()["source_ids"]
        assert "2204.12985" in source_ids
        assert "2301.00001" in source_ids

    def test_bulk_add_reports_unknown_papers_but_adds_rest(self, client):
        pid = self._setup(client)
        r = client.post(
            f"/api/projects/{pid}/papers/bulk",
            json={"source_ids": ["2204.12985", "9999.99999"]},
        )
        assert r.status_code == 200
        assert r.json() == {"ok": False, "failed": ["9999.99999"]}
        assert "2204.12985" in client.get(f"/api/projects/{pid}").json()["source_ids"]

    def test_bulk_add_to_nonexistent_project_returns_404(self, client):
        r = client.post(
            "/api/projects/999/papers/bulk", json={"source_ids": ["2204.12985"]}
        )
        assert r.status_code == 404

    def test_bulk_add_empty_list_returns_422(self, client):
        pid = self._setup(client)
        r = client.post(f"/api/projects/{pid}/papers/bulk", json={"source_ids": []})
        assert r.status_code == 422

    def test_remove_from_nonexistent_project_returns_404(self, client):
        r = client.delete("/api/projects/999/papers/2204.12985")
        assert r.status_code == 404

    def test_add_to_deleted_project_returns_400(self, client):
        pid = self._setup(client)
        client.delete(f"/api/projects/{pid}")
        r = client.post(f"/api/projects/{pid}/papers", json={"source_id": "2204.12985"})
        assert r.status_code == 400

    def test_bulk_add_to_deleted_project_returns_400(self, client):
        pid = self._setup(client)
        client.delete(f"/api/projects/{pid}")
        r = client.post(
            f"/api/projects/{pid}/papers/bulk", json={"source_ids": ["2204.12985"]}
        )
        assert r.status_code == 400

    def test_remove_from_deleted_project_returns_400(self, client):
        pid = self._setup(client)
        client.post(f"/api/projects/{pid}/papers", json={"source_id": "2204.12985"})
        client.delete(f"/api/projects/{pid}")
        r = client.delete(f"/api/projects/{pid}/papers/2204.12985")
        assert r.status_code == 400


# ---------------------------------------------------------------------------
# BibTeX import
# ---------------------------------------------------------------------------

class TestBibtexImport:
    def _post(self, client, bib: str, project_id=None):
        params = {} if project_id is None else {"project_id": project_id}
        return client.post(
            "/api/papers/import/bibtex",
            params=params,
            files={"file": ("refs.bib", bib.encode(), "text/x-bibtex")},
        )

    def test_import_without_project(self, client, minimal_bib):
        r = self._post(client, minimal_bib)
        assert r.status_code == 200
        assert r.json() == {"saved_count": 1, "source_ids": ["smith2020"]}

    def test_import_links_to_project(self, client, minimal_bib):
        pid = client.post("/api/projects", json={"name": "Bib"}).json()["project"]["id"]
        r = self._post(client, minimal_bib, project_id=pid)
        assert r.status_code == 200
        assert "smith2020" in client.get(f"/api/projects/{pid}").json()["source_ids"]

    def test_import_to_unknown_project_returns_404_and_saves_nothing(self, client, minimal_bib):
        import storage.db as db
        r = self._post(client, minimal_bib, project_id=999)
        assert r.status_code == 404
        assert db.get_paper_root("smith2020") is None

    def test_import_to_deleted_project_returns_400_and_saves_nothing(self, client, minimal_bib):
        import storage.db as db
        pid = client.post("/api/projects", json={"name": "Bin"}).json()["project"]["id"]
        client.delete(f"/api/projects/{pid}")
        r = self._post(client, minimal_bib, project_id=pid)
        assert r.status_code == 400
        assert db.get_paper_root("smith2020") is None


# ---------------------------------------------------------------------------
# PDF import — link-failure boundary
# ---------------------------------------------------------------------------

class TestPdfImportLinkFailure:
    def test_link_failure_returns_400_naming_the_imported_paper(self, client, monkeypatch):
        # Simulates the project vanishing between import_pdf's pre-guard and
        # its post-import link step.
        from service.paper import PaperLinkError

        def _raise(content, project_id=None):
            raise PaperLinkError(
                "paper local:abc was imported but could not be linked to project 1: gone"
            )

        monkeypatch.setattr("api.app.svc_import_pdf", _raise)
        r = client.post(
            "/api/papers/import/pdf",
            params={"project_id": 1},
            files={"file": ("p.pdf", b"%PDF-1.4 fake", "application/pdf")},
        )
        assert r.status_code == 400
        assert "was imported but could not be linked" in r.json()["detail"]


# ---------------------------------------------------------------------------
# Notes
# ---------------------------------------------------------------------------

class TestNotes:
    def test_get_notes_empty(self, client):
        r = client.get("/api/notes?source_id=2204.12985")
        assert r.status_code == 200
        assert r.json() == {"notes": []}

    def test_create_and_get_note(self, client):
        r = client.post("/api/notes", json={
            "source_id": "2204.12985",
            "title": "My Note",
            "content": "Some content",
        })
        assert r.status_code == 200
        assert isinstance(r.json()["id"], int)

        notes = client.get("/api/notes?source_id=2204.12985").json()["notes"]
        assert len(notes) == 1
        assert notes[0]["title"] == "My Note"
        assert notes[0]["content"] == "Some content"
        assert notes[0]["created_at"]

    def test_note_with_project(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        client.post("/api/notes", json={"source_id": "2204.12985", "project_id": pid, "title": "T"})

        notes = client.get(f"/api/notes?source_id=2204.12985&project_id={pid}").json()["notes"]
        assert len(notes) == 1
        assert notes[0]["project_id"] == pid

    def test_all_projects_flag_returns_all_notes(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        client.post("/api/notes", json={"source_id": "X", "title": "No project"})
        client.post("/api/notes", json={"source_id": "X", "project_id": pid, "title": "With project"})

        r = client.get("/api/notes?source_id=X&all_projects=true")
        assert r.status_code == 200
        assert len(r.json()["notes"]) == 2

    def test_notes_isolated_by_paper(self, client):
        client.post("/api/notes", json={"source_id": "A", "title": "Note A"})
        client.post("/api/notes", json={"source_id": "B", "title": "Note B"})
        assert len(client.get("/api/notes?source_id=A").json()["notes"]) == 1
        assert len(client.get("/api/notes?source_id=B").json()["notes"]) == 1


# ---------------------------------------------------------------------------
# arXiv endpoints (external calls mocked)
# ---------------------------------------------------------------------------

class TestArxivEndpoints:
    def test_search_success(self, client):
        meta = _meta(source_id="arxiv:2204.12985", title="Mock Paper")
        with patch("api.app._arxiv_source.search", return_value=[meta]):
            r = client.post("/api/arxiv/search", json={"query": "transformers"})
        assert r.status_code == 200
        data = r.json()
        assert len(data["results"]) == 1
        assert data["results"][0]["title"] == "Mock Paper"
        assert data["saved_source_ids"] == []

    def test_search_save_flag(self, client):
        meta = _meta(source_id="arxiv:2204.12985", title="Mock Paper")
        with patch("api.app._arxiv_source.search", return_value=[meta]), \
             patch("api.app.save_papers_metadata", return_value=[("arxiv:2204.12985", 1)]):
            r = client.post("/api/arxiv/search", json={"query": "test", "save": True})
        assert r.status_code == 200
        assert len(r.json()["saved_source_ids"]) == 1

    def test_search_empty_results(self, client):
        with patch("api.app._arxiv_source.search", return_value=[]):
            r = client.post("/api/arxiv/search", json={"query": "xyzxyzxyz"})
        assert r.status_code == 200
        assert r.json()["results"] == []

    def test_search_error_returns_502(self, client):
        with patch("api.app._arxiv_source.search", side_effect=Exception("timeout")):
            r = client.post("/api/arxiv/search", json={"query": "test"})
        assert r.status_code == 502

    def test_search_missing_query_returns_422(self, client):
        r = client.post("/api/arxiv/search", json={})
        assert r.status_code == 422

    def test_fetch_success(self, client):
        meta = _meta(source_id="arxiv:2204.12985", title="Mock Paper")
        with patch("api.app._arxiv_source.fetch_by_id", return_value=meta):
            r = client.post("/api/arxiv/fetch", json={"source_id": "2204.12985", "save": False})
        assert r.status_code == 200
        data = r.json()
        assert data["paper"]["title"] == "Mock Paper"
        assert data["source_id"] == "2204.12985"
        assert data["saved"] is False

    def test_fetch_error_returns_502(self, client):
        with patch("api.app._arxiv_source.fetch_by_id", side_effect=Exception("not found")):
            r = client.post("/api/arxiv/fetch", json={"source_id": "9999.99999"})
        assert r.status_code == 502


# ---------------------------------------------------------------------------
# DOI endpoints (external calls mocked)
# ---------------------------------------------------------------------------

class TestDoiEndpoints:
    _doi = "10.1234/test"

    def test_resolve_success(self, client):
        with patch("api.app.resolve_doi", return_value=_meta(source_id=self._doi)):
            r = client.post("/api/doi/resolve", json={"doi": self._doi})
        assert r.status_code == 200
        assert "metadata" in r.json()
        assert r.json()["metadata"]["source_id"] == self._doi

    def test_resolve_not_found_returns_400(self, client):
        with patch("api.app.resolve_doi", side_effect=ValueError("not found")):
            r = client.post("/api/doi/resolve", json={"doi": "10.bad/doi"})
        assert r.status_code == 400

    def test_resolve_missing_doi_returns_422(self, client):
        r = client.post("/api/doi/resolve", json={})
        assert r.status_code == 422

    def test_save_success(self, client):
        with patch("api.app.resolve_doi", return_value=_meta(source_id=self._doi)):
            r = client.post("/api/doi/save", json={"doi": self._doi})
        assert r.status_code == 200
        data = r.json()
        assert data["saved"] is True
        assert "metadata" in data

    def test_save_not_found_returns_400(self, client):
        with patch("api.app.resolve_doi", side_effect=ValueError("bad doi")):
            r = client.post("/api/doi/save", json={"doi": "10.bad/doi"})
        assert r.status_code == 400


# ---------------------------------------------------------------------------
# PDF endpoint
# ---------------------------------------------------------------------------

class TestPdfEndpoint:
    def test_no_paper_returns_404(self, client):
        r = client.get("/api/papers/9999.00001/pdf")
        assert r.status_code == 404

    def test_paper_with_url_redirects(self, client):
        import storage.db as db
        db.save_paper_metadata(_meta(
            source_id="2204.12985",
            url="https://arxiv.org/pdf/2204.12985v1",
        ))
        r = client.get("/api/papers/2204.12985/pdf", follow_redirects=False)
        assert r.status_code in (301, 302, 303, 307, 308)
        assert "arxiv.org" in r.headers["location"]

    def test_paper_with_local_pdf(self, client, tmp_path):
        import storage.db as db
        pdf_file = tmp_path / "2204.12985v1.pdf"
        pdf_file.write_bytes(b"%PDF-1.4 fake content")

        db.save_paper_metadata(_meta(source_id="2204.12985", url=None))

        with patch("api.app.PDF_DIR", tmp_path):
            r = client.get("/api/papers/2204.12985/pdf")
        assert r.status_code == 200
        assert r.headers["content-type"] == "application/pdf"

    def test_paper_with_no_pdf_source_returns_404(self, client):
        import storage.db as db
        db.save_paper_metadata(_meta(source_id="2204.12985", url=None))
        r = client.get("/api/papers/2204.12985/pdf")
        assert r.status_code == 404


# ---------------------------------------------------------------------------
# Saved-PDF list / delete (Settings → Storage)
# ---------------------------------------------------------------------------

class TestSavedPdfs:
    def test_list_includes_only_papers_with_local_pdf(self, client, tmp_path):
        import storage.db as db
        import service.paper as svc
        (tmp_path / "2204.12985v1.pdf").write_bytes(b"%PDF-1.4 abc")
        (tmp_path / "2305.00003v1.pdf").write_bytes(b"%PDF-1.4 " + b"x" * 500)
        db.save_paper_metadata(_meta(source_id="2204.12985", title="Has PDF"))
        db.save_paper_metadata(_meta(source_id="2305.00003", title="Bigger PDF"))
        db.save_paper_metadata(_meta(source_id="2301.00002", title="No PDF"))
        svc.set_has_pdf_by_source("2204.12985", True)
        svc.set_has_pdf_by_source("2305.00003", True)
        with patch("api.app.PDF_DIR", tmp_path):
            r = client.get("/api/pdfs")
        assert r.status_code == 200
        pdfs = r.json()["pdfs"]
        ids = {p["source_id"] for p in pdfs}
        assert "2204.12985" in ids
        assert "2305.00003" in ids
        assert "2301.00002" not in ids
        entry = next(p for p in pdfs if p["source_id"] == "2204.12985")
        assert entry["title"] == "Has PDF"
        assert entry["size_bytes"] == (tmp_path / "2204.12985v1.pdf").stat().st_size
        assert pdfs[0]["size_bytes"] >= pdfs[1]["size_bytes"]
        assert pdfs[0]["source_id"] == "2305.00003"

    def test_delete_removes_file_and_clears_flag(self, client, tmp_path):
        import storage.db as db
        import service.paper as svc
        (tmp_path / "2204.12985v1.pdf").write_bytes(b"%PDF-1.4 abc")
        db.save_paper_metadata(_meta(source_id="2204.12985", url=None))
        svc.set_has_pdf_by_source("2204.12985", True)
        deleted: list[str] = []
        with patch("api.app.PDF_DIR", tmp_path), patch(
            "api.app.delete_local_pdf",
            side_effect=lambda p: (deleted.append(p), True)[1],
        ):
            r = client.delete("/api/pdfs/2204.12985")
        assert r.status_code == 200
        assert r.json()["deleted"] is True
        assert deleted and deleted[0].endswith("2204.12985v1.pdf")
        assert client.get("/api/papers/2204.12985").json()["has_pdf"] is False

    def test_delete_removes_every_version_file_and_clears_all_flags(self, client, tmp_path):
        import storage.db as db
        (tmp_path / "2204.12985v1.pdf").write_bytes(b"%PDF-1.4 v1")
        (tmp_path / "2204.12985v2.pdf").write_bytes(b"%PDF-1.4 v2")
        db.save_paper_metadata(_meta(source_id="2204.12985", version=1))
        db.save_paper_metadata(_meta(source_id="2204.12985", version=2))
        db.set_has_pdf("2204.12985", 1, True)
        db.set_has_pdf("2204.12985", 2, True)
        deleted: list[str] = []
        with patch("api.app.PDF_DIR", tmp_path), patch(
            "api.app.delete_local_pdf",
            side_effect=lambda p: (deleted.append(p), True)[1],
        ):
            r = client.delete("/api/pdfs/2204.12985")
        assert r.status_code == 200
        assert r.json()["deleted"] is True
        deleted_names = {os.path.basename(p) for p in deleted}
        assert deleted_names == {"2204.12985v1.pdf", "2204.12985v2.pdf"}
        rows = db.get_all_versions("2204.12985")
        flags = {row["version"]: bool(row["has_pdf"]) for row in rows}
        assert flags[1] is False
        assert flags[2] is False

    def test_delete_clears_stale_flag_when_no_file_on_disk(self, client, tmp_path):
        import storage.db as db
        import service.paper as svc
        db.save_paper_metadata(_meta(source_id="2204.12985", url=None))
        svc.set_has_pdf_by_source("2204.12985", True)
        with patch("api.app.PDF_DIR", tmp_path):
            # No file on disk: the stale flag does not put the row in the list.
            before = client.get("/api/pdfs")
            assert before.status_code == 200
            assert "2204.12985" not in {p["source_id"] for p in before.json()["pdfs"]}
            r = client.delete("/api/pdfs/2204.12985")
            assert r.status_code == 200
            assert r.json()["deleted"] is True
            after = client.get("/api/pdfs")
            assert "2204.12985" not in {p["source_id"] for p in after.json()["pdfs"]}
        assert client.get("/api/papers/2204.12985").json()["has_pdf"] is False

    def test_delete_outside_managed_dir_keeps_flag(self, client, tmp_path):
        import storage.db as db
        import service.paper as svc
        (tmp_path / "2204.12985v1.pdf").write_bytes(b"%PDF-1.4 abc")
        db.save_paper_metadata(_meta(source_id="2204.12985", url=None))
        svc.set_has_pdf_by_source("2204.12985", True)
        # delete_local_pdf returns False for a file outside the managed dir.
        with patch("api.app.PDF_DIR", tmp_path), patch(
            "api.app.delete_local_pdf", return_value=False,
        ):
            r = client.delete("/api/pdfs/2204.12985")
        assert r.status_code == 409
        assert client.get("/api/papers/2204.12985").json()["has_pdf"] is True

    def test_delete_clears_pdf_path_in_meta(self, client, tmp_path):
        import storage.db as db
        import service.paper as svc
        f = tmp_path / "2204.12985v1.pdf"
        f.write_bytes(b"%PDF-1.4 abc")
        db.save_paper_metadata(_meta(source_id="2204.12985", url=None))
        svc.set_has_pdf_by_source("2204.12985", True)
        svc.set_pdf_path("2204.12985", str(f), 1)
        assert client.get("/api/papers/2204.12985").json()["pdf_path"] == str(f)
        with patch("api.app.PDF_DIR", tmp_path), patch(
            "api.app.delete_local_pdf", return_value=True,
        ):
            r = client.delete("/api/pdfs/2204.12985")
        assert r.status_code == 200
        assert client.get("/api/papers/2204.12985").json()["pdf_path"] in (None, "")

    def test_delete_mixed_versions_clears_only_deleted_before_409(self, client, tmp_path):
        import storage.db as db
        (tmp_path / "2204.12985v1.pdf").write_bytes(b"%PDF-1.4 v1")
        (tmp_path / "2204.12985v2.pdf").write_bytes(b"%PDF-1.4 v2")
        db.save_paper_metadata(_meta(source_id="2204.12985", version=1))
        db.save_paper_metadata(_meta(source_id="2204.12985", version=2))
        db.set_has_pdf("2204.12985", 1, True)
        db.set_has_pdf("2204.12985", 2, True)

        # v1 deletes cleanly; v2 reports outside the managed dir, raising 409.
        def _delete(path: str) -> bool:
            return not path.endswith("v2.pdf")

        with patch("api.app.PDF_DIR", tmp_path), patch(
            "api.app.delete_local_pdf", side_effect=_delete,
        ):
            r = client.delete("/api/pdfs/2204.12985")
        assert r.status_code == 409
        rows = db.get_all_versions("2204.12985")
        flags = {row["version"]: bool(row["has_pdf"]) for row in rows}
        # v1's file was deleted, so its flag must be cleared even though v2 raised.
        assert flags[1] is False
        assert flags[2] is True

    def test_delete_missing_paper_returns_404(self, client):
        r = client.delete("/api/pdfs/9999.99999")
        assert r.status_code == 404
