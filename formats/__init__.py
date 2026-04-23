"""File-format registry for paper import/export.

Usage
-----
from formats import registry, format_for_extension

fmt = format_for_extension(".bib")   # -> BibTeXFormat instance
papers = fmt.import_file("refs.bib") # -> list[PaperMetadata]

Adding a new format
-------------------
1. Create formats/<name>.py implementing PaperFileFormat (see formats/base.py).
2. Import and register it here.
"""

from __future__ import annotations

from formats.base import PaperFileFormat
from formats.json_fmt import JSONFormat
from formats.csv_fmt import CSVFormat, TSVFormat
from formats.bibtex import BibTeXFormat
from formats.markdown import MarkdownFormat, ObsidianFormat

registry: dict[str, PaperFileFormat] = {
    "json":     JSONFormat(),
    "csv":      CSVFormat(),
    "tsv":      TSVFormat(),
    "bibtex":   BibTeXFormat(),
    "markdown": MarkdownFormat(),
    "obsidian": ObsidianFormat(),
}


def format_for_extension(ext: str) -> PaperFileFormat | None:
    """Return the format handler for a file extension (e.g. '.bib'), or None."""
    ext = ext.lower()
    for fmt in registry.values():
        if ext in fmt.extensions:
            return fmt
    return None


__all__ = ["PaperFileFormat", "registry", "format_for_extension"]
