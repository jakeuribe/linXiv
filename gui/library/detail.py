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

from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED
from gui.theme import (
    FONT_SUBHEADING, FONT_BODY, FONT_SECONDARY, FONT_TERTIARY,
    SPACE_XL, SPACE_LG, SPACE_SM, SPACE_XS,
    RADIUS_LG, RADIUS_SM,
    BTN_H_SM,
    PAGE_MARGIN_H, CARD_PAD_H, CARD_PAD_V,
    NOTE_HEIGHT, ABSTRACT_HEIGHT,
)

from gui.views import MarkdownView
from storage.projects import filter_projects, Status, get_project
from storage.notes import get_notes

# NoteEditorDialog is imported lazily inside _edit_note to break a circular
# import: gui.projects → gui.library.page → gui.library.detail → gui.projects

_RED               = "#e05c5c"
_FONT_DETAIL_TITLE = 22   # paper title in detail view; between FONT_HEADING and FONT_TITLE

# Chip hover surfaces; no theme equivalents exist for these semantic states
_CHIP_BG_ACTIVE   = _PANEL
_CHIP_BG_ARCHIVED = _PANEL
_CHIP_HOVER_ACTIVE   = "#1a1f2e"
_CHIP_HOVER_ARCHIVED = "#2a1a1a"


class PaperDetailView(QWidget):
    back_requested      = pyqtSignal()
    navigate_to_project = pyqtSignal(object)   # emits Project

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._current_row = None
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        outer = QVBoxLayout(self)
        outer.setContentsMargins(PAGE_MARGIN_H, 32, PAGE_MARGIN_H, 24)
        outer.setSpacing(0)

        back_btn = QPushButton("← Back")
        back_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        back_btn.setStyleSheet(f"""
            QPushButton {{ background: transparent; border: none;
                color: {_ACCENT}; font-size: {FONT_BODY}px; padding: 0; }}
            QPushButton:hover {{ color: #7aa3f5; }}
        """)
        back_btn.setFixedHeight(BTN_H_SM)
        back_btn.clicked.connect(self.back_requested)
        outer.addWidget(back_btn, alignment=Qt.AlignmentFlag.AlignLeft)
        outer.addSpacing(SPACE_LG)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")

        self._body = QWidget()
        self._body.setStyleSheet("background: transparent;")
        self._body_layout = QVBoxLayout(self._body)
        self._body_layout.setContentsMargins(0, 0, 0, 24)
        self._body_layout.setSpacing(0)

        scroll.setWidget(self._body)
        outer.addWidget(scroll, stretch=1)

    # ── Public API ────────────────────────────────────────────────────────────

    def load(self, row) -> None:
        self._current_row = row
        self._clear_body()

        paper_id = row["paper_id"]
        self._build_header(row)
        self._build_abstract(row)
        containing = self._build_projects(paper_id)
        self._build_notes(paper_id, containing)
        self._body_layout.addStretch()

    def get_current_paper_id(self) -> str | None:
        if self._current_row is None:
            return None
        return self._current_row["paper_id"]

    # ── Section builders ──────────────────────────────────────────────────────

    def _clear_body(self) -> None:
        while self._body_layout.count():
            item = self._body_layout.takeAt(0)
            if item:
                w = item.widget()
                if w:
                    w.deleteLater()

    def _build_header(self, row) -> None:
        title_lbl = QLabel(row["title"] or "(untitled)")
        title_lbl.setWordWrap(True)
        title_lbl.setStyleSheet(
            f"font-size: {_FONT_DETAIL_TITLE}px; font-weight: bold;"
            f" color: {_TEXT}; background: transparent;"
        )
        self._body_layout.addWidget(title_lbl)
        self._body_layout.addSpacing(SPACE_SM)

        authors: list[str] = row["authors"] or []
        auth_str = ", ".join(authors) if authors else "Unknown authors"
        date_str = row["published"].isoformat() if row["published"] else ""
        cat_str  = row["category"] or ""
        doi_str  = row["doi"] if "doi" in row.keys() else None

        meta_lbl = QLabel("  ·  ".join(filter(None, [auth_str, date_str, cat_str])))
        meta_lbl.setWordWrap(True)
        meta_lbl.setStyleSheet(
            f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;"
        )
        self._body_layout.addWidget(meta_lbl)

        if doi_str:
            doi_lbl = QLabel(f"DOI: {doi_str}")
            doi_lbl.setStyleSheet(
                f"font-size: {FONT_TERTIARY}px; color: {_MUTED}; background: transparent;"
            )
            self._body_layout.addWidget(doi_lbl)

        tags: list[str] = row["tags"] or []
        if tags:
            tags_lbl = QLabel("  ".join(f"#{t}" for t in tags))
            tags_lbl.setStyleSheet(
                f"font-size: {FONT_SECONDARY}px; color: {_ACCENT}; background: transparent;"
            )
            self._body_layout.addSpacing(SPACE_XS)
            self._body_layout.addWidget(tags_lbl)

        self._body_layout.addSpacing(SPACE_LG)

    def _build_abstract(self, row) -> None:
        self._body_layout.addWidget(self._section_label("Abstract"))
        self._body_layout.addSpacing(SPACE_SM)

        summary = row["summary"] if "summary" in row.keys() else None
        if summary:
            md = MarkdownView()
            md.set_content(summary)
            md.setFixedHeight(ABSTRACT_HEIGHT)
            self._body_layout.addWidget(md)
        else:
            no_abs = QLabel("No abstract available.")
            no_abs.setStyleSheet(
                f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;"
            )
            self._body_layout.addWidget(no_abs)

        self._body_layout.addSpacing(SPACE_XL)

    def _build_projects(self, paper_id: str) -> list:
        self._body_layout.addWidget(self._section_label("Projects"))
        self._body_layout.addSpacing(SPACE_SM)

        try:
            all_projects = filter_projects()
            containing = [
                p for p in all_projects
                if paper_id in (p.paper_ids or []) and p.status != Status.DELETED
            ]
        except Exception:
            containing = []

        if containing:
            proj_row = QHBoxLayout()
            proj_row.setSpacing(SPACE_SM)
            proj_row.setContentsMargins(0, 0, 0, 0)
            for proj in containing:
                proj_row.addWidget(self._project_chip(proj))
            proj_row.addStretch()
            proj_widget = QWidget()
            proj_widget.setStyleSheet("background: transparent;")
            proj_widget.setLayout(proj_row)
            self._body_layout.addWidget(proj_widget)
        else:
            np_lbl = QLabel("Not in any project.")
            np_lbl.setStyleSheet(
                f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;"
            )
            self._body_layout.addWidget(np_lbl)

        self._body_layout.addSpacing(SPACE_XL)
        return containing

    def _build_notes(self, paper_id: str, containing: list) -> None:
        self._body_layout.addWidget(self._section_label("Notes"))
        self._body_layout.addSpacing(SPACE_SM)

        try:
            all_notes = get_notes(paper_id, all_projects=True)
        except Exception:
            all_notes = []

        if not all_notes:
            nn_lbl = QLabel("No notes for this paper yet.")
            nn_lbl.setStyleSheet(
                f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;"
            )
            self._body_layout.addWidget(nn_lbl)
            return

        proj_names: dict[int, str] = {p.id: p.name for p in containing if p.id is not None}
        note_proj_ids = {n.project_id for n in all_notes if n.project_id is not None}
        for pid in note_proj_ids - set(proj_names):
            try:
                p = get_project(pid)
                if p and p.id is not None:
                    proj_names[p.id] = p.name
            except Exception:
                pass

        for note in all_notes:
            self._body_layout.addWidget(self._note_card(note, proj_names))
            self._body_layout.addSpacing(SPACE_SM)

    # ── Widget factories ──────────────────────────────────────────────────────

    def _project_chip(self, proj) -> QPushButton:
        archived = proj.status.value == "archived"
        color    = _RED if archived else _ACCENT
        bg       = _CHIP_BG_ARCHIVED if archived else _CHIP_BG_ACTIVE
        hover_bg = _CHIP_HOVER_ARCHIVED if archived else _CHIP_HOVER_ACTIVE
        chip = QPushButton(proj.name)
        chip.setCursor(
            Qt.CursorShape.ArrowCursor if archived
            else Qt.CursorShape.PointingHandCursor
        )
        chip.setStyleSheet(f"""
            QPushButton {{
                background: {bg}; border: 1px solid {color};
                border-radius: {RADIUS_SM}px; color: {color};
                font-size: {FONT_SECONDARY}px; padding: {SPACE_XS}px {SPACE_SM}px;
                {'text-decoration: underline;' if not archived else ''}
            }}
            QPushButton:hover {{ background: {hover_bg}; }}
        """)
        if not archived:
            chip.clicked.connect(lambda _checked=False, p=proj: self.navigate_to_project.emit(p))
        return chip

    @staticmethod
    def _section_label(text: str) -> QLabel:
        lbl = QLabel(text)
        lbl.setStyleSheet(
            f"font-size: {FONT_SUBHEADING}px; font-weight: 600;"
            f" color: {_TEXT}; background: transparent;"
        )
        return lbl

    def _note_card(self, note, proj_names: dict[int, str]) -> QFrame:
        card = QFrame()
        card.setStyleSheet(f"""
            QFrame {{ background: {_PANEL}; border: 1px solid {_BORDER};
                border-radius: {RADIUS_LG}px; }}
            QLabel {{ border: none; background: transparent; }}
        """)
        col = QVBoxLayout(card)
        col.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)
        col.setSpacing(SPACE_XS)

        hdr = QHBoxLayout()
        if note.project_id is not None:
            proj_name = proj_names.get(note.project_id, f"Project {note.project_id}")
            chip = QLabel(f"📁 {proj_name}")
            chip.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_ACCENT};")
            hdr.addWidget(chip)
        else:
            standalone = QLabel("Standalone note")
            standalone.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
            hdr.addWidget(standalone)

        hdr.addStretch()
        if note.created_at:
            date_lbl = QLabel(note.created_at.strftime("%Y-%m-%d"))
            date_lbl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
            hdr.addWidget(date_lbl)

        edit_btn = QPushButton("Edit")
        edit_btn.setStyleSheet(f"""
            QPushButton {{ background: transparent; border: none;
                font-size: {FONT_TERTIARY}px; padding: 0 {SPACE_XS}px; color: {_MUTED}; }}
            QPushButton:hover {{ color: {_TEXT}; }}
        """)
        edit_btn.clicked.connect(lambda _, n=note: self._edit_note(n))
        hdr.addWidget(edit_btn)
        col.addLayout(hdr)

        md = MarkdownView()
        md.set_title(note.title or "Untitled")
        md.set_content(note.content or "")
        md.setFixedHeight(NOTE_HEIGHT)
        col.addWidget(md)

        return card

    def _edit_note(self, note) -> None:
        from gui.projects import NoteEditorDialog  # lazy: circular import
        dlg = NoteEditorDialog(note=note, parent=self)
        if dlg.exec():
            self.load(self._current_row)
