from __future__ import annotations

from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtWidgets import (
    QDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QPushButton,
    QScrollArea,
    QSizePolicy,
    QStackedWidget,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from projects import color_to_hex
from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED

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
_BTN_SMALL_STYLE = f"""
    QPushButton {{
        background: transparent; border: 1px solid {_BORDER}; border-radius: 4px;
        color: {_MUTED}; font-size: 11px; padding: 3px 10px;
    }}
    QPushButton:hover {{ border-color: {_ACCENT}; color: {_ACCENT}; }}
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

        lay.addWidget(self._field_label("Name"))
        self._name = QLineEdit()
        self._name.setPlaceholderText("e.g. Diffusion Models")
        self._name.setStyleSheet(_INPUT_STYLE)
        lay.addWidget(self._name)

        lay.addWidget(self._field_label("Description  (optional)"))
        self._desc = QTextEdit()
        self._desc.setPlaceholderText("What is this project about?")
        self._desc.setFixedHeight(72)
        self._desc.setStyleSheet(_INPUT_STYLE)
        lay.addWidget(self._desc)

        lay.addWidget(self._field_label("Project tags  (comma-separated, optional)"))
        self._project_tags = QLineEdit()
        self._project_tags.setPlaceholderText("e.g. generative, vision")
        self._project_tags.setStyleSheet(_INPUT_STYLE)
        lay.addWidget(self._project_tags)

        lay.addWidget(self._field_label("Colour"))
        lay.addLayout(self._build_swatches())
        lay.addSpacing(4)

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
        raw = self._project_tags.text().strip()
        project_tags = [t.strip() for t in raw.split(",") if t.strip()] if raw else []

        from projects import Project, ensure_projects_db
        from notes import ensure_notes_db
        ensure_projects_db()
        ensure_notes_db()

        p = Project(name=name, description=desc, color=self._color, project_tags=project_tags)
        p.save()
        self.accept()


# ── Add-paper dialog ──────────────────────────────────────────────────────────

class AddPaperDialog(QDialog):
    def __init__(self, project, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._project = project
        self.setWindowTitle("Add Paper to Project")
        self.setFixedSize(560, 440)
        self.setStyleSheet(f"background: {_PANEL}; color: {_TEXT};")

        lay = QVBoxLayout(self)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)

        title = QLabel("Add Paper")
        title.setStyleSheet(f"font-size: 18px; font-weight: bold; color: {_ACCENT};")
        lay.addWidget(title)

        self._filter = QLineEdit()
        self._filter.setPlaceholderText("Filter by title or arXiv ID…")
        self._filter.setStyleSheet(_INPUT_STYLE)
        self._filter.textChanged.connect(self._apply_filter)
        lay.addWidget(self._filter)

        self._list = QListWidget()
        self._list.setStyleSheet(f"""
            QListWidget {{
                background: {_BG}; border: 1px solid {_BORDER}; border-radius: 6px;
                color: {_TEXT}; font-size: 12px;
            }}
            QListWidget::item:selected {{ background: {_ACCENT}; color: #fff; }}
            QListWidget::item:hover    {{ background: #2a2a4a; }}
        """)
        self._list.itemDoubleClicked.connect(self._on_add)
        lay.addWidget(self._list)

        btn_row = QHBoxLayout()
        cancel = QPushButton("Cancel")
        cancel.setStyleSheet(_BTN_MUTED_STYLE)
        cancel.clicked.connect(self.reject)
        self._add_btn = QPushButton("Add to Project")
        self._add_btn.setStyleSheet(_BTN_STYLE)
        self._add_btn.clicked.connect(self._on_add)
        btn_row.addWidget(cancel)
        btn_row.addStretch()
        btn_row.addWidget(self._add_btn)
        lay.addLayout(btn_row)

        self._load_papers()

    def _load_papers(self) -> None:
        from db import list_papers
        self._all_papers = list_papers()
        already = set(self._project.paper_ids)
        self._papers = [r for r in self._all_papers if r["paper_id"] not in already]
        self._populate(self._papers)

    def _populate(self, papers) -> None:
        self._list.clear()
        for row in papers:
            item = QListWidgetItem(f"{row['title']}  [{row['paper_id']}]")
            item.setData(Qt.ItemDataRole.UserRole, row["paper_id"])
            self._list.addItem(item)

    def _apply_filter(self, text: str) -> None:
        q = text.lower()
        filtered = [
            r for r in self._papers
            if q in r["title"].lower() or q in r["paper_id"].lower()
        ] if q else self._papers
        self._populate(filtered)

    def _on_add(self) -> None:
        item = self._list.currentItem()
        if item is None:
            return
        paper_id = item.data(Qt.ItemDataRole.UserRole)
        self._project.add_paper(paper_id)
        self.accept()


# ── Add-note dialog ───────────────────────────────────────────────────────────

class AddNoteDialog(QDialog):
    def __init__(self, paper_id: str, project_id: int, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._paper_id  = paper_id
        self._project_id = project_id
        self.setWindowTitle("Add Note")
        self.setFixedSize(480, 320)
        self.setStyleSheet(f"background: {_PANEL}; color: {_TEXT};")

        lay = QVBoxLayout(self)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)

        title = QLabel("Add Note")
        title.setStyleSheet(f"font-size: 18px; font-weight: bold; color: {_ACCENT};")
        lay.addWidget(title)

        self._note_title = QLineEdit()
        self._note_title.setPlaceholderText("Title  (optional)")
        self._note_title.setStyleSheet(_INPUT_STYLE)
        lay.addWidget(self._note_title)

        self._content = QTextEdit()
        self._content.setPlaceholderText("Note content…")
        self._content.setStyleSheet(_INPUT_STYLE)
        lay.addWidget(self._content)

        btn_row = QHBoxLayout()
        cancel = QPushButton("Cancel")
        cancel.setStyleSheet(_BTN_MUTED_STYLE)
        cancel.clicked.connect(self.reject)
        save_btn = QPushButton("Save Note")
        save_btn.setStyleSheet(_BTN_STYLE)
        save_btn.clicked.connect(self._on_save)
        btn_row.addWidget(cancel)
        btn_row.addStretch()
        btn_row.addWidget(save_btn)
        lay.addLayout(btn_row)

    def _on_save(self) -> None:
        from notes import Note, ensure_notes_db
        ensure_notes_db()
        note = Note(
            paper_id   = self._paper_id,
            project_id = self._project_id,
            title      = self._note_title.text().strip(),
            content    = self._content.toPlainText().strip(),
        )
        note.save()
        self.accept()


# ── Notes viewer dialog ───────────────────────────────────────────────────────

class NotesDialog(QDialog):
    """Shows all notes for a paper in a project, with add and delete actions."""

    def __init__(self, paper_id: str, project_id: int, paper_title: str,
                 parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._paper_id   = paper_id
        self._project_id = project_id
        self.setWindowTitle("Notes")
        self.setFixedSize(560, 520)
        self.setStyleSheet(f"background: {_PANEL}; color: {_TEXT};")

        lay = QVBoxLayout(self)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)

        header_row = QHBoxLayout()
        title_lbl = QLabel("Notes")
        title_lbl.setStyleSheet(f"font-size: 18px; font-weight: bold; color: {_ACCENT};")
        paper_lbl = QLabel(paper_title)
        paper_lbl.setStyleSheet(f"font-size: 11px; color: {_MUTED};")
        paper_lbl.setWordWrap(True)
        header_col = QVBoxLayout()
        header_col.setSpacing(2)
        header_col.addWidget(title_lbl)
        header_col.addWidget(paper_lbl)
        header_row.addLayout(header_col, stretch=1)

        add_btn = QPushButton("＋  Add Note")
        add_btn.setStyleSheet(_BTN_STYLE)
        add_btn.setFixedHeight(34)
        add_btn.clicked.connect(self._on_add)
        header_row.addWidget(add_btn, alignment=Qt.AlignmentFlag.AlignBottom)
        lay.addLayout(header_row)

        # Scrollable notes list
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")

        self._notes_widget = QWidget()
        self._notes_widget.setStyleSheet("background: transparent;")
        self._notes_layout = QVBoxLayout(self._notes_widget)
        self._notes_layout.setContentsMargins(0, 0, 0, 0)
        self._notes_layout.setSpacing(10)
        self._notes_layout.addStretch()
        scroll.setWidget(self._notes_widget)
        lay.addWidget(scroll)

        self._empty_lbl = QLabel("No notes yet.")
        self._empty_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._empty_lbl.setStyleSheet(f"font-size: 13px; color: {_MUTED};")
        lay.addWidget(self._empty_lbl)

        close_btn = QPushButton("Close")
        close_btn.setStyleSheet(_BTN_MUTED_STYLE)
        close_btn.clicked.connect(self.accept)
        lay.addWidget(close_btn, alignment=Qt.AlignmentFlag.AlignRight)

        self._rebuild()

    def _rebuild(self) -> None:
        while self._notes_layout.count() > 1:
            item = self._notes_layout.takeAt(0)
            if item.widget():
                item.widget().deleteLater()

        from notes import get_notes, ensure_notes_db
        ensure_notes_db()
        notes = get_notes(self._paper_id, project_id=self._project_id)

        if notes:
            self._empty_lbl.setVisible(False)
            for note in notes:
                self._notes_layout.insertWidget(
                    self._notes_layout.count() - 1,
                    self._make_note_card(note),
                )
        else:
            self._empty_lbl.setVisible(True)

    def _make_note_card(self, note) -> QFrame:
        card = QFrame()
        card.setStyleSheet(f"""
            QFrame {{ background: {_BG}; border: 1px solid {_BORDER}; border-radius: 6px; }}
            QLabel {{ border: none; background: transparent; }}
        """)
        col = QVBoxLayout(card)
        col.setContentsMargins(14, 10, 14, 10)
        col.setSpacing(4)

        top_row = QHBoxLayout()
        note_title = note.title or "Untitled"
        title_lbl = QLabel(note_title)
        title_lbl.setStyleSheet(f"font-size: 13px; font-weight: 600; color: {_TEXT};")
        top_row.addWidget(title_lbl, stretch=1)

        if note.created_at:
            date_lbl = QLabel(note.created_at.strftime("%Y-%m-%d"))
            date_lbl.setStyleSheet(f"font-size: 11px; color: {_MUTED};")
            top_row.addWidget(date_lbl)

        del_btn = QPushButton("Delete")
        del_btn.setStyleSheet(f"""
            QPushButton {{
                background: transparent; border: none;
                color: #e05c5c; font-size: 11px; padding: 0 4px;
            }}
            QPushButton:hover {{ color: #ff7070; }}
        """)
        del_btn.clicked.connect(lambda _, n=note: self._delete_note(n))
        top_row.addWidget(del_btn)
        col.addLayout(top_row)

        if note.content:
            content_lbl = QLabel(note.content)
            content_lbl.setStyleSheet(f"font-size: 12px; color: {_MUTED};")
            content_lbl.setWordWrap(True)
            col.addWidget(content_lbl)

        return card

    def _delete_note(self, note) -> None:
        note.delete()
        self._rebuild()

    def _on_add(self) -> None:
        dlg = AddNoteDialog(self._paper_id, self._project_id, self)
        if dlg.exec() == QDialog.DialogCode.Accepted:
            self._rebuild()


# ── Paper row (inside detail view) ────────────────────────────────────────────

class _PaperRow(QFrame):
    def __init__(self, paper_id: str, project_id: int, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._paper_id   = paper_id
        self._project_id = project_id
        self.setStyleSheet(f"""
            QFrame {{ background: {_BG}; border: 1px solid {_BORDER}; border-radius: 6px; }}
            QLabel {{ border: none; background: transparent; }}
        """)

        row = QHBoxLayout(self)
        row.setContentsMargins(12, 8, 12, 8)
        row.setSpacing(12)

        title_str = self._fetch_title()
        title_lbl = QLabel(title_str)
        title_lbl.setStyleSheet(f"font-size: 13px; color: {_TEXT};")
        title_lbl.setWordWrap(True)
        row.addWidget(title_lbl, stretch=1)

        self._note_btn = QPushButton(self._note_label())
        self._note_btn.setStyleSheet(_BTN_SMALL_STYLE)
        self._note_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._note_btn.clicked.connect(self._on_open_notes)
        row.addWidget(self._note_btn)

    def _fetch_title(self) -> str:
        try:
            from db import get_paper
            row = get_paper(self._paper_id)
            return row["title"] if row else self._paper_id
        except Exception:
            return self._paper_id

    def _note_count(self) -> int:
        try:
            from notes import count_paper_notes
            return count_paper_notes(self._paper_id, self._project_id)
        except Exception:
            return 0

    def _note_label(self) -> str:
        n = self._note_count()
        return f"📝 {n} {'note' if n == 1 else 'notes'}"

    def _on_open_notes(self) -> None:
        dlg = NotesDialog(self._paper_id, self._project_id, self._fetch_title(), self)
        dlg.exec()
        self._note_btn.setText(self._note_label())


# ── Project detail view ───────────────────────────────────────────────────────

class ProjectDetailView(QWidget):
    back_requested = pyqtSignal()

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")
        self._project = None

        outer = QVBoxLayout(self)
        outer.setContentsMargins(48, 32, 48, 32)
        outer.setSpacing(0)

        # Header
        header = QHBoxLayout()
        header.setSpacing(16)

        back_btn = QPushButton("← Back")
        back_btn.setStyleSheet(_BTN_MUTED_STYLE)
        back_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        back_btn.setFixedWidth(90)
        back_btn.clicked.connect(self.back_requested)
        header.addWidget(back_btn)

        self._color_stripe = QWidget()
        self._color_stripe.setFixedSize(6, 36)
        self._color_stripe.setStyleSheet(f"background: {_ACCENT}; border-radius: 3px;")
        header.addWidget(self._color_stripe)

        self._title_lbl = QLabel()
        self._title_lbl.setStyleSheet(
            f"font-size: 28px; font-weight: bold; color: {_TEXT}; background: transparent;"
        )
        header.addWidget(self._title_lbl, stretch=1)

        outer.addLayout(header)
        outer.addSpacing(16)

        # Meta (description + tags)
        self._desc_lbl = QLabel()
        self._desc_lbl.setWordWrap(True)
        self._desc_lbl.setStyleSheet(f"font-size: 13px; color: {_MUTED}; background: transparent;")
        outer.addWidget(self._desc_lbl)

        self._tags_lbl = QLabel()
        self._tags_lbl.setStyleSheet(f"font-size: 12px; color: {_ACCENT}; background: transparent;")
        outer.addWidget(self._tags_lbl)
        outer.addSpacing(20)

        # Papers section header
        papers_header = QHBoxLayout()
        self._papers_lbl = QLabel("Papers")
        self._papers_lbl.setStyleSheet(
            f"font-size: 16px; font-weight: 600; color: {_TEXT}; background: transparent;"
        )
        self._add_paper_btn = QPushButton("＋  Add Paper")
        self._add_paper_btn.setStyleSheet(_BTN_STYLE)
        self._add_paper_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._add_paper_btn.setFixedHeight(34)
        self._add_paper_btn.clicked.connect(self._on_add_paper)
        papers_header.addWidget(self._papers_lbl)
        papers_header.addStretch()
        papers_header.addWidget(self._add_paper_btn)
        outer.addLayout(papers_header)
        outer.addSpacing(10)

        # Scrollable papers list
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")

        self._papers_widget = QWidget()
        self._papers_widget.setStyleSheet("background: transparent;")
        self._papers_layout = QVBoxLayout(self._papers_widget)
        self._papers_layout.setContentsMargins(0, 0, 0, 0)
        self._papers_layout.setSpacing(8)
        self._papers_layout.addStretch()

        scroll.setWidget(self._papers_widget)
        outer.addWidget(scroll)

        self._empty_papers_lbl = QLabel("No papers yet — add one to get started.")
        self._empty_papers_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._empty_papers_lbl.setStyleSheet(
            f"font-size: 13px; color: {_MUTED}; background: transparent;"
        )
        outer.addWidget(self._empty_papers_lbl)
        outer.addStretch()

    def load(self, project) -> None:
        self._project = project

        hex_color = color_to_hex(project.color) if project.color is not None else _ACCENT
        self._color_stripe.setStyleSheet(f"background: {hex_color}; border-radius: 3px;")
        self._title_lbl.setText(project.name)
        self._desc_lbl.setText(project.description)
        self._desc_lbl.setVisible(bool(project.description))
        self._tags_lbl.setText("  ".join(f"#{t}" for t in project.project_tags))
        self._tags_lbl.setVisible(bool(project.project_tags))

        self._rebuild_papers()

    def _rebuild_papers(self) -> None:
        while self._papers_layout.count() > 1:
            item = self._papers_layout.takeAt(0)
            if item.widget():
                item.widget().deleteLater()

        paper_ids = self._project.paper_ids if self._project else []
        count = len(paper_ids)
        self._papers_lbl.setText(f"Papers  ({count})")

        if paper_ids:
            self._empty_papers_lbl.setVisible(False)
            for pid in paper_ids:
                row_widget = _PaperRow(pid, self._project.id)
                self._papers_layout.insertWidget(self._papers_layout.count() - 1, row_widget)
        else:
            self._empty_papers_lbl.setVisible(True)

    def _on_add_paper(self) -> None:
        if self._project is None:
            return
        dlg = AddPaperDialog(self._project, self)
        if dlg.exec() == QDialog.DialogCode.Accepted:
            self._rebuild_papers()


# ── Project card ──────────────────────────────────────────────────────────────

class ProjectCard(QFrame):
    clicked = pyqtSignal(object)   # emits the Project

    def __init__(self, project, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._project = project
        self.setCursor(Qt.CursorShape.PointingHandCursor)
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

    def mousePressEvent(self, event) -> None:
        self.clicked.emit(self._project)
        super().mousePressEvent(event)


# ── Projects page ─────────────────────────────────────────────────────────────

class ProjectsPage(QWidget):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)

        self._inner = QStackedWidget()
        self._inner.addWidget(self._build_list_page())   # index 0

        self._detail_view = ProjectDetailView()
        self._detail_view.back_requested.connect(self._on_back)
        self._inner.addWidget(self._detail_view)          # index 1

        outer.addWidget(self._inner)
        self._refresh()

    # ── List page ─────────────────────────────────────────────────────────────

    def _build_list_page(self) -> QWidget:
        page = QWidget()
        page.setStyleSheet(f"background: {_BG};")
        outer = QVBoxLayout(page)
        outer.setContentsMargins(48, 40, 48, 40)
        outer.setSpacing(0)

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

        self._empty_lbl = QLabel("No projects yet — create one to get started.")
        self._empty_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._empty_lbl.setStyleSheet(
            f"font-size: 14px; color: {_MUTED}; background: transparent;"
        )
        outer.addWidget(self._empty_lbl)
        outer.addStretch()

        return page

    def _refresh(self) -> None:
        while self._list_layout.count() > 1:
            item = self._list_layout.takeAt(0)
            if item.widget():
                item.widget().deleteLater()

        try:
            from projects import filter_projects, ensure_projects_db, Q, Status
            ensure_projects_db()
            projects = filter_projects(Q("status = ?", Status.ACTIVE))
        except Exception:
            projects = []

        if projects:
            self._empty_lbl.setVisible(False)
            for p in projects:
                card = ProjectCard(p)
                card.clicked.connect(self._open_project)
                self._list_layout.insertWidget(self._list_layout.count() - 1, card)
        else:
            self._empty_lbl.setVisible(True)

    # ── Navigation ────────────────────────────────────────────────────────────

    def _open_project(self, project) -> None:
        self._detail_view.load(project)
        self._inner.setCurrentIndex(1)

    def _on_back(self) -> None:
        self._inner.setCurrentIndex(0)
        self._refresh()

    def _on_add(self) -> None:
        dlg = NewProjectDialog(self)
        if dlg.exec() == QDialog.DialogCode.Accepted:
            self._refresh()
