import os
from pathlib import Path

from PyQt6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QLineEdit, QPushButton, QListWidget, QListWidgetItem,
    QLabel, QSplitter, QCheckBox, QComboBox, QSpinBox,
    QFrame, QFileDialog
)
from PyQt6.QtCore import Qt, QThread, pyqtSignal
import arxiv
from fetch_paper_metadata import search_papers
from db import save_paper, delete_paper, get_paper, set_has_pdf, parse_entry_id

_PDF_DIR = Path(__file__).parent.parent / "pdfs"
from downloads import download_pdf, cleanup_pdfs as _cleanup_pdfs, saved_pdfs_size
from .tex_view import TexView
from .pdf_window import PdfWindow

_FIELD_OPTIONS = [
    ("Author",     "au:"),
    ("Title",      "ti:"),
    ("Abstract",   "abs:"),
    ("Category",   "cat:"),
    ("Comment",    "co:"),
    ("Journal Ref","jr:"),
    ("All fields", ""),
]

_SORT_BY_OPTIONS = [
    ("Relevance",     arxiv.SortCriterion.Relevance),
    ("Submitted Date",arxiv.SortCriterion.SubmittedDate),
    ("Last Updated",  arxiv.SortCriterion.LastUpdatedDate),
]

_SORT_ORDER_OPTIONS = [
    ("Descending", arxiv.SortOrder.Descending),
    ("Ascending",  arxiv.SortOrder.Ascending),
]


class _ClauseRow(QWidget):
    changed = pyqtSignal()
    remove_requested = pyqtSignal(object)

    def __init__(self, show_operator: bool = False, parent=None):
        super().__init__(parent)
        self._layout = QHBoxLayout(self)
        self._layout.setContentsMargins(0, 0, 0, 0)
        self._layout.setSpacing(6)

        self._op_combo = QComboBox()
        self._op_combo.addItems(["AND", "OR", "ANDNOT"])
        self._op_combo.setFixedWidth(80)
        self._op_combo.currentIndexChanged.connect(self.changed)
        self._op_combo.setVisible(show_operator)
        self._layout.addWidget(self._op_combo)

        self._field_combo = QComboBox()
        for label, _ in _FIELD_OPTIONS:
            self._field_combo.addItem(label)
        self._field_combo.currentIndexChanged.connect(self.changed)
        self._layout.addWidget(self._field_combo)

        self._value = QLineEdit()
        self._value.setPlaceholderText("value…")
        self._value.textChanged.connect(self.changed)
        self._layout.addWidget(self._value, stretch=1)

        rm = QPushButton("×")
        rm.setFixedWidth(28)
        rm.clicked.connect(lambda: self.remove_requested.emit(self))
        self._layout.addWidget(rm)

    def set_operator_visible(self, visible: bool) -> None:
        self._op_combo.setVisible(visible)

    @property
    def operator(self) -> str:
        return self._op_combo.currentText()

    @property
    def prefix(self) -> str:
        return _FIELD_OPTIONS[self._field_combo.currentIndex()][1]

    @property
    def value(self) -> str:
        return self._value.text().strip()

    def to_clause(self) -> str:
        if not self.value:
            return ""
        v = self.value
        if " " in v:
            v = f'"{v}"'
        return f"{self.prefix}{v}"


class _ResultList(QListWidget):
    def resizeEvent(self, event):
        super().resizeEvent(event)
        w = self.viewport().width()  # pyright: ignore[reportOptionalMemberAccess] — technically fixable but awkward with current setup
        for i in range(self.count()):
            item = self.item(i)
            widget = self.itemWidget(item)
            if widget is not None:
                widget.setFixedWidth(w)
                item.setSizeHint(widget.sizeHint())  # pyright: ignore[reportOptionalMemberAccess] — technically fixable but awkward with current setup


class _ResultRow(QWidget):
    def __init__(self, title: str, parent=None):
        super().__init__(parent)
        layout = QHBoxLayout(self)
        layout.setContentsMargins(4, 2, 4, 2)
        layout.setSpacing(8)

        self._label = QLabel(title)
        self._label.setWordWrap(True)
        layout.addWidget(self._label, stretch=1)

        self._checkbox = QCheckBox("Save")
        self._checkbox.setFixedWidth(60)
        layout.addWidget(self._checkbox, alignment=Qt.AlignmentFlag.AlignTop)

    @property
    def checked(self) -> bool:
        return self._checkbox.isChecked()

    def set_checked(self, value: bool) -> None:
        self._checkbox.setChecked(value)


class _SearchWorker(QThread):
    done = pyqtSignal(list)

    def __init__(self, query: str, max_results: int,
                 sort_by: arxiv.SortCriterion, sort_order: arxiv.SortOrder):
        super().__init__()
        self.query = query
        self.max_results = max_results
        self.sort_by = sort_by
        self.sort_order = sort_order

    def run(self) -> None:
        results = search_papers(
            self.query,
            max_results=self.max_results,
            sort_by=self.sort_by,
            sort_order=self.sort_order,
        )
        self.done.emit(results)


class _PdfWorker(QThread):
    done = pyqtSignal(str)

    def __init__(self, paper: arxiv.Result):
        super().__init__()
        self.paper = paper

    def run(self) -> None:
        _PDF_DIR.mkdir(parents=True, exist_ok=True)
        path = download_pdf(self.paper, dirpath=str(_PDF_DIR))
        self.done.emit(path)


class SearchWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("arXiv Search")
        self.resize(1000, 600)
        self._results: list[arxiv.Result] = []
        self._row_widgets: list[_ResultRow] = []
        self._clauses: list[_ClauseRow] = []
        self._pdf_window = PdfWindow()
        self._saved_papers: set[tuple[str, int]] = set()          # (paper_id, version) marked to keep
        self._paper_pdf_paths: dict[tuple[str, int], str] = {}    # (paper_id, version) → local pdf path
        self._current_paper_key: tuple[str, int] | None = None
        self._save_limit_bytes: int = 1 * 1024 ** 3               # 1 GB cap on saved PDFs
        self._build_ui()

    def _build_ui(self) -> None:
        root = QWidget()
        self.setCentralWidget(root)
        layout = QVBoxLayout(root)
        layout.setContentsMargins(12, 12, 12, 12)
        layout.setSpacing(8)

        # Search bar row
        search_row = QHBoxLayout()
        self._search_box = QLineEdit()
        self._search_box.setPlaceholderText("Search arXiv…")
        self._search_box.returnPressed.connect(self._on_search)
        self._adv_btn = QPushButton("Advanced ▾")
        self._adv_btn.setFixedWidth(100)
        self._adv_btn.clicked.connect(self._toggle_advanced)
        self._search_btn = QPushButton("Search")
        self._search_btn.clicked.connect(self._on_search)
        search_row.addWidget(self._search_box)
        search_row.addWidget(self._adv_btn)
        search_row.addWidget(self._search_btn)
        layout.addLayout(search_row)

        # Advanced panel
        self._adv_panel = QFrame()
        self._adv_panel.setFrameShape(QFrame.Shape.StyledPanel)
        adv_outer = QVBoxLayout(self._adv_panel)
        adv_outer.setContentsMargins(8, 6, 8, 6)
        adv_outer.setSpacing(6)

        # Clause rows container
        self._clause_container = QWidget()
        self._clause_layout = QVBoxLayout(self._clause_container)
        self._clause_layout.setContentsMargins(0, 0, 0, 0)
        self._clause_layout.setSpacing(4)
        adv_outer.addWidget(self._clause_container)

        # Add clause button
        add_btn = QPushButton("+ Add clause")
        add_btn.setFixedWidth(110)
        add_btn.clicked.connect(self._add_clause)
        adv_outer.addWidget(add_btn, alignment=Qt.AlignmentFlag.AlignLeft)

        # Preview + insert row
        preview_row = QHBoxLayout()
        self._preview_label = QLabel()
        self._preview_label.setStyleSheet("color: grey; font-family: monospace;")
        self._preview_label.setWordWrap(True)
        preview_row.addWidget(self._preview_label, stretch=1)

        clear_btn = QPushButton("Clear")
        clear_btn.setFixedWidth(60)
        clear_btn.clicked.connect(self._clear_builder)
        preview_row.addWidget(clear_btn)

        insert_btn = QPushButton("Insert Query →")
        insert_btn.clicked.connect(self._insert_query)
        preview_row.addWidget(insert_btn)
        adv_outer.addLayout(preview_row)

        # Divider
        line = QFrame()
        line.setFrameShape(QFrame.Shape.HLine)
        line.setFrameShadow(QFrame.Shadow.Sunken)
        adv_outer.addWidget(line)

        # Sort / order / max row
        opts_row = QHBoxLayout()
        opts_row.setSpacing(12)

        opts_row.addWidget(QLabel("Sort:"))
        self._sort_combo = QComboBox()
        for label, _ in _SORT_BY_OPTIONS:
            self._sort_combo.addItem(label)
        opts_row.addWidget(self._sort_combo)

        opts_row.addWidget(QLabel("Order:"))
        self._order_combo = QComboBox()
        for label, _ in _SORT_ORDER_OPTIONS:
            self._order_combo.addItem(label)
        opts_row.addWidget(self._order_combo)

        opts_row.addWidget(QLabel("Max:"))
        self._max_spin = QSpinBox()
        self._max_spin.setRange(1, 200)
        self._max_spin.setValue(25)
        self._max_spin.setFixedWidth(80)
        opts_row.addWidget(self._max_spin)

        opts_row.addStretch()
        adv_outer.addLayout(opts_row)

        self._adv_panel.setVisible(False)
        layout.addWidget(self._adv_panel)

        # Seed with one blank clause
        self._add_clause()

        # Results area
        outer = QSplitter(Qt.Orientation.Vertical)
        layout.addWidget(outer)

        top = QSplitter(Qt.Orientation.Horizontal)

        left = QWidget()
        left_layout = QVBoxLayout(left)
        left_layout.setContentsMargins(0, 0, 0, 0)
        left_layout.setSpacing(4)
        self._list = _ResultList()
        self._list.currentRowChanged.connect(self._on_select)
        self._status = QLabel("")
        self._status.setStyleSheet("color: grey;")
        left_layout.addWidget(self._list)
        left_layout.addWidget(self._status)
        top.addWidget(left)

        meta = QWidget()
        meta_layout = QVBoxLayout(meta)
        meta_layout.setContentsMargins(8, 0, 0, 0)
        meta_layout.setSpacing(4)

        self._sidebar_title = TexView()
        self._sidebar_title.setFixedHeight(70)
        self._sidebar_meta = TexView()
        self._sidebar_meta.setFixedHeight(40)

        tag_row = QHBoxLayout()
        tag_label = QLabel("Tags:")
        tag_label.setFixedWidth(36)
        self._tag_input = QLineEdit()
        self._tag_input.setPlaceholderText("comma-separated tags…")
        tag_row.addWidget(tag_label)
        tag_row.addWidget(self._tag_input)

        pdf_row = QHBoxLayout()
        self._pdf_btn = QPushButton("View PDF")
        self._pdf_btn.setEnabled(False)
        self._pdf_btn.clicked.connect(self._on_view_pdf)
        pdf_row.addWidget(self._pdf_btn)

        self._save_pdf_btn = QCheckBox("Save PDF")
        self._save_pdf_btn.setEnabled(False)
        self._save_pdf_btn.toggled.connect(self._on_save_pdf_toggled)
        pdf_row.addWidget(self._save_pdf_btn)

        self._link_pdf_btn = QPushButton("Link PDF")
        self._link_pdf_btn.setToolTip("Link an external PDF file to this paper")
        self._link_pdf_btn.setEnabled(False)
        self._link_pdf_btn.clicked.connect(self._on_link_pdf)
        pdf_row.addWidget(self._link_pdf_btn)

        self._linked_indicator = QLabel("")
        self._linked_indicator.setStyleSheet("color: #4caf7d; font-size: 11px;")
        pdf_row.addWidget(self._linked_indicator)

        meta_layout.addWidget(self._sidebar_title)
        meta_layout.addWidget(self._sidebar_meta)
        meta_layout.addLayout(tag_row)
        meta_layout.addLayout(pdf_row)
        meta_layout.addStretch()
        top.addWidget(meta)

        top.setSizes([400, 600])
        outer.addWidget(top)

        self._sidebar_abstract = TexView()
        outer.addWidget(self._sidebar_abstract)
        outer.setSizes([300, 300])

    # --- query builder ---

    def _add_clause(self) -> None:
        show_op = len(self._clauses) > 0
        clause = _ClauseRow(show_operator=show_op)
        clause.changed.connect(self._update_preview)
        clause.remove_requested.connect(self._remove_clause)
        self._clauses.append(clause)
        self._clause_layout.addWidget(clause)
        self._update_preview()

    def _remove_clause(self, clause: _ClauseRow) -> None:
        idx = self._clauses.index(clause)
        self._clauses.pop(idx)
        self._clause_layout.removeWidget(clause)
        clause.deleteLater()
        if self._clauses:
            self._clauses[0].set_operator_visible(False)
        if not self._clauses:
            self._add_clause()
        self._update_preview()

    def _update_preview(self) -> None:
        self._preview_label.setText(self._build_clause_query() or "(empty)")

    def _build_clause_query(self) -> str:
        parts = []
        for i, clause in enumerate(self._clauses):
            part = clause.to_clause()
            if not part:
                continue
            if parts:
                parts.append(clause.operator)
            parts.append(part)
        return " ".join(parts)

    def _clear_builder(self) -> None:
        for clause in list(self._clauses):
            self._clause_layout.removeWidget(clause)
            clause.deleteLater()
        self._clauses.clear()
        self._add_clause()

    def _insert_query(self) -> None:
        q = self._build_clause_query()
        if q:
            self._search_box.setText(q)
        self._adv_panel.setVisible(False)
        self._adv_btn.setText("Advanced ▾")

    def _toggle_advanced(self) -> None:
        visible = not self._adv_panel.isVisible()
        self._adv_panel.setVisible(visible)
        self._adv_btn.setText("Advanced ▴" if visible else "Advanced ▾")

    # --- search ---

    def _on_search(self) -> None:
        query = self._search_box.text().strip()
        if not query:
            return
        sort_by     = _SORT_BY_OPTIONS[self._sort_combo.currentIndex()][1]
        sort_order  = _SORT_ORDER_OPTIONS[self._order_combo.currentIndex()][1]
        max_results = self._max_spin.value()
        self._set_busy(True)
        self._list.clear()
        self._row_widgets = []
        self._results = []
        self._clear_sidebar()
        self._worker = _SearchWorker(query, max_results, sort_by, sort_order)
        self._worker.done.connect(self._on_done)
        self._worker.start()

    def _on_done(self, results: list) -> None:
        self._results = results
        for paper in results:
            row_widget = _ResultRow(paper.title)
            paper_id, _ = parse_entry_id(paper.entry_id)
            row_widget.set_checked(get_paper(paper_id) is not None)
            row_widget._checkbox.stateChanged.connect(
                lambda state, rw=row_widget, p=paper: self._on_checkbox_changed(rw, p, state)
            )
            self._row_widgets.append(row_widget)
            item = QListWidgetItem()
            item.setSizeHint(row_widget.sizeHint())
            self._list.addItem(item)
            self._list.setItemWidget(item, row_widget)
        self._set_busy(False)
        self._status.setText(f"{len(results)} results")

    def _parse_tags(self) -> list[str]:
        raw = self._tag_input.text().strip()
        if not raw:
            return []
        return [t.strip() for t in raw.split(",") if t.strip()]

    def _on_checkbox_changed(self, row_widget: _ResultRow, paper: arxiv.Result, state: int) -> None:
        if state == Qt.CheckState.Checked.value:
            tags = self._parse_tags()
            save_paper(paper, tags=tags if tags else None)
        else:
            paper_id, _ = parse_entry_id(paper.entry_id)
            delete_paper(paper_id)

    def _on_select(self, row: int) -> None:
        if row < 0 or row >= len(self._results):
            self._clear_sidebar()
            return
        paper = self._results[row]
        key = parse_entry_id(paper.entry_id)
        self._current_paper_key = key
        authors = ", ".join(a.name for a in paper.authors[:5])
        if len(paper.authors) > 5:
            authors += f" +{len(paper.authors) - 5} more"
        self._sidebar_title.set_content(paper.title)
        self._sidebar_meta.set_content(
            f"{authors}  ·  {paper.published.strftime('%Y-%m-%d')}  ·  {paper.primary_category}"
        )
        self._sidebar_abstract.set_content(paper.summary)
        self._pdf_btn.setEnabled(True)
        self._link_pdf_btn.setEnabled(True)
        # Show linked indicator if paper has an external pdf_path
        db_row = get_paper(key[0], key[1])
        if db_row and db_row["pdf_path"]:
            self._linked_indicator.setText("Linked")
        else:
            self._linked_indicator.setText("")
        # Auto-check if already saved in session OR PDF exists on disk from a prior session
        already_saved = key in self._saved_papers or os.path.isfile(self._pdf_path_for_key(key))
        if already_saved:
            self._saved_papers.add(key)
        self._save_pdf_btn.setEnabled(True)
        self._save_pdf_btn.blockSignals(True)
        self._save_pdf_btn.setChecked(already_saved)
        self._save_pdf_btn.blockSignals(False)

    def _on_view_pdf(self) -> None:
        row = self._list.currentRow()
        if row < 0 or row >= len(self._results):
            return
        # Capture key now — _current_paper_key may change before download completes
        key = self._current_paper_key
        # Check for linked external PDF first
        if key:
            db_row = get_paper(key[0], key[1])
            if db_row and db_row["pdf_path"] and os.path.isfile(db_row["pdf_path"]):
                self._pdf_window.load_pdf(db_row["pdf_path"], is_external=True)
                return
        self._pdf_btn.setEnabled(False)
        self._pdf_btn.setText("Downloading…")
        self._pdf_worker = _PdfWorker(self._results[row])
        self._pdf_worker.done.connect(lambda path, k=key: self._on_pdf_ready(path, k))
        self._pdf_worker.start()

    def _on_pdf_ready(self, path: str, key: tuple[str, int] | None = None) -> None:
        self._pdf_btn.setEnabled(True)
        self._pdf_btn.setText("View PDF")
        if key:
            self._paper_pdf_paths[key] = path
            print(f"[pdf] downloaded {key} → {path}")

        # If this paper is marked to save, check size limit before displaying
        if key and key in self._saved_papers:
            saved_paths = {
                self._paper_pdf_paths.get(k) or self._pdf_path_for_key(k)
                for k in self._saved_papers
            }
            total = saved_pdfs_size(saved_paths)
            limit_mb = self._save_limit_bytes / 1024 ** 2
            total_mb = total / 1024 ** 2
            print(f"[size] saved total: {total_mb:.1f} MB / {limit_mb:.0f} MB limit")
            if total > self._save_limit_bytes:
                self._saved_papers.discard(key)
                self._save_pdf_btn.blockSignals(True)
                self._save_pdf_btn.setChecked(False)
                self._save_pdf_btn.blockSignals(False)
                self._status.setText(
                    f"Save limit reached ({total_mb:.0f} MB / {limit_mb:.0f} MB) — PDF not saved."
                )
                print(f"[size] limit exceeded — not saving {key}")
                return  # don't open viewer

        self._pdf_window.load_pdf(path)

    def _on_save_pdf_toggled(self, checked: bool) -> None:
        if self._current_paper_key is None:
            return
        if checked:
            self._saved_papers.add(self._current_paper_key)
            print(f"[save] marked {self._current_paper_key} as saved | saved set: {self._saved_papers}")
        else:
            self._saved_papers.discard(self._current_paper_key)
            print(f"[save] unmarked {self._current_paper_key} | saved set: {self._saved_papers}")

    def _on_link_pdf(self) -> None:
        if self._current_paper_key is None:
            return
        paper_id, version = self._current_paper_key
        # Check if the paper is saved in the DB first
        row = get_paper(paper_id, version)
        if row is None:
            self._status.setText("Save the paper first before linking a PDF.")
            return
        path, _ = QFileDialog.getOpenFileName(
            self, "Link PDF to paper", "", "PDF Files (*.pdf)"
        )
        if not path:
            return
        self._set_pdf_path(paper_id, version, path)
        self._linked_indicator.setText("Linked")
        self._status.setText(f"Linked PDF: {os.path.basename(path)}")

    @staticmethod
    def _set_pdf_path(paper_id: str, version: int, path: str) -> None:
        """Write pdf_path for a paper directly (no backend function available yet)."""
        import sqlite3
        from db import DB_PATH
        conn = sqlite3.connect(DB_PATH)
        try:
            conn.execute(
                "UPDATE papers SET pdf_path = ? WHERE paper_id = ? AND version = ?",
                (path, paper_id, version),
            )
            conn.commit()
        finally:
            conn.close()

    def _pdf_path_for_key(self, key: tuple[str, int]) -> str:
        """Reconstruct the expected PDF path from a (paper_id, version) key."""
        paper_id, version = key
        return str(_PDF_DIR / f"{paper_id}v{version}.pdf")

    def cleanup_pdfs(self) -> list[str]:
        """Delete all unsaved PDFs. Always runs — no size condition for deletion."""
        self._pdf_window._doc.close()  # release Windows file lock before deleting
        from PyQt6.QtWidgets import QApplication
        QApplication.processEvents()   # flush handle release (required on Windows)
        if not _PDF_DIR.is_dir():
            return []
        keep = {
            self._paper_pdf_paths.get(key) or self._pdf_path_for_key(key)
            for key in self._saved_papers
        }
        deleted = _cleanup_pdfs(str(_PDF_DIR), keep=keep)

        # Update has_pdf flag in DB
        for key in self._saved_papers:
            path = self._paper_pdf_paths.get(key) or self._pdf_path_for_key(key)
            set_has_pdf(key[0], key[1], os.path.isfile(path))
        for path in deleted:
            fname = os.path.splitext(os.path.basename(path))[0]  # e.g. '2204.12985v4'
            key = parse_entry_id(fname)
            set_has_pdf(key[0], key[1], False)

        print(f"[cleanup] kept: {self._saved_papers} | deleted {len(deleted)} file(s): {deleted}")
        return deleted

    def closeEvent(self, event) -> None:
        self.cleanup_pdfs()
        super().closeEvent(event)

    def _clear_sidebar(self) -> None:
        self._sidebar_title.set_content("")
        self._sidebar_meta.set_content("")
        self._sidebar_abstract.set_content("")
        self._pdf_btn.setEnabled(False)
        self._save_pdf_btn.setEnabled(False)
        self._save_pdf_btn.blockSignals(True)
        self._save_pdf_btn.setChecked(False)
        self._save_pdf_btn.blockSignals(False)
        self._link_pdf_btn.setEnabled(False)
        self._linked_indicator.setText("")
        self._current_paper_key = None

    def _set_busy(self, busy: bool) -> None:
        self._search_btn.setEnabled(not busy)
        self._search_box.setEnabled(not busy)
        self._status.setText("Fetching…" if busy else "")
