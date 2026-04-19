from __future__ import annotations

from PyQt6.QtCore import Qt
from PyQt6.QtWidgets import (
    QHBoxLayout,
    QMainWindow,
    QPushButton,
    QStackedWidget,
    QVBoxLayout,
    QWidget,
)

from .theme import BG as _BG, TEXT as _TEXT, FONT_BODY, NAV_WIDTH, SPACE_MD, SPACE_SM

_SIDEBAR_STYLE = f"""
    QWidget#sidebar {{ background: #1a1a2e; }}
    QPushButton {{
        color: #ccccdd;
        background: transparent;
        border: none;
        padding: {SPACE_MD}px {SPACE_SM}px;
        font-family: 'Segoe UI', sans-serif;
        font-size: {FONT_BODY}px;
        text-align: left;
    }}
    QPushButton:hover   {{ background: #2a2a4a; }}
    QPushButton:checked {{ background: #5b8dee; color: #ffffff; }}
"""


class AppShell(QMainWindow):
    """Outer application shell: fixed sidebar nav + QStackedWidget for pages."""

    def __init__(self) -> None:
        super().__init__()
        self.setWindowTitle("linXiv")
        self.resize(1500, 950)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")
        self._page_btns: list[QPushButton] = []
        self._stack = QStackedWidget()

        self._sidebar = QWidget()
        self._sidebar.setObjectName("sidebar")
        self._sidebar.setFixedWidth(NAV_WIDTH)
        self._sidebar.setStyleSheet(_SIDEBAR_STYLE)

        self._nav = QVBoxLayout(self._sidebar)
        self._nav.setContentsMargins(0, SPACE_SM, 0, SPACE_SM)
        self._nav.setSpacing(2)
        self._nav.setAlignment(Qt.AlignmentFlag.AlignTop)

        central = QWidget()
        h = QHBoxLayout(central)
        h.setContentsMargins(0, 0, 0, 0)
        h.setSpacing(0)
        h.addWidget(self._sidebar)
        h.addWidget(self._stack)
        self.setCentralWidget(central)

    # ── Public API ────────────────────────────────────────────────────────────

    def add_page(self, label: str, widget: QWidget) -> int:
        """Embed widget as a switchable page; returns its stack index."""
        idx = self._stack.addWidget(widget)
        btn = QPushButton(label)
        btn.setCheckable(True)
        btn.clicked.connect(lambda: self._go_to(idx))
        self._nav.addWidget(btn)
        self._page_btns.append(btn)
        if idx == 0:
            btn.setChecked(True)
        return idx

    def add_launcher(self, label: str, callback) -> None:
        """Add a nav button that runs callback (e.g. opens a floating window)."""
        btn = QPushButton(label)
        btn.clicked.connect(callback)
        self._nav.addWidget(btn)

    def go_to_widget(self, widget: QWidget) -> None:
        """Navigate to the page containing widget (must have been added via add_page)."""
        idx = self._stack.indexOf(widget)
        if idx >= 0:
            self._go_to(idx)

    # ── Internal ──────────────────────────────────────────────────────────────

    def _go_to(self, idx: int) -> None:
        self._stack.setCurrentIndex(idx)
        for i, btn in enumerate(self._page_btns):
            btn.setChecked(i == idx)
