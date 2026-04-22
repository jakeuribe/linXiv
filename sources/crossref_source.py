"""CrossRef paper source — REST API, no authentication required.

Include a mailto address in CROSSREF_MAILTO env var to use CrossRef's polite pool.
"""

from __future__ import annotations

import datetime
import os
import re

import httpx

from sources.base import PaperMetadata

CROSSREF_BASE = "https://api.crossref.org/works"


def _mailto_header() -> str:
    addr = os.environ.get("CROSSREF_MAILTO", "")
    return f"linXiv/1.0 (mailto:{addr})" if addr else "linXiv/1.0"


def _parse_crossref_work(msg: dict, doi: str = "") -> PaperMetadata:
    """Convert a CrossRef work message dict to PaperMetadata."""
    titles = msg.get("title", [])
    title = titles[0] if titles else ""

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
    paper_doi = doi or msg.get("DOI", "")
    url = msg.get("URL") or (f"https://doi.org/{paper_doi}" if paper_doi else None)

    return PaperMetadata(
        paper_id=paper_doi,
        version=1,
        title=title,
        authors=authors,
        published=pub_date,
        summary=abstract,
        category=journal or None,
        doi=paper_doi or None,
        url=url,
        source="crossref",
    )


def fetch_by_doi(doi: str) -> PaperMetadata | None:
    """Fetch CrossRef metadata for a DOI. Returns None on any error."""
    try:
        with httpx.Client(headers={"User-Agent": _mailto_header()}, timeout=10.0) as client:
            resp = client.get(f"{CROSSREF_BASE}/{doi}")
        if resp.status_code != 200:
            return None
        msg = resp.json().get("message", {})
        if not msg.get("title"):
            return None
        return _parse_crossref_work(msg, doi=doi)
    except Exception:
        return None


def search_by_title(title: str, limit: int = 5) -> list[PaperMetadata]:
    """Search CrossRef by title. Returns empty list on any error."""
    try:
        with httpx.Client(headers={"User-Agent": _mailto_header()}, timeout=10.0) as client:
            resp = client.get(
                CROSSREF_BASE,
                params={"query.title": title, "rows": limit},
            )
        if resp.status_code != 200:
            return []
        items = resp.json().get("message", {}).get("items", [])
        results = []
        for item in items:
            doi = item.get("DOI", "")
            if item.get("title") and doi:
                results.append(_parse_crossref_work(item, doi=doi))
        return results
    except Exception:
        return []


class CrossRefSource:
    """Paper source backed by the CrossRef REST API."""

    source_name: str = "crossref"

    def search(self, query: str, max_results: int = 10) -> list[PaperMetadata]:
        results = search_by_title(query, limit=max_results)
        if results is None:
            raise ValueError("CrossRef search failed")
        return results

    def fetch_by_id(self, doi: str) -> PaperMetadata:
        meta = fetch_by_doi(doi)
        if meta is None:
            raise ValueError(f"CrossRef: no record found for DOI '{doi}'")
        return meta
