from __future__ import annotations

from PyQt6.QtCore import Qt
from PyQt6.QtWidgets import (
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

from projects import color_to_hex

_BG     = "#0f0f1a"
_PANEL  = "#1a1a2e"
_BORDER = "#2e2e50"
_ACCENT = "#5b8dee"
_TEXT   = "#ccccdd"
_MUTED  = "#7777aa"

_PRESET_COLORS: list[int] = [
    0x5b8dee,  # blue (default)
    0x9b59b6,  # purple
    0x4caf7d,  # green
    0xe67e22,  # orange
    0xe05c5c,  # red
    0x1abc9c,  # teal
]

_BTN_STYLE = f"""
    QPushButton {{
        background: {_ACCENT}; border: none; border-radius: 6px;
        color: #fff; font-size: 13px; font-weight: 600; padding: 8px 20px;
    }}
    QPushButton:hover   {{ background: #7aa3f5; }}
    QPushButton:pressed {{ background: #4a7add; }}
    QPushButton:disabled {{ background: #2a2a4a; color: {_MUTED}; }}
"""
_BTN_MUTED_STYLE = f"""
    QPushButton {{
        background: transparent; border: 1px solid {_BORDER}; border-radius: 6px;
        color: {_MUTED}; font-size: 13px; padding: 8px 20px;
    }}
    QPushButton:hover {{ border-color: {_TEXT}; color: {_TEXT}; }}
"""
_INPUT_STYLE = f"""
    QLineEdit, QTextEdit {{
        background: {_BG}; border: 1px solid {_BORDER}; border-radius: 6px;
        color: {_TEXT}; font-size: 13px; padding: 8px 10px;
    }}
    QLineEdit:focus, QTextEdit:focus {{ border-color: {_ACCENT}; }}
"""


# ── New-project dialog ────────────────────────────────────────────────────────

class NewProjectDialog(QDialog):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle("New Project")
        self.setFixedWidth(440)
        self.setStyleSheet(f"background: {_PANEL}; color: {_TEXT};")
        self._color: int = _PRESET_COLORS[0]

        lay = QVBoxLayout(self)
        lay.setContentsMargins(28, 24, 28, 24)
        lay.setSpacing(14)

        title = QLabel("New Project")
        title.setStyleSheet(f"font-size: 20px; font-weight: bold; color: {_ACCENT};")
        lay.addWidget(title)

        # Name
        lay.addWidget(self._field_label("Name"))
        self._name = QLineEdit()
        self._name.setPlaceholderText("e.g. Diffusion Models")
        self._name.setStyleSheet(_INPUT_STYLE)
        lay.addWidget(self._name)

        # Description
        lay.addWidget(self._field_label("Description  (optional)"))
        self._desc = QTextEdit()
        self._desc.setPlaceholderText("What is this project about?")
        self._desc.setFixedHeight(72)
        self._desc.setStyleSheet(_INPUT_STYLE)
        lay.addWidget(self._desc)

        # Tags
        lay.addWidget(self._field_label("Tags  (comma-separated, optional)"))
        self._tags = QLineEdit()
        self._tags.setPlaceholderText("e.g. generative, vision")
        self._tags.setStyleSheet(_INPUT_STYLE)
        lay.addWidget(self._tags)

        # Color
        lay.addWidget(self._field_label("Colour"))
        lay.addLayout(self._build_swatches())

        lay.addSpacing(4)

        # Error + buttons
        self._err = QLabel("")
        self._err.setStyleSheet("font-size: 12px; color: #e05c5c;")
        lay.addWidget(self._err)

        btn_row = QHBoxLayout()
        cancel = QPushButton("Cancel")
        cancel.setStyleSheet(_BTN_MUTED_STYLE)
        cancel.clicked.connect(self.reject)

        self._create_btn = QPushButton("Create")
        self._create_btn.setStyleSheet(_BTN_STYLE)
        self._create_btn.clicked.connect(self._on_create)

        btn_row.addWidget(cancel)
        btn_row.addStretch()
        btn_row.addWidget(self._create_btn)
        lay.addLayout(btn_row)

    def _field_label(self, text: str) -> QLabel:
        lbl = QLabel(text)
        lbl.setStyleSheet(f"font-size: 12px; color: {_MUTED}; font-weight: 600;")
        return lbl

    def _build_swatches(self) -> QHBoxLayout:
        row = QHBoxLayout()
        row.setSpacing(8)
        self._swatch_btns: list[QPushButton] = []
        for color in _PRESET_COLORS:
            btn = QPushButton()
            btn.setFixedSize(28, 28)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setCheckable(True)
            btn.setChecked(color == self._color)
            btn.setStyleSheet(self._swatch_css(color, color == self._color))
            btn.clicked.connect(lambda _, c=color: self._select_color(c))
            self._swatch_btns.append(btn)
            row.addWidget(btn)
        row.addStretch()
        return row

    def _swatch_css(self, color: int, checked: bool) -> str:
        hex_color = color_to_hex(color)
        border = "#ffffff" if checked else hex_color
        return (
            f"QPushButton {{ background: {hex_color}; border-radius: 14px;"
            f" border: 2px solid {border}; }}"
        )

    def _select_color(self, color: int) -> None:
        self._color = color
        for btn, c in zip(self._swatch_btns, _PRESET_COLORS):
            btn.setChecked(c == color)
            btn.setStyleSheet(self._swatch_css(c, c == color))

    def _on_create(self) -> None:
        name = self._name.text().strip()
        if not name:
            self._err.setText("Project name is required.")
            return
        desc = self._desc.toPlainText().strip()
        raw_tags = self._tags.text().strip()
        tags = [t.strip() for t in raw_tags.split(",") if t.strip()] if raw_tags else []

        from projects import Project, ensure_projects_db
        from notes import ensure_notes_db
        ensure_projects_db()
        ensure_notes_db()

        p = Project(name=name, description=desc, color=self._color, project_tags=tags)
        p.save()
        self.accept()


# ── Project card ──────────────────────────────────────────────────────────────

class ProjectCard(QFrame):
    def __init__(self, project, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"""
            QFrame {{
                background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: 10px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)

        outer = QHBoxLayout(self)
        outer.setContentsMargins(0, 0, 16, 0)
        outer.setSpacing(0)

        # Coloured left stripe
        stripe = QWidget()
        stripe.setFixedWidth(6)
        hex_color = color_to_hex(project.color) if project.color is not None else _ACCENT
        stripe.setStyleSheet(f"background: {hex_color}; border-radius: 10px 0 0 10px;")
        outer.addWidget(stripe)

        inner = QVBoxLayout()
        inner.setContentsMargins(16, 12, 0, 12)
        inner.setSpacing(4)

        name_lbl = QLabel(project.name)
        name_lbl.setStyleSheet(f"font-size: 15px; font-weight: 600; color: {_TEXT};")
        inner.addWidget(name_lbl)

        if project.description:
            desc_lbl = QLabel(project.description)
            desc_lbl.setStyleSheet(f"font-size: 12px; color: {_MUTED};")
            desc_lbl.setWordWrap(True)
            inner.addWidget(desc_lbl)

        # Stats row
        stats_row = QHBoxLayout()
        stats_row.setSpacing(12)
        stats_row.setContentsMargins(0, 4, 0, 0)

        paper_count = project.paper_count
        note_count = self._note_count(project)

        for icon, value, label in [
            ("📄", paper_count, "paper" if paper_count == 1 else "papers"),
            ("📝", note_count,  "note"  if note_count  == 1 else "notes"),
        ]:
            lbl = QLabel(f"{icon} {value} {label}")
            lbl.setStyleSheet(f"font-size: 11px; color: {_MUTED};")
            stats_row.addWidget(lbl)

        if project.project_tags:
            tags_lbl = QLabel("  ".join(f"#{t}" for t in project.project_tags))
            tags_lbl.setStyleSheet(f"font-size: 11px; color: {_ACCENT};")
            stats_row.addWidget(tags_lbl)

        stats_row.addStretch()
        inner.addLayout(stats_row)
        outer.addLayout(inner)

    def _note_count(self, project) -> int:
        if project.id is None:
            return 0
        try:
            from notes import count_project_notes
            return count_project_notes(project.id)
        except Exception:
            return 0


# ── Projects page ─────────────────────────────────────────────────────────────

class ProjectsPage(QWidget):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        outer = QVBoxLayout(self)
        outer.setContentsMargins(48, 40, 48, 40)
        outer.setSpacing(0)

        # Header row
        header = QHBoxLayout()
        col = QVBoxLayout()
        col.setSpacing(4)
        title = QLabel("Projects")
        title.setStyleSheet(
            f"font-size: 34px; font-weight: bold; color: {_ACCENT}; background: transparent;"
        )
        subtitle = QLabel("Organise papers into focused reading projects")
        subtitle.setStyleSheet(f"font-size: 13px; color: {_MUTED}; background: transparent;")
        col.addWidget(title)
        col.addWidget(subtitle)

        add_btn = QPushButton("＋  New Project")
        add_btn.setFixedSize(160, 40)
        add_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        add_btn.setStyleSheet(_BTN_STYLE)
        add_btn.clicked.connect(self._on_add)

        header.addLayout(col)
        header.addStretch()
        header.addWidget(add_btn, alignment=Qt.AlignmentFlag.AlignBottom)
        outer.addLayout(header)
        outer.addSpacing(28)

        # Scroll area for project cards
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")

        self._list_widget = QWidget()
        self._list_widget.setStyleSheet("background: transparent;")
        self._list_layout = QVBoxLayout(self._list_widget)
        self._list_layout.setContentsMargins(0, 0, 0, 0)
        self._list_layout.setSpacing(12)
        self._list_layout.addStretch()

        scroll.setWidget(self._list_widget)
        outer.addWidget(scroll)

        # Empty state (shown when no projects exist)
        self._empty_lbl = QLabel("No projects yet — create one to get started.")
        self._empty_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._empty_lbl.setStyleSheet(
            f"font-size: 14px; color: {_MUTED}; background: transparent;"
        )
        outer.addWidget(self._empty_lbl)
        outer.addStretch()

        self._refresh()

    def _refresh(self) -> None:
        while self._list_layout.count() > 1:
            item = self._list_layout.takeAt(0)
            if item.widget():
                item.widget().deleteLater()

        try:
            from projects import list_projects, ensure_projects_db
            ensure_projects_db()
            projects = list_projects()
        except Exception:
            projects = []

        if projects:
            self._empty_lbl.setVisible(False)
            for p in projects:
                card = ProjectCard(p)
                self._list_layout.insertWidget(self._list_layout.count() - 1, card)
        else:
            self._empty_lbl.setVisible(True)

    def _on_add(self) -> None:
        dlg = NewProjectDialog(self)
        if dlg.exec() == QDialog.DialogCode.Accepted:
            self._refresh()
