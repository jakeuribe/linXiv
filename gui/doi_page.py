from __future__ import annotations

import json
import re
from urllib.error import URLError
from urllib.request import urlopen, Request

import arxiv
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

from db import get_paper, save_paper, _parse_entry_id
from fetch_paper_metadata import fetch_paper_metadata, search_papers

_BG     = "#0f0f1a"
_PANEL  = "#1a1a2e"
_BORDER = "#2e2e50"
_ACCENT = "#5b8dee"
_TEXT   = "#ccccdd"
_MUTED  = "#7777aa"
_GREEN  = "#4caf7d"
_RED    = "#e05c5c"

# ── DOI resolution logic ──────────────────────────────────────────────────────

_ARXIV_DOI_RE = re.compile(
    r'10\.48550/arXiv\.(\d{4}\.\d{4,5}|[a-z\-]+/\d+)', re.IGNORECASE
)
_ARXIV_ID_RE = re.compile(r'\d{4}\.\d{4,5}|[a-z\-]+/\d+')


def _strip_doi_url(doi: str) -> str:
    return re.sub(r'^https?://(dx\.)?doi\.org/', '', doi.strip())


def _resolve_doi(doi: str) -> arxiv.Result:
    """
    Try three strategies in order:
      1. arXiv-issued DOI  → extract ID, fetch directly
      2. jr: field search  → arXiv journal-reference index
      3. CrossRef lookup   → get title, then search arXiv by title
    Raises ValueError with a human-readable message on failure.
    """
    doi = _strip_doi_url(doi)
    if not doi:
        raise ValueError("Please enter a DOI.")

    # Strategy 1: arXiv DOI (10.48550/arXiv.XXXX.XXXXX)
    m = _ARXIV_DOI_RE.search(doi)
    if m:
        arxiv_id = m.group(1)
        try:
            return fetch_paper_metadata(arxiv_id)
        except Exception:
            pass

    # Strategy 2: journal-reference search
    try:
        results = search_papers(f'jr:"{doi}"', max_results=3)
        if results:
            return results[0]
    except Exception:
        pass

    # Strategy 3: CrossRef → title → arXiv search
    title = _crossref_title(doi)
    if title:
        try:
            results = search_papers(f'ti:"{title}"', max_results=5)
            if results:
                return results[0]
        except Exception:
            pass
        raise ValueError(
            f"Found on CrossRef as \"{title}\" but could not locate it on arXiv.\n"
            "This paper may not have an arXiv preprint."
        )

    raise ValueError(
        "Could not resolve this DOI.\n"
        "• Check the DOI is correct\n"
        "• The paper may not be on arXiv\n"
        "• arXiv DOIs start with 10.48550/arXiv."
    )


def _crossref_title(doi: str) -> str | None:
    """Query the free CrossRef API for a paper's title. Returns None on failure."""
    try:
        url = f"https://api.crossref.org/works/{doi}"
        req = Request(url, headers={"User-Agent": "linXiv/1.0 (mailto:user@example.com)"})
        with urlopen(req, timeout=8) as resp:
            data = json.loads(resp.read())
        titles = data.get("message", {}).get("title", [])
        return titles[0] if titles else None
    except (URLError, KeyError, json.JSONDecodeError, IndexError):
        return None


# ── Worker thread ─────────────────────────────────────────────────────────────

class _LookupWorker(QThread):
    success = pyqtSignal(object)   # arxiv.Result
    error   = pyqtSignal(str)

    def __init__(self, doi: str) -> None:
        super().__init__()
        self.doi = doi

    def run(self) -> None:
        try:
            result = _resolve_doi(self.doi)
            self.success.emit(result)
        except ValueError as e:
            self.error.emit(str(e))
        except Exception as e:
            self.error.emit(f"Unexpected error: {e}")


# ── Page widget ───────────────────────────────────────────────────────────────

class DoiPage(QWidget):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")
        self._result: arxiv.Result | None = None
        self._worker: _LookupWorker | None = None

        outer = QVBoxLayout(self)
        outer.setContentsMargins(48, 40, 48, 40)
        outer.setSpacing(0)

        # Header
        title_lbl = QLabel("Add by DOI")
        title_lbl.setStyleSheet(
            f"font-size: 34px; font-weight: bold; color: {_ACCENT}; background: transparent;"
        )
        sub_lbl = QLabel("Look up any paper by its DOI and add it to your library.")
        sub_lbl.setStyleSheet(f"font-size: 13px; color: {_MUTED}; background: transparent;")
        outer.addWidget(title_lbl)
        outer.addSpacing(4)
        outer.addWidget(sub_lbl)
        outer.addSpacing(28)

        # Input row
        input_row = QHBoxLayout()
        input_row.setSpacing(10)
        self._doi_input = QLineEdit()
        self._doi_input.setPlaceholderText("e.g.  10.48550/arXiv.1706.03762  or  https://doi.org/10.1038/…")
        self._doi_input.setStyleSheet(f"""
            QLineEdit {{
                background: {_PANEL}; border: 1px solid {_BORDER};
                border-radius: 6px; color: {_TEXT}; font-size: 13px;
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
                background: {_ACCENT}; border: none; border-radius: 6px;
                color: #fff; font-size: 13px; font-weight: 600;
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
        self._status.setStyleSheet(f"font-size: 12px; color: {_MUTED}; background: transparent;")
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
                background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: 10px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """)
        lay = QVBoxLayout(card)
        lay.setContentsMargins(20, 16, 20, 16)
        lay.setSpacing(6)

        self._res_title  = QLabel()
        self._res_title.setWordWrap(True)
        self._res_title.setStyleSheet(f"font-size: 15px; font-weight: 600; color: {_TEXT};")

        self._res_meta   = QLabel()
        self._res_meta.setStyleSheet(f"font-size: 12px; color: {_MUTED};")

        self._res_abstract = QLabel()
        self._res_abstract.setWordWrap(True)
        self._res_abstract.setStyleSheet(f"font-size: 12px; color: {_TEXT}; line-height: 1.5;")

        btn_row = QHBoxLayout()
        btn_row.setSpacing(10)

        self._save_btn = QPushButton("Save to library")
        self._save_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._save_btn.setStyleSheet(f"""
            QPushButton {{
                background: {_GREEN}; border: none; border-radius: 6px;
                color: #fff; font-size: 13px; font-weight: 600; padding: 8px 18px;
            }}
            QPushButton:hover   {{ background: #5dcc8f; }}
            QPushButton:pressed {{ background: #3a9e60; }}
            QPushButton:disabled {{ background: #2a2a4a; color: {_MUTED}; }}
        """)
        self._save_btn.clicked.connect(self._on_save)

        self._arxiv_lbl = QLabel()
        self._arxiv_lbl.setStyleSheet(f"font-size: 11px; color: {_MUTED};")

        btn_row.addWidget(self._save_btn)
        btn_row.addStretch()
        btn_row.addWidget(self._arxiv_lbl)

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

    def _on_success(self, result: arxiv.Result) -> None:
        self._result = result
        self._set_busy(False)
        self._status.setStyleSheet(f"font-size: 12px; color: {_GREEN}; background: transparent;")
        self._status.setText("Paper found.")

        authors = ", ".join(a.name for a in result.authors[:4])
        if len(result.authors) > 4:
            authors += f" +{len(result.authors) - 4} more"
        date = result.published.strftime("%Y-%m-%d") if result.published else ""
        cat  = result.primary_category or ""

        self._res_title.setText(result.title)
        self._res_meta.setText("  ·  ".join(filter(None, [authors, date, cat])))
        abstract = result.summary or ""
        self._res_abstract.setText(abstract[:400] + ("…" if len(abstract) > 400 else ""))

        pid, _ = _parse_entry_id(result.entry_id)
        already = get_paper(pid) is not None
        self._arxiv_lbl.setText(f"arXiv:{pid}")
        if already:
            self._save_btn.setText("Already in library")
            self._save_btn.setEnabled(False)
        else:
            self._save_btn.setText("Save to library")
            self._save_btn.setEnabled(True)

        self._result_card.setVisible(True)

    def _on_error(self, msg: str) -> None:
        self._set_busy(False)
        self._status.setStyleSheet(f"font-size: 12px; color: {_RED}; background: transparent;")
        self._status.setText(msg)

    def _on_save(self) -> None:
        if self._result is None:
            return
        save_paper(self._result)
        self._save_btn.setText("Saved ✓")
        self._save_btn.setEnabled(False)
        self._status.setStyleSheet(f"font-size: 12px; color: {_GREEN}; background: transparent;")
        self._status.setText("Paper saved to library.")

    def _set_busy(self, busy: bool) -> None:
        self._lookup_btn.setEnabled(not busy)
        self._doi_input.setEnabled(not busy)
        self._status.setStyleSheet(f"font-size: 12px; color: {_MUTED}; background: transparent;")
        self._status.setText("Looking up…" if busy else "")
