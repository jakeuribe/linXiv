"""
FastAPI JSON API for linXiv (backend only — UI lives in separate clients).

Serves ``/api/...`` routes and, for the graph viewer, static files under
``/assets/graph/`` (from ``gui/graph/web/``) so a frontend can iframe them or proxy the path.
"""

from __future__ import annotations

import os
from contextlib import asynccontextmanager
from pathlib import Path

from dotenv import load_dotenv

from config import ENV_PATH
load_dotenv(ENV_PATH)

import arxiv
from fastapi import FastAPI, HTTPException, Query
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, RedirectResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, Field

from .graph_payload import get_augmented_graph_data, project_filter_options
from storage.db import (
    delete_paper,
    get_categories,
    get_paper,
    get_tags,
    init_db,
    list_papers,
    parse_entry_id,
    save_paper,
    save_papers,
    save_paper_metadata,
)
from sources import resolve_doi, fetch_paper_metadata, search_papers
from storage.notes import Note, ensure_notes_db, get_notes
from storage.projects import (
    Project,
    Status,
    color_from_hex,
    color_to_hex,
    ensure_projects_db,
    filter_projects,
    get_project,
)

ROOT = Path(__file__).resolve().parent.parent
PDF_DIR = ROOT / "pdfs"
GUI_WEB = ROOT / "gui" / "graph" / "web"


def _cors_config() -> tuple[list[str], bool]:
    """Return (origins, allow_credentials). Wildcard * disables credentials."""
    raw = os.environ.get("CORS_ORIGINS", "").strip()
    if not raw:
        return ["*"], False
    origins = [o.strip() for o in raw.split(",") if o.strip()]
    return origins, True


def _paper_row_dict(row) -> dict:
    out: dict = {}
    for k in row.keys():
        v = row[k]
        if hasattr(v, "isoformat"):
            out[k] = v.isoformat()
        else:
            out[k] = v
    return out


def _arxiv_result_summary(p: arxiv.Result) -> dict:
    pid, ver = parse_entry_id(p.entry_id)
    return {
        "paper_id": pid,
        "version": ver,
        "title": p.title,
        "summary": p.summary,
        "authors": [a.name for a in p.authors],
        "published": p.published.date().isoformat(),
        "pdf_url": p.pdf_url,
        "primary_category": p.primary_category,
        "entry_id": p.entry_id,
    }


def _resolve_local_pdf(paper_id: str, version: int | None) -> str | None:
    row = get_paper(paper_id) if version is None else get_paper(paper_id, version)
    if not row:
        return None
    ver = row["version"] if version is None else version
    if row["pdf_path"] and os.path.isfile(row["pdf_path"]):
        return row["pdf_path"]
    std = PDF_DIR / f"{paper_id}v{ver}.pdf"
    if std.is_file():
        return str(std)
    return None


@asynccontextmanager
async def _lifespan(_: FastAPI):
    init_db()
    ensure_projects_db()
    ensure_notes_db()
    PDF_DIR.mkdir(parents=True, exist_ok=True)
    yield


app = FastAPI(title="linXiv API", lifespan=_lifespan)
_cors_origins, _cors_credentials = _cors_config()
app.add_middleware(
    CORSMiddleware,
    allow_origins=_cors_origins,
    allow_credentials=_cors_credentials,
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.get("/")
def api_root() -> dict:
    """Service metadata. Interactive docs at ``/docs``."""
    return {
        "service": "linXiv",
        "api": "JSON over HTTP; routes under /api/…",
        "docs": "/docs",
        "openapi": "/openapi.json",
        "graph_assets": "/assets/graph/graph.html",
    }


@app.get("/api/health")
def health() -> dict:
    return {"ok": True}


@app.get("/api/stats")
def stats() -> dict:
    papers = list_papers(latest_only=True)
    tags = get_tags()
    recent = [_paper_row_dict(r) for r in papers[:10]]
    return {
        "paper_count": len(papers),
        "tag_count": len(tags),
        "recent_papers": recent,
    }


@app.get("/api/papers")
def api_list_papers(
    limit: int | None = Query(default=200, ge=1, le=5000),
    offset: int = Query(default=0, ge=0),
) -> dict:
    rows = list_papers(latest_only=True, limit=limit, offset=offset)
    return {"papers": [_paper_row_dict(r) for r in rows]}


@app.get("/api/papers/{paper_id}")
def api_get_paper(paper_id: str) -> dict:
    row = get_paper(paper_id)
    if not row:
        raise HTTPException(status_code=404, detail="Paper not found")
    return _paper_row_dict(row)


@app.delete("/api/papers/{paper_id}")
def api_delete_paper(paper_id: str) -> dict:
    row = get_paper(paper_id)
    if not row:
        raise HTTPException(status_code=404, detail="Paper not found")
    delete_paper(paper_id)
    return {"deleted": paper_id}


@app.get("/api/graph")
def api_graph() -> dict:
    return get_augmented_graph_data()


@app.get("/api/categories")
def api_categories() -> dict:
    return {"categories": get_categories()}


@app.get("/api/tags")
def api_tags() -> dict:
    return {"tags": get_tags()}


@app.get("/api/projects")
def api_projects() -> dict:
    projects = filter_projects()
    out = []
    for p in projects:
        if p.id is None:
            continue
        out.append(
            {
                "id": p.id,
                "name": p.name,
                "description": p.description or "",
                "color_hex": color_to_hex(p.color) if p.color else None,
                "project_tags": p.project_tags or [],
                "paper_ids": p.paper_ids or [],
                "status": p.status.value,
                "paper_count": len(p.paper_ids or []),
            }
        )
    return {"projects": out}


@app.get("/api/graph/project-options")
def api_graph_project_options() -> dict:
    return {"projects": project_filter_options()}


class ProjectCreate(BaseModel):
    name: str = Field(min_length=1)
    description: str = ""
    color_hex: str | None = None


class ProjectUpdate(BaseModel):
    name: str | None = None
    description: str | None = None
    color_hex: str | None = None
    status: str | None = None


@app.post("/api/projects")
def api_project_create(body: ProjectCreate) -> dict:
    color = color_from_hex(body.color_hex) if body.color_hex else None
    p = Project(name=body.name.strip(), description=body.description.strip(), color=color)
    p.save()
    assert p.id is not None
    return {"project": {"id": p.id, "name": p.name}}


@app.get("/api/projects/{project_id}")
def api_project_get(project_id: int) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    return {
        "id": p.id,
        "name": p.name,
        "description": p.description or "",
        "color_hex": color_to_hex(p.color) if p.color else None,
        "project_tags": p.project_tags or [],
        "paper_ids": p.paper_ids or [],
        "status": p.status.value,
    }


@app.patch("/api/projects/{project_id}")
def api_project_patch(project_id: int, body: ProjectUpdate) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    if body.name is not None:
        p.name = body.name.strip()
    if body.description is not None:
        p.description = body.description
    if body.color_hex is not None:
        p.color = color_from_hex(body.color_hex)
    if body.status is not None:
        try:
            p.status = Status(body.status)
        except ValueError as e:
            raise HTTPException(status_code=400, detail="Invalid status") from e
    p.save()
    return {"ok": True}


@app.delete("/api/projects/{project_id}")
def api_project_delete(project_id: int) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    p.delete()
    return {"ok": True}


class ProjectPaperBody(BaseModel):
    paper_id: str = Field(min_length=1)


@app.post("/api/projects/{project_id}/papers")
def api_project_add_paper(project_id: int, body: ProjectPaperBody) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    p.add_paper(body.paper_id.strip())
    return {"ok": True}


@app.delete("/api/projects/{project_id}/papers/{paper_id}")
def api_project_remove_paper(project_id: int, paper_id: str) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    p.remove_paper(paper_id)
    return {"ok": True}


class ArxivSearchBody(BaseModel):
    query: str = Field(min_length=1)
    max_results: int = Field(default=25, ge=1, le=100)
    save: bool = False


@app.post("/api/arxiv/search")
def api_arxiv_search(body: ArxivSearchBody) -> dict:
    try:
        results = search_papers(body.query.strip(), max_results=body.max_results)
    except Exception as e:
        raise HTTPException(status_code=502, detail=str(e)) from e
    summaries = [_arxiv_result_summary(p) for p in results]
    saved: list[str] = []
    if body.save and results:
        save_papers(results)
        saved = [parse_entry_id(p.entry_id)[0] for p in results]
    return {"results": summaries, "saved_paper_ids": saved if body.save else []}


class ArxivFetchBody(BaseModel):
    paper_id: str = Field(min_length=1)
    save: bool = True


@app.post("/api/arxiv/fetch")
def api_arxiv_fetch(body: ArxivFetchBody) -> dict:
    try:
        paper = fetch_paper_metadata(body.paper_id.strip())
    except Exception as e:
        raise HTTPException(status_code=502, detail=str(e)) from e
    pid, _ = parse_entry_id(paper.entry_id)
    if body.save:
        save_paper(paper)
    return {"paper": _arxiv_result_summary(paper), "saved": body.save, "paper_id": pid}


class DoiResolveBody(BaseModel):
    doi: str = Field(min_length=1)


@app.post("/api/doi/resolve")
def api_doi_resolve(body: DoiResolveBody) -> dict:
    try:
        meta = resolve_doi(body.doi.strip())
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return {"metadata": meta.model_dump(mode="json")}


class DoiSaveBody(BaseModel):
    doi: str = Field(min_length=1)


@app.post("/api/doi/save")
def api_doi_save(body: DoiSaveBody) -> dict:
    try:
        meta = resolve_doi(body.doi.strip())
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    save_paper_metadata(meta)
    return {"metadata": meta.model_dump(mode="json"), "saved": True}


class NoteCreate(BaseModel):
    paper_id: str
    project_id: int | None = None
    title: str = ""
    content: str = ""


@app.get("/api/notes")
def api_notes(
    paper_id: str,
    project_id: int | None = None,
    all_projects: bool = Query(default=False),
) -> dict:
    notes = get_notes(paper_id, project_id=project_id, all_projects=all_projects)
    return {
        "notes": [
            {
                "id": n.id,
                "paper_id": n.paper_id,
                "project_id": n.project_id,
                "title": n.title,
                "content": n.content,
                "created_at": n.created_at.isoformat() if n.created_at else None,
                "updated_at": n.updated_at.isoformat() if n.updated_at else None,
            }
            for n in notes
        ]
    }


@app.post("/api/notes")
def api_note_create(body: NoteCreate) -> dict:
    n = Note(
        paper_id=body.paper_id.strip(),
        project_id=body.project_id,
        title=body.title,
        content=body.content,
    )
    n.save()
    return {"id": n.id}


@app.get("/api/papers/{paper_id}/pdf", response_model=None)
def api_paper_pdf(paper_id: str, version: int | None = Query(default=None)):
    row = get_paper(paper_id) if version is None else get_paper(paper_id, version)
    if not row:
        raise HTTPException(status_code=404, detail="Paper not found")
    path = _resolve_local_pdf(paper_id, version)
    if path:
        return FileResponse(path, media_type="application/pdf", filename=os.path.basename(path))
    url = row["url"]
    if url:
        return RedirectResponse(url)
    raise HTTPException(status_code=404, detail="No PDF available")


# Graph viewer static bundle (used by PyQt, external frontends via iframe/proxy)
app.mount("/assets/graph", StaticFiles(directory=str(GUI_WEB), html=True), name="graph_assets")
