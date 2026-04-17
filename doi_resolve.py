"""DOI resolution (no GUI dependencies). Used by the desktop DOI page and the web API."""

from __future__ import annotations

import datetime
import json
import re
from urllib.error import URLError
from urllib.request import Request, urlopen

from sources.base import PaperMetadata

_ARXIV_DOI_RE = re.compile(
    r"10\.48550/arXiv\.(\d{4}\.\d{4,5}|[a-z\-]+/\d+)", re.IGNORECASE
)

_S2_FIELDS = "title,authors,year,abstract,externalIds,venue,publicationDate,url"


def _strip_doi_url(doi: str) -> str:
    return re.sub(r"^https?://(dx\.)?doi\.org/", "", doi.strip())


def _is_ratelimited(e: Exception) -> bool:
    return "429" in str(e)


def _fetch_url(url: str, timeout: int = 8) -> dict:
    """GET a JSON URL and return parsed dict. Raises on HTTP/network error."""
    req = Request(url, headers={"User-Agent": "linXiv/1.0 (mailto:user@example.com)"})
    with urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read())


def _try_arxiv_doi(doi: str) -> PaperMetadata | None:
    """If doi matches 10.48550/arXiv.ID, fetch directly from arXiv."""
    m = _ARXIV_DOI_RE.search(doi)
    if not m:
        return None
    arxiv_id = m.group(1)
    from fetch_paper_metadata import fetch_paper_metadata
    from sources.arxiv_source import _result_to_metadata

    try:
        result = fetch_paper_metadata(arxiv_id)
        return _result_to_metadata(result)
    except Exception as e:
        if _is_ratelimited(e):
            raise ValueError(
                "arXiv rate limit reached. Please wait ~60 s and try again."
            ) from e
        return None


def _try_semantic_scholar(doi: str) -> PaperMetadata | None:
    """
    Look up by DOI on Semantic Scholar.
    If the paper has an arXiv ID, fetch the full arXiv record.
    Otherwise build PaperMetadata from S2 fields.
    """
    try:
        data = _fetch_url(
            f"https://api.semanticscholar.org/graph/v1/paper/DOI:{doi}"
            f"?fields={_S2_FIELDS}"
        )
    except (URLError, json.JSONDecodeError, Exception):
        return None

    if not data or "title" not in data:
        return None

    arxiv_id = (data.get("externalIds") or {}).get("ArXiv")
    if arxiv_id:
        from fetch_paper_metadata import fetch_paper_metadata
        from sources.arxiv_source import _result_to_metadata

        try:
            result = fetch_paper_metadata(arxiv_id)
            return _result_to_metadata(result)
        except Exception as e:
            if _is_ratelimited(e):
                raise ValueError(
                    "arXiv rate limit reached. Please wait ~60 s and try again."
                ) from e

    pub_date: datetime.date = datetime.date.today()
    raw_date = data.get("publicationDate")
    if raw_date:
        try:
            pub_date = datetime.date.fromisoformat(raw_date)
        except ValueError:
            year = data.get("year")
            if year:
                pub_date = datetime.date(int(year), 1, 1)
    elif data.get("year"):
        pub_date = datetime.date(int(data["year"]), 1, 1)

    authors = [a["name"] for a in (data.get("authors") or []) if a.get("name")]
    venue = data.get("venue") or ""
    s2_url = data.get("url") or f"https://www.semanticscholar.org/paper/{data.get('paperId', '')}"

    return PaperMetadata(
        paper_id=doi,
        version=1,
        title=data["title"],
        authors=authors,
        published=pub_date,
        summary=data.get("abstract") or "",
        category=venue or None,
        doi=doi,
        url=s2_url,
        source="semanticscholar",
    )


def _try_crossref(doi: str) -> PaperMetadata | None:
    """Full CrossRef metadata fetch — returns whatever the registry knows."""
    try:
        data = _fetch_url(f"https://api.crossref.org/works/{doi}")
    except (URLError, json.JSONDecodeError, Exception):
        return None

    msg = data.get("message", {})
    titles = msg.get("title", [])
    if not titles:
        return None

    title = titles[0]

    authors: list[str] = []
    for a in msg.get("author", []):
        given = a.get("given", "")
        family = a.get("family", "")
        name = f"{given} {family}".strip()
        if name:
            authors.append(name)

    pub_date = datetime.date.today()
    dp = msg.get("published", {}).get("date-parts", [[]])
    if dp and dp[0]:
        parts = dp[0]
        try:
            pub_date = datetime.date(
                parts[0],
                parts[1] if len(parts) > 1 else 1,
                parts[2] if len(parts) > 2 else 1,
            )
        except (ValueError, TypeError):
            pass

    abstract = msg.get("abstract") or ""
    abstract = re.sub(r"<[^>]+>", "", abstract).strip()

    journal = (msg.get("container-title") or [""])[0]
    cr_url = msg.get("URL") or f"https://doi.org/{doi}"

    return PaperMetadata(
        paper_id=doi,
        version=1,
        title=title,
        authors=authors,
        published=pub_date,
        summary=abstract,
        category=journal or None,
        doi=doi,
        url=cr_url,
        source="crossref",
    )


def resolve_doi(doi: str) -> PaperMetadata:
    """
    Resolve a DOI to PaperMetadata via three strategies:
      1. arXiv-issued DOI  → fetch directly from arXiv
      2. Semantic Scholar  → resolves any DOI; uses arXiv ID when available
      3. CrossRef          → last resort; broadest DOI coverage
    Raises ValueError with a human-readable message on failure.
    """
    doi = _strip_doi_url(doi)
    if not doi:
        raise ValueError("Please enter a DOI.")

    meta = _try_arxiv_doi(doi)
    if meta:
        return meta

    meta = _try_semantic_scholar(doi)
    if meta:
        return meta

    meta = _try_crossref(doi)
    if meta:
        return meta

    raise ValueError(
        "Could not resolve this DOI.\n"
        "• Check the DOI is correct\n"
        "• The paper may not be indexed by Semantic Scholar or CrossRef\n"
        "• arXiv-hosted papers use DOIs starting with 10.48550/arXiv."
    )


def _resolve_doi(doi: str) -> PaperMetadata:
    """Backward-compatible name used by the PyQt DOI page."""
    return resolve_doi(doi)
