from __future__ import annotations

from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from storage.db import get_tags, list_papers
from gui.qt_assets import PaperCard
from gui.qt_assets.styles import BTN_PANEL_SM as _BTN_PANEL_SM
from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED
from gui.theme import (
    FONT_TITLE, FONT_SUBHEADING, FONT_BODY, FONT_TERTIARY,
    SPACE_XL, SPACE_SM, SPACE_XS,
    RADIUS_LG,
    PAGE_MARGIN_H, PAGE_MARGIN_V,
    CARD_PAD_H, CARD_PAD_V,
    DIALOG_PAD,
)
_RECENT_N = 10


class HomePage(QWidget):
    """Landing page: stat cards + recent papers list."""

    navigate_to_paper = pyqtSignal(str)   # paper_id

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        outer = QVBoxLayout(self)
        outer.setContentsMargins(PAGE_MARGIN_H, PAGE_MARGIN_V, PAGE_MARGIN_H, PAGE_MARGIN_V)
        outer.setSpacing(SPACE_XL)

        # ── Header ────────────────────────────────────────────────────────────
        title = QLabel("linXiv")
        title.setStyleSheet(f"font-size: {FONT_TITLE}px; font-weight: bold; color: {_ACCENT}; background: transparent;")
        subtitle = QLabel("Your arXiv paper collection")
        subtitle.setStyleSheet(f"font-size: {FONT_BODY}px; color: {_MUTED}; background: transparent;")
        outer.addWidget(title)
        outer.addWidget(subtitle)

        # ── Stat cards ────────────────────────────────────────────────────────
        self._stats_container = QWidget()
        self._stats_container.setStyleSheet("background: transparent;")
        self._stats_row = QHBoxLayout(self._stats_container)
        self._stats_row.setContentsMargins(0, 0, 0, 0)
        self._stats_row.setSpacing(CARD_PAD_H)
        outer.addWidget(self._stats_container)

        # ── Recent papers ─────────────────────────────────────────────────────
        recent_hdr = QHBoxLayout()
        recent_lbl = QLabel("Recent papers")
        recent_lbl.setStyleSheet(
            f"font-size: {FONT_SUBHEADING}px; font-weight: 600; color: {_TEXT}; background: transparent;"
        )
        refresh_btn = QPushButton("Refresh")
        refresh_btn.setFixedWidth(80)
        refresh_btn.setStyleSheet(_BTN_PANEL_SM)
        refresh_btn.clicked.connect(self._load)
        recent_hdr.addWidget(recent_lbl)
        recent_hdr.addStretch()
        recent_hdr.addWidget(refresh_btn)
        outer.addLayout(recent_hdr)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")

        self._recent_widget = QWidget()
        self._recent_widget.setStyleSheet("background: transparent;")
        self._recent_layout = QVBoxLayout(self._recent_widget)
        self._recent_layout.setContentsMargins(0, 0, 0, 0)
        self._recent_layout.setSpacing(SPACE_SM)
        self._recent_layout.addStretch()

        scroll.setWidget(self._recent_widget)
        outer.addWidget(scroll, stretch=1)

        self._load()

    # ── Data ──────────────────────────────────────────────────────────────────

    def refresh(self) -> None:
        self._load()

    def _load(self) -> None:
        papers   = list_papers(latest_only=True)
        tags     = get_tags()
        total    = len(papers)
        with_pdf = sum(1 for p in papers if p["has_pdf"])
        cats     = len({p["category"] for p in papers if p["category"]})
        tag_cnt  = len(tags)

        # Rebuild stat cards
        while self._stats_row.count():
            item = self._stats_row.takeAt(0)
            if item.widget():  # pyright: ignore[reportOptionalMemberAccess]
                item.widget().deleteLater()  # pyright: ignore[reportOptionalMemberAccess]

        for value, label in (
            (str(total),    "Papers saved"),
            (str(with_pdf), "PDFs on disk"),
            (str(cats),     "Categories"),
            (str(tag_cnt),  "Tags"),
        ):
            self._stats_row.addWidget(self._make_card(value, label))

        # Rebuild recent papers
        while self._recent_layout.count() > 1:
            item = self._recent_layout.takeAt(0)
            if item.widget():  # pyright: ignore[reportOptionalMemberAccess]
                item.widget().deleteLater()  # pyright: ignore[reportOptionalMemberAccess]

        for p in papers[:_RECENT_N]:
            card = PaperCard(p, parent=self._recent_widget)
            card.double_clicked.connect(
                lambda _row, pid=p["paper_id"]: self.navigate_to_paper.emit(pid)
            )
            self._recent_layout.insertWidget(self._recent_layout.count() - 1, card)

    # ── Helpers ───────────────────────────────────────────────────────────────

    def _make_card(self, value: str, label: str) -> QFrame:
        card = QFrame()
        card.setStyleSheet(f"""
            QFrame {{
                background: {_PANEL};
                border: 1px solid {_BORDER};
                border-radius: {RADIUS_LG}px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """)
        lay = QVBoxLayout(card)
        lay.setContentsMargins(DIALOG_PAD, CARD_PAD_V, DIALOG_PAD, CARD_PAD_V)
        lay.setSpacing(SPACE_XS)

        num = QLabel(value)
        num.setStyleSheet(f"font-size: 30px; font-weight: bold; color: {_ACCENT};")
        num.setAlignment(Qt.AlignmentFlag.AlignCenter)

        lbl = QLabel(label)
        lbl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED}; letter-spacing: 0.05em;")
        lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)

        lay.addWidget(num)
        lay.addWidget(lbl)
        return card
