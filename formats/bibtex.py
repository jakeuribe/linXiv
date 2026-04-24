"""BibTeX import/export via pybtex."""

from __future__ import annotations

import datetime

from pybtex.database import parse_file, parse_string, BibliographyData, Entry  # type: ignore[import-untyped]

from sources.base import PaperMetadata

_FALLBACK_DATE = datetime.date(1900, 1, 1)


def _parse_year(fields: dict) -> datetime.date:
    raw = fields.get("year", "")
    try:
        return datetime.date(int(raw), 1, 1)
    except (ValueError, TypeError):
        return _FALLBACK_DATE


def _bib_to_metadata(bib) -> list[PaperMetadata]:
    results: list[PaperMetadata] = []
    for key, entry in bib.entries.items():
        f = entry.fields
        authors = [str(p) for p in entry.persons.get("author", [])]
        doi = f.get("doi") or None
        results.append(PaperMetadata(
            paper_id    = doi or key,
            version     = 1,
            title       = f.get("title", key),
            authors     = authors,
            published   = _parse_year(f),
            summary     = f.get("abstract", ""),
            doi         = doi,
            journal_ref = f.get("journal") or f.get("booktitle") or None,
            url         = f.get("url") or None,
            source      = "bibtex",
        ))
    return results


class BibTeXFormat:
    format_name = "bibtex"
    extensions = [".bib"]

    def import_file(self, path: str) -> list[PaperMetadata]:
        return _bib_to_metadata(parse_file(path))

    def import_string(self, text: str) -> list[PaperMetadata]:
        return _bib_to_metadata(parse_string(text, bib_format="bibtex"))

    def export_papers(self, papers: list[dict]) -> str:
        bib = BibliographyData()
        for p in papers:
            key = (p.get("paper_id") or "unknown").replace("/", "_").replace(".", "_")
            pub = p.get("published", "")
            year = pub.isoformat()[:4] if isinstance(pub, (datetime.date, datetime.datetime)) else str(pub)[:4]
            fields: dict[str, str] = {
                "title":    p.get("title", ""),
                "year":     year,
                "abstract": p.get("summary", ""),
            }
            if p.get("doi"):
                fields["doi"] = p["doi"]
            if p.get("journal_ref"):
                fields["journal"] = p["journal_ref"]
            if p.get("url"):
                fields["url"] = p["url"]
            bib.entries[key] = Entry("article", fields=fields)
        return bib.to_string("bibtex")
