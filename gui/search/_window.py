import os

from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout,
    QLineEdit, QPushButton, QListWidgetItem,
    QLabel, QSplitter, QCheckBox, QComboBox, QSpinBox,
    QFrame, QFileDialog,
)
from PyQt6.QtCore import Qt
import arxiv
from storage.db import (
    save_paper, save_paper_metadata, delete_paper,
    get_paper, set_has_pdf, set_pdf_path, parse_entry_id,
    search_full_text,
)
from sources.base import PaperMetadata
from sources.arxiv_downloads import cleanup_pdfs as _cleanup_pdfs, saved_pdfs_size
from gui.views import TexView, PdfWindow
from gui.theme import FONT_TERTIARY, SPACE_XS, SPACE_SM, SPACE_MD
from gui.search._workers import _SearchWorker, _SourceSearchWorker, _PdfWorker, _PDF_DIR
from gui.search._widgets import _ClauseRow, _ResultList, _ResultRow

_SORT_BY_OPTIONS = [
    ("Relevance",     arxiv.SortCriterion.Relevance),
    ("Submitted Date",arxiv.SortCriterion.SubmittedDate),
    ("Last Updated",  arxiv.SortCriterion.LastUpdatedDate),
]

_SORT_ORDER_OPTIONS = [
    ("Descending", arxiv.SortOrder.Descending),
    ("Ascending",  arxiv.SortOrder.Ascending),
]

_SOURCE_OPTIONS = [
    ("arXiv", "arxiv"),
    ("OpenAlex", "openalex"),
    ("Local source", "local"),
]


class SearchPage(QWidget):
    def __init__(self):
        super().__init__()
        self.setStyleSheet("""
            background: #ffffff; color: #111111;
            QLineEdit, QComboBox, QSpinBox {
                border: 1px solid #cccccc;
                border-radius: 4px;
                padding: 2px 4px;
                background: #ffffff;
                color: #111111;
            }
            QListWidget {
                border: 1px solid #cccccc;
                background: #ffffff;
                color: #111111;
            }
            QListWidget::item:selected {
                background: #5b8dee;
                color: #ffffff;
            }
            QListWidget::item:hover {
                background: #eef2fd;
            }
            QPushButton {
                background: #f0f0f0;
                color: #111111;
                border: 1px solid #cccccc;
                border-radius: 4px;
                padding: 2px 8px;
            }
            QPushButton:hover { background: #e0e0e0; }
            QPushButton:disabled { color: #999999; }
            QCheckBox { color: #111111; }
            QLabel { color: #111111; }
            QFrame[frameShape="1"] { border: 1px solid #cccccc; }
        """)
        self._results: list[arxiv.Result] = []
        self._meta_results: list[PaperMetadata] = []  # unified results from any source
        self._local_results: list[dict] = []  # results from local FTS
        self._active_source: str = "arxiv"
        self._row_widgets: list[_ResultRow] = []
        self._clauses: list[_ClauseRow] = []
        self._pdf_window = PdfWindow()
        self._saved_papers: set[tuple[str, int]] = set()          # (paper_id, version) marked to keep
        self._paper_pdf_paths: dict[tuple[str, int], str] = {}    # (paper_id, version) → local pdf path
        self._current_paper_key: tuple[str, int] | None = None
        # TODO: Make configurable in user specific settings
        self._save_limit_bytes: int = 1 * 1024 ** 3               # 1 GB cap on saved PDFs

        self._build_ui()

    def _build_ui(self) -> None:
        layout = QVBoxLayout(self)
        layout.setContentsMargins(SPACE_MD, SPACE_MD, SPACE_MD, SPACE_MD)
        layout.setSpacing(SPACE_SM)

        # Search bar row
        search_row = QHBoxLayout()
        self._source_combo = QComboBox()
        for label, _ in _SOURCE_OPTIONS:
            self._source_combo.addItem(label)
        self._source_combo.setFixedWidth(100)  # TODO: Make more customizable
        self._source_combo.currentIndexChanged.connect(self._on_source_changed)
        search_row.addWidget(self._source_combo)
        self._search_box = QLineEdit()
        self._search_box.setPlaceholderText("Search arXiv…")
        self._search_box.returnPressed.connect(self._on_search)
        self._adv_btn = QPushButton("Advanced ▾")
        self._adv_btn.setFixedWidth(100)  # TODO: Make more customizable
        self._adv_btn.clicked.connect(self._toggle_advanced)
        self._search_btn = QPushButton("Search")
        self._search_btn.clicked.connect(self._on_search)
        self._cleanup_btn = QPushButton("Clean up PDFs")
        self._cleanup_btn.setToolTip("Delete unsaved PDFs and update the database")
        self._cleanup_btn.clicked.connect(self._on_cleanup_pdfs)
        search_row.addWidget(self._search_box)
        search_row.addWidget(self._adv_btn)
        search_row.addWidget(self._search_btn)
        search_row.addWidget(self._cleanup_btn)
        layout.addLayout(search_row)

        # Advanced panel
        self._adv_panel = QFrame()
        self._adv_panel.setFrameShape(QFrame.Shape.StyledPanel)
        adv_outer = QVBoxLayout(self._adv_panel)
        adv_outer.setContentsMargins(SPACE_SM, SPACE_XS, SPACE_SM, SPACE_XS)
        adv_outer.setSpacing(SPACE_SM)

        # Clause rows container
        self._clause_container = QWidget()
        self._clause_layout = QVBoxLayout(self._clause_container)
        self._clause_layout.setContentsMargins(0, 0, 0, 0)
        self._clause_layout.setSpacing(SPACE_XS)
        adv_outer.addWidget(self._clause_container)

        # Add clause button
        add_btn = QPushButton("+ Add clause")
        add_btn.setFixedWidth(110)  # TODO: Make more customizable
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
        opts_row.setSpacing(SPACE_MD)

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
        self._max_spin.setFixedWidth(80)  # TODO: Make more customizable
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
        left_layout.setSpacing(SPACE_XS)
        self._list = _ResultList()
        self._list.setStyleSheet("""
            QListWidget { background: #ffffff; border: 1px solid #cccccc; }
            QListWidget::item:selected { background: #5b8dee; color: #ffffff; }
            QListWidget::item:hover { background: #eef2fd; color: #111111; }
        """)
        self._list.currentRowChanged.connect(self._on_select)
        self._status = QLabel("")
        self._status.setStyleSheet("color: grey;")
        left_layout.addWidget(self._list)
        left_layout.addWidget(self._status)
        top.addWidget(left)

        meta = QWidget()
        meta_layout = QVBoxLayout(meta)
        meta_layout.setContentsMargins(SPACE_SM, 0, 0, 0)
        meta_layout.setSpacing(SPACE_XS)

        self._sidebar_title = TexView(color="#111111", bg="#ffffff")
        self._sidebar_title.setFixedHeight(70)  # TODO: Make more customizable
        self._sidebar_meta = TexView(color="#111111", bg="#ffffff")
        self._sidebar_meta.setFixedHeight(40)  # TODO: Make more customizable

        tag_row = QHBoxLayout()
        tag_label = QLabel("Tags:")
        tag_label.setFixedWidth(36)  # TODO: Make more customizable
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
        self._linked_indicator.setStyleSheet(f"color: #444444; font-size: {FONT_TERTIARY}px;")
        pdf_row.addWidget(self._linked_indicator)

        meta_layout.addWidget(self._sidebar_title)
        meta_layout.addWidget(self._sidebar_meta)
        meta_layout.addLayout(tag_row)
        meta_layout.addLayout(pdf_row)
        meta_layout.addStretch()
        top.addWidget(meta)

        top.setSizes([400, 600])  # TODO: Make more customizable
        outer.addWidget(top)

        self._sidebar_abstract = TexView(color="#111111", bg="#ffffff")
        outer.addWidget(self._sidebar_abstract)
        outer.setSizes([300, 300])  # TODO: Make more customizable

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
        for _, clause in enumerate(self._clauses):
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

    # --- source selection ---

    def _on_source_changed(self, index: int) -> None:
        self._active_source = _SOURCE_OPTIONS[index][1]
        is_arxiv = self._active_source == "arxiv"
        # Advanced query builder and sort options are arXiv-specific
        self._adv_btn.setVisible(is_arxiv)
        if not is_arxiv:
            self._adv_panel.setVisible(False)
        placeholders = {
            "arxiv": "Search arXiv…",
            "openalex": "Search OpenAlex…",
            "local": "Search downloaded TeX sources…",
        }
        self._search_box.setPlaceholderText(
            placeholders.get(self._active_source, "Search…")
        )

    # --- search ---

    def _on_search(self) -> None:
        if not self._search_btn.isEnabled():
            return
        query = self._search_box.text().strip()
        if not query:
            return
        max_results = self._max_spin.value()
        self._set_busy(True)
        self._list.clear()
        self._row_widgets = []
        self._results = []
        self._meta_results = []
        self._local_results = []
        self._clear_sidebar()
        if self._active_source == "local":
            self._on_local_search(query, max_results)
            return
        if self._active_source == "arxiv":
            sort_by     = _SORT_BY_OPTIONS[self._sort_combo.currentIndex()][1]
            sort_order  = _SORT_ORDER_OPTIONS[self._order_combo.currentIndex()][1]
            self._worker = _SearchWorker(query, max_results, sort_by, sort_order)
            self._worker.done.connect(self._on_done)
            self._worker.error.connect(self._on_search_error)
            self._worker.start()
        else:
            self._source_worker = _SourceSearchWorker(
                self._active_source, query, max_results
            )
            self._source_worker.done.connect(self._on_source_done)
            self._source_worker.error.connect(self._on_search_error)
            self._source_worker.start()

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

    def _on_source_done(self, results: list) -> None:
        self._meta_results = results
        for paper in results:
            row_widget = _ResultRow(paper.title, source=paper.source)
            row_widget.set_checked(get_paper(paper.paper_id) is not None)
            row_widget._checkbox.stateChanged.connect(
                lambda state, rw=row_widget, p=paper: self._on_meta_checkbox_changed(rw, p, state)
            )
            self._row_widgets.append(row_widget)
            item = QListWidgetItem()
            item.setSizeHint(row_widget.sizeHint())
            self._list.addItem(item)
            self._list.setItemWidget(item, row_widget)
        self._set_busy(False)
        self._status.setText(f"{len(results)} results from {self._active_source}")

    def _on_search_error(self, msg: str) -> None:
        self._set_busy(False)
        self._status.setText(f"Search failed: {msg}")

    def _on_pdf_error(self, msg: str) -> None:
        self._pdf_btn.setEnabled(True)
        self._pdf_btn.setText("View PDF")
        self._status.setText(f"PDF download failed: {msg}")

    def _on_local_search(self, query: str, limit: int) -> None:
        try:
            rows = search_full_text(query, limit=limit)
        except Exception as exc:
            self._set_busy(False)
            self._status.setText(f"FTS error: {exc}")
            return
        self._local_results = [dict(r) for r in rows]
        for paper in self._local_results:
            title = paper.get("title") or "(untitled)"
            row_widget = _ResultRow(title, source="local")
            # Already saved in DB — pre-check and disable
            row_widget.set_checked(True)
            row_widget._checkbox.setEnabled(False)
            self._row_widgets.append(row_widget)
            item = QListWidgetItem()
            item.setSizeHint(row_widget.sizeHint())
            self._list.addItem(item)
            self._list.setItemWidget(item, row_widget)
        self._set_busy(False)
        self._status.setText(f"{len(self._local_results)} results from local source")

    def _on_meta_checkbox_changed(
        self, _row_widget: _ResultRow, paper: PaperMetadata, state: int
    ) -> None:
        if state == Qt.CheckState.Checked.value:
            tags = self._parse_tags()
            save_paper_metadata(paper, tags=tags if tags else None)
        else:
            delete_paper(paper.paper_id)

    def _parse_tags(self) -> list[str]:
        raw = self._tag_input.text().strip()
        if not raw:
            return []
        return [t.strip() for t in raw.split(",") if t.strip()]

    def _on_checkbox_changed(self, _row_widget: _ResultRow, paper: arxiv.Result, state: int) -> None:
        if state == Qt.CheckState.Checked.value:
            tags = self._parse_tags()
            save_paper(paper, tags=tags if tags else None)
        else:
            paper_id, _ = parse_entry_id(paper.entry_id)
            delete_paper(paper_id)

    def _on_select(self, row: int) -> None:
        # Determine which result list is active
        if self._local_results:
            if row < 0 or row >= len(self._local_results):
                self._clear_sidebar()
                return
            paper_dict = self._local_results[row]
            key = (paper_dict["paper_id"], paper_dict["version"])
            self._current_paper_key = key
            authors_raw = paper_dict.get("authors") or []
            if isinstance(authors_raw, str):
                authors_raw = [authors_raw]
            authors = ", ".join(authors_raw[:5])
            if len(authors_raw) > 5:
                authors += f" +{len(authors_raw) - 5} more"
            cat = paper_dict.get("category") or ""
            pub = paper_dict.get("published")
            pub_str = pub.isoformat() if pub else ""
            self._sidebar_title.set_content(paper_dict.get("title") or "")
            self._sidebar_meta.set_content(
                f"[local]  {authors}  ·  {pub_str}  ·  {cat}"
            )
            self._sidebar_abstract.set_content(paper_dict.get("summary") or "")
            has_pdf = paper_dict.get("has_pdf", False)
            self._pdf_btn.setEnabled(bool(has_pdf))
            self._save_pdf_btn.setEnabled(False)
            self._link_pdf_btn.setEnabled(True)
        elif self._meta_results:
            if row < 0 or row >= len(self._meta_results):
                self._clear_sidebar()
                return
            paper = self._meta_results[row]
            key = (paper.paper_id, paper.version)
            self._current_paper_key = key
            authors = ", ".join(paper.authors[:5])
            if len(paper.authors) > 5:
                authors += f" +{len(paper.authors) - 5} more"
            cat = paper.category or ""
            source_tag = f"[{paper.source}]  " if paper.source != "arxiv" else ""
            self._sidebar_title.set_content(paper.title)
            self._sidebar_meta.set_content(
                f"{source_tag}{authors}  ·  {paper.published.isoformat()}  ·  {cat}"
            )
            self._sidebar_abstract.set_content(paper.summary)
            # PDF download only available for arXiv results
            self._pdf_btn.setEnabled(paper.source == "arxiv")
            self._save_pdf_btn.setEnabled(paper.source == "arxiv")
            self._link_pdf_btn.setEnabled(True)
        elif self._results:
            if row < 0 or row >= len(self._results):
                self._clear_sidebar()
                return
            paper_arxiv = self._results[row]
            key = parse_entry_id(paper_arxiv.entry_id)
            self._current_paper_key = key
            authors = ", ".join(a.name for a in paper_arxiv.authors[:5])
            if len(paper_arxiv.authors) > 5:
                authors += f" +{len(paper_arxiv.authors) - 5} more"
            self._sidebar_title.set_content(paper_arxiv.title)
            self._sidebar_meta.set_content(
                f"{authors}  ·  {paper_arxiv.published.strftime('%Y-%m-%d')}  ·  {paper_arxiv.primary_category}"
            )
            self._sidebar_abstract.set_content(paper_arxiv.summary)
            self._pdf_btn.setEnabled(True)
            self._save_pdf_btn.setEnabled(True)
            self._link_pdf_btn.setEnabled(True)
        else:
            self._clear_sidebar()
            return

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
        self._save_pdf_btn.blockSignals(True)
        self._save_pdf_btn.setChecked(already_saved)
        self._save_pdf_btn.blockSignals(False)

    def _on_view_pdf(self) -> None:
        row = self._list.currentRow()
        # Capture key now — _current_paper_key may change before download completes
        key = self._current_paper_key
        # Check for linked external PDF first
        if key:
            db_row = get_paper(key[0], key[1])
            if db_row and db_row["pdf_path"] and os.path.isfile(db_row["pdf_path"]):
                self._pdf_window.load_pdf(db_row["pdf_path"], is_external=True)
                return
        # Only arXiv results support direct PDF download
        if row < 0 or row >= len(self._results):
            return
        self._pdf_btn.setEnabled(False)
        self._pdf_btn.setText("Downloading…")
        self._pdf_worker = _PdfWorker(self._results[row])
        self._pdf_worker.done.connect(lambda path, k=key: self._on_pdf_ready(path, k))
        self._pdf_worker.error.connect(self._on_pdf_error)
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
        set_pdf_path(paper_id, path)
        self._linked_indicator.setText("Linked")
        self._status.setText(f"Linked PDF: {os.path.basename(path)}")

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
        # Also keep any PDF that's already recorded in the DB (e.g. downloaded via Library page)
        from storage.db import list_papers as _list_papers
        for row in _list_papers():
            pdf_path = row["pdf_path"] if "pdf_path" in row.keys() else None
            if pdf_path and os.path.isfile(pdf_path):
                keep.add(pdf_path)
            if row["has_pdf"]:
                keep.add(self._pdf_path_for_key((row["paper_id"], row["version"])))
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

    def _on_cleanup_pdfs(self) -> None:
        deleted = self.cleanup_pdfs()
        self._status.setText(f"Cleaned up {len(deleted)} PDF(s).")

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
