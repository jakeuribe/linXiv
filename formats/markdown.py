"""Markdown and Obsidian import/export."""

from __future__ import annotations

import datetime
import re

from sources.base import PaperMetadata

_FALLBACK_DATE = datetime.date(1900, 1, 1)
_ARXIV_URL_RE = re.compile(r"https?://arxiv\.org/abs/([^\s\)]+)")
_MD_LINK_RE   = re.compile(r"\[([^\]]+)\]\(([^\)]+)\)")


def _parse_date(val: str) -> datetime.date:
    if val:
        try:
            return datetime.date.fromisoformat(val[:10])
        except ValueError:
            pass
    return _FALLBACK_DATE


def _paper_id_from_url(url: str) -> str:
    m = _ARXIV_URL_RE.match(url)
    return m.group(1) if m else url


# ── Markdown ──────────────────────────────────────────────────────────────────

class MarkdownFormat:
    """Flat bullet-list Markdown, one paper per item."""

    format_name = "markdown"
    extensions  = [".md"]

    # ── export ────────────────────────────────────────────────────────────────

    def export_papers(self, papers: list[dict]) -> str:
        lines = ["# Selected Papers\n"]
        for p in papers:
            pid    = p.get("paper_id", "")
            title  = p.get("title", pid)
            authors = ", ".join(p.get("authors") or [])
            url    = f"https://arxiv.org/abs/{pid}"
            lines.append(f"- **[{title}]({url})**")
            if authors:
                lines.append(f"  - Authors: {authors}")
            cat = p.get("category", "")
            if cat:
                lines.append(f"  - Category: {cat}")
            tags = p.get("tags") or []
            if tags:
                lines.append(f"  - Tags: {', '.join(tags)}")
            lines.append("")
        return "\n".join(lines)

    # ── import ────────────────────────────────────────────────────────────────

    def import_file(self, path: str) -> list[PaperMetadata]:
        with open(path, encoding="utf-8") as f:
            return self._parse(f.read())

    def import_string(self, text: str) -> list[PaperMetadata]:
        return self._parse(text)

    def _parse(self, text: str) -> list[PaperMetadata]:
        results: list[PaperMetadata] = []
        current: dict | None = None

        for raw in text.splitlines():
            line = raw.strip()

            # Top-level bullet: - **[title](url)**
            if line.startswith("- **") and line.endswith("**"):
                if current is not None:
                    results.append(_dict_to_metadata(current))
                inner = line[4:-2]  # strip '- **' and '**'
                m = _MD_LINK_RE.match(inner)
                if m:
                    current = {"title": m.group(1), "url": m.group(2),
                               "paper_id": _paper_id_from_url(m.group(2))}
                else:
                    current = {"title": inner, "url": "", "paper_id": inner}
                continue

            if current is None:
                continue

            # Sub-bullets: '  - Key: value'
            if line.startswith("- Authors: "):
                current["authors"] = [a.strip() for a in line[11:].split(",") if a.strip()]
            elif line.startswith("- Category: "):
                current["category"] = line[12:].strip()
            elif line.startswith("- Tags: "):
                current["tags"] = [t.strip() for t in line[8:].split(",") if t.strip()]

        if current is not None:
            results.append(_dict_to_metadata(current))
        return results


# ── Obsidian ──────────────────────────────────────────────────────────────────

class ObsidianFormat:
    """Obsidian-flavoured Markdown with YAML frontmatter, one section per paper."""

    format_name = "obsidian"
    extensions  = [".md"]

    # ── export ────────────────────────────────────────────────────────────────

    def export_papers(self, papers: list[dict]) -> str:
        all_tags: list[str] = []
        for p in papers:
            all_tags.extend(p.get("tags") or [])
        unique_tags = sorted(set(all_tags))

        lines = ["---", f"papers: {len(papers)}"]
        if unique_tags:
            lines.append("tags:")
            for t in unique_tags:
                lines.append(f"  - {t}")
        lines += ["---", "", "# Selected Papers", ""]

        for p in papers:
            pid    = p.get("paper_id", "")
            title  = p.get("title", pid)
            authors = ", ".join(p.get("authors") or [])
            url    = f"https://arxiv.org/abs/{pid}"
            lines.append(f"## [{title}]({url})")
            lines.append("")
            if authors:
                lines.append(f"**Authors:** {authors}")
            cat = p.get("category", "")
            if cat:
                lines.append(f"**Category:** {cat}")
            tags = p.get("tags") or []
            if tags:
                lines.append(f"**Tags:** {', '.join(tags)}")
            lines.append("")

        return "\n".join(lines)

    # ── import ────────────────────────────────────────────────────────────────

    def import_file(self, path: str) -> list[PaperMetadata]:
        with open(path, encoding="utf-8") as f:
            return self._parse(f.read())

    def import_string(self, text: str) -> list[PaperMetadata]:
        return self._parse(text)

    def _parse(self, text: str) -> list[PaperMetadata]:
        # Strip YAML frontmatter
        if text.startswith("---"):
            end = text.find("\n---", 3)
            if end != -1:
                text = text[end + 4:]

        results: list[PaperMetadata] = []
        current: dict | None = None

        for raw in text.splitlines():
            line = raw.strip()

            # Section header: ## [title](url)
            if line.startswith("## "):
                if current is not None:
                    results.append(_dict_to_metadata(current))
                m = _MD_LINK_RE.match(line[3:])
                if m:
                    current = {"title": m.group(1), "url": m.group(2),
                               "paper_id": _paper_id_from_url(m.group(2))}
                else:
                    label = line[3:].strip()
                    current = {"title": label, "url": "", "paper_id": label}
                continue

            if current is None:
                continue

            if line.startswith("**Authors:**"):
                current["authors"] = [a.strip() for a in line[12:].split(",") if a.strip()]
            elif line.startswith("**Category:**"):
                current["category"] = line[13:].strip()
            elif line.startswith("**Tags:**"):
                current["tags"] = [t.strip() for t in line[9:].split(",") if t.strip()]

        if current is not None:
            results.append(_dict_to_metadata(current))
        return results


# ── shared helper ─────────────────────────────────────────────────────────────

def _dict_to_metadata(d: dict) -> PaperMetadata:
    return PaperMetadata(
        paper_id  = d.get("paper_id", ""),
        version   = 1,
        title     = d.get("title", ""),
        authors   = d.get("authors", []),
        published = _parse_date(d.get("published", "")),
        summary   = "",
        category  = d.get("category") or None,
        tags      = d.get("tags") or None,
        url       = d.get("url") or None,
        source    = "import",
    )
