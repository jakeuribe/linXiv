"""Base protocol for file-based paper import/export formats.

File formats (JSON, CSV, BibTeX, …) live here.
Source formats (arXiv, OpenAlex, …) use sources.base.PaperSource — both
converge on PaperMetadata so the DB layer treats them identically.
"""

from __future__ import annotations

from typing import Protocol, runtime_checkable

from sources.base import PaperMetadata


@runtime_checkable
class PaperFileFormat(Protocol):
    format_name: str       # e.g. "json", "csv", "bibtex"
    extensions: list[str]  # e.g. [".json"], [".bib"]

    def import_file(self, path: str) -> list[PaperMetadata]:
        """Parse a file and return PaperMetadata records."""
        ...

    def export_papers(self, papers: list[dict]) -> str:
        """Serialize paper dicts to a string in this format."""
        ...
