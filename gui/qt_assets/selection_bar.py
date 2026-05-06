from __future__ import annotations

from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtWidgets import QFrame, QHBoxLayout, QLabel, QPushButton, QWidget

from gui.theme import (
    ACCENT, BORDER, TEXT,
    FONT_BODY, FONT_SECONDARY,
    RADIUS_MD,
    SPACE_MD, SPACE_SM,
    BTN_H_MD,
)

_BLUE = "#5b8dee"
_RED  = "#e05c5c"

from gui.qt_assets.styles import BTN_OUTLINE as _BTN, BTN_DANGER as _BTN_DANGER, btn_colored_outline as _colored_outline


def _action_btn(label: str, color: str) -> QPushButton:
    b = QPushButton(label)
    b.setFixedHeight(BTN_H_MD)
    b.setCursor(Qt.CursorShape.PointingHandCursor)
    b.setStyleSheet(_colored_outline(color))
    return b


class SelectionBar(QFrame):
    download_requested            = pyqtSignal()
    remove_pdfs_requested         = pyqtSignal()
    add_to_project_requested      = pyqtSignal()
    remove_from_project_requested = pyqtSignal()
    remove_from_library_requested = pyqtSignal()
    select_all_requested          = pyqtSignal()
    clear_requested               = pyqtSignal()

    def __init__(
        self,
        *,
        show_remove: bool = False,
        show_remove_from_library: bool = False,
        show_remove_pdfs: bool = False,
        show_select_all: bool = False,
        parent: QWidget | None = None,
    ) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"""
            QFrame {{ background: #1e1e36; border-top: 1px solid {BORDER}; }}
            QLabel {{ background: transparent; border: none; }}
        """)
        self.setFixedHeight(52)
        self.setVisible(False)

        row = QHBoxLayout(self)
        row.setContentsMargins(24, 0, 24, 0)
        row.setSpacing(SPACE_MD)

        self._count_lbl = QLabel()
        self._count_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; font-weight: 600; color: {TEXT};")
        row.addWidget(self._count_lbl)
        row.addSpacing(SPACE_SM)

        dl_btn = _action_btn("Download PDFs", _BLUE)
        dl_btn.clicked.connect(self.download_requested)
        row.addWidget(dl_btn)

        if show_remove_pdfs:
            rm_pdf_btn = QPushButton("Remove PDFs")
            rm_pdf_btn.setFixedHeight(BTN_H_MD)
            rm_pdf_btn.setCursor(Qt.CursorShape.PointingHandCursor)
            rm_pdf_btn.setStyleSheet(_BTN_DANGER)
            rm_pdf_btn.clicked.connect(self.remove_pdfs_requested)
            row.addWidget(rm_pdf_btn)

        proj_btn = _action_btn("Add to Project", ACCENT)
        proj_btn.clicked.connect(self.add_to_project_requested)
        row.addWidget(proj_btn)

        if show_remove:
            rm_btn = QPushButton("Remove from project")
            rm_btn.setFixedHeight(BTN_H_MD)
            rm_btn.setCursor(Qt.CursorShape.PointingHandCursor)
            rm_btn.setStyleSheet(_BTN_DANGER)
            rm_btn.clicked.connect(self.remove_from_project_requested)
            row.addWidget(rm_btn)

        if show_remove_from_library:
            rm_lib_btn = QPushButton("Remove from library")
            rm_lib_btn.setFixedHeight(BTN_H_MD)
            rm_lib_btn.setCursor(Qt.CursorShape.PointingHandCursor)
            rm_lib_btn.setStyleSheet(_BTN_DANGER)
            rm_lib_btn.clicked.connect(self.remove_from_library_requested)
            row.addWidget(rm_lib_btn)

        row.addStretch()

        if show_select_all:
            sel_all = QPushButton("Select All")
            sel_all.setFixedHeight(BTN_H_MD)
            sel_all.setCursor(Qt.CursorShape.PointingHandCursor)
            sel_all.setStyleSheet(_BTN)
            sel_all.clicked.connect(self.select_all_requested)
            row.addWidget(sel_all)

        clr = QPushButton("Clear")
        clr.setFixedHeight(BTN_H_MD)
        clr.setCursor(Qt.CursorShape.PointingHandCursor)
        clr.setStyleSheet(_BTN)
        clr.clicked.connect(self.clear_requested)
        row.addWidget(clr)

    def set_count(self, n: int) -> None:
        """Update label text and show/hide the bar (visible when n > 0)."""
        self._count_lbl.setText(f"{n} selected")
        self.setVisible(n > 0)
