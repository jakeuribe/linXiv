from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout,
    QLineEdit, QPushButton, QListWidgetItem,
    QLabel, QSplitter, QCheckBox, QComboBox,
)
from PyQt6.QtCore import Qt
import arxiv
from storage.db import (
    save_paper, save_paper_metadata, delete_paper,
    get_paper, parse_entry_id,
    search_full_text,
)
from sources.base import PaperMetadata
from gui.views import TexView, PdfWindow
from gui.theme import FONT_TERTIARY, SPACE_XS, SPACE_SM, SPACE_MD
from gui.search._workers import _SearchWorker, _SourceSearchWorker
from gui.search._widgets import _ResultList, _ResultRow
from gui.search._query_builder import _QueryBuilderPanel
from gui.search._pdf_controller import _PdfController

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
        self._current_paper_key: tuple[str, int] | None = None

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
        self._adv_btn.clicked.connect(lambda: self._query_panel.toggle())
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

        self._query_panel = _QueryBuilderPanel()
        self._query_panel.query_inserted.connect(self._search_box.setText)
        self._query_panel.toggled.connect(
            lambda v: self._adv_btn.setText("Advanced ▴" if v else "Advanced ▾")
        )
        layout.addWidget(self._query_panel)

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
        pdf_row.addWidget(self._pdf_btn)

        self._save_pdf_btn = QCheckBox("Save PDF")
        self._save_pdf_btn.setEnabled(False)
        pdf_row.addWidget(self._save_pdf_btn)

        self._link_pdf_btn = QPushButton("Link PDF")
        self._link_pdf_btn.setToolTip("Link an external PDF file to this paper")
        self._link_pdf_btn.setEnabled(False)
        pdf_row.addWidget(self._link_pdf_btn)

        self._linked_indicator = QLabel("")
        self._linked_indicator.setStyleSheet(f"color: #444444; font-size: {FONT_TERTIARY}px;")
        pdf_row.addWidget(self._linked_indicator)

        self._pdf = _PdfController(
            self._pdf_btn, self._save_pdf_btn, self._link_pdf_btn,
            self._linked_indicator, self._status, PdfWindow(),
        )
        self._pdf_btn.clicked.connect(
            lambda: self._pdf.on_view_pdf(
                self._current_paper_key, self._results, self._list.currentRow()
            )
        )
        self._save_pdf_btn.toggled.connect(
            lambda c: self._pdf.on_save_pdf_toggled(c, self._current_paper_key)
        )
        self._link_pdf_btn.clicked.connect(
            lambda: self._pdf.on_link_pdf(self._current_paper_key, self)
        )

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

    # --- source selection ---

    def _on_source_changed(self, index: int) -> None:
        self._active_source = _SOURCE_OPTIONS[index][1]
        is_arxiv = self._active_source == "arxiv"
        # Advanced query builder and sort options are arXiv-specific
        self._adv_btn.setVisible(is_arxiv)
        if not is_arxiv:
            self._query_panel.setVisible(False)
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
        query = self._search_box.text().strip()
        if not query:
            return
        max_results = self._query_panel.max_results()
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
            sort_by    = self._query_panel.sort_by()
            sort_order = self._query_panel.sort_order()
            self._worker = _SearchWorker(query, max_results, sort_by, sort_order)
            self._worker.done.connect(self._on_done)
            self._worker.start()
        else:
            self._source_worker = _SourceSearchWorker(
                self._active_source, query, max_results
            )
            self._source_worker.done.connect(self._on_source_done)
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
        self._linked_indicator.setText("Linked" if db_row and db_row["pdf_path"] else "")
        self._pdf.sync_save_state(key)

    def cleanup_pdfs(self) -> list[str]:
        return self._pdf.cleanup_pdfs()

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
