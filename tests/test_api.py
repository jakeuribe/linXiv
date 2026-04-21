"""Integration tests for the FastAPI API endpoints (api/app.py).

Uses FastAPI TestClient with a patched in-memory SQLite DB so no real
database or external network calls are made (external calls are mocked).
"""

from __future__ import annotations

import datetime
from unittest.mock import MagicMock, patch

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
        paper_id=kwargs.get("paper_id", "2204.12985"),
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


def _arxiv_result(paper_id="2204.12985", version=1) -> MagicMock:
    r = MagicMock()
    r.entry_id = f"http://arxiv.org/abs/{paper_id}v{version}"
    r.title = "Mock Paper"
    r.summary = "Mock abstract."
    author = MagicMock()
    author.name = "Mock Author"
    r.authors = [author]
    r.published = MagicMock()
    r.published.date.return_value = datetime.date(2022, 4, 1)
    r.pdf_url = f"https://arxiv.org/pdf/{paper_id}v{version}"
    r.primary_category = "cs.AI"
    return r


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
        assert r.json() == {"ok": True}


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
        db.save_paper_metadata(_meta(paper_id="2204.12985"))

        r = client.get("/api/papers")
        assert r.status_code == 200
        papers = r.json()["papers"]
        assert len(papers) == 1
        assert papers[0]["paper_id"] == "2204.12985"

        r = client.get("/api/papers/2204.12985")
        assert r.status_code == 200
        assert r.json()["title"] == "Test Paper"

    def test_delete_removes_paper(self, client):
        import storage.db as db
        db.save_paper_metadata(_meta(paper_id="2204.12985"))

        r = client.delete("/api/papers/2204.12985")
        assert r.status_code == 200
        assert r.json()["deleted"] == "2204.12985"

        assert client.get("/api/papers/2204.12985").status_code == 404

    def test_list_pagination(self, client):
        import storage.db as db
        for i in range(5):
            db.save_paper_metadata(_meta(paper_id=f"2204.{10000 + i}"))
        r = client.get("/api/papers?limit=2&offset=0")
        assert r.status_code == 200
        assert len(r.json()["papers"]) == 2

    def test_list_offset(self, client):
        import storage.db as db
        for i in range(3):
            db.save_paper_metadata(_meta(paper_id=f"2204.{10000 + i}"))
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
        db.save_paper_metadata(_meta(paper_id="2204.12985"))
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        return pid

    def test_add_paper_to_project(self, client):
        pid = self._setup(client)
        r = client.post(f"/api/projects/{pid}/papers", json={"paper_id": "2204.12985"})
        assert r.status_code == 200
        assert r.json() == {"ok": True}
        assert "2204.12985" in client.get(f"/api/projects/{pid}").json()["paper_ids"]

    def test_remove_paper_from_project(self, client):
        pid = self._setup(client)
        client.post(f"/api/projects/{pid}/papers", json={"paper_id": "2204.12985"})
        r = client.delete(f"/api/projects/{pid}/papers/2204.12985")
        assert r.status_code == 200
        assert "2204.12985" not in client.get(f"/api/projects/{pid}").json()["paper_ids"]

    def test_add_to_nonexistent_project_returns_404(self, client):
        r = client.post("/api/projects/999/papers", json={"paper_id": "2204.12985"})
        assert r.status_code == 404

    def test_remove_from_nonexistent_project_returns_404(self, client):
        r = client.delete("/api/projects/999/papers/2204.12985")
        assert r.status_code == 404


# ---------------------------------------------------------------------------
# Notes
# ---------------------------------------------------------------------------

class TestNotes:
    def test_get_notes_empty(self, client):
        r = client.get("/api/notes?paper_id=2204.12985")
        assert r.status_code == 200
        assert r.json() == {"notes": []}

    def test_create_and_get_note(self, client):
        r = client.post("/api/notes", json={
            "paper_id": "2204.12985",
            "title": "My Note",
            "content": "Some content",
        })
        assert r.status_code == 200
        assert isinstance(r.json()["id"], int)

        notes = client.get("/api/notes?paper_id=2204.12985").json()["notes"]
        assert len(notes) == 1
        assert notes[0]["title"] == "My Note"
        assert notes[0]["content"] == "Some content"
        assert notes[0]["created_at"] is not None

    def test_note_with_project(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        client.post("/api/notes", json={"paper_id": "2204.12985", "project_id": pid, "title": "T"})

        notes = client.get(f"/api/notes?paper_id=2204.12985&project_id={pid}").json()["notes"]
        assert len(notes) == 1
        assert notes[0]["project_id"] == pid

    def test_all_projects_flag_returns_all_notes(self, client):
        pid = client.post("/api/projects", json={"name": "P"}).json()["project"]["id"]
        client.post("/api/notes", json={"paper_id": "X", "title": "No project"})
        client.post("/api/notes", json={"paper_id": "X", "project_id": pid, "title": "With project"})

        r = client.get("/api/notes?paper_id=X&all_projects=true")
        assert r.status_code == 200
        assert len(r.json()["notes"]) == 2

    def test_notes_isolated_by_paper(self, client):
        client.post("/api/notes", json={"paper_id": "A", "title": "Note A"})
        client.post("/api/notes", json={"paper_id": "B", "title": "Note B"})
        assert len(client.get("/api/notes?paper_id=A").json()["notes"]) == 1
        assert len(client.get("/api/notes?paper_id=B").json()["notes"]) == 1


# ---------------------------------------------------------------------------
# arXiv endpoints (external calls mocked)
# ---------------------------------------------------------------------------

class TestArxivEndpoints:
    def test_search_success(self, client):
        result = _arxiv_result()
        with patch("api.app.search_papers", return_value=[result]):
            r = client.post("/api/arxiv/search", json={"query": "transformers"})
        assert r.status_code == 200
        data = r.json()
        assert len(data["results"]) == 1
        assert data["results"][0]["title"] == "Mock Paper"
        assert data["saved_paper_ids"] == []

    def test_search_save_flag(self, client):
        result = _arxiv_result()
        with patch("api.app.search_papers", return_value=[result]), \
             patch("api.app.save_papers"):
            r = client.post("/api/arxiv/search", json={"query": "test", "save": True})
        assert r.status_code == 200
        assert len(r.json()["saved_paper_ids"]) == 1

    def test_search_empty_results(self, client):
        with patch("api.app.search_papers", return_value=[]):
            r = client.post("/api/arxiv/search", json={"query": "xyzxyzxyz"})
        assert r.status_code == 200
        assert r.json()["results"] == []

    def test_search_error_returns_502(self, client):
        with patch("api.app.search_papers", side_effect=Exception("timeout")):
            r = client.post("/api/arxiv/search", json={"query": "test"})
        assert r.status_code == 502

    def test_search_missing_query_returns_422(self, client):
        r = client.post("/api/arxiv/search", json={})
        assert r.status_code == 422

    def test_fetch_success(self, client):
        result = _arxiv_result()
        with patch("api.app.fetch_paper_metadata", return_value=result):
            r = client.post("/api/arxiv/fetch", json={"paper_id": "2204.12985", "save": False})
        assert r.status_code == 200
        data = r.json()
        assert data["paper"]["title"] == "Mock Paper"
        assert data["paper_id"] == "2204.12985"
        assert data["saved"] is False

    def test_fetch_error_returns_502(self, client):
        with patch("api.app.fetch_paper_metadata", side_effect=Exception("not found")):
            r = client.post("/api/arxiv/fetch", json={"paper_id": "9999.99999"})
        assert r.status_code == 502


# ---------------------------------------------------------------------------
# DOI endpoints (external calls mocked)
# ---------------------------------------------------------------------------

class TestDoiEndpoints:
    _doi = "10.1234/test"

    def test_resolve_success(self, client):
        with patch("api.app.resolve_doi", return_value=_meta(paper_id=self._doi)):
            r = client.post("/api/doi/resolve", json={"doi": self._doi})
        assert r.status_code == 200
        assert "metadata" in r.json()
        assert r.json()["metadata"]["paper_id"] == self._doi

    def test_resolve_not_found_returns_400(self, client):
        with patch("api.app.resolve_doi", side_effect=ValueError("not found")):
            r = client.post("/api/doi/resolve", json={"doi": "10.bad/doi"})
        assert r.status_code == 400

    def test_resolve_missing_doi_returns_422(self, client):
        r = client.post("/api/doi/resolve", json={})
        assert r.status_code == 422

    def test_save_success(self, client):
        with patch("api.app.resolve_doi", return_value=_meta(paper_id=self._doi)):
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
            paper_id="2204.12985",
            url="https://arxiv.org/pdf/2204.12985v1",
        ))
        r = client.get("/api/papers/2204.12985/pdf", follow_redirects=False)
        assert r.status_code in (301, 302, 303, 307, 308)
        assert "arxiv.org" in r.headers["location"]

    def test_paper_with_local_pdf(self, client, tmp_path):
        import storage.db as db
        pdf_file = tmp_path / "2204.12985v1.pdf"
        pdf_file.write_bytes(b"%PDF-1.4 fake content")

        db.save_paper_metadata(_meta(paper_id="2204.12985", url=None))

        with patch("api.app.PDF_DIR", tmp_path):
            r = client.get("/api/papers/2204.12985/pdf")
        assert r.status_code == 200
        assert r.headers["content-type"] == "application/pdf"

    def test_paper_with_no_pdf_source_returns_404(self, client):
        import storage.db as db
        db.save_paper_metadata(_meta(paper_id="2204.12985", url=None))
        r = client.get("/api/papers/2204.12985/pdf")
        assert r.status_code == 404
