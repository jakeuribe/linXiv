from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

import storage.authors as _authors_storage
from service.models.author import BasicAuthorDetails, FullAuthorDetails


# ---------------------------------------------------------------------------
# Service models — derived from AUTHOR and PAPER_TO_AUTHOR schema
# ---------------------------------------------------------------------------

@dataclass
class Author:
    author_id: int | None = None  # AUTHOR_FK
    orcid:     str | None = None  # look up by ORCID when author_id absent


@dataclass
class Authors:
    paper_id:   int | None          = None
    name:       Optional[list[str]] = None
    author_ids: list[int] | None    = None


# Basic model sent from the GUI when creating or updating an author
@dataclass
class AuthorIn:
    full_name:  str
    first_name: str | None = None
    last_name:  str | None = None
    orcid:      str | None = None


# ---------------------------------------------------------------------------
# Internal helpers (pending storage/authors.py)
# ---------------------------------------------------------------------------

def _get_author(author_id: int) -> Optional[BasicAuthorDetails]:
    return _authors_storage.get_author(author_id)


def _list_authors(paper_id: int | None = None, name: str | None = None) -> list[BasicAuthorDetails]:
    return _authors_storage.list_authors(paper_id=paper_id, name=name)


def _get_author_papers(author_id: int) -> list[dict]:
    return _authors_storage.get_author_papers(author_id)


# ---------------------------------------------------------------------------
# Master functions
# ---------------------------------------------------------------------------

def get(author: Author) -> Optional[BasicAuthorDetails]:
    """Fetch a single author. Resolution order: author_id → orcid."""
    if author.author_id is not None:
        return _get_author(author.author_id)
    if author.orcid is not None:
        for row in _list_authors():
            if row.orcid == author.orcid:
                return row
    return None


def get_many(authors: Authors) -> list[BasicAuthorDetails]:
    """Fetch authors matching any combination of Authors filter fields."""
    name_filter = authors.name[0] if authors.name and len(authors.name) == 1 else None
    rows = _list_authors(paper_id=authors.paper_id, name=name_filter)
    if authors.name and len(authors.name) > 1:
        name_set = set(authors.name)
        rows = [r for r in rows if r.full_name in name_set]
    if authors.author_ids:
        id_set = set(authors.author_ids)
        rows = [r for r in rows if r.author_id in id_set]
    return rows


def upsert(author: AuthorIn) -> int | None:
    return _authors_storage.create_author(
        full_name  = author.full_name,
        first_name = author.first_name,
        last_name  = author.last_name,
        orcid      = author.orcid,
    )


def delete(author: Author) -> None:
    if author.author_id is not None:
        _authors_storage.delete_author(author.author_id)


# ---------------------------------------------------------------------------
# Low-level reads
# ---------------------------------------------------------------------------

def get_author_details(author: Author) -> Optional[BasicAuthorDetails]:
    if author.author_id is None:
        return None
    return _get_author(author.author_id)


def get_full_author_details(author: Author) -> Optional[FullAuthorDetails]:
    if author.author_id is None:
        return None
    row = _get_author(author.author_id)
    if row is None:
        return None
    paper_ids = [p["paper_id"] for p in _get_author_papers(author.author_id)]
    return FullAuthorDetails(
        author_id  = row.author_id,
        orcid      = row.orcid,
        full_name  = row.full_name,
        first_name = row.first_name,
        last_name  = row.last_name,
        paper_ids  = paper_ids,
    )


def get_authors(authors: Authors) -> list[BasicAuthorDetails]:
    name_filter = authors.name[0] if authors.name and len(authors.name) == 1 else None
    rows = _list_authors(paper_id=authors.paper_id, name=name_filter)
    if authors.name and len(authors.name) > 1:
        name_set = set(authors.name)
        rows = [r for r in rows if r.full_name in name_set]
    return rows


def get_paper_authors(paper_id: int) -> list[BasicAuthorDetails]:
    """Return authors for a paper version ordered by AUTHOR_INDEX."""
    return _list_authors(paper_id=paper_id)


# ---------------------------------------------------------------------------
# AUTHOR writes
# ---------------------------------------------------------------------------

def create_author(author: AuthorIn) -> int | None:
    return _authors_storage.create_author(
        full_name  = author.full_name,
        first_name = author.first_name,
        last_name  = author.last_name,
        orcid      = author.orcid,
    )


def update_author(author_id: int, update: AuthorIn) -> None:
    _authors_storage.update_author(
        author_id  = author_id,
        full_name  = update.full_name,
        first_name = update.first_name,
        last_name  = update.last_name,
        orcid      = update.orcid,
    )


def delete_author(author_id: int) -> None:
    _authors_storage.delete_author(author_id)


# ---------------------------------------------------------------------------
# PAPER_TO_AUTHOR link management
# ---------------------------------------------------------------------------

def link_author_to_paper(author_id: int, paper_id: int, author_index: int | None = None) -> None:
    _authors_storage.link_author_to_paper(author_id, paper_id, author_index)


def unlink_author_from_paper(author_id: int, paper_id: int) -> None:
    _authors_storage.unlink_author_from_paper(author_id, paper_id)
