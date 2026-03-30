from __future__ import annotations

from PyQt6.QtCore import Qt
from PyQt6.QtWidgets import (
    QLabel,
    QMessageBox,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

_BG     = "#0f0f1a"
_PANEL  = "#1a1a2e"
_BORDER = "#2e2e50"
_ACCENT = "#5b8dee"
_TEXT   = "#ccccdd"
_MUTED  = "#7777aa"


class ProjectsPage(QWidget):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        outer = QVBoxLayout(self)
        outer.setContentsMargins(48, 40, 48, 40)
        outer.setSpacing(12)

        title = QLabel("Projects")
        title.setStyleSheet(
            f"font-size: 34px; font-weight: bold; color: {_ACCENT}; background: transparent;"
        )
        subtitle = QLabel("Organise papers into focused reading projects")
        subtitle.setStyleSheet(f"font-size: 13px; color: {_MUTED}; background: transparent;")
        outer.addWidget(title)
        outer.addWidget(subtitle)
        outer.addSpacing(24)

        # Empty-state area
        outer.addStretch()
        empty_lbl = QLabel("No projects yet")
        empty_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        empty_lbl.setStyleSheet(f"font-size: 15px; color: {_MUTED}; background: transparent;")
        outer.addWidget(empty_lbl)
        outer.addSpacing(20)

        add_btn = QPushButton("＋  New Project")
        add_btn.setFixedSize(200, 48)
        add_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        add_btn.setStyleSheet(f"""
            QPushButton {{
                background: {_ACCENT};
                border: none;
                border-radius: 8px;
                color: #ffffff;
                font-size: 14px;
                font-family: 'Segoe UI', sans-serif;
                font-weight: 600;
            }}
            QPushButton:hover   {{ background: #7aa3f5; }}
            QPushButton:pressed {{ background: #4a7add; }}
        """)
        add_btn.clicked.connect(self._on_add)
        outer.addWidget(add_btn, alignment=Qt.AlignmentFlag.AlignHCenter)
        outer.addStretch()

    def _on_add(self) -> None:
        msg = QMessageBox(self)
        msg.setWindowTitle("Coming soon")
        msg.setText("Projects are not yet implemented.")
        msg.setInformativeText("Check back later — this feature is on the roadmap.")
        msg.setIcon(QMessageBox.Icon.Information)
        msg.setStyleSheet(f"""
            QMessageBox {{ background: {_PANEL}; color: {_TEXT}; }}
            QLabel {{ color: {_TEXT}; }}
            QPushButton {{
                background: {_ACCENT}; border: none; border-radius: 4px;
                color: #fff; padding: 6px 16px; font-size: 12px;
            }}
            QPushButton:hover {{ background: #7aa3f5; }}
        """)
        msg.exec()
