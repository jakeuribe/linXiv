"""Paper metadata source abstraction layer.

Add new sources by implementing PaperSource and importing them here.
"""

from .base import PaperMetadata, PaperSource
from .arxiv_source import ArxivSource
from .openalex_source import OpenAlexSource
from .crossref_source import CrossRefSource, fetch_by_doi, search_by_title
from .fetch_paper_metadata import fetch_paper_metadata, search_papers, gen_md_file, gen_md_files
from .doi_resolve import resolve_doi, _resolve_doi
from .pdf_metadata import extract_pdf_metadata, resolve_pdf_metadata

__all__ = [
    "PaperMetadata", "PaperSource", "ArxivSource", "OpenAlexSource",
    "CrossRefSource", "fetch_by_doi", "search_by_title",
    "fetch_paper_metadata", "search_papers", "gen_md_file", "gen_md_files",
    "resolve_doi", "_resolve_doi",
    "extract_pdf_metadata", "resolve_pdf_metadata",
]
