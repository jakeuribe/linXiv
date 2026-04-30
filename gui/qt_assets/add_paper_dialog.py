from __future__ import annotations

import datetime
import uuid

from PyQt6.QtCore import QDate, QSize, Qt
from PyQt6.QtWidgets import (
    QDateEdit,
    QDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPushButton,
    QScrollArea,
    QSizePolicy,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from sources.base import PaperMetadata
from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED
from gui.theme import (
    FONT_HEADING, FONT_BODY, FONT_SECONDARY,
    SPACE_XS,
    RADIUS_MD,
    DIALOG_PAD,
)

from gui.qt_assets.styles import BTN_PRIMARY as _BTN_STYLE, BTN_MUTED as _BTN_MUTED_STYLE
_INPUT_STYLE = f"""
    QLineEdit, QTextEdit, QDateEdit {{
        background: {_BG}; border: 1px solid {_BORDER}; border-radius: {RADIUS_MD}px;
        color: {_TEXT}; font-size: {FONT_BODY}px; padding: {SPACE_XS}px 10px;
    }}
    QLineEdit:focus, QTextEdit:focus, QDateEdit:focus {{ border-color: {_ACCENT}; }}
"""
_TOGGLE_STYLE = f"""
    QPushButton {{
        background: transparent; border: none; text-align: left;
        color: {_MUTED}; font-size: {FONT_SECONDARY}px; font-weight: 600; padding: 0;
    }}
    QPushButton:hover {{ color: {_TEXT}; }}
"""

_MIN_H = 36


class _GrowingTextEdit(QTextEdit):
    """QTextEdit that grows to fit wrapped content."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Minimum)
        self.setMinimumHeight(_MIN_H)
        doc = self.document()
        if doc is not None:
            doc.contentsChanged.connect(self.updateGeometry)

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        self.updateGeometry()

    def sizeHint(self) -> QSize:
        doc = self.document()
        vp  = self.viewport()
        if doc is None or vp is None:
            return super().sizeHint()
        m = self.contentsMargins()
        h = int(doc.size().height()) + m.top() + m.bottom() + 6
        return QSize(super().sizeHint().width(), max(_MIN_H, h))

    def minimumSizeHint(self) -> QSize:
        return QSize(100, _MIN_H)


class _Section(QWidget):
    """Collapsible field: toggle label on top, content below."""

    def __init__(
        self,
        label: str,
        content: QWidget,
        *,
        collapsed: bool = True,
        parent: QWidget | None = None,
    ) -> None:
        super().__init__(parent)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Minimum)
        self._label    = label
        self._content  = content
        self._collapsed = collapsed

        vlay = QVBoxLayout(self)
        vlay.setContentsMargins(0, 0, 0, 0)
        vlay.setSpacing(SPACE_XS)

        self._btn = QPushButton()
        self._btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._btn.setStyleSheet(_TOGGLE_STYLE)
        self._btn.clicked.connect(self._toggle)
        vlay.addWidget(self._btn)
        vlay.addWidget(content)

        self._sync()

    def _sync(self) -> None:
        arrow = "▶" if self._collapsed else "▼"
        self._btn.setText(f"{arrow}  {self._label}")
        self._content.setVisible(not self._collapsed)

    def _toggle(self) -> None:
        self._collapsed = not self._collapsed
        self._sync()

    def expand(self) -> None:
        self._collapsed = False
        self._sync()


class AddPaperManuallyDialog(QDialog):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle("Add Paper Manually")
        self.setMinimumWidth(560)
        self.resize(680, 420)
        self.setStyleSheet(f"background: {_PANEL}; color: {_TEXT};")

        lay = QVBoxLayout(self)
        lay.setContentsMargins(DIALOG_PAD, DIALOG_PAD, DIALOG_PAD, DIALOG_PAD)
        lay.setSpacing(SPACE_XS)

        heading = QLabel("Add Paper Manually")
        heading.setStyleSheet(f"font-size: {FONT_HEADING}px; font-weight: bold; color: {_ACCENT};")
        lay.addWidget(heading)

        # Scrollable sections
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")

        sections_widget = QWidget()
        sections_widget.setStyleSheet("background: transparent;")
        slay = QVBoxLayout(sections_widget)
        slay.setContentsMargins(0, 0, 0, 0)
        slay.setSpacing(SPACE_XS)

        # Title — always expanded
        self._title = _GrowingTextEdit()
        self._title.setPlaceholderText("e.g. Attention Is All You Need")
        self._title.setStyleSheet(_INPUT_STYLE)
        slay.addWidget(_Section("Title *", self._title, collapsed=False))

        # Authors
        self._authors = _GrowingTextEdit()
        self._authors.setPlaceholderText("e.g. Vaswani, A., Shazeer, N., Parmar, N.")
        self._authors.setStyleSheet(_INPUT_STYLE)
        slay.addWidget(_Section("Authors *  (comma-separated)", self._authors))

        # Published | Category
        self._published = QDateEdit()
        self._published.setDate(QDate.currentDate())
        self._published.setDisplayFormat("yyyy-MM-dd")
        self._published.setStyleSheet(_INPUT_STYLE)

        self._category = QLineEdit()
        self._category.setPlaceholderText("e.g. cs.LG")
        self._category.setStyleSheet(_INPUT_STYLE)

        date_cat = QWidget()
        date_cat.setStyleSheet("background: transparent;")
        dc_row = QHBoxLayout(date_cat)
        dc_row.setContentsMargins(0, 0, 0, 0)
        dc_row.setSpacing(SPACE_XS)
        for lbl_text, w in [("Published *", self._published), ("Category  (optional)", self._category)]:
            col = QVBoxLayout()
            col.setSpacing(SPACE_XS)
            col.addWidget(self._lbl(lbl_text))
            col.addWidget(w)
            dc_row.addLayout(col)
        slay.addWidget(_Section("Published / Category", date_cat))

        # DOI | URL
        self._doi = QLineEdit()
        self._doi.setPlaceholderText("e.g. 10.48550/arXiv.1706.03762")
        self._doi.setStyleSheet(_INPUT_STYLE)

        self._url = QLineEdit()
        self._url.setPlaceholderText("https://…")
        self._url.setStyleSheet(_INPUT_STYLE)

        doi_url = QWidget()
        doi_url.setStyleSheet("background: transparent;")
        du_row = QHBoxLayout(doi_url)
        du_row.setContentsMargins(0, 0, 0, 0)
        du_row.setSpacing(SPACE_XS)
        for lbl_text, w in [("DOI  (optional)", self._doi), ("URL  (optional)", self._url)]:
            col = QVBoxLayout()
            col.setSpacing(SPACE_XS)
            col.addWidget(self._lbl(lbl_text))
            col.addWidget(w)
            du_row.addLayout(col)
        slay.addWidget(_Section("DOI / URL", doi_url))

        # Abstract
        self._summary = _GrowingTextEdit()
        self._summary.setPlaceholderText("Paper abstract…")
        self._summary.setStyleSheet(_INPUT_STYLE)
        slay.addWidget(_Section("Abstract  (optional)", self._summary))

        # Tags
        self._tags = QLineEdit()
        self._tags.setPlaceholderText("e.g. transformers, nlp")
        self._tags.setStyleSheet(_INPUT_STYLE)
        slay.addWidget(_Section("Tags  (comma-separated, optional)", self._tags))

        slay.addStretch()
        scroll.setWidget(sections_widget)
        lay.addWidget(scroll, stretch=1)

        btn_row = QHBoxLayout()
        cancel = QPushButton("Cancel")
        cancel.setStyleSheet(_BTN_MUTED_STYLE)
        cancel.clicked.connect(self.reject)
        add_btn = QPushButton("Add to Library")
        add_btn.setStyleSheet(_BTN_STYLE)
        add_btn.clicked.connect(self.accept)
        btn_row.addWidget(cancel)
        btn_row.addStretch()
        btn_row.addWidget(add_btn)
        lay.addLayout(btn_row)

    @staticmethod
    def _lbl(text: str) -> QLabel:
        lbl = QLabel(text)
        lbl.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; font-weight: 600;")
        return lbl

    def load_from_row(self, row) -> None:
        self._title.setPlainText(row["title"] or "")
        authors: list[str] = row["authors"] or []
        self._authors.setPlainText(", ".join(authors))
        if row["published"]:
            d = row["published"]
            self._published.setDate(QDate(d.year, d.month, d.day))
        summary = row["summary"] if "summary" in row.keys() else None
        self._summary.setPlainText(summary or "")
        doi = row["doi"] if "doi" in row.keys() else None
        self._doi.setText(doi or "")
        url = row["url"] if "url" in row.keys() else None
        self._url.setText(url or "")
        self._category.setText(row["category"] or "")
        tags: list[str] = row["tags"] or []
        self._tags.setText(", ".join(tags))

    def get_metadata(self, original_paper_id: str | None = None) -> PaperMetadata:
        """Build a PaperMetadata from the current field values.

        paper_id priority: DOI field → original_paper_id → new manual-{uuid} slug.
        """
        doi     = self._doi.text().strip() or None
        url     = self._url.text().strip() or None
        title   = self._title.toPlainText().strip()
        authors = [a.strip() for a in self._authors.toPlainText().split(",") if a.strip()]
        qd      = self._published.date()
        published = datetime.date(qd.year(), qd.month(), qd.day())
        summary  = self._summary.toPlainText().strip()
        category = self._category.text().strip() or None
        tags_raw = [t.strip() for t in self._tags.text().split(",") if t.strip()]
        tags: list[str] | None = tags_raw or None

        if doi:
            paper_id = doi
        elif original_paper_id:
            paper_id = original_paper_id
        else:
            paper_id = f"manual-{uuid.uuid4().hex[:12]}"

        return PaperMetadata(
            paper_id  = paper_id,
            version   = 1,
            title     = title,
            authors   = authors,
            published = published,
            summary   = summary,
            category  = category,
            doi       = doi,
            url       = url,
            tags      = tags,
            source    = "manual",
        )
