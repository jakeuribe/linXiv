"""JSON import/export — mirrors the graph page's 'Export as JSON' output."""

from __future__ import annotations

import datetime
import json

from sources.base import PaperMetadata

_FALLBACK_DATE = datetime.date(1900, 1, 1)


def _parse_date(val: object) -> datetime.date:
    if isinstance(val, datetime.date):
        return val
    if isinstance(val, str) and val:
        try:
            return datetime.date.fromisoformat(val[:10])
        except ValueError:
            pass
    return _FALLBACK_DATE


def _parse_list(val: object, sep: str = ";") -> list[str]:
    if isinstance(val, list):
        return [str(v) for v in val]
    if isinstance(val, str) and val:
        return [v.strip() for v in val.split(sep) if v.strip()]
    return []


class JSONFormat:
    format_name = "json"
    extensions = [".json"]

    def import_file(self, path: str) -> list[PaperMetadata]:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
        raw: list[dict] = data.get("papers", data) if isinstance(data, dict) else data
        results: list[PaperMetadata] = []
        for p in raw:
            tags = _parse_list(p.get("tags"), sep=",")
            results.append(PaperMetadata(
                paper_id    = p["paper_id"],
                version     = int(p.get("version", 1)),
                title       = p.get("title", ""),
                authors     = _parse_list(p.get("authors"), sep=";"),
                published   = _parse_date(p.get("published")),
                updated     = _parse_date(p.get("updated")) if p.get("updated") else None,
                summary     = p.get("summary", ""),
                category    = p.get("category") or None,
                categories  = p.get("categories") or None,
                doi         = p.get("doi") or None,
                journal_ref = p.get("journal_ref") or None,
                comment     = p.get("comment") or None,
                url         = p.get("url") or None,
                tags        = tags or None,
                source      = p.get("source", "import"),
            ))
        return results

    def export_papers(self, papers: list[dict]) -> str:
        out = []
        for p in papers:
            entry = dict(p)
            if isinstance(entry.get("published"), (datetime.date, datetime.datetime)):
                entry["published"] = entry["published"].isoformat()
            if isinstance(entry.get("updated"), (datetime.date, datetime.datetime)):
                entry["updated"] = entry["updated"].isoformat()
            out.append(entry)
        return json.dumps({"papers": out}, indent=2, ensure_ascii=False)
