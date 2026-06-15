"""OpenAlex paper source — REST API, no authentication required.

Include a mailto address in OPENALEX_MAILTO env var to use OpenAlex's polite pool.
"""

from __future__ import annotations

import datetime
import os
import re
import httpx
from sources.base import PaperMetadata, PaperSource

_BASE_URL = "https://api.openalex.org"
_USER_AGENT = "linXiv/1.0"


def _user_agent() -> str:
    addr = os.environ.get("OPENALEX_MAILTO", "").replace("\r", "").replace("\n", "")
    return f"{_USER_AGENT} (mailto:{addr})" if addr else _USER_AGENT


_OPENALEX_WORK_FIELDS = (
    "id,title,authorships,publication_date,doi,"
    "primary_topic,abstract_inverted_index"
)
_WORK_ID_RE = re.compile(r"^W\d+$")

# OpenAlex parses | ! * ? in the `search` value as query operators and returns
# HTTP 400 for free-text containing them; replace each run with a space.
_SEARCH_OPERATOR_RE = re.compile(r"[|!*?]+")


def _sanitize_search_query(query: str) -> str:
    return " ".join(_SEARCH_OPERATOR_RE.sub(" ", query).split())


class OpenAlexNotFoundError(LookupError):
    """Raised by fetch_by_id when the work ID does not exist on OpenAlex."""


class OpenAlexHTTPError(Exception):
    """Raised when OpenAlex returns a non-404 HTTP error status."""

    def __init__(self, message: str, status: int) -> None:
        super().__init__(message)
        self.status = status


class OpenAlexInputError(ValueError):
    """Raised for invalid or malformed source_id inputs before any network call."""


# Maps the UI sort key to the OpenAlex sort query parameter value.
_SORT_PARAM: dict[str, str] = {
    "relevance": "relevance_score:desc",
    "newest":    "publication_date:desc",
    "oldest":    "publication_date:asc",
    "citations": "cited_by_count:desc",
}


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
        published = datetime.date.fromisoformat(pub_str) if pub_str else datetime.date.min
    except ValueError:
        published = datetime.date.min

    # Category — use the primary topic's subfield if available
    primary_topic = work.get("primary_topic") or {}
    subfield = primary_topic.get("subfield") or {}
    category = subfield.get("display_name")

    # URL — prefer DOI landing page, fall back to OpenAlex URL
    doi = work.get("doi")
    url = doi if doi else work.get("id")

    # Abstract — OpenAlex returns an inverted index; reconstruct it
    abstract = _reconstruct_abstract(work.get("abstract_inverted_index"))

    return PaperMetadata(
        source_id=f"openalex:{openalex_id}",
        version=1,
        title=work.get("title") or "",
        authors=authors,
        published=published,
        summary=abstract or "",
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

    @property
    def source_name(self) -> str:
        return "openalex"

    def __init__(self) -> None:
        self._http = httpx.Client(
            base_url=_BASE_URL,
            timeout=30.0,
        )

    def search(
        self,
        query: str,
        max_results: int = 10,
        sort: str = "relevance",
    ) -> list[PaperMetadata]:
        if sort not in _SORT_PARAM:
            raise ValueError(f"unknown sort {sort!r}; valid: {sorted(_SORT_PARAM)}")
        sanitized = _sanitize_search_query(query)
        # An empty search returns OpenAlex's unfiltered work list; skip the call.
        if not sanitized:
            return []
        params: dict[str, str | int] = {
            "search": sanitized,
            "per_page": max_results,
            "select": _OPENALEX_WORK_FIELDS,
            "sort": _SORT_PARAM[sort],
        }
        try:
            response = self._http.get(
                "/works", params=params, headers={"User-Agent": _user_agent()}
            )
            response.raise_for_status()
            raw_results = response.json().get("results", [])
        except httpx.HTTPStatusError as e:
            status = e.response.status_code
            raise OpenAlexHTTPError(
                f"OpenAlex search failed: HTTP {status}", status
            ) from e
        except Exception as e:
            raise ValueError(f"OpenAlex search failed: {e}") from e
        results = []
        for work in raw_results:
            try:
                results.append(_work_to_metadata(work))
            except Exception as e:
                print(f"[openalex] skipping malformed work record: {e}")
        return results

    def fetch_by_id(self, source_id: str) -> PaperMetadata:
        bare_id = source_id.removeprefix("openalex:")
        # Normalise any URL form (API or landing page) to a bare work ID.
        if bare_id.startswith(("http://", "https://")):
            bare_id = bare_id.rsplit("/", 1)[-1]
        if not bare_id:
            raise OpenAlexInputError(
                f"source_id '{source_id}' resolves to an empty work ID."
            )
        if not _WORK_ID_RE.fullmatch(bare_id):
            raise OpenAlexInputError(
                f"Invalid OpenAlex work ID '{bare_id}': expected 'W' followed by digits."
            )
        try:
            response = self._http.get(
                f"/works/{bare_id}",
                params={"select": _OPENALEX_WORK_FIELDS},
                headers={"User-Agent": _user_agent()},
            )
            response.raise_for_status()
            return _work_to_metadata(response.json())
        except httpx.HTTPStatusError as e:
            status = e.response.status_code
            if status == 404:
                raise OpenAlexNotFoundError(
                    f"Paper '{source_id}' not found on OpenAlex."
                ) from e
            raise OpenAlexHTTPError(
                f"OpenAlex returned HTTP {status} for '{source_id}'.", status
            ) from e
        except Exception as e:
            raise ValueError(f"OpenAlex fetch failed for '{source_id}': {e}") from e
