from __future__ import annotations

import datetime
import json
import re
from urllib.error import URLError
from urllib.request import urlopen, Request

from PyQt6.QtCore import Qt, QThread, pyqtSignal
from PyQt6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from db import get_paper, save_paper_metadata
from sources.base import PaperMetadata

from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED
from gui.theme import (
    FONT_TITLE, FONT_SUBHEADING, FONT_BODY, FONT_SECONDARY, FONT_TERTIARY,
    SPACE_XL, SPACE_XS,
    RADIUS_MD, RADIUS_LG,
    PAGE_MARGIN_H,
    DIALOG_PAD,
)

_GREEN  = "#4caf7d"
_RED    = "#e05c5c"

# ── DOI resolution logic ──────────────────────────────────────────────────────

_ARXIV_DOI_RE = re.compile(
    r'10\.48550/arXiv\.(\d{4}\.\d{4,5}|[a-z\-]+/\d+)', re.IGNORECASE
)

_S2_FIELDS = "title,authors,year,abstract,externalIds,venue,publicationDate,url"


def _strip_doi_url(doi: str) -> str:
    return re.sub(r'^https?://(dx\.)?doi\.org/', '', doi.strip())


def _is_ratelimited(e: Exception) -> bool:
    return "429" in str(e)


def _fetch_url(url: str, timeout: int = 8) -> dict:
    """GET a JSON URL and return parsed dict. Raises on HTTP/network error."""
    req = Request(url, headers={"User-Agent": "linXiv/1.0 (mailto:user@example.com)"})
    with urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read())


# ── Strategy 1: arXiv DOI ────────────────────────────────────────────────────

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
            raise ValueError("arXiv rate limit reached. Please wait ~60 s and try again.") from e
        return None


# ── Strategy 2: Semantic Scholar ─────────────────────────────────────────────

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

    # If there's an arXiv ID, fetch the richer arXiv record
    arxiv_id = (data.get("externalIds") or {}).get("ArXiv")
    if arxiv_id:
        from fetch_paper_metadata import fetch_paper_metadata
        from sources.arxiv_source import _result_to_metadata
        try:
            result = fetch_paper_metadata(arxiv_id)
            return _result_to_metadata(result)
        except Exception as e:
            if _is_ratelimited(e):
                raise ValueError("arXiv rate limit reached. Please wait ~60 s and try again.") from e
            # arXiv fetch failed but we have S2 data — fall through

    # Build PaperMetadata from Semantic Scholar fields
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


# ── Strategy 3: CrossRef (last resort) ───────────────────────────────────────

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

    # Authors
    authors: list[str] = []
    for a in msg.get("author", []):
        given  = a.get("given", "")
        family = a.get("family", "")
        name   = f"{given} {family}".strip()
        if name:
            authors.append(name)

    # Date
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
    # CrossRef abstracts often include JATS XML tags — strip them simply
    abstract = re.sub(r'<[^>]+>', '', abstract).strip()

    journal = (msg.get("container-title") or [""])[0]
    cr_url  = msg.get("URL") or f"https://doi.org/{doi}"

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


# ── Top-level resolver ────────────────────────────────────────────────────────

def _resolve_doi(doi: str) -> PaperMetadata:
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


# ── Worker thread ─────────────────────────────────────────────────────────────

class _LookupWorker(QThread):
    success = pyqtSignal(object)   # PaperMetadata
    error   = pyqtSignal(str)

    def __init__(self, doi: str) -> None:
        super().__init__()
        self.doi = doi

    def run(self) -> None:
        try:
            meta = _resolve_doi(self.doi)
            self.success.emit(meta)
        except ValueError as e:
            self.error.emit(str(e))
        except Exception as e:
            self.error.emit(f"Unexpected error: {e}")


# ── Page widget ───────────────────────────────────────────────────────────────

class DoiPage(QWidget):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")
        self._result: PaperMetadata | None = None
        self._worker: _LookupWorker | None = None

        outer = QVBoxLayout(self)
        outer.setContentsMargins(PAGE_MARGIN_H, 40, PAGE_MARGIN_H, 40)
        outer.setSpacing(0)

        # Header
        title_lbl = QLabel("Add by DOI")
        title_lbl.setStyleSheet(
            f"font-size: {FONT_TITLE}px; font-weight: bold; color: {_ACCENT}; background: transparent;"
        )
        sub_lbl = QLabel("Look up any paper by its DOI and add it to your library.")
        sub_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; color: {_MUTED}; background: transparent;")
        outer.addWidget(title_lbl)
        outer.addSpacing(SPACE_XS)
        outer.addWidget(sub_lbl)
        outer.addSpacing(SPACE_XL)

        # Input row
        input_row = QHBoxLayout()
        input_row.setSpacing(10)
        self._doi_input = QLineEdit()
        self._doi_input.setPlaceholderText("e.g.  10.48550/arXiv.1706.03762  or  https://doi.org/10.1038/…")
        self._doi_input.setStyleSheet(f"""
            QLineEdit {{
                background: {_PANEL}; border: 1px solid {_BORDER};
                border-radius: {RADIUS_MD}px; color: {_TEXT}; font-size: {FONT_BODY}px;
                padding: 8px 12px;
            }}
            QLineEdit:focus {{ border-color: {_ACCENT}; }}
        """)
        self._doi_input.returnPressed.connect(self._on_lookup)

        self._lookup_btn = QPushButton("Look up")
        self._lookup_btn.setFixedSize(96, 38)
        self._lookup_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._lookup_btn.setStyleSheet(f"""
            QPushButton {{
                background: {_ACCENT}; border: none; border-radius: {RADIUS_MD}px;
                color: #fff; font-size: {FONT_BODY}px; font-weight: 600;
            }}
            QPushButton:hover   {{ background: #7aa3f5; }}
            QPushButton:pressed {{ background: #4a7add; }}
            QPushButton:disabled {{ background: #2a2a4a; color: {_MUTED}; }}
        """)
        self._lookup_btn.clicked.connect(self._on_lookup)

        input_row.addWidget(self._doi_input)
        input_row.addWidget(self._lookup_btn)
        outer.addLayout(input_row)
        outer.addSpacing(16)

        # Status label
        self._status = QLabel("")
        self._status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;")
        self._status.setWordWrap(True)
        outer.addWidget(self._status)
        outer.addSpacing(16)

        # Result card (hidden until a result arrives)
        self._result_card = self._build_result_card()
        self._result_card.setVisible(False)
        outer.addWidget(self._result_card)

        outer.addStretch()

    # ── Result card ───────────────────────────────────────────────────────────

    def _build_result_card(self) -> QFrame:
        card = QFrame()
        card.setStyleSheet(f"""
            QFrame {{
                background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: {RADIUS_LG}px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """)
        lay = QVBoxLayout(card)
        lay.setContentsMargins(DIALOG_PAD, 16, DIALOG_PAD, 16)
        lay.setSpacing(6)

        self._res_title = QLabel()
        self._res_title.setWordWrap(True)
        self._res_title.setStyleSheet(f"font-size: {FONT_SUBHEADING}px; font-weight: 600; color: {_TEXT};")

        self._res_meta = QLabel()
        self._res_meta.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED};")

        self._res_abstract = QLabel()
        self._res_abstract.setWordWrap(True)
        self._res_abstract.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_TEXT}; line-height: 1.5;")

        btn_row = QHBoxLayout()
        btn_row.setSpacing(10)

        self._save_btn = QPushButton("Save to library")
        self._save_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._save_btn.setStyleSheet(f"""
            QPushButton {{
                background: {_GREEN}; border: none; border-radius: {RADIUS_MD}px;
                color: #fff; font-size: {FONT_BODY}px; font-weight: 600; padding: 8px 18px;
            }}
            QPushButton:hover   {{ background: #5dcc8f; }}
            QPushButton:pressed {{ background: #3a9e60; }}
            QPushButton:disabled {{ background: #2a2a4a; color: {_MUTED}; }}
        """)
        self._save_btn.clicked.connect(self._on_save)

        self._source_lbl = QLabel()
        self._source_lbl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")

        btn_row.addWidget(self._save_btn)
        btn_row.addStretch()
        btn_row.addWidget(self._source_lbl)

        lay.addWidget(self._res_title)
        lay.addWidget(self._res_meta)
        lay.addSpacing(6)
        lay.addWidget(self._res_abstract)
        lay.addSpacing(10)
        lay.addLayout(btn_row)
        return card

    # ── Actions ───────────────────────────────────────────────────────────────

    def _on_lookup(self) -> None:
        doi = self._doi_input.text().strip()
        if not doi:
            return
        self._set_busy(True)
        self._result_card.setVisible(False)
        self._result = None
        self._worker = _LookupWorker(doi)
        self._worker.success.connect(self._on_success)
        self._worker.error.connect(self._on_error)
        self._worker.start()

    def _on_success(self, meta: PaperMetadata) -> None:
        self._result = meta
        self._set_busy(False)
        self._status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_GREEN}; background: transparent;")
        self._status.setText("Paper found.")

        authors = ", ".join(meta.authors[:4])
        if len(meta.authors) > 4:
            authors += f" +{len(meta.authors) - 4} more"
        date = meta.published.strftime("%Y-%m-%d") if meta.published else ""
        cat  = meta.category or ""

        self._res_title.setText(meta.title)
        self._res_meta.setText("  ·  ".join(filter(None, [authors, date, cat])))
        abstract = meta.summary or ""
        self._res_abstract.setText(abstract[:400] + ("…" if len(abstract) > 400 else ""))

        source_labels = {
            "arxiv":         "arXiv",
            "semanticscholar": "Semantic Scholar",
            "crossref":      "CrossRef",
        }
        self._source_lbl.setText(f"via {source_labels.get(meta.source, meta.source)}: {meta.paper_id}")

        already = get_paper(meta.paper_id) is not None
        if already:
            self._save_btn.setText("Already in library")
            self._save_btn.setEnabled(False)
        else:
            self._save_btn.setText("Save to library")
            self._save_btn.setEnabled(True)

        self._result_card.setVisible(True)

    def _on_error(self, msg: str) -> None:
        self._set_busy(False)
        self._status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_RED}; background: transparent;")
        self._status.setText(msg)

    def _on_save(self) -> None:
        if self._result is None:
            return
        save_paper_metadata(self._result)
        self._save_btn.setText("Saved ✓")
        self._save_btn.setEnabled(False)
        self._status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_GREEN}; background: transparent;")
        self._status.setText("Paper saved to library.")

    def _set_busy(self, busy: bool) -> None:
        self._lookup_btn.setEnabled(not busy)
        self._doi_input.setEnabled(not busy)
        self._status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;")
        self._status.setText("Looking up…" if busy else "")
