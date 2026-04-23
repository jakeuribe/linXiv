"""MCP server exposing linXiv tools to Claude and other MCP clients."""

from __future__ import annotations

from typing import Any, Optional

from mcp.server.fastmcp import FastMCP # pyright: ignore

import storage.db as db
from storage.notes import Note, ensure_notes_db, get_notes, get_project_notes
from storage.projects import (
    Project, Status, Q,
    ensure_projects_db, filter_projects, get_project,
)
from sources.arxiv_source import ArxivSource
from sources.openalex_source import OpenAlexSource

mcp = FastMCP("linxiv")

_SOURCES = {
    "arxiv":    ArxivSource,
    "openalex": OpenAlexSource,
}

db.init_db()
ensure_projects_db()
ensure_notes_db()


def _row(row) -> dict[str, Any]:
    return {k: row[k] for k in row.keys()}


# ── Paper tools ───────────────────────────────────────────────────────────────

@mcp.tool()
def search_papers(query: str, source: str = "arxiv", max_results: int = 10) -> list[dict]:
    """Search for academic papers by keyword.

    Args:
        query: Search query string (e.g. "transformer attention mechanism").
        source: Data source — "arxiv" or "openalex".
        max_results: Maximum number of results to return (default 10).
    """
    cls = _SOURCES.get(source)
    if cls is None:
        raise ValueError(f"Unknown source {source!r}. Use 'arxiv' or 'openalex'.")
    return [r.model_dump(mode="json") for r in cls().search(query, max_results=max_results)]


@mcp.tool()
def fetch_paper(paper_id: str, source: str = "arxiv") -> dict:
    """Fetch full metadata for a paper by ID and save it to the local database.

    Args:
        paper_id: arXiv style (e.g. "2204.12985") or OpenAlex style (e.g. "W3123456789").
        source: Data source — "arxiv" or "openalex".
    """
    cls = _SOURCES.get(source)
    if cls is None:
        raise ValueError(f"Unknown source {source!r}. Use 'arxiv' or 'openalex'.")
    meta = cls().fetch_by_id(paper_id)
    db.save_paper_metadata(meta)
    return meta.model_dump(mode="json")


@mcp.tool()
def list_papers(limit: Optional[int] = None, offset: int = 0, category: Optional[str] = None) -> list[dict]:
    """List papers stored in the local database.

    Args:
        limit: Maximum number of papers to return (default: all).
        offset: Number of papers to skip for pagination.
        category: Filter by arXiv primary category (e.g. "cs.LG").
    """
    rows = db.list_papers(limit=limit, offset=offset)
    papers = [_row(r) for r in rows]
    if category:
        papers = [p for p in papers if p.get("category") == category]
    return papers


@mcp.tool()
def get_paper(paper_id: str) -> Optional[dict]:
    """Get full metadata for a single paper from the local database.

    Args:
        paper_id: The paper ID (e.g. "2204.12985").
    """
    row = db.get_paper(paper_id)
    return _row(row) if row is not None else None


@mcp.tool()
def search_full_text(query: str, limit: int = 20) -> list[dict]:
    """Full-text search over downloaded TeX source content.

    Only papers whose TeX source has been downloaded will appear.

    Args:
        query: SQLite FTS5 query string.
        limit: Maximum number of results (default 20).
    """
    return [_row(r) for r in db.search_full_text(query, limit=limit)]


@mcp.tool()
def tag_paper(paper_id: str) -> dict:
    """Generate AI tags for a paper using Google Gemini.

    The paper must already be in the local database (run fetch_paper first).

    Args:
        paper_id: The paper ID to tag (e.g. "2204.12985").
    """
    from AI_tools import PaperContent, tag

    row = db.get_paper(paper_id)
    if row is None:
        raise ValueError(f"Paper {paper_id!r} not found. Run fetch_paper first.")
    content = PaperContent(
        abstract=row["summary"] or "",
        full_text=row["full_text"] if "full_text" in row.keys() else None,
    )
    return {"paper_id": paper_id, "tags": tag(content)}


# ── Project tools ─────────────────────────────────────────────────────────────

@mcp.tool()
def list_projects(status: Optional[str] = None) -> list[dict]:
    """List research projects.

    Args:
        status: Filter by status — "active", "archived", or "deleted".
                Defaults to all non-deleted projects.
    """
    if status:
        projects = filter_projects(Q("status = ?", Status(status)))
    else:
        projects = filter_projects(~Q("status = ?", Status.DELETED))
    return [
        {
            "id": p.id,
            "name": p.name,
            "description": p.description,
            "status": p.status.value,
            "paper_count": p.paper_count,
            "paper_ids": p.paper_ids,
        }
        for p in projects
    ]


@mcp.tool()
def create_project(name: str, description: str = "") -> dict:
    """Create a new research project.

    Args:
        name: Project name.
        description: Optional description.
    """
    p = Project(name=name, description=description)
    p.save()
    return {"id": p.id, "name": p.name, "status": p.status.value}


@mcp.tool()
def add_paper_to_project(project_id: int, paper_id: str) -> dict:
    """Add a paper to an existing project.

    Args:
        project_id: Numeric project ID.
        paper_id: Paper ID to add (e.g. "2204.12985").
    """
    p = get_project(project_id)
    if p is None:
        raise ValueError(f"Project {project_id} not found.")
    p.add_paper(paper_id)
    return {"project_id": p.id, "paper_id": paper_id, "paper_count": p.paper_count}


@mcp.tool()
def remove_paper_from_project(project_id: int, paper_id: str) -> dict:
    """Remove a paper from a project.

    Args:
        project_id: Numeric project ID.
        paper_id: Paper ID to remove.
    """
    p = get_project(project_id)
    if p is None:
        raise ValueError(f"Project {project_id} not found.")
    p.remove_paper(paper_id)
    return {"project_id": p.id, "paper_id": paper_id, "paper_count": p.paper_count}


# ── Note tools ────────────────────────────────────────────────────────────────

@mcp.tool()
def create_note(
    paper_id: str,
    content: str,
    title: str = "",
    project_id: Optional[int] = None,
) -> dict:
    """Create a note attached to a paper, optionally scoped to a project.

    The paper must be in the local database (run fetch_paper first).

    Args:
        paper_id: Paper ID the note is attached to.
        content: Body text of the note.
        title: Optional note title.
        project_id: Associate the note with a specific project.
    """
    if db.get_paper(paper_id) is None:
        raise ValueError(f"Paper {paper_id!r} not found. Run fetch_paper first.")
    note = Note(paper_id=paper_id, project_id=project_id, title=title, content=content)
    note.save()
    return {"id": note.id, "paper_id": note.paper_id, "project_id": note.project_id, "title": note.title}


@mcp.tool()
def get_notes_for_paper(paper_id: str, project_id: Optional[int] = None) -> list[dict]:
    """Retrieve notes attached to a paper.

    Args:
        paper_id: Paper ID to look up notes for.
        project_id: Scope to a specific project (None returns unscoped notes).
    """
    return [
        {
            "id": n.id,
            "paper_id": n.paper_id,
            "project_id": n.project_id,
            "title": n.title,
            "content": n.content,
            "created_at": n.created_at.isoformat() if n.created_at else None,
        }
        for n in get_notes(paper_id, project_id=project_id)
    ]


@mcp.tool()
def get_notes_for_project(project_id: int) -> list[dict]:
    """Retrieve all notes scoped to a project, across all its papers.

    Args:
        project_id: Numeric project ID.
    """
    return [
        {
            "id": n.id,
            "paper_id": n.paper_id,
            "project_id": n.project_id,
            "title": n.title,
            "content": n.content,
            "created_at": n.created_at.isoformat() if n.created_at else None,
        }
        for n in get_project_notes(project_id)
    ]


if __name__ == "__main__":
    mcp.run()
