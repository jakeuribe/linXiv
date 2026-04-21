"""CSV/TSV import/export — mirrors the graph page's 'Export as CSV' output.

Column order: paper_id, title, authors, category, tags, published, has_pdf
- authors: semicolon-separated
- tags:    comma-separated
- has_pdf: "Y" / "N"
"""

from __future__ import annotations

import csv
import datetime
import io

from sources.base import PaperMetadata

_FALLBACK_DATE = datetime.date(1900, 1, 1)
_FIELDS = ["paper_id", "title", "authors", "category", "tags", "published", "has_pdf"]


def _parse_date(val: str) -> datetime.date:
    if val:
        try:
            return datetime.date.fromisoformat(val[:10])
        except ValueError:
            pass
    return _FALLBACK_DATE


class CSVFormat:
    format_name = "csv"
    extensions = [".csv"]
    _delimiter = ","

    def import_file(self, path: str) -> list[PaperMetadata]:
        results: list[PaperMetadata] = []
        with open(path, encoding="utf-8", newline="") as f:
            reader = csv.DictReader(f, delimiter=self._delimiter)
            for row in reader:
                paper_id = row.get("paper_id", "").strip()
                if not paper_id:
                    continue
                authors_raw = row.get("authors", "")
                authors = [a.strip() for a in authors_raw.split(";") if a.strip()]
                tags_raw = row.get("tags", "")
                tags = [t.strip() for t in tags_raw.split(",") if t.strip()]
                results.append(PaperMetadata(
                    paper_id  = paper_id,
                    version   = 1,
                    title     = row.get("title", ""),
                    authors   = authors,
                    published = _parse_date(row.get("published", "")),
                    summary   = "",
                    category  = row.get("category") or None,
                    tags      = tags or None,
                    source    = "import",
                ))
        return results

    def export_papers(self, papers: list[dict]) -> str:
        buf = io.StringIO()
        writer = csv.DictWriter(
            buf, fieldnames=_FIELDS, extrasaction="ignore", delimiter=self._delimiter
        )
        writer.writeheader()
        for p in papers:
            pub = p.get("published", "")
            if isinstance(pub, (datetime.date, datetime.datetime)):
                pub = pub.isoformat()
            writer.writerow({
                "paper_id":  p.get("paper_id", ""),
                "title":     p.get("title", ""),
                "authors":   "; ".join(p.get("authors") or []),
                "category":  p.get("category", ""),
                "tags":      ", ".join(p.get("tags") or []),
                "published": pub,
                "has_pdf":   "Y" if p.get("has_pdf") else "N",
            })
        return buf.getvalue()


class TSVFormat(CSVFormat):
    format_name = "tsv"
    extensions = [".tsv"]
    _delimiter = "\t"
