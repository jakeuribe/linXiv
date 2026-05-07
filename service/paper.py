from __future__ import annotations

import sqlite3
from dataclasses import dataclass
from datetime import date
from typing import TYPE_CHECKING

import storage.db as db
from service.models.paper import PaperDetails, PaperDetailsAll
from sources.base import PaperMetadata

if TYPE_CHECKING:
    import arxiv


# ---------------------------------------------------------------------------
# Service models — derived from the `papers` view schema
# ---------------------------------------------------------------------------

@dataclass
class Paper:
    """Identifies a single paper — supply exactly one of the three keys.

    source_fk  — PAPER_ROOTS.SOURCE_FK (paper identity, any version)
    paper_id   — PAPER.PAPER_ID        (specific version, PK)
    source_id  — human-readable arxiv ID; pair with version to pin a version
    """
    source_fk:  int | None = None
    paper_id:   int | None = None
    source_id:  str | None = None
    version:    int | None = None  # only meaningful alongside source_id


@dataclass
class Papers:
    """Filter criteria for listing multiple papers."""
    source_fks:  list[int] | None = None
    paper_ids:   list[int] | None = None
    source_ids:  list[str] | None = None
    project_fks: list[int] | None = None
    tags:        list[str] | None = None


# Basic model sent from the GUI when manually adding a paper
@dataclass
class PaperIn:
    title:       str
    published:   date
    source_id:   str | None       = None
    version:     int | None       = None
    authors:     list[str] | None = None
    summary:     str | None       = None
    category:    str | None       = None
    doi:         str | None       = None
    url:         str | None       = None
    tags:        list[str] | None = None
    source:      str | None       = None


# ---------------------------------------------------------------------------
# DB lifecycle
# ---------------------------------------------------------------------------

def init_db() -> None:
    db.init_db()


# ---------------------------------------------------------------------------
# Utility
# ---------------------------------------------------------------------------

def parse_entry_id(entry_id: str) -> tuple[str, int]:
    return db.parse_entry_id(entry_id)


# ---------------------------------------------------------------------------
# Row → dataclass helpers
# ---------------------------------------------------------------------------

def _row_to_paper_details(row: sqlite3.Row) -> PaperDetails:
    return PaperDetails(
        paper_id=row["paper_id"],
        source_id=row["source_id"],
        version=row["version"],
        title=row["title"],
        summary=row["summary"],
        published=row["published"],
        updated=row["updated"],
        url=row["url"],
        doi=row["doi"],
        category=row["category"],
        categories=row["categories"],
        journal_ref=row["journal_ref"],
        comment=row["comment"],
        authors=row["authors"],
        tags=row["tags"],
        has_pdf=row["has_pdf"],
        pdf_path=row["pdf_path"],
        source=row["source"],
        full_text=row["full_text"],
        downloaded_source=row["downloaded_source"],
    )


# ---------------------------------------------------------------------------
# Master getter — dispatches on whichever Paper field is populated
# ---------------------------------------------------------------------------

def get(paper: Paper) -> PaperDetails | None:
    """Fetch a single paper version.

    Resolution order:
      paper_id   → exact PAPER row by PK
      source_fk  → latest version for this PAPER_ROOTS row
      source_id  → latest version, or pinned version if Paper.version is set
    """
    if paper.paper_id is not None:
        row = db.get_paper_by_id(paper.paper_id)
    elif paper.source_fk is not None:
        row = db.get_paper_by_source_fk(paper.source_fk)
    elif paper.source_id is not None:
        row = db.get_paper(paper.source_id, paper.version)
    else:
        return None
    if row is None:
        return None
    return _row_to_paper_details(row)


def get_all(paper: Paper) -> PaperDetailsAll | None:
    """Fetch all stored versions of a paper, display fields from the latest.

    Accepts the same Paper key variants as get().
    """
    if paper.source_id is not None:
        source_id = paper.source_id
    elif paper.paper_id is not None:
        row = db.get_paper_by_id(paper.paper_id)
        if row is None:
            return None
        source_id = row["source_id"]
    elif paper.source_fk is not None:
        row = db.get_paper_by_source_fk(paper.source_fk)
        if row is None:
            return None
        source_id = row["source_id"]
    else:
        return None

    rows = db.get_all_versions(source_id)
    if not rows:
        return None

    latest = rows[-1]
    versions = [_row_to_paper_details(r) for r in rows]

    return PaperDetailsAll(
        source_id=source_id,
        latest_version=latest["version"],
        title=latest["title"],
        authors=latest["authors"],
        summary=latest["summary"],
        published=rows[0]["published"],
        updated=latest["updated"],
        doi=latest["doi"],
        url=latest["url"],
        category=latest["category"],
        categories=latest["categories"],
        journal_ref=latest["journal_ref"],
        comment=latest["comment"],
        tags=latest["tags"],
        source=latest["source"],
        versions=versions,
    )


def get_many(papers: Papers) -> list[PaperDetails]:
    """Fetch multiple papers matching any combination of Papers filter fields."""
    rows = db.list_papers(latest_only=True)
    results: list[PaperDetails] = []
    for row in rows:
        if papers.paper_ids is not None and row["paper_id"] not in papers.paper_ids:
            continue
        if papers.source_ids is not None and row["source_id"] not in papers.source_ids:
            continue
        if papers.tags is not None:
            row_tags = row["tags"] or []
            if not any(t in row_tags for t in papers.tags):
                continue
        results.append(_row_to_paper_details(row))

    if papers.source_fks is not None:
        fk_set = set(papers.source_fks)
        fk_results: list[PaperDetails] = []
        for detail in results:
            root = db.get_paper_root(detail.source_id)
            if root is not None and root["SOURCE_FK"] in fk_set:
                fk_results.append(detail)
        results = fk_results

    return results


# ---------------------------------------------------------------------------
# Master upsert — inserts or updates based on whether the paper already exists
# ---------------------------------------------------------------------------

def upsert(paper: PaperIn, tags: list[str] | None = None) -> tuple[str, int]:
    """Insert a new paper or update an existing one.

    Existence is determined by source_id + version when both are present,
    or source_id alone (updating latest) when version is omitted.
    Returns (source_id, version).
    """
    meta = PaperMetadata(
        paper_id=paper.source_id or "",
        version=paper.version or 1,
        title=paper.title,
        authors=paper.authors or [],
        published=paper.published,
        updated=None,
        summary=paper.summary or "",
        category=paper.category,
        categories=None,
        doi=paper.doi,
        journal_ref=None,
        comment=None,
        url=paper.url,
        tags=paper.tags,
        source=paper.source,
    )
    return db.save_paper_metadata(meta, tags)


# ---------------------------------------------------------------------------
# Low-level reads (used internally and by specific callsites)
# ---------------------------------------------------------------------------

def get_paper(source_id: str, version: int | None = None) -> sqlite3.Row | None:
    return db.get_paper(source_id, version)

def get_paper_details(paper: Paper) -> PaperDetails | None:
    return get(paper)

def get_paper_details_all(paper: Paper) -> PaperDetailsAll | None:
    return get_all(paper)

def get_all_versions(source_id: str) -> list[sqlite3.Row]:
    return db.get_all_versions(source_id)

def get_paper_root(source_id: str) -> sqlite3.Row | None:
    """Return the PAPER_ROOTS row for a given source_id."""
    return db.get_paper_root(source_id)


# ---------------------------------------------------------------------------
# Multi-paper reads
# ---------------------------------------------------------------------------

def list_papers(
    latest_only: bool = True,
    limit: int | None = None,
    offset: int = 0,
) -> list[sqlite3.Row]:
    return db.list_papers(latest_only=latest_only, limit=limit, offset=offset)

def get_papers(papers: Papers) -> list[PaperDetails]:
    return get_many(papers)

def get_graph_data() -> tuple[list[dict], list[dict]]:
    return db.get_graph_data()

def get_categories() -> list[str]:
    return db.get_categories()


# ---------------------------------------------------------------------------
# Write / mutate
# ---------------------------------------------------------------------------

def save_paper(paper: arxiv.Result, tags: list[str] | None = None) -> tuple[str, int]:
    return db.save_paper(paper, tags)

def save_papers(papers: list[arxiv.Result], tags: list[str] | None = None) -> list[tuple[str, int]]:
    return db.save_papers(papers, tags)

def save_paper_metadata(meta: PaperMetadata, tags: list[str] | None = None) -> tuple[str, int]:
    return db.save_paper_metadata(meta, tags)

def save_papers_metadata(metas: list[PaperMetadata], tags: list[str] | None = None) -> list[tuple[str, int]]:
    return db.save_papers_metadata(metas, tags)

def repair_paper(old_source_id: str, meta: PaperMetadata) -> str:
    """Re-write a paper's metadata in-place, migrating FK references if source_id changes."""
    return db.repair_paper(old_source_id, meta)

def delete_paper(source_id: str) -> None:
    db.delete_paper(source_id)


# ---------------------------------------------------------------------------
# PDF management
# ---------------------------------------------------------------------------

def set_has_pdf(source_id: str, version: int, has: bool) -> None:
    db.set_has_pdf(source_id, version, has)

def set_pdf_path(source_id: str, path: str) -> None:
    db.set_pdf_path(source_id, path)


# ---------------------------------------------------------------------------
# Full-text / FTS (papers_fts virtual table)
# ---------------------------------------------------------------------------

def set_full_text(source_id: str, version: int, full_text: str) -> None:
    db.set_full_text(source_id, version, full_text)

def search_full_text(query: str, limit: int = 20) -> list[sqlite3.Row]:
    return db.search_full_text(query, limit)


# ---------------------------------------------------------------------------
# Tag associations on papers (PAPER_TO_TAG)
# ---------------------------------------------------------------------------

def add_paper_tags(source_id: str, tags: list[str]) -> list[str]:
    return db.add_paper_tags(source_id, tags)

def remove_paper_tags(source_id: str, tags: list[str]) -> list[str]:
    return db.remove_paper_tags(source_id, tags)