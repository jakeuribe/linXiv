from __future__ import annotations

import os
from pathlib import Path

from PyQt6.QtCore import Qt, QThread, pyqtSignal
from PyQt6.QtWidgets import (
    QCheckBox,
    QFileDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPushButton,
    QScrollArea,
    QSizePolicy,
    QStackedWidget,
    QVBoxLayout,
    QWidget,
)

from storage.db import list_papers, set_has_pdf, set_pdf_path
from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED
from gui.theme import (
    FONT_TITLE, FONT_BODY, FONT_SECONDARY, FONT_TERTIARY,
    SPACE_XL, SPACE_LG, SPACE_MD, SPACE_SM,
    RADIUS_LG, RADIUS_MD, RADIUS_SM,
    BTN_H_MD, BTN_H_SM,
    PAGE_MARGIN_H, PAGE_MARGIN_V, CARD_PAD_H, CARD_PAD_V, DIALOG_PAD,
    NOTE_HEIGHT, ABSTRACT_HEIGHT,
)

_GREEN = "#4caf7d"
_BLUE  = "#5b8dee"
_RED   = "#e05c5c"

_PDF_DIR = Path(__file__).parent.parent / "pdfs"

_BTN = f"""
    QPushButton {{
        background: {_PANEL}; border: 1px solid {_BORDER};
        border-radius: {RADIUS_MD}px; color: {_TEXT}; font-size: {FONT_SECONDARY}px; padding: 4px 14px;
    }}
    QPushButton:hover {{ background: #2a2a4a; }}
"""


# ── PDF download worker ───────────────────────────────────────────────────────

class _DownloadWorker(QThread):
    finished = pyqtSignal(str, int, str)   # paper_id, version, local_path
    failed   = pyqtSignal(str, int, str)   # paper_id, version, error

    def __init__(self, paper_id: str, version: int) -> None:
        super().__init__()
        self.paper_id = paper_id
        self.version  = version

    def run(self) -> None:
        import urllib.request
        _PDF_DIR.mkdir(parents=True, exist_ok=True)
        dest = _PDF_DIR / f"{self.paper_id}v{self.version}.pdf"
        url  = f"https://arxiv.org/pdf/{self.paper_id}v{self.version}"
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "linXiv/1.0"})
            with urllib.request.urlopen(req, timeout=30) as resp:
                dest.write_bytes(resp.read())
            self.finished.emit(self.paper_id, self.version, str(dest))
        except Exception as e:
            self.failed.emit(self.paper_id, self.version, str(e))


# ── Paper card ────────────────────────────────────────────────────────────────

class _PaperCard(QFrame):
    selection_toggled = pyqtSignal(str, bool)   # paper_id, selected
    clicked           = pyqtSignal(object)       # emits the DB row

    _CAT_COLORS: dict[str, str] = {
        "cs.LG": "#5b8dee", "cs.AI": "#7b6dee", "cs.CV": "#4db8c0",
        "cs.CL": "#ee8d5b", "cs.NE": "#5bbf8a", "physics": "#bf8a5b",
        "math":  "#bf5b8a", "stat":  "#8abf5b",
    }

    def __init__(self, row, pdf_window, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._row        = row
        self._pdf_window = pdf_window
        self._worker: _DownloadWorker | None = None
        self._selected   = False

        self._base_style = f"""
            QFrame#paperCard {{
                background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: {RADIUS_LG}px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """
        self._sel_style = f"""
            QFrame#paperCard {{
                background: {_PANEL}; border: 1px solid {_ACCENT}; border-radius: {RADIUS_LG}px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """
        self.setObjectName("paperCard")
        self.setStyleSheet(self._base_style)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)

        outer = QHBoxLayout(self)
        outer.setContentsMargins(10, 0, CARD_PAD_H-10, 0)
        outer.setSpacing(0)

        # Checkbox
        outer.addSpacing(CARD_PAD_H)
        self._chk = QCheckBox()
        self._chk.setFixedSize(44, 44)
        self._chk.setStyleSheet("QCheckBox { background: transparent; } QCheckBox::indicator { width: 14px; height: 14px; }")
        self._chk.stateChanged.connect(self._on_checkbox)
        outer.addWidget(self._chk)

        # Category colour stripe
        cat   = (row["category"] or "").split(".")[0] if row["category"] else ""
        color = self._CAT_COLORS.get(row["category"] or "", self._CAT_COLORS.get(cat, _ACCENT))
        stripe = QWidget()
        stripe.setFixedWidth(4)
        stripe.setStyleSheet(f"background: {color}; border-radius: 0;")
        outer.addWidget(stripe)

        # Body
        body = QVBoxLayout()
        body.setContentsMargins(14, 11, 0, 11)
        body.setSpacing(3)

        title_lbl = QLabel(row["title"] or "(untitled)")
        title_lbl.setWordWrap(True)
        title_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; font-weight: 600; color: {_TEXT};")
        body.addWidget(title_lbl)

        authors: list[str] = row["authors"] or []
        if authors:
            shown = ", ".join(authors[:3])
            if len(authors) > 3:
                shown += f" +{len(authors) - 3} more"
            body.addWidget(QLabel(shown))
            body.itemAt(body.count() - 1).widget().setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")  # type: ignore[union-attr]

        date_str = row["published"].isoformat() if row["published"] else ""
        cat_str  = row["category"] or ""
        meta     = "  ·  ".join(filter(None, [date_str, cat_str]))
        if meta:
            ml = QLabel(meta)
            ml.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
            body.addWidget(ml)

        tags: list[str] = row["tags"] or []
        if tags:
            tl = QLabel("  ".join(f"#{t}" for t in tags[:6]))
            tl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_ACCENT};")
            body.addWidget(tl)

        outer.addLayout(body, stretch=1)

        # PDF action button
        self._pdf_btn = QPushButton()
        self._pdf_btn.setFixedSize(116, BTN_H_SM)
        self._pdf_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._refresh_pdf_btn()
        self._pdf_btn.clicked.connect(self._on_pdf_action)
        outer.addWidget(self._pdf_btn, alignment=Qt.AlignmentFlag.AlignVCenter)

    # ── Selection ─────────────────────────────────────────────────────────────

    def set_selected(self, selected: bool) -> None:
        if self._selected == selected:
            return
        self._selected = selected
        self._chk.blockSignals(True)
        self._chk.setChecked(selected)
        self._chk.blockSignals(False)
        self.setStyleSheet(self._sel_style if selected else self._base_style)

    def is_selected(self) -> bool:
        return self._selected

    def _on_checkbox(self, state: int) -> None:
        checked = state == Qt.CheckState.Checked.value
        self._selected = checked
        self.setStyleSheet(self._sel_style if checked else self._base_style)
        self.selection_toggled.emit(self._row["paper_id"], checked)

    def mousePressEvent(self, event) -> None:
        if event.button() == Qt.MouseButton.LeftButton:
            if event.modifiers() & Qt.KeyboardModifier.ControlModifier:
                self.set_selected(not self._selected)
                self.selection_toggled.emit(self._row["paper_id"], self._selected)
                return
            # Plain click on card body → open detail view
            self.clicked.emit(self._row)
        super().mousePressEvent(event)

    # ── PDF helpers ───────────────────────────────────────────────────────────

    def paper_id(self) -> str:
        return self._row["paper_id"]

    def is_arxiv(self) -> bool:
        src = self._row["source"] if "source" in self._row.keys() else "arxiv"
        return (src or "arxiv") == "arxiv"

    def local_pdf_path(self) -> str | None:
        p = self._row["pdf_path"] if "pdf_path" in self._row.keys() else None
        if p and os.path.isfile(p):
            return p
        std = _PDF_DIR / f"{self._row['paper_id']}v{self._row['version']}.pdf"
        return str(std) if std.is_file() else None

    def _refresh_pdf_btn(self) -> None:
        path = self.local_pdf_path()
        if path:
            self._pdf_btn.setText("Open PDF")
            self._pdf_btn.setStyleSheet(f"""
                QPushButton {{ background: transparent; border: 1px solid {_GREEN};
                    border-radius: 5px; color: {_GREEN}; font-size: {FONT_TERTIARY}px; }}
                QPushButton:hover {{ background: #1a2e1f; }}
            """)
        elif self.is_arxiv():
            self._pdf_btn.setText("Download PDF")
            self._pdf_btn.setStyleSheet(f"""
                QPushButton {{ background: transparent; border: 1px solid {_BLUE};
                    border-radius: 5px; color: {_BLUE}; font-size: {FONT_TERTIARY}px; }}
                QPushButton:hover {{ background: #1a1f2e; }}
            """)
        else:
            self._pdf_btn.setText("Link PDF")
            self._pdf_btn.setStyleSheet(f"""
                QPushButton {{ background: transparent; border: 1px solid {_BORDER};
                    border-radius: 5px; color: {_MUTED}; font-size: {FONT_TERTIARY}px; }}
                QPushButton:hover {{ background: #1a1a2a; }}
            """)

    def _on_pdf_action(self) -> None:
        path = self.local_pdf_path()
        if path:
            self._pdf_window.load_pdf(path)
        elif self.is_arxiv():
            self._start_download()
        else:
            self._link_pdf()

    def start_download_if_needed(self) -> None:
        """Called by bulk download. No-op if already has a PDF or not arXiv."""
        if self.local_pdf_path() or not self.is_arxiv():
            return
        self._start_download()

    def _start_download(self) -> None:
        self._pdf_btn.setText("Downloading…")
        self._pdf_btn.setEnabled(False)
        pid, ver = self._row["paper_id"], self._row["version"]
        self._worker = _DownloadWorker(pid, ver)
        self._worker.finished.connect(self._on_download_done)
        self._worker.failed.connect(self._on_download_failed)
        self._worker.start()

    def _on_download_done(self, paper_id: str, version: int, path: str) -> None:
        set_pdf_path(paper_id, path)
        set_has_pdf(paper_id, version, True)
        self._pdf_btn.setEnabled(True)
        self._refresh_pdf_btn()
        self._pdf_window.load_pdf(path)

    def _on_download_failed(self, _pid: str, _ver: int, _err: str) -> None:
        self._pdf_btn.setEnabled(True)
        self._pdf_btn.setText("Failed — retry?")
        self._pdf_btn.setStyleSheet(f"""
            QPushButton {{ background: transparent; border: 1px solid {_RED};
                border-radius: 5px; color: {_RED}; font-size: {FONT_TERTIARY}px; }}
        """)

    def _link_pdf(self) -> None:
        path, _ = QFileDialog.getOpenFileName(self, "Link PDF", "", "PDF Files (*.pdf)")
        if not path:
            return
        pid, ver = self._row["paper_id"], self._row["version"]
        set_pdf_path(pid, path)
        set_has_pdf(pid, ver, True)
        self._refresh_pdf_btn()


# ── Action bar (shown when papers are selected) ───────────────────────────────

class _ActionBar(QFrame):
    download_requested     = pyqtSignal()
    add_to_project_requested = pyqtSignal()
    clear_requested        = pyqtSignal()
    select_all_requested   = pyqtSignal()

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"""
            QFrame {{ background: #1e1e36; border-top: 1px solid {_BORDER}; }}
            QLabel {{ background: transparent; border: none; }}
        """)
        self.setFixedHeight(52)

        row = QHBoxLayout(self)
        row.setContentsMargins(24, 0, 24, 0)
        row.setSpacing(SPACE_MD)

        self._count_lbl = QLabel()
        self._count_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; font-weight: 600; color: {_TEXT};")
        row.addWidget(self._count_lbl)
        row.addSpacing(SPACE_SM)

        self._dl_btn = self._btn("Download PDFs", _BLUE)
        self._dl_btn.clicked.connect(self.download_requested)
        row.addWidget(self._dl_btn)

        self._proj_btn = self._btn("Add to Project", _ACCENT)
        self._proj_btn.clicked.connect(self.add_to_project_requested)
        row.addWidget(self._proj_btn)

        row.addStretch()

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
        self._count_lbl.setText(f"{n} selected")

    @staticmethod
    def _btn(label: str, color: str) -> QPushButton:
        b = QPushButton(label)
        b.setFixedHeight(BTN_H_MD)
        b.setCursor(Qt.CursorShape.PointingHandCursor)
        b.setStyleSheet(f"""
            QPushButton {{ background: transparent; border: 1px solid {color};
                border-radius: {RADIUS_MD}px; color: {color}; font-size: {FONT_SECONDARY}px; padding: 4px 14px; }}
            QPushButton:hover {{ background: rgba(91,141,238,0.08); }}
        """)
        return b


# ── Paper detail view ────────────────────────────────────────────────────────

class PaperDetailView(QWidget):
    back_requested       = pyqtSignal()
    navigate_to_project  = pyqtSignal(object)   # emits Project

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._current_row = None
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        outer = QVBoxLayout(self)
        outer.setContentsMargins(48, 32, 48, 24)
        outer.setSpacing(0)

        # Back button
        back_btn = QPushButton("← Back to Library")
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

        # Scroll area holds everything below the back button
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

    def load(self, row) -> None:
        self._current_row = row
        # Clear previous content
        while self._body_layout.count():
            item = self._body_layout.takeAt(0)
            if item.widget():
                item.widget().deleteLater()  # pyright: ignore[reportOptionalMemberAccess]

        paper_id = row["paper_id"]

        # ── Title ─────────────────────────────────────────────────────────────
        title_lbl = QLabel(row["title"] or "(untitled)")
        title_lbl.setWordWrap(True)
        title_lbl.setStyleSheet(
            f"font-size: 22px; font-weight: bold; color: {_TEXT}; background: transparent;"
        )
        self._body_layout.addWidget(title_lbl)
        self._body_layout.addSpacing(10)

        # ── Meta row ──────────────────────────────────────────────────────────
        authors: list[str] = row["authors"] or []
        auth_str = ", ".join(authors) if authors else "Unknown authors"
        date_str = row["published"].isoformat() if row["published"] else ""
        cat_str  = row["category"] or ""
        doi_str  = row["doi"] if "doi" in row.keys() else None

        meta_parts = [auth_str, date_str, cat_str]
        meta_lbl = QLabel("  ·  ".join(filter(None, meta_parts)))
        meta_lbl.setWordWrap(True)
        meta_lbl.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;")
        self._body_layout.addWidget(meta_lbl)

        if doi_str:
            doi_lbl = QLabel(f"DOI: {doi_str}")
            doi_lbl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED}; background: transparent;")
            self._body_layout.addWidget(doi_lbl)

        tags: list[str] = row["tags"] or []
        if tags:
            tags_lbl = QLabel("  ".join(f"#{t}" for t in tags))
            tags_lbl.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_ACCENT}; background: transparent;")
            self._body_layout.addSpacing(6)
            self._body_layout.addWidget(tags_lbl)

        self._body_layout.addSpacing(SPACE_LG)

        # ── Abstract ──────────────────────────────────────────────────────────
        self._body_layout.addWidget(self._section_label("Abstract"))
        self._body_layout.addSpacing(SPACE_SM)

        summary = row["summary"] if "summary" in row.keys() else None
        if summary:
            from gui.markdown_view import MarkdownView
            md = MarkdownView()
            md.set_content(summary)
            md.setFixedHeight(ABSTRACT_HEIGHT)
            self._body_layout.addWidget(md)
        else:
            no_abs = QLabel("No abstract available.")
            no_abs.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;")
            self._body_layout.addWidget(no_abs)

        self._body_layout.addSpacing(SPACE_XL)

        # ── Projects containing this paper ────────────────────────────────────
        self._body_layout.addWidget(self._section_label("Projects"))
        self._body_layout.addSpacing(SPACE_SM)

        try:
            from projects import filter_projects, Status
            all_projects = filter_projects()
            containing = [p for p in all_projects
                          if paper_id in (p.paper_ids or [])
                          and p.status != Status.DELETED]
        except Exception:
            containing = []

        if containing:
            proj_row = QHBoxLayout()
            proj_row.setSpacing(SPACE_SM)
            proj_row.setContentsMargins(0, 0, 0, 0)
            for proj in containing:
                archived = proj.status.value == "archived"
                color    = _RED if archived else _BLUE
                chip     = QPushButton(proj.name)
                chip.setCursor(
                    Qt.CursorShape.ArrowCursor if archived
                    else Qt.CursorShape.PointingHandCursor
                )
                chip.setStyleSheet(f"""
                    QPushButton {{
                        background: #1e1e36; border: 1px solid {color};
                        border-radius: 12px; color: {color};
                        font-size: {FONT_SECONDARY}px; padding: 3px 12px;
                        {'text-decoration: underline;' if not archived else ''}
                    }}
                    QPushButton:hover {{
                        background: {'#2a1a1a' if archived else '#1a1f2e'};
                    }}
                """)
                if not archived:
                    chip.clicked.connect(
                        lambda _checked=False, p=proj: self.navigate_to_project.emit(p)
                    )
                proj_row.addWidget(chip)
            proj_row.addStretch()
            proj_widget = QWidget()
            proj_widget.setStyleSheet("background: transparent;")
            proj_widget.setLayout(proj_row)
            self._body_layout.addWidget(proj_widget)
        else:
            np_lbl = QLabel("Not in any project.")
            np_lbl.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;")
            self._body_layout.addWidget(np_lbl)

        self._body_layout.addSpacing(SPACE_XL)

        # ── Notes ─────────────────────────────────────────────────────────────
        self._body_layout.addWidget(self._section_label("Notes"))
        self._body_layout.addSpacing(SPACE_SM)

        try:
            from storage.notes import get_notes
            all_notes = get_notes(paper_id, all_projects=True)
        except Exception:
            all_notes = []

        if not all_notes:
            nn_lbl = QLabel("No notes for this paper yet.")
            nn_lbl.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; background: transparent;")
            self._body_layout.addWidget(nn_lbl)
        else:
            # Build project name lookup
            proj_names: dict[int, str] = {}
            for p in (containing if containing else []):
                if p.id is not None:
                    proj_names[p.id] = p.name
            # Also fetch any projects not already in containing (e.g. archived ones holding notes)
            note_proj_ids = {n.project_id for n in all_notes if n.project_id is not None}
            missing_ids = note_proj_ids - set(proj_names)
            if missing_ids:
                try:
                    from projects import get_project
                    for pid in missing_ids:
                        p = get_project(pid)
                        if p and p.id is not None:
                            proj_names[p.id] = p.name
                except Exception:
                    pass

            for note in all_notes:
                self._body_layout.addWidget(self._note_card(note, proj_names))
                self._body_layout.addSpacing(SPACE_SM)

        self._body_layout.addStretch()

    # ── Helpers ───────────────────────────────────────────────────────────────

    @staticmethod
    def _section_label(text: str) -> QLabel:
        lbl = QLabel(text)
        lbl.setStyleSheet(
            f"font-size: 16px; font-weight: 600; color: {_TEXT}; background: transparent;"
        )
        return lbl

    def _note_card(self, note, proj_names: dict[int, str]) -> QFrame:
        card = QFrame()
        card.setStyleSheet(f"""
            QFrame {{ background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: 8px; }}
            QLabel {{ border: none; background: transparent; }}
        """)
        col = QVBoxLayout(card)
        col.setContentsMargins(14, 10, 14, 10)
        col.setSpacing(6)

        # Note header: project chip + date + edit button
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

        _ghost = f"QPushButton {{ background: transparent; border: none; font-size: {FONT_TERTIARY}px; padding: 0 4px; }}"
        edit_btn = QPushButton("Edit")
        edit_btn.setStyleSheet(_ghost + f" QPushButton {{ color: {_MUTED}; }} QPushButton:hover {{ color: {_TEXT}; }}")
        edit_btn.clicked.connect(lambda _, n=note: self._edit_note(n))
        hdr.addWidget(edit_btn)
        col.addLayout(hdr)

        from gui.markdown_view import MarkdownView
        md = MarkdownView()
        md.set_title(note.title or "Untitled")
        md.set_content(note.content or "")
        md.setFixedHeight(NOTE_HEIGHT)
        col.addWidget(md)

        return card

    def _edit_note(self, note) -> None:
        from gui.projects_page import NoteEditorDialog
        dlg = NoteEditorDialog(note=note, parent=self)
        if dlg.exec():
            self.load(self._current_row)


# ── Library page ──────────────────────────────────────────────────────────────

class LibraryPage(QWidget):
    navigate_to_project = pyqtSignal(object)   # bubbled from PaperDetailView

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")
        self._all_rows: list = []
        self._cards:    list[_PaperCard] = []
        self._selected: set[str] = set()   # paper_ids

        from gui.pdf_window import PdfWindow
        self._pdf_window = PdfWindow(self)

        root = QVBoxLayout(self)
        root.setContentsMargins(0, 0, 0, 0)
        root.setSpacing(0)

        self._stack = QStackedWidget()
        root.addWidget(self._stack)

        # ── Page 0: list ──────────────────────────────────────────────────────
        list_page = QWidget()
        list_page.setStyleSheet(f"background: {_BG};")
        outer = QVBoxLayout(list_page)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.setSpacing(0)
        self._stack.addWidget(list_page)

        # ── Page 1: detail ────────────────────────────────────────────────────
        self._detail_view = PaperDetailView()
        self._back_override = None
        self._detail_view.back_requested.connect(self._on_back_requested)
        self._detail_view.navigate_to_project.connect(self.navigate_to_project)
        self._stack.addWidget(self._detail_view)

        # ── Inner (scrollable area + header + filter) ─────────────────────────
        inner_widget = QWidget()
        inner_widget.setStyleSheet(f"background: {_BG};")
        inner = QVBoxLayout(inner_widget)
        inner.setContentsMargins(PAGE_MARGIN_H, PAGE_MARGIN_V, PAGE_MARGIN_H, 16)
        inner.setSpacing(0)

        # Header row
        hdr = QHBoxLayout()
        title_lbl = QLabel("Library")
        title_lbl.setStyleSheet(
            f"font-size: {FONT_TITLE}px; font-weight: bold; color: {_ACCENT}; background: transparent;"
        )
        self._count_lbl = QLabel("")
        self._count_lbl.setStyleSheet(
            f"font-size: {FONT_BODY}px; color: {_MUTED}; background: transparent;"
        )
        hdr.addWidget(title_lbl)
        hdr.addSpacing(14)
        hdr.addWidget(self._count_lbl, alignment=Qt.AlignmentFlag.AlignBottom)
        hdr.addStretch()

        refresh_btn = QPushButton("Refresh")
        refresh_btn.setFixedHeight(BTN_H_MD)
        refresh_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        refresh_btn.setStyleSheet(_BTN)
        refresh_btn.clicked.connect(self.refresh)
        hdr.addWidget(refresh_btn)
        inner.addLayout(hdr)
        inner.addSpacing(SPACE_LG)

        # Filter bar
        filter_row = QHBoxLayout()
        filter_row.setSpacing(10)

        self._search = QLineEdit()
        self._search.setPlaceholderText("Search title or author…")
        self._search.setStyleSheet(f"""
            QLineEdit {{
                background: {_PANEL}; border: 1px solid {_BORDER};
                border-radius: {RADIUS_MD}px; color: {_TEXT}; font-size: {FONT_BODY}px; padding: 6px 12px;
            }}
            QLineEdit:focus {{ border-color: {_ACCENT}; }}
        """)
        self._search.textChanged.connect(self._apply_filter)

        self._filter_mode = "all"
        self._btn_all    = self._filter_btn("All",     "all")
        self._btn_haspdf = self._filter_btn("Has PDF", "has_pdf")
        self._btn_nopdf  = self._filter_btn("No PDF",  "no_pdf")
        self._sync_filter_btns()

        filter_row.addWidget(self._search, stretch=1)
        filter_row.addWidget(self._btn_all)
        filter_row.addWidget(self._btn_haspdf)
        filter_row.addWidget(self._btn_nopdf)
        inner.addLayout(filter_row)
        inner.addSpacing(SPACE_LG)

        # Cards scroll area
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")

        self._cards_widget = QWidget()
        self._cards_widget.setStyleSheet("background: transparent;")
        self._cards_layout = QVBoxLayout(self._cards_widget)
        self._cards_layout.setContentsMargins(0, 0, 0, 0)
        self._cards_layout.setSpacing(SPACE_SM)
        self._cards_layout.addStretch()

        scroll.setWidget(self._cards_widget)
        inner.addWidget(scroll, stretch=1)

        self._empty_lbl = QLabel("No papers match the current filter.")
        self._empty_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._empty_lbl.setStyleSheet(
            f"font-size: {FONT_BODY}px; color: {_MUTED}; background: transparent;"
        )
        self._empty_lbl.setVisible(False)
        inner.addWidget(self._empty_lbl)

        outer.addWidget(inner_widget, stretch=1)

        # ── Action bar (pinned to bottom, hidden when nothing selected) ───────
        self._action_bar = _ActionBar(self)
        self._action_bar.setVisible(False)
        self._action_bar.download_requested.connect(self._on_bulk_download)
        self._action_bar.add_to_project_requested.connect(self._on_add_to_project)
        self._action_bar.clear_requested.connect(self._clear_selection)
        self._action_bar.select_all_requested.connect(self._select_all)
        outer.addWidget(self._action_bar)

        self.refresh()

    # ── Filter buttons ────────────────────────────────────────────────────────

    def _filter_btn(self, label: str, mode: str) -> QPushButton:
        btn = QPushButton(label)
        btn.setFixedHeight(BTN_H_MD)
        btn.setCursor(Qt.CursorShape.PointingHandCursor)
        btn.clicked.connect(lambda: self._set_filter(mode))
        return btn

    def _set_filter(self, mode: str) -> None:
        self._filter_mode = mode
        self._sync_filter_btns()
        self._apply_filter()

    def _sync_filter_btns(self) -> None:
        for btn, mode in [
            (self._btn_all,    "all"),
            (self._btn_haspdf, "has_pdf"),
            (self._btn_nopdf,  "no_pdf"),
        ]:
            if mode == self._filter_mode:
                btn.setStyleSheet(f"""
                    QPushButton {{ background: {_ACCENT}; border: none;
                        border-radius: {RADIUS_MD}px; color: #fff; font-size: {FONT_SECONDARY}px; padding: 4px 14px; }}
                """)
            else:
                btn.setStyleSheet(_BTN)

    # ── Data ──────────────────────────────────────────────────────────────────

    def refresh(self) -> None:
        self._all_rows = list_papers(latest_only=True)
        self._apply_filter()

    def _apply_filter(self) -> None:
        query = self._search.text().strip().lower()
        mode  = self._filter_mode
        filtered = []
        for row in self._all_rows:
            has_pdf  = bool(row["has_pdf"])
            pdf_path = row["pdf_path"] if "pdf_path" in row.keys() else None
            local    = has_pdf or (pdf_path and os.path.isfile(pdf_path))
            if mode == "has_pdf" and not local:
                continue
            if mode == "no_pdf" and local:
                continue
            if query:
                title    = (row["title"] or "").lower()
                authors: list[str] = row["authors"] or []
                if query not in title and query not in " ".join(authors).lower():
                    continue
            filtered.append(row)

        self._rebuild_cards(filtered)
        self._count_lbl.setText(f"{len(filtered)} of {len(self._all_rows)} papers")

    def _rebuild_cards(self, rows: list) -> None:
        # Remove old cards
        while self._cards_layout.count() > 1:
            item = self._cards_layout.takeAt(0)
            if item.widget():
                item.widget().deleteLater()  # pyright: ignore[reportOptionalMemberAccess]
        self._cards = []

        self._empty_lbl.setVisible(not rows)
        for row in rows:
            card = _PaperCard(row, self._pdf_window, self._cards_widget)
            if row["paper_id"] in self._selected:
                card.set_selected(True)
            card.selection_toggled.connect(self._on_card_toggle)
            card.clicked.connect(self._open_detail)
            self._cards_layout.insertWidget(self._cards_layout.count() - 1, card)
            self._cards.append(card)

    # ── Selection ─────────────────────────────────────────────────────────────

    def open_paper(self, paper_id: str, on_back=None) -> None:
        """Navigate directly to the detail view for a given paper_id.

        on_back: optional callable invoked when the back button is pressed
                 instead of returning to the library list.
        """
        from storage.db import get_paper
        row = get_paper(paper_id)
        if row is not None:
            self._back_override = on_back
            self._open_detail(row)

    def _on_back_requested(self) -> None:
        if self._back_override is not None:
            cb, self._back_override = self._back_override, None
            cb()
        else:
            self._stack.setCurrentIndex(0)

    def _open_detail(self, row) -> None:
        self._back_override = None
        self._detail_view.load(row)
        self._stack.setCurrentIndex(1)

    def _on_card_toggle(self, paper_id: str, selected: bool) -> None:
        if selected:
            self._selected.add(paper_id)
        else:
            self._selected.discard(paper_id)
        self._sync_action_bar()

    def _clear_selection(self) -> None:
        self._selected.clear()
        for card in self._cards:
            card.set_selected(False)
        self._sync_action_bar()

    def _select_all(self) -> None:
        for card in self._cards:
            self._selected.add(card.paper_id())
            card.set_selected(True)
        self._sync_action_bar()

    def _sync_action_bar(self) -> None:
        n = len(self._selected)
        self._action_bar.setVisible(n > 0)
        self._action_bar.set_count(n)

    # ── Bulk actions ──────────────────────────────────────────────────────────

    def _on_bulk_download(self) -> None:
        for card in self._cards:
            if card.paper_id() in self._selected:
                card.start_download_if_needed()

    def _on_add_to_project(self) -> None:
        if not self._selected:
            return
        from PyQt6.QtWidgets import QDialog, QComboBox, QDialogButtonBox, QMessageBox
        from projects import filter_projects, Q

        projects = filter_projects(Q("status = 'active'"))
        if not projects:
            QMessageBox.information(self, "No Projects", "Create a project first.")
            return

        dlg = QDialog(self)
        dlg.setWindowTitle("Add to Project")
        dlg.setStyleSheet(f"background: {_BG}; color: {_TEXT};")
        lay = QVBoxLayout(dlg)
        lay.setContentsMargins(DIALOG_PAD, CARD_PAD_V, DIALOG_PAD, CARD_PAD_V)
        lay.setSpacing(SPACE_MD)

        lay.addWidget(QLabel(f"Add {len(self._selected)} paper(s) to:"))
        combo = QComboBox()
        combo.setStyleSheet(f"""
            QComboBox {{ background: {_PANEL}; border: 1px solid {_BORDER};
                border-radius: {RADIUS_SM}px; color: {_TEXT}; padding: 4px 8px; font-size: {FONT_BODY}px; }}
        """)
        for p in projects:
            combo.addItem(p.name, userData=p)
        lay.addWidget(combo)

        btns = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel
        )
        btns.accepted.connect(dlg.accept)
        btns.rejected.connect(dlg.reject)
        lay.addWidget(btns)

        if dlg.exec() != QDialog.DialogCode.Accepted:
            return

        project = combo.currentData()
        for pid in self._selected:
            try:
                project.add_paper(pid)
            except Exception:
                pass
