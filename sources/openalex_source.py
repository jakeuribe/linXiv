"""OpenAlex paper source — REST API, no authentication required."""

from __future__ import annotations

import datetime
import httpx
from sources.base import PaperMetadata, PaperSource

_BASE_URL = "https://api.openalex.org"
_USER_AGENT = "linXiv/1.0 (mailto:contact@example.com)"


def _work_to_metadata(work: dict) -> PaperMetadata:
    """Convert an OpenAlex Work object to PaperMetadata."""
    raw_id = work.get("id", "")
    openalex_id = raw_id.rsplit("/", 1)[-1] if raw_id else ""
    if not openalex_id:
        raise ValueError(f"OpenAlex work has no valid ID: {work!r}")

    # Authors
    authorships = work.get("authorships", [])
    authors = [
        a["author"]["display_name"]
        for a in authorships
        if a.get("author", {}).get("display_name")
    ]

    # Published date
    pub_str = work.get("publication_date", "")
    try:
        published = datetime.date.fromisoformat(pub_str) if pub_str else datetime.date.today()
    except ValueError:
        published = datetime.date.today()

    # Category — use the primary topic's subfield if available
    primary_topic = work.get("primary_topic") or {}
    subfield = primary_topic.get("subfield", {})
    category = subfield.get("display_name")

    # URL — prefer DOI landing page, fall back to OpenAlex URL
    doi = work.get("doi")
    url = doi if doi else work.get("id")

    # Abstract — OpenAlex returns an inverted index; reconstruct it
    abstract = _reconstruct_abstract(work.get("abstract_inverted_index"))

    return PaperMetadata(
        paper_id=openalex_id,
        version=1,
        title=work.get("title", "Untitled"),
        authors=authors,
        published=published,
        summary=abstract or "No abstract available.",
        category=category,
        doi=doi,
        url=url,
        source="openalex",
    )


def _reconstruct_abstract(inverted_index: dict | None) -> str | None:
    """Reconstruct abstract text from OpenAlex's inverted index format."""
    if not inverted_index:
        return None
    word_positions: list[tuple[int, str]] = []
    for word, positions in inverted_index.items():
        for pos in positions:
            word_positions.append((pos, word))
    word_positions.sort()
    return " ".join(word for _, word in word_positions)


class OpenAlexSource(PaperSource):
    """Paper source backed by the OpenAlex REST API."""

    source_name: str = "openalex"  # written into PaperMetadata.source for every record this class produces

    def __init__(self) -> None:
        self._http = httpx.Client(
            base_url=_BASE_URL,
            headers={"User-Agent": _USER_AGENT},
            timeout=30.0,
        )

    def search(self, query: str, max_results: int = 10) -> list[PaperMetadata]:
        try:
            response = self._http.get(
                "/works",
                params={
                    "search": query,
                    "per_page": max_results,
                    "select": "id,title,authorships,publication_date,doi,"
                              "primary_topic,abstract_inverted_index",
                },
            )
            response.raise_for_status()
        except Exception as e:
            raise ValueError(f"OpenAlex search failed: {e}") from e
        results = response.json().get("results", [])
        return [_work_to_metadata(w) for w in results]

    def fetch_by_id(self, paper_id: str) -> PaperMetadata:
        # Accept both bare IDs ("W3123456789") and full URLs
        url = paper_id if paper_id.startswith("http") else f"{_BASE_URL}/works/{paper_id}"
        try:
            response = self._http.get(
                url,
                params={
                    "select": "id,title,authorships,publication_date,doi,"
                              "primary_topic,abstract_inverted_index",
                },
            )
            response.raise_for_status()
        except Exception as e:
            raise ValueError(f"OpenAlex fetch failed for '{paper_id}': {e}") from e
        return _work_to_metadata(response.json())
