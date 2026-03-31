from __future__ import annotations

from PyQt6.QtCore import Qt
from PyQt6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QListWidget,
    QListWidgetItem,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from db import get_tags, list_papers
from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED
_RECENT_N = 10


class HomePage(QWidget):
    """Landing page: stat cards + recent papers list."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        outer = QVBoxLayout(self)
        outer.setContentsMargins(48, 40, 48, 40)
        outer.setSpacing(28)

        # ── Header ────────────────────────────────────────────────────────────
        title = QLabel("linXiv")
        title.setStyleSheet(f"font-size: 34px; font-weight: bold; color: {_ACCENT}; background: transparent;")
        subtitle = QLabel("Your arXiv paper collection")
        subtitle.setStyleSheet(f"font-size: 13px; color: {_MUTED}; background: transparent;")
        outer.addWidget(title)
        outer.addWidget(subtitle)

        # ── Stat cards ────────────────────────────────────────────────────────
        self._stats_container = QWidget()
        self._stats_container.setStyleSheet("background: transparent;")
        self._stats_row = QHBoxLayout(self._stats_container)
        self._stats_row.setContentsMargins(0, 0, 0, 0)
        self._stats_row.setSpacing(16)
        outer.addWidget(self._stats_container)

        # ── Recent papers ─────────────────────────────────────────────────────
        recent_hdr = QHBoxLayout()
        recent_lbl = QLabel("Recent papers")
        recent_lbl.setStyleSheet(
            f"font-size: 15px; font-weight: 600; color: {_TEXT}; background: transparent;"
        )
        refresh_btn = QPushButton("Refresh")
        refresh_btn.setFixedWidth(80)
        refresh_btn.setStyleSheet(f"""
            QPushButton {{
                background: {_PANEL}; border: 1px solid {_BORDER};
                border-radius: 4px; color: {_TEXT}; font-size: 12px; padding: 4px 8px;
            }}
            QPushButton:hover {{ background: #2a2a4a; }}
        """)
        refresh_btn.clicked.connect(self._load)
        recent_hdr.addWidget(recent_lbl)
        recent_hdr.addStretch()
        recent_hdr.addWidget(refresh_btn)
        outer.addLayout(recent_hdr)

        self._recent_list = QListWidget()
        self._recent_list.setStyleSheet(f"""
            QListWidget {{
                background: {_PANEL}; border: 1px solid {_BORDER};
                border-radius: 8px; color: {_TEXT}; font-size: 13px;
            }}
            QListWidget::item {{
                padding: 7px 12px;
                border-bottom: 1px solid {_BORDER};
            }}
            QListWidget::item:selected {{
                background: #2a2a4a;
            }}
            QListWidget::item:last-child {{
                border-bottom: none;
            }}
        """)
        outer.addWidget(self._recent_list, stretch=1)

        self._load()

    # ── Data ──────────────────────────────────────────────────────────────────

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
            if item.widget():  # pyright: ignore[reportOptionalMemberAccess] — technically fixable but awkward with current setup
                item.widget().deleteLater()  # pyright: ignore[reportOptionalMemberAccess]

        for value, label in (
            (str(total),    "Papers saved"),
            (str(with_pdf), "PDFs on disk"),
            (str(cats),     "Categories"),
            (str(tag_cnt),  "Tags"),
        ):
            self._stats_row.addWidget(self._make_card(value, label))

        # Rebuild recent list
        self._recent_list.clear()
        for p in papers[:_RECENT_N]:
            date = p["published"].isoformat() if p["published"] else ""
            cat  = p["category"] or ""
            txt  = p["title"] or "(untitled)"
            linked = p["pdf_path"] if "pdf_path" in p.keys() else None
            if linked:
                txt = f"[PDF] {txt}"
            meta = "  ·  ".join(filter(None, [cat, date]))
            item = QListWidgetItem(txt)
            item.setToolTip(f"Linked: {linked}\n{meta}" if linked else meta)
            # Show metadata as a second line via the display role trick
            item.setData(Qt.ItemDataRole.UserRole, meta)
            self._recent_list.addItem(item)
            # Append a dim meta label via a second item indented
            meta_item = QListWidgetItem(f"    {meta}")
            meta_item.setForeground(self._muted_color())
            meta_item.setFlags(Qt.ItemFlag.NoItemFlags)
            self._recent_list.addItem(meta_item)

    # ── Helpers ───────────────────────────────────────────────────────────────

    def _make_card(self, value: str, label: str) -> QFrame:
        card = QFrame()
        card.setStyleSheet(f"""
            QFrame {{
                background: {_PANEL};
                border: 1px solid {_BORDER};
                border-radius: 8px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """)
        lay = QVBoxLayout(card)
        lay.setContentsMargins(20, 14, 20, 14)
        lay.setSpacing(4)

        num = QLabel(value)
        num.setStyleSheet(f"font-size: 30px; font-weight: bold; color: {_ACCENT};")
        num.setAlignment(Qt.AlignmentFlag.AlignCenter)

        lbl = QLabel(label)
        lbl.setStyleSheet(f"font-size: 11px; color: {_MUTED}; letter-spacing: 0.05em;")
        lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)

        lay.addWidget(num)
        lay.addWidget(lbl)
        return card

    def _muted_color(self):
        from PyQt6.QtGui import QColor
        return QColor(_MUTED)
