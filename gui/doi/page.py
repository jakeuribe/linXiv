from __future__ import annotations

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

from storage.db import get_paper, save_paper_metadata
from sources import _resolve_doi
from sources.base import PaperMetadata

from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED
from gui.theme import (
    FONT_TITLE, FONT_SUBHEADING, FONT_BODY, FONT_SECONDARY, FONT_TERTIARY,
    SPACE_XL, SPACE_MD, SPACE_XS,
    RADIUS_MD, RADIUS_LG,
    BTN_H_LG,
    PAGE_MARGIN_H, PAGE_MARGIN_V,
    CARD_PAD_H, DIALOG_PAD,
)

_GREEN  = "#4caf7d"
_RED    = "#e05c5c"

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
        outer.setContentsMargins(PAGE_MARGIN_H, PAGE_MARGIN_V, PAGE_MARGIN_H, PAGE_MARGIN_V)
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
        input_row.setSpacing(SPACE_MD)
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
        self._lookup_btn.setFixedSize(96, BTN_H_LG)  # TODO: Make more customizable (width)
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
        outer.addSpacing(CARD_PAD_H)

        # Status label
        self._status = QLabel("")
        self._status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;")
        self._status.setWordWrap(True)
        outer.addWidget(self._status)
        outer.addSpacing(CARD_PAD_H)

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
        lay.setContentsMargins(DIALOG_PAD, CARD_PAD_H, DIALOG_PAD, CARD_PAD_H)
        lay.setSpacing(SPACE_XS)

        self._res_title = QLabel()
        self._res_title.setWordWrap(True)
        self._res_title.setStyleSheet(f"font-size: {FONT_SUBHEADING}px; font-weight: 600; color: {_TEXT};")

        self._res_meta = QLabel()
        self._res_meta.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED};")

        self._res_abstract = QLabel()
        self._res_abstract.setWordWrap(True)
        self._res_abstract.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_TEXT}; line-height: 1.5;")

        btn_row = QHBoxLayout()
        btn_row.setSpacing(SPACE_MD)

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
        lay.addSpacing(SPACE_XS)
        lay.addWidget(self._res_abstract)
        lay.addSpacing(SPACE_MD)
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
        src = meta.source or ""
        self._source_lbl.setText(f"via {source_labels.get(src, src)}: {meta.paper_id}")

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
