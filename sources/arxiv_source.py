"""ArXiv paper source — wraps the arxiv Python client."""

from __future__ import annotations

import re
import arxiv
from sources.base import PaperMetadata, PaperSource
from .fetch_paper_metadata import _check_ratelimit, _record_ratelimit


def _parse_arxiv_id(entry_id: str) -> tuple[str, int]:
    """Split 'http://arxiv.org/abs/2204.12985v4' into ('2204.12985', 4)."""
    raw = entry_id.split("/")[-1]
    match = re.match(r"^(.+?)(?:v(\d+))?$", raw)
    assert match is not None
    paper_id = match.group(1)
    version = int(match.group(2)) if match.group(2) else 1
    return paper_id, version


def _result_to_metadata(result: arxiv.Result) -> PaperMetadata:
    """Convert an arxiv.Result to a PaperMetadata."""
    paper_id, version = _parse_arxiv_id(result.entry_id)
    return PaperMetadata(
        paper_id=paper_id,
        version=version,
        title=result.title,
        authors=[a.name for a in result.authors],
        published=result.published.date(),
        updated=result.updated.date() if result.updated else None,
        summary=result.summary,
        category=result.primary_category,
        categories=result.categories,
        doi=result.doi,
        journal_ref=result.journal_ref,
        comment=result.comment,
        url=result.pdf_url,
        source="arxiv",
    )


class ArxivSource(PaperSource):
    """Paper source backed by the arXiv API."""

    source_name: str = "arxiv"  # written into PaperMetadata.source for every record this class produces

    # TODO: should these be hardcoded?
    def __init__(self) -> None:
        self._client = arxiv.Client(num_retries=1, delay_seconds=7.0)

    def search(self, query: str, max_results: int = 10) -> list[PaperMetadata]:
        search = arxiv.Search(
            query=query,
            max_results=max_results,
            sort_by=arxiv.SortCriterion.Relevance,
            sort_order=arxiv.SortOrder.Descending,
        )
        _check_ratelimit()
        try:
            results = list(self._client.results(search))
        except Exception as e:
            print(f"[arxiv] search error: {e}")
            if "429" in str(e):
                _record_ratelimit()
            raise ValueError(f"arXiv search failed: {e}") from e
        return [_result_to_metadata(r) for r in results]

    def fetch_by_id(self, paper_id: str) -> PaperMetadata:
        search = arxiv.Search(id_list=[paper_id])
        _check_ratelimit()
        try:
            result = next(self._client.results(search))
        except StopIteration:
            raise ValueError(f"Paper '{paper_id}' not found on arXiv.")
        except Exception as e:
            print(f"[arxiv] fetch error: {e}")
            if "429" in str(e):
                _record_ratelimit()
            raise ValueError(f"arXiv fetch failed for '{paper_id}': {e}") from e
        return _result_to_metadata(result)
