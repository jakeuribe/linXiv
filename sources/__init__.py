"""Paper metadata source abstraction layer.

Add new sources by implementing PaperSource and importing them here.
"""

from sources.base import PaperMetadata, PaperSource
from sources.arxiv_source import ArxivSource
from sources.openalex_source import OpenAlexSource

__all__ = ["PaperMetadata", "PaperSource", "ArxivSource", "OpenAlexSource"]
