"""Paper metadata source abstraction layer."""

from sources.base import PaperMetadata, PaperSource
from sources.arxiv_source import ArxivSource
from sources.openalex_source import OpenAlexSource

__all__ = ["PaperMetadata", "PaperSource", "ArxivSource", "OpenAlexSource"]
