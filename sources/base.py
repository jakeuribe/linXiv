"""Base protocol and data model for paper metadata sources."""

from __future__ import annotations

import datetime
from typing import Protocol
from pydantic import BaseModel


class PaperMetadata(BaseModel):
    """Normalized paper representation (source-agnostic)."""
    paper_id: str       # source-specific ID (arxiv: 2204.12985, openalex: W3123456789)
    version: int        # defaults to 1 for non-arxiv sources
    title: str
    authors: list[str]
    published: datetime.date
    updated: datetime.date | None = None
    summary: str
    category: str | None = None
    categories: list[str] | None = None
    doi: str | None = None
    journal_ref: str | None = None
    comment: str | None = None
    url: str | None = None
    tags: list[str] | None = None
    source: str = "arxiv"


class PaperSource(Protocol):
    """Unified interface for paper metadata providers."""

    @property
    def source_name(self) -> str:
        """Short identifier for this source (e.g. 'arxiv', 'openalex')."""
        ...

    def search(self, query: str, max_results: int = 10) -> list[PaperMetadata]:
        """Search for papers matching a query string."""
        ...

    def fetch_by_id(self, paper_id: str) -> PaperMetadata:
        """Fetch metadata for a specific paper by its source-specific ID."""
        ...
