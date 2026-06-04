"""FastAPI JSON API for linXiv (backend only — UI lives in separate clients)."""

from __future__ import annotations

import asyncio
import datetime
import logging
import os
import sqlite3
from contextlib import asynccontextmanager
from typing import Literal
from pathlib import Path

import httpx

from dotenv import load_dotenv

from config import ENV_PATH, init_data_dir
load_dotenv(ENV_PATH)

from fastapi import FastAPI, File, HTTPException, Query, UploadFile
from fastapi.concurrency import run_in_threadpool
from fastapi.background import BackgroundTasks
from dotenv import set_key
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, PlainTextResponse, RedirectResponse, StreamingResponse
from pydantic import BaseModel, Field, field_validator, model_validator

from .graph_payload import get_augmented_graph_data, project_filter_options
from sources import resolve_doi
from sources.base import PaperMetadata
from sources.arxiv_source import ArxivSource, ArxivNotFoundError
from sources.openalex_source import OpenAlexSource, OpenAlexNotFoundError, OpenAlexInputError
from formats.bibtex import BibTeXFormat
from formats.markdown import ObsidianFormat
from service.paper import (
    Paper,
    ensure_paper_root,
    get as get_paper_details,
    get_all as get_all_paper_versions,
    get_source_id as get_paper_source_id,
    get_paper_root,
    list_paper_details,
    list_deleted as list_deleted_papers,
    delete as soft_delete_paper,
    restore as restore_paper,
    hard_delete as hard_delete_paper,
    repair_paper as svc_repair_paper,
    sfks_to_source_ids,
    get_papers_by_tag,
    save_paper_metadata,
    save_papers_metadata,
    get_categories,
    init_db,
    search_papers,
    import_pdf as svc_import_pdf,
    PdfImportError,
    pdf_on_disk_name,
    set_has_pdf,
    set_pdf_path,
)
from service.models.paper import PaperDetails
from service.tag import list_all_tags
import user_settings
import service.note as _service_note
from storage.projects import (
    Project,
    Status,
    ensure_projects_db,
    filter_projects,
    get_project,
    remove_paper_from_all_projects,
)
from service.project import (
    Project as SvcProject,
    ProjectIn as SvcProjectIn,
    color_from_hex,
    color_to_hex,
    create as create_project_svc,
    update as update_project_svc,
    hard_delete as hard_delete_project,
    list_deleted as list_deleted_projects,
    purge_old as purge_old_projects,
    restore as restore_project_svc,
)
from storage.tags import get_project_tags
from storage.config.queries import Q, list_project_tags_bulk, list_project_source_ids_bulk, list_projects_by_tag, get_tag_by_label
import storage.search_history as _search_history
import storage.search_state as _search_state

from storage.paths import pdf_dir as _storage_pdf_dir
PDF_DIR = _storage_pdf_dir()

_arxiv_source = ArxivSource()
_openalex = OpenAlexSource()


def _strip_namespace(source_id: str) -> str:
    """Return the bare paper ID with the source namespace prefix removed."""
    return source_id.split(":", 1)[-1]


def _cors_config() -> tuple[list[str], bool]:
    """Return (origins, allow_credentials). Wildcard * disables credentials."""
    raw = os.environ.get("CORS_ORIGINS", "").strip()
    if not raw:
        return ["*"], False
    origins = [o.strip() for o in raw.split(",") if o.strip()]
    return origins, True


def _resolve_local_pdf(paper: PaperDetails, version: int | None) -> str | None:
    ver = paper.version if version is None else version
    if paper.pdf_path and os.path.isfile(paper.pdf_path):
        return paper.pdf_path
    # Standard-filename fallback is meaningless for ver<=0: the download pipeline
    # never writes a version-0 filename, so the probe cannot succeed.
    if ver <= 0:
        return None
    std = PDF_DIR / pdf_on_disk_name(paper.source_id, ver)
    if std.is_file():
        return str(std)
    return None


@asynccontextmanager
async def _lifespan(_: FastAPI):
    init_data_dir()
    init_db()
    ensure_projects_db()
    _service_note.ensure_notes_db()
    PDF_DIR.mkdir(parents=True, exist_ok=True)
    async def _purge():
        try:
            await asyncio.to_thread(purge_old_projects, 30)
        except Exception as exc:
            print(f"[linxiv] purge_old_projects failed: {exc}")

    asyncio.create_task(_purge())
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
    }


@app.get("/api/health")
def health() -> dict:
    # The "service" field is checked by the Tauri launcher to verify that a
    # response on the expected port is actually our API and not a rogue process
    # that happened to grab the port between bind-probe and uvicorn startup.
    return {"ok": True, "service": "linxiv-api"}


@app.get("/api/stats")
def stats() -> dict:
    papers = list_paper_details(latest_only=True)
    tags = list_all_tags()
    categories = get_categories()
    return {
        "paper_count": len(papers),
        "tag_count": len(tags),
        "category_count": len(categories),
        "pdf_count": sum(1 for p in papers if p.has_pdf),
        "recent_papers": [p.to_dict() for p in papers[:10]],
    }


@app.get("/api/papers")
def api_list_papers(
    limit: int | None = Query(default=200, ge=1, le=5000),
    offset: int = Query(default=0, ge=0),
) -> dict:
    papers = list_paper_details(latest_only=True, limit=limit, offset=offset)
    return {"papers": [p.to_dict() for p in papers]}


@app.get("/api/papers/sfk/{source_fk}/versions")
def api_get_paper_versions(source_fk: int) -> dict:
    all_data = get_all_paper_versions(Paper(source_fk=source_fk))
    if not all_data:
        raise HTTPException(status_code=404, detail="Paper not found")
    return {
        "source_id": all_data.source_id,
        "latest_version": all_data.latest_version,
        "versions": [
            {
                "version": v.version,
                "published": v.published.isoformat() if v.published else None,
                "updated": v.updated.isoformat() if v.updated else None,
                "has_pdf": v.has_pdf,
            }
            for v in all_data.versions
        ],
    }


@app.get("/api/papers/sfk/{source_fk}")
def api_get_paper_by_sfk(
    source_fk: int,
    version: int | None = Query(default=None, ge=1),
) -> dict:
    if version is not None:
        source_id = get_paper_source_id(source_fk)
        if not source_id:
            raise HTTPException(status_code=404, detail="Paper not found")
        paper = get_paper_details(Paper(source_id=source_id, version=version))
        if not paper:
            raise HTTPException(status_code=404, detail=f"Version {version} not stored")
    else:
        paper = get_paper_details(Paper(source_fk=source_fk))
        if not paper:
            raise HTTPException(status_code=404, detail="Paper not found")
    return paper.to_dict()


@app.get("/api/papers/search")
def api_search_papers(
    q: str = Query(),
    limit: int = Query(default=50, ge=1, le=100),
) -> dict:
    q = q.strip()
    if len(q) < 3:
        raise HTTPException(status_code=422, detail="Query must contain at least 3 non-whitespace characters")
    papers = search_papers(q, limit)
    return {"papers": [p.to_dict() for p in papers]}


@app.get("/api/papers/{source_id:path}/pdf", response_model=None)
def api_paper_pdf(source_id: str, version: int | None = Query(default=None, ge=1)):
    paper = get_paper_details(Paper(source_id=source_id, version=version))
    if not paper:
        raise HTTPException(status_code=404, detail="Paper not found")
    path = _resolve_local_pdf(paper, version)
    if path:
        return FileResponse(path, media_type="application/pdf", filename=os.path.basename(path), content_disposition_type="inline")
    if paper.has_pdf:
        # Database says a PDF should be on disk but the file is missing — don't
        # silently fall through to the remote URL, which would mask the data drift.
        raise HTTPException(status_code=404, detail="PDF file not found on disk")
    if paper.url:
        return RedirectResponse(paper.url)
    raise HTTPException(status_code=404, detail="No PDF available")


@app.get("/api/papers/{source_id:path}/pdf-path")
def api_paper_pdf_path(source_id: str, version: int | None = Query(default=None, ge=1)) -> dict:
    paper = get_paper_details(Paper(source_id=source_id, version=version))
    if not paper:
        raise HTTPException(status_code=404, detail="Paper not found")
    path = _resolve_local_pdf(paper, version)
    if not path:
        raise HTTPException(status_code=404, detail="PDF file not found on disk")
    return {"path": path}


@app.put("/api/papers/{source_id:path}/pdf", response_model=None)
async def api_attach_pdf(source_id: str, file: UploadFile = File(...)) -> dict:
    """Store the PDF bytes already fetched by the client for a saved paper."""
    paper = get_paper_details(Paper(source_id=source_id))
    if not paper:
        raise HTTPException(status_code=404, detail="Paper not found")
    if file.size is not None and file.size > _MAX_PDF_BYTES:
        raise HTTPException(status_code=413, detail="PDF exceeds size limit")
    content = await file.read()
    if len(content) > _MAX_PDF_BYTES:
        raise HTTPException(status_code=413, detail="PDF exceeds size limit")
    if not content.startswith(b"%PDF"):
        raise HTTPException(status_code=400, detail="Not a valid PDF")
    ver = paper.version
    dest = PDF_DIR / pdf_on_disk_name(source_id, ver)
    try:
        dest.resolve().relative_to(PDF_DIR.resolve())
    except ValueError:
        raise HTTPException(status_code=400, detail="Invalid source_id")
    dest.write_bytes(content)
    set_pdf_path(source_id, str(dest), ver)
    set_has_pdf(source_id, ver, True)
    return {"ok": True}


@app.get("/api/papers/{source_id:path}")
def api_get_paper(source_id: str) -> dict:
    paper = get_paper_details(Paper(source_id=source_id))
    if not paper:
        raise HTTPException(status_code=404, detail="Paper not found")
    return paper.to_dict()


@app.delete("/api/papers/{source_id:path}")
def api_delete_paper(source_id: str) -> dict:
    paper = get_paper_details(Paper(source_id=source_id))
    if not paper:
        raise HTTPException(status_code=404, detail="Paper not found")
    soft_delete_paper(Paper(source_id=source_id))
    return {"deleted": source_id}


class PaperRepairBody(BaseModel):
    title: str
    authors: list[str]
    published: datetime.date
    summary: str = ""
    category: str | None = None
    # min_length=1 on Optional[str]: Pydantic v2 skips the constraint when the
    # value is None, so null/omitted → None (valid) but "" → validation error.
    doi: str | None = Field(default=None, min_length=1)
    url: str | None = None
    tags: list[str] | None = None

    @field_validator("title")
    @classmethod
    def title_not_blank(cls, v: str) -> str:
        v = v.strip()
        if not v:
            raise ValueError("title must not be blank")
        return v

    @field_validator("authors")
    @classmethod
    def authors_not_empty(cls, v: list[str]) -> list[str]:
        # Preserve insertion order (author rank matters) while deduplicating.
        seen: dict[str, None] = {}
        for a in v:
            s = a.strip()
            if s:
                seen[s] = None
        if not seen:
            raise ValueError("at least one author is required")
        return list(seen)

    @field_validator("summary")
    @classmethod
    def summary_strip(cls, v: str) -> str:
        return v.strip()

    @field_validator("tags")
    @classmethod
    def tags_strip_and_dedup(cls, v: list[str] | None) -> list[str] | None:
        if v is None:
            return None
        cleaned = list(dict.fromkeys(t.strip() for t in v if t.strip()))
        return cleaned or None


# PUT semantics: all required fields are replaced on every call. The editor
# always submits a complete payload, so partial-update semantics add no value
# and would require merging logic that can silently drop user edits on race
# conditions. See docs/adr/0008-repair-endpoint-scope.md.
@app.put("/api/papers/sfk/{source_fk}")
def api_repair_paper(source_fk: int, body: PaperRepairBody) -> dict:
    paper = get_paper_details(Paper(source_fk=source_fk))
    if not paper:
        raise HTTPException(status_code=404, detail="Paper not found")
    # source_id is intentionally not changeable via this endpoint — it is the
    # paper's identity key and changing it is an internal import/export operation.
    # See docs/adr/0008-repair-endpoint-scope.md.
    meta = PaperMetadata(
        source_id=paper.source_id,
        version=paper.version,
        title=body.title,
        authors=body.authors,
        published=body.published,
        summary=body.summary,
        category=body.category,
        doi=body.doi,
        url=body.url,
        tags=body.tags,
        source=paper.source,
    )
    try:
        svc_repair_paper(source_fk, meta)
    except sqlite3.IntegrityError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    updated = get_paper_details(Paper(source_fk=source_fk))
    if not updated:
        raise HTTPException(status_code=500, detail="Repair failed")
    return updated.to_dict()


@app.get("/api/graph")
def api_graph() -> dict:
    return get_augmented_graph_data()


@app.get("/api/categories")
def api_categories() -> dict:
    return {"categories": get_categories()}


@app.get("/api/tags")
def api_tags() -> dict:
    return {"tags": list_all_tags()}


@app.get("/api/tags/{label}")
def api_tag_detail(label: str) -> dict:
    papers = get_papers_by_tag(label)

    tag_row = get_tag_by_label(label)
    canonical_label = tag_row["TAG"] if tag_row else label
    projects: list[dict] = []
    if tag_row is not None:
        tag_fk = int(tag_row["TAG_FK"])
        proj_rows = list_projects_by_tag(tag_fk)
        project_ids = [r["PROJECT_FK"] for r in proj_rows]
        tags_by_proj = list_project_tags_bulk(project_ids)
        source_ids_by_proj = list_project_source_ids_bulk(project_ids)
        for row in proj_rows:
            pid = int(row["PROJECT_FK"])
            projects.append({
                "id": pid,
                "name": row["NAME"],
                "description": row["DESCRIPTION"] or "",
                "color_hex": color_to_hex(row["COLOR"]) if row["COLOR"] is not None else None,
                "project_tags": tags_by_proj.get(pid, []),
                "source_ids": source_ids_by_proj.get(pid, []),
                "status": row["STATUS"],
                "paper_count": len(source_ids_by_proj.get(pid, [])),
            })

    return {
        "label": canonical_label,
        "papers": [p.to_dict() for p in papers],
        "projects": projects,
    }


@app.get("/api/projects")
def api_projects(status: str = "active") -> dict:
    condition = Q("STATUS = ?", status) if status != "all" else None
    projects = filter_projects(condition, load_sources=False)
    valid = [(p, p.id) for p in projects if p.id is not None]
    null_count = len(projects) - len(valid)
    if null_count:
        print(f"[linxiv] api_projects: {null_count} project(s) with null id excluded (data integrity error)")
    project_ids = [pid for _, pid in valid]
    tags_by_project = list_project_tags_bulk(project_ids)
    source_ids_by_project = list_project_source_ids_bulk(project_ids)
    out = [
        {
            "id": pid,
            "name": p.name,
            "description": p.description,
            "color_hex": color_to_hex(p.color) if p.color is not None else None,
            "project_tags": tags_by_project.get(pid, []),
            "source_ids": source_ids_by_project.get(pid, []),
            "status": p.status.value,
            "paper_count": len(source_ids_by_project.get(pid, [])),
        }
        for p, pid in valid
    ]
    return {"projects": out}


@app.get("/api/graph/project-options")
def api_graph_project_options() -> dict:
    return {"projects": project_filter_options()}


class ProjectCreate(BaseModel):
    name: str = Field(min_length=1)
    description: str = ""
    color_hex: str | None = None
    project_tags: list[str] = []


class ProjectUpdate(BaseModel):
    name: str | None = None
    description: str | None = None
    color_hex: str | None = None
    status: str | None = None
    project_tags: list[str] | None = None


@app.post("/api/projects")
def api_project_create(body: ProjectCreate) -> dict:
    name = body.name.strip()
    if not name:
        raise HTTPException(status_code=400, detail="name cannot be blank")
    try:
        color = color_from_hex(body.color_hex) if body.color_hex else None
    except ValueError as e:
        raise HTTPException(status_code=400, detail="Invalid color_hex") from e
    # source_fks intentionally omitted: papers are linked via POST /projects/{id}/papers
    fk = create_project_svc(SvcProjectIn(
        name        = name,
        description = body.description.strip(),
        color       = color,
        tags        = body.project_tags,
    ))
    return {"project": {"id": fk, "name": name}}


@app.get("/api/projects/{project_id}")
def api_project_get(project_id: int) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    return {
        "id": p.id,
        "name": p.name,
        "description": p.description or "",
        "color_hex": color_to_hex(p.color) if p.color is not None else None,
        "project_tags": get_project_tags(p.id) if p.id else [],
        "source_ids": sfks_to_source_ids(p.source_fks),
        "status": p.status.value,
    }


@app.patch("/api/projects/{project_id}")
def api_project_patch(project_id: int, body: ProjectUpdate) -> dict:
    status: Status | None = None
    if body.status is not None:
        try:
            status = Status(body.status)
        except ValueError as e:
            raise HTTPException(status_code=400, detail="Invalid status") from e
    # Build color kwarg only when color_hex was explicitly sent so that omitting
    # the field is a no-op while sending null explicitly clears the color.
    color_kwarg: dict = {}
    if "color_hex" in body.model_fields_set:
        try:
            color_kwarg["color"] = color_from_hex(body.color_hex) if body.color_hex else None
        except ValueError as e:
            raise HTTPException(status_code=400, detail="Invalid color_hex") from e
    name: str | None = None
    if body.name is not None:
        name = body.name.strip()
        if not name:
            raise HTTPException(status_code=400, detail="name cannot be blank")
    try:
        update_project_svc(
            project_fk   = project_id,
            name         = name,
            # description is a non-nullable string (never NULL in DB); clients
            # clear it with "" not null, so None here safely means "no change".
            # color_hex uses model_fields_set because its DB column IS nullable.
            description  = body.description.strip() if body.description is not None else None,
            project_tags = body.project_tags,
            status       = status,
            **color_kwarg,
        )
    except LookupError as e:
        raise HTTPException(status_code=404, detail="Project not found") from e
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return {"ok": True}


@app.delete("/api/projects/{project_id}")
def api_project_delete(project_id: int) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    p.delete()
    return {"ok": True}


class ProjectPaperBody(BaseModel):
    source_id: str = Field(min_length=1)


@app.post("/api/projects/{project_id}/papers")
def api_project_add_paper(project_id: int, body: ProjectPaperBody) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    root = get_paper_root(body.source_id.strip())
    if root is None:
        raise HTTPException(status_code=404, detail="Paper not found")
    p.add_paper(int(root["SOURCE_FK"]))
    return {"ok": True}


@app.delete("/api/projects/{project_id}/papers/{source_id:path}")
def api_project_remove_paper(project_id: int, source_id: str) -> dict:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    root = get_paper_root(source_id)
    if root is None:
        raise HTTPException(status_code=404, detail="Paper not found")
    p.remove_paper(int(root["SOURCE_FK"]))
    return {"ok": True}


class SearchResultOut(BaseModel):
    source_id: str
    version: int
    title: str
    summary: str
    authors: list[str]
    published: str
    paper_url: str
    primary_category: str
    entry_id: str

    @classmethod
    def from_metadata(cls, meta: PaperMetadata) -> "SearchResultOut":
        return cls(
            source_id=_strip_namespace(meta.source_id),
            version=meta.version,
            title=meta.title,
            summary=meta.summary,
            authors=meta.authors,
            published="" if meta.published == datetime.date.min else meta.published.isoformat(),
            paper_url=meta.url or "",
            primary_category=meta.category or "",
            entry_id=meta.source_id,
        )


class ArxivSearchOut(BaseModel):
    results: list[SearchResultOut]
    saved_source_ids: list[str]


class ArxivFetchOut(BaseModel):
    paper: SearchResultOut
    saved: bool
    source_id: str


class ArxivFetchBody(BaseModel):
    source_id: str = Field(min_length=1)
    save: bool = True


class ArxivSearchBody(BaseModel):
    query: str = Field(min_length=1)
    max_results: int = Field(default=25, ge=1, le=100)
    save: bool = False
    sort: Literal["relevance", "newest", "oldest", "lastUpdated"] = "relevance"


@app.post("/api/arxiv/search", response_model=ArxivSearchOut)
def api_arxiv_search(body: ArxivSearchBody) -> ArxivSearchOut:
    try:
        results = _arxiv_source.search(
            body.query.strip(),
            max_results=body.max_results,
            sort=body.sort,
        )
    except Exception as e:
        raise HTTPException(status_code=502, detail=str(e)) from e
    summaries = [SearchResultOut.from_metadata(m) for m in results]
    saved: list[str] = []
    if body.save and results:
        try:
            pairs = save_papers_metadata(results, tags=None)
            saved = [_strip_namespace(sid) for sid, _ in pairs]
        except sqlite3.IntegrityError as exc:
            print(f"[linxiv] batch save IntegrityError (results still returned): {exc}")
    return ArxivSearchOut(results=summaries, saved_source_ids=saved)


# ── Search history / state ────────────────────────────────────────────────────

@app.get("/api/search/history")
def api_search_history(prefix: str = Query(default="", min_length=0), limit: int = Query(default=10, ge=1, le=50)) -> dict:
    suggestions = _search_history.get_suggestions(prefix, limit)
    return {"suggestions": suggestions}


class SearchStateBody(BaseModel):
    clauses: list[dict] = Field(default_factory=list)
    source: str = Field(default="arxiv")
    max_results: int = Field(default=25)
    results: list[dict] = Field(default_factory=list)
    saved_ids: list[str] = Field(default_factory=list)
    sort_prefs: dict[str, str] | None = None


@app.get("/api/search/state")
def api_search_state_get() -> dict:
    state = _search_state.load_state()
    if state is None:
        return {"state": None}
    return {"state": state}


@app.post("/api/search/state")
def api_search_state_save(body: SearchStateBody) -> dict:
    # Record each non-empty clause term to history.
    for clause in body.clauses:
        term = clause.get("value", "")
        if isinstance(term, str) and term.strip():
            _search_history.add_term(term)
    _search_state.save_state(
        clauses=body.clauses,
        source=body.source,
        max_results=body.max_results,
        results=body.results,
        saved_ids=body.saved_ids,
        sort_prefs=body.sort_prefs,
    )
    return {"ok": True}


@app.post("/api/arxiv/fetch", response_model=ArxivFetchOut)
def api_arxiv_fetch(body: ArxivFetchBody) -> ArxivFetchOut:
    try:
        meta = _arxiv_source.fetch_by_id(body.source_id.strip())
    except ArxivNotFoundError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e
    except Exception as e:
        raise HTTPException(status_code=502, detail=str(e)) from e
    source_id = _strip_namespace(meta.source_id)
    if body.save:
        try:
            stored_sid, _ = save_paper_metadata(meta)
            source_id = _strip_namespace(stored_sid)
        except sqlite3.IntegrityError as exc:
            raise HTTPException(status_code=409, detail=str(exc)) from exc
    return ArxivFetchOut(
        paper=SearchResultOut.from_metadata(meta),
        saved=body.save,
        source_id=source_id,
    )


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
    source_id:  str = Field(min_length=1)
    project_id: int | None = None
    paper_id:   int | None = None
    title:      str = ""
    content:    str = ""


class NoteUpdate(BaseModel):
    title:   str | None = None
    content: str | None = None

    @model_validator(mode="after")
    def _require_at_least_one(self):
        if self.title is None and self.content is None:
            raise ValueError("at least one of title or content must be provided")
        return self


@app.get("/api/notes")
def api_notes(
    source_id: str,
    project_id: int | None = None,
    all_projects: bool = Query(default=False),
) -> dict:
    root = get_paper_root(source_id)
    if root is None:
        return {"notes": []}
    notes = _service_note.get_many(_service_note.Notes(
        source_fk=int(root["SOURCE_FK"]),
        project_fk=project_id,
        all_projects=all_projects,
    ))
    return {
        "notes": [
            {
                "id": n.note_id,
                "source_fk": n.source_fk,
                "paper_id_fk": n.paper_id_fk,
                "project_id": n.project_id,
                "title": n.title,
                "content": n.content,
                "created_at": n.created_at.isoformat() if isinstance(n.created_at, datetime.datetime) else n.created_at,
                "updated_at": n.updated_at.isoformat() if isinstance(n.updated_at, datetime.datetime) else n.updated_at,
            }
            for n in notes
        ]
    }


@app.post("/api/notes")
def api_note_create(body: NoteCreate) -> dict:
    source_fk = ensure_paper_root(body.source_id.strip())
    note_id = _service_note.create(_service_note.NoteIn(
        source_fk=source_fk,
        project_fk=body.project_id,
        paper_id=body.paper_id,
        title=body.title,
        content=body.content,
    ))
    return {"id": note_id}


@app.patch("/api/notes/{note_id}")
def api_note_update(note_id: int, body: NoteUpdate) -> dict:
    if not _service_note.update(_service_note.NoteUpdateIn(note_id=note_id, title=body.title, content=body.content)):
        raise HTTPException(status_code=404, detail="Note not found")
    return {"ok": True}


@app.delete("/api/notes/{note_id}")
def api_note_delete(note_id: int) -> dict:
    if not _service_note.delete(_service_note.Note(note_id=note_id)):
        raise HTTPException(status_code=404, detail="Note not found")
    return {"ok": True}


import service.author as _service_author


class AuthorUpdate(BaseModel):
    full_name:  str | None = None
    first_name: str | None = None
    last_name:  str | None = None
    orcid:      str | None = None


def _author_ref(author_id: int) -> _service_author.Author:
    return _service_author.Author(author_id=author_id)


def _author_detail_response(author_id: int) -> dict:
    """Build the full author detail dict (used by GET and PATCH)."""
    author = _service_author.get(_author_ref(author_id))
    if author is None:
        raise HTTPException(status_code=404, detail="Author not found")
    papers = _service_author.get_paper_previews(author_id)
    return {
        "author_id":   author.author_id,
        "full_name":   author.full_name,
        "first_name":  author.first_name,
        "last_name":   author.last_name,
        "orcid":       author.orcid,
        "paper_count": len(papers),
        "papers": [
            {
                "paper_id":  p.paper_id,
                "source_id": p.source_id,
                "source_fk": p.source_fk,
                "version":   p.version,
                "title":     p.title,
            }
            for p in papers
        ],
    }


@app.get("/api/authors")
def api_authors_list() -> dict:
    authors = _service_author.list_with_paper_count()
    return {
        "authors": [
            {
                "author_id":   a.author_id,
                "full_name":   a.full_name,
                "first_name":  a.first_name,
                "last_name":   a.last_name,
                "orcid":       a.orcid,
                "paper_count": a.paper_count,
            }
            for a in authors
        ]
    }


@app.get("/api/authors/{author_id}")
def api_author_get(author_id: int) -> dict:
    return _author_detail_response(author_id)


@app.patch("/api/authors/{author_id}")
def api_author_update(author_id: int, body: AuthorUpdate) -> dict:
    _service_author.update_fields(
        author_id  = author_id,
        full_name  = body.full_name,
        first_name = body.first_name,
        last_name  = body.last_name,
        orcid      = body.orcid,
    )
    return _author_detail_response(author_id)


@app.delete("/api/authors/{author_id}")
def api_author_delete(author_id: int) -> dict:
    if _service_author.get(_author_ref(author_id)) is None:
        raise HTTPException(status_code=404, detail="Author not found")
    total_links = _service_author.count_paper_links(author_id)
    if total_links > 0:
        raise HTTPException(
            status_code=409,
            detail=f"Author is linked to {total_links} paper(s); unlink before deleting.",
        )
    _service_author.delete_author(author_id)
    return {"ok": True}


@app.get("/api/settings")
def api_settings_get() -> dict:
    return user_settings.all_settings()


class SettingsUpdate(BaseModel):
    updates: dict


@app.patch("/api/settings")
def api_settings_patch(body: SettingsUpdate) -> dict:
    for key, value in body.updates.items():
        user_settings.set(key, value)
    return {"ok": True}


class EnvUpdate(BaseModel):
    key: str
    value: str


@app.patch("/api/env")
def api_env_patch(body: EnvUpdate) -> dict:
    set_key(str(ENV_PATH), body.key, body.value)
    os.environ[body.key] = body.value
    return {"ok": True}


# Graph viewer static bundle (used by PyQt, external frontends via iframe/proxy)
# ── Trash (soft-delete) ───────────────────────────────────────────────────────

@app.get("/api/trash")
def api_trash_list() -> dict:
    deleted_papers = list_deleted_papers()
    deleted_projects = list_deleted_projects()
    return {
        "papers": [
            {
                "source_fk": d.source_fk,
                "source_id": d.source_id,
                "title": d.title,
                "authors": d.authors,
                "published": d.published.isoformat() if d.published else None,
                "deleted_at": d.deleted_at.isoformat() if d.deleted_at else None,
                "had_pdf": d.had_pdf,
            }
            for d in deleted_papers
        ],
        "projects": [
            {
                "id": p.id,
                "name": p.name,
                # archived_at is overwritten by delete(), so it holds the deletion timestamp
                "deleted_at": p.archived_at.isoformat() if p.archived_at else None,
                "paper_count": len(p.source_fks),
            }
            for p in deleted_projects
        ],
    }


@app.post("/api/trash/{source_id:path}/restore")
def api_trash_restore(source_id: str) -> dict:
    pdf_path, project_fks = restore_paper(Paper(source_id=source_id))
    return {"ok": True, "pdf_path": pdf_path, "project_fks": project_fks}


@app.delete("/api/papers/sfk/{source_fk}/projects")
def api_remove_paper_from_all_projects(source_fk: int) -> dict:
    """Remove a restored paper from all its projects. Call after restore if user declines project re-linking."""
    removed = remove_paper_from_all_projects(source_fk)
    return {"ok": True, "removed_from": removed}


@app.delete("/api/trash/{source_id:path}")
def api_trash_hard_delete(source_id: str) -> dict:
    hard_delete_paper(Paper(source_id=source_id))
    return {"ok": True}


@app.post("/api/trash/projects/{project_id}/restore")
def api_trash_project_restore(project_id: int) -> dict:
    if get_project(project_id) is None:
        raise HTTPException(status_code=404, detail="Project not found")
    restore_project_svc(SvcProject(project_fk=project_id))
    return {"ok": True}


@app.delete("/api/trash/projects/{project_id}")
def api_trash_project_hard_delete(project_id: int) -> dict:
    if get_project(project_id) is None:
        raise HTTPException(status_code=404, detail="Project not found")
    hard_delete_project(SvcProject(project_fk=project_id))
    return {"ok": True}



# ── OpenAlex search ───────────────────────────────────────────────────────────

class OpenAlexSearchOut(BaseModel):
    results: list[SearchResultOut]


class OpenAlexSearchBody(BaseModel):
    query: str = Field(min_length=1)
    max_results: int = Field(default=25, ge=1, le=100)  # OpenAlex supports up to 200 per_page
    sort: Literal["relevance", "newest", "oldest", "citations"] = "relevance"


@app.post("/api/openalex/search", response_model=OpenAlexSearchOut)
def api_openalex_search(body: OpenAlexSearchBody) -> OpenAlexSearchOut:
    try:
        results = _openalex.search(
            body.query.strip(),
            max_results=body.max_results,
            sort=body.sort,
        )
    except Exception as e:
        raise HTTPException(status_code=502, detail=str(e)) from e
    return OpenAlexSearchOut(results=[SearchResultOut.from_metadata(m) for m in results])


# ── Export / Import ───────────────────────────────────────────────────────────

import tempfile
import shutil
from service import export_import as _export_import


class ExportBody(BaseModel):
    project_id: int
    include_pdfs: bool = False
    dest_path: str | None = None


@app.post("/api/projects/{project_id}/export", response_model=None)
def api_project_export(
    project_id: int,
    body: ExportBody,
    background: BackgroundTasks,
) -> FileResponse | dict:
    if body.dest_path:
        dest = Path(body.dest_path)
        try:
            _export_import.export_project(
                project_id,
                dest_path=dest.parent / dest.stem,
                include_pdfs=body.include_pdfs,
            )
        except ValueError as e:
            raise HTTPException(status_code=404, detail=str(e)) from e
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e)) from e
        return {"ok": True}
    tmp_dir = Path(tempfile.mkdtemp())
    try:
        out = _export_import.export_project(
            project_id,
            dest_path=tmp_dir / "export",
            include_pdfs=body.include_pdfs,
        )
    except ValueError as e:
        shutil.rmtree(tmp_dir, ignore_errors=True)
        raise HTTPException(status_code=404, detail=str(e)) from e
    except Exception as e:
        shutil.rmtree(tmp_dir, ignore_errors=True)
        raise HTTPException(status_code=500, detail=str(e)) from e
    background.add_task(shutil.rmtree, tmp_dir, True)
    return FileResponse(
        path=str(out),
        filename=out.name,
        media_type="application/zip",
    )


@app.post("/api/projects/import/preview")
async def api_import_preview(file: UploadFile = File(...)) -> dict:
    tmp = Path(tempfile.mktemp(suffix=".lxproj"))
    try:
        content = await file.read()
        tmp.write_bytes(content)
        preview = _export_import.preview_import(tmp)
    except Exception as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    finally:
        tmp.unlink(missing_ok=True)
    return {
        "project_name": preview.project_name,
        "description": preview.description,
        "paper_count": preview.paper_count,
        "note_count": preview.note_count,
        "has_pdfs": preview.has_pdfs,
        "format_version": preview.format_version,
    }


@app.post("/api/projects/import/commit")
async def api_import_commit(
    file: UploadFile = File(...),
    on_conflict: str = Query(default="merge", pattern="^(merge|overwrite)$"),
) -> dict:
    tmp = Path(tempfile.mktemp(suffix=".lxproj"))
    try:
        content = await file.read()
        tmp.write_bytes(content)
        project_fk = _export_import.commit_import(
            tmp, on_conflict=on_conflict  # type: ignore[arg-type]
        )
    except _export_import.ProjectImportError as e:
        raise HTTPException(status_code=422, detail=str(e)) from e
    except Exception as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    finally:
        tmp.unlink(missing_ok=True)
    return {"project_id": project_fk}


# ── Plain-text export helpers ─────────────────────────────────────────────────

def _plain_export(project_id: int, fmt_obj: object, media_type: str, ext: str) -> PlainTextResponse:
    p = get_project(project_id)
    if not p:
        raise HTTPException(status_code=404, detail="Project not found")
    ids = set(sfks_to_source_ids(p.source_fks))
    project_papers = [
        pp.to_dict() for pp in list_paper_details(latest_only=True)
        if pp.source_id in ids
    ]
    safe_name = "".join(c if c.isalnum() or c in "-_ " else "_" for c in (p.name or "project"))
    return PlainTextResponse(
        content=fmt_obj.export_papers(project_papers),  # type: ignore[union-attr]
        media_type=media_type,
        headers={"Content-Disposition": f'attachment; filename="{safe_name}.{ext}"'},
    )


@app.get("/api/projects/{project_id}/export/bibtex", response_model=None)
def api_project_export_bibtex(
    project_id: int,
    dest_path: str | None = Query(default=None),
) -> PlainTextResponse | dict:
    if dest_path:
        p = get_project(project_id)
        if not p:
            raise HTTPException(status_code=404, detail="Project not found")
        ids = set(sfks_to_source_ids(p.source_fks))
        papers = [pp.to_dict() for pp in list_paper_details(latest_only=True) if pp.source_id in ids]
        Path(dest_path).write_text(BibTeXFormat().export_papers(papers), encoding="utf-8")
        return {"ok": True}
    return _plain_export(project_id, BibTeXFormat(), "text/x-bibtex", "bib")


@app.get("/api/projects/{project_id}/export/obsidian", response_model=None)
def api_project_export_obsidian(
    project_id: int,
    dest_path: str | None = Query(default=None),
) -> PlainTextResponse | dict:
    if dest_path:
        p = get_project(project_id)
        if not p:
            raise HTTPException(status_code=404, detail="Project not found")
        ids = set(sfks_to_source_ids(p.source_fks))
        papers = [pp.to_dict() for pp in list_paper_details(latest_only=True) if pp.source_id in ids]
        Path(dest_path).write_text(ObsidianFormat().export_papers(papers), encoding="utf-8")
        return {"ok": True}
    return _plain_export(project_id, ObsidianFormat(), "text/markdown", "md")


@app.post("/api/papers/import/bibtex")
async def api_import_bibtex(
    file: UploadFile = File(...),
    project_id: int | None = Query(default=None),
) -> dict:
    text = (await file.read()).decode("utf-8", errors="replace")
    try:
        metas = BibTeXFormat().import_string(text)
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"BibTeX parse error: {e}") from e
    # Single transaction: all entries commit or none do (unlike the prior per-row loop).
    pairs = save_papers_metadata(metas, tags=None)
    saved = [sid for sid, _ in pairs]
    if project_id is not None and saved:
        proj = get_project(project_id)
        if proj and proj.status == Status.ACTIVE:
            for sid in saved:
                root = get_paper_root(sid)
                if root:
                    proj.add_paper(int(root["SOURCE_FK"]))
    return {"saved_count": len(saved), "source_ids": saved}


# ── PDF import ────────────────────────────────────────────────────────────────

_MAX_PDF_BYTES = 100 * 1024 * 1024  # 100 MB
_api_log = logging.getLogger(__name__)


@app.post("/api/papers/import/pdf")
async def api_import_pdf(
    file: UploadFile = File(...),
    project_id: int | None = Query(default=None),
) -> dict:
    if not file.filename or not file.filename.lower().endswith(".pdf"):
        raise HTTPException(status_code=400, detail="File must be a PDF")
    if file.size is not None and file.size > _MAX_PDF_BYTES:
        raise HTTPException(status_code=413, detail=f"Upload rejected: declared size exceeds {_MAX_PDF_BYTES // 1024 // 1024} MB limit")
    content = await file.read()
    if len(content) > _MAX_PDF_BYTES:
        raise HTTPException(status_code=413, detail=f"Upload rejected: file size exceeds {_MAX_PDF_BYTES // 1024 // 1024} MB limit")
    if not content.startswith(b"%PDF"):
        raise HTTPException(status_code=400, detail="File does not appear to be a valid PDF")
    try:
        result = await run_in_threadpool(svc_import_pdf, content, project_id)
    except PdfImportError as e:
        raise HTTPException(status_code=422, detail=f"Could not extract PDF metadata: {e}") from e
    except Exception as e:
        _api_log.exception("PDF import failed")
        raise HTTPException(status_code=500, detail="PDF import failed") from e
    return {"source_id": result.source_id, "title": result.title}


# ── PDF proxy ─────────────────────────────────────────────────────────────────

_ALLOWED_PDF_HOSTS = {"arxiv.org", "ar5iv.labs.arxiv.org", "export.arxiv.org"}
_PDF_PROXY_TIMEOUT = 30.0


@app.get("/api/pdf/proxy")
async def api_pdf_proxy(url: str = Query(...)) -> StreamingResponse:
    from urllib.parse import urlparse
    host = urlparse(url).hostname or ""
    if not any(host == h or host.endswith("." + h) for h in _ALLOWED_PDF_HOSTS):
        raise HTTPException(status_code=400, detail="URL host not allowed")
    async def _guard_redirect(request: httpx.Request) -> None:
        rhost = request.url.host
        if not any(rhost == h or rhost.endswith("." + h) for h in _ALLOWED_PDF_HOSTS):
            raise httpx.RequestError(f"Redirect to disallowed host: {rhost}", request=request)

    client = httpx.AsyncClient(
        follow_redirects=True,
        timeout=_PDF_PROXY_TIMEOUT,
        event_hooks={"request": [_guard_redirect]},
    )
    resp = None
    try:
        req = client.build_request("GET", url, headers={"User-Agent": "linXiv/1.0"})
        resp = await client.send(req, stream=True)
        resp.raise_for_status()
    except httpx.HTTPStatusError as e:
        if resp is not None:
            await resp.aclose()
        await client.aclose()
        raise HTTPException(status_code=502, detail=f"Upstream returned {e.response.status_code}") from e
    except httpx.RequestError as e:
        await client.aclose()
        raise HTTPException(status_code=502, detail=f"Could not reach PDF host: {e}") from e
    except Exception:
        if resp is not None:
            await resp.aclose()
        await client.aclose()
        raise

    async def stream():
        try:
            async for chunk in resp.aiter_bytes(chunk_size=65536):
                yield chunk
        finally:
            await resp.aclose()
            await client.aclose()

    return StreamingResponse(
        stream(),
        media_type="application/pdf",
        headers={"Content-Disposition": "inline"},
    )

# ── OpenAlex save ─────────────────────────────────────────────────────────────

class OpenAlexSaveBody(BaseModel):
    source_id: str = Field(min_length=1)


class OpenAlexSaveOut(BaseModel):
    saved: bool
    source_id: str


@app.post("/api/openalex/save", response_model=OpenAlexSaveOut)
def api_openalex_save(body: OpenAlexSaveBody) -> OpenAlexSaveOut:
    try:
        meta = _openalex.fetch_by_id(body.source_id.strip())
    except OpenAlexNotFoundError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e
    except OpenAlexInputError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    except Exception as e:
        raise HTTPException(status_code=502, detail=str(e)) from e
    try:
        stored_sid, _ = save_paper_metadata(meta)
    except sqlite3.IntegrityError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    return OpenAlexSaveOut(saved=True, source_id=_strip_namespace(stored_sid))


