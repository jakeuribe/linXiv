from __future__ import annotations

import datetime

from PyQt6.QtCore import Qt, QTimer, QStringListModel
from PyQt6.QtGui import QAction
from PyQt6.QtWidgets import (
    QCheckBox,
    QCompleter,
    QDockWidget,
    QGroupBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QMainWindow,
    QPushButton,
    QSizePolicy,
    QSplitter,
    QTableWidget,
    QTableWidgetItem,
    QToolBar,
    QVBoxLayout,
    QWidget,
)

from .graph_view import GraphView
from db import get_categories, get_graph_data, get_tags, list_papers


# ── Helpers ──────────────────────────────────────────────────────────────────

def _fmt_date(val) -> str:
    if val is None:
        return ""
    if isinstance(val, (datetime.date, datetime.datetime)):
        return val.isoformat()
    return str(val)


def _fmt_tags(val) -> str:
    if not val:
        return ""
    if isinstance(val, list):
        return ", ".join(val)
    return str(val)


# ── Collapsible section ───────────────────────────────────────────────────────

class _CollapsibleSection(QWidget):
    """A titled section with a toggle button that shows/hides its body."""

    def __init__(self, title: str, collapsed: bool = False, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._title = title
        self._collapsed = collapsed

        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 2, 0, 2)
        outer.setSpacing(0)

        self._toggle_btn = QPushButton()
        self._toggle_btn.setFlat(True)
        self._toggle_btn.setStyleSheet(
            "QPushButton { text-align: left; font-weight: bold; padding: 4px 4px; }"
            "QPushButton:hover { background: rgba(128,128,128,0.15); border-radius: 3px; }"
        )
        self._toggle_btn.clicked.connect(self._toggle)
        outer.addWidget(self._toggle_btn)

        self._body = QWidget()
        self._body_layout = QVBoxLayout(self._body)
        self._body_layout.setContentsMargins(10, 2, 0, 6)
        self._body_layout.setSpacing(4)
        outer.addWidget(self._body)

        self._apply_state()

    def _apply_state(self) -> None:
        arrow = "▶" if self._collapsed else "▼"
        self._toggle_btn.setText(f"{arrow}  {self._title}")
        self._body.setVisible(not self._collapsed)

    def _toggle(self) -> None:
        self._collapsed = not self._collapsed
        self._apply_state()

    def add_widget(self, w: QWidget) -> QWidget:
        self._body_layout.addWidget(w)
        return w

    def add_layout(self, layout) -> None:
        self._body_layout.addLayout(layout)


# ── Sidebar (filter controls) ─────────────────────────────────────────────────

class FilterSidebar(QWidget):
    """Left-hand filter panel.  Callers connect to the public widget attributes."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setMinimumWidth(200)

        root = QVBoxLayout(self)
        root.setContentsMargins(8, 8, 8, 8)
        root.setSpacing(4)

        # ── Node visibility ──────────────────────────────────────────────────
        vis_sec = _CollapsibleSection("Node visibility")
        self.show_papers = QCheckBox("Show papers")
        self.show_papers.setChecked(True)
        self.show_authors = QCheckBox("Show authors")
        self.show_authors.setChecked(True)
        vis_sec.add_widget(self.show_papers)
        vis_sec.add_widget(self.show_authors)
        root.addWidget(vis_sec)

        # ── Attribute filters ────────────────────────────────────────────────
        attr_sec = _CollapsibleSection("Attribute filters")

        attr_sec.add_widget(QLabel("Category:"))
        self.category_edit = QLineEdit()
        self.category_edit.setPlaceholderText("type to filter…")
        self._cat_completer = QCompleter([], self)
        self._cat_completer.setCaseSensitivity(Qt.CaseSensitivity.CaseInsensitive)
        self._cat_completer.setFilterMode(Qt.MatchFlag.MatchContains)
        self.category_edit.setCompleter(self._cat_completer)
        attr_sec.add_widget(self.category_edit)

        attr_sec.add_widget(QLabel("Tag:"))
        self.tag_edit = QLineEdit()
        self.tag_edit.setPlaceholderText("type to filter…")
        self._tag_completer = QCompleter([], self)
        self._tag_completer.setCaseSensitivity(Qt.CaseSensitivity.CaseInsensitive)
        self._tag_completer.setFilterMode(Qt.MatchFlag.MatchContains)
        self.tag_edit.setCompleter(self._tag_completer)
        attr_sec.add_widget(self.tag_edit)

        self.has_pdf_check = QCheckBox("Has PDF only")
        attr_sec.add_widget(self.has_pdf_check)
        root.addWidget(attr_sec)

        # ── Date range ───────────────────────────────────────────────────────
        date_sec = _CollapsibleSection("Date range", collapsed=True)

        from_row = QHBoxLayout()
        from_row.addWidget(QLabel("From:"))
        self.date_from_edit = QLineEdit()
        self.date_from_edit.setPlaceholderText("YYYY-MM-DD")
        from_row.addWidget(self.date_from_edit)
        date_sec.add_layout(from_row)

        to_row = QHBoxLayout()
        to_row.addWidget(QLabel("To:  "))
        self.date_to_edit = QLineEdit()
        self.date_to_edit.setPlaceholderText("YYYY-MM-DD")
        to_row.addWidget(self.date_to_edit)
        date_sec.add_layout(to_row)
        root.addWidget(date_sec)

        # ── Highlight / search ───────────────────────────────────────────────
        hl_sec = _CollapsibleSection("Highlight / search")

        hl_sec.add_widget(QLabel("Title contains:"))
        self.highlight_edit = QLineEdit()
        self.highlight_edit.setPlaceholderText("e.g. transformer")
        hl_sec.add_widget(self.highlight_edit)

        hl_sec.add_widget(QLabel("Author contains:"))
        self.author_edit = QLineEdit()
        self.author_edit.setPlaceholderText("e.g. Hinton")
        hl_sec.add_widget(self.author_edit)

        self.isolate_btn = QPushButton("Show highlighted only")
        self.isolate_btn.setCheckable(True)
        self.isolate_btn.setStyleSheet(
            "QPushButton { padding: 4px; }"
            "QPushButton:checked { background-color: #5b8dee; color: white; border-radius: 3px; }"
        )
        hl_sec.add_widget(self.isolate_btn)
        root.addWidget(hl_sec)

        root.addStretch()

    def populate_dropdowns(self, categories: list[str], tags: list[str]) -> None:
        self._cat_completer.setModel(QStringListModel(categories, self))
        self._tag_completer.setModel(QStringListModel(tags, self))


# ── Paper list panel ──────────────────────────────────────────────────────────

_COLUMNS = ["Title", "Category", "Published", "Tags", "PDF"]
_COL_WIDTHS = [320, 80, 90, 180, 40]


class PaperListPanel(QWidget):
    """Bottom/right panel showing saved papers as a table."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)

        self._table = QTableWidget(0, len(_COLUMNS))
        self._table.setHorizontalHeaderLabels(_COLUMNS)
        self._table.setSelectionBehavior(QTableWidget.SelectionBehavior.SelectRows)
        self._table.setEditTriggers(QTableWidget.EditTrigger.NoEditTriggers)
        self._table.setAlternatingRowColors(True)
        self._table.verticalHeader().setVisible(False)
        self._table.setSortingEnabled(True)

        hdr = self._table.horizontalHeader()
        for i, w in enumerate(_COL_WIDTHS):
            self._table.setColumnWidth(i, w)
        hdr.setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)

        layout.addWidget(self._table)

    @property
    def table(self) -> QTableWidget:
        return self._table

    def paper_id_for_row(self, row: int) -> str | None:
        item = self._table.item(row, 0)
        if item is None:
            return None
        return item.data(Qt.ItemDataRole.UserRole)

    def load_papers(self, rows) -> None:
        self._table.setSortingEnabled(False)
        self._table.setRowCount(0)

        for row in rows:
            r = self._table.rowCount()
            self._table.insertRow(r)

            title_item = QTableWidgetItem(row["title"] or "")
            title_item.setData(Qt.ItemDataRole.UserRole, row["paper_id"])
            self._table.setItem(r, 0, title_item)
            self._table.setItem(r, 1, QTableWidgetItem(row["category"] or ""))
            self._table.setItem(r, 2, QTableWidgetItem(_fmt_date(row["published"])))
            self._table.setItem(r, 3, QTableWidgetItem(_fmt_tags(row["tags"])))
            pdf_item = QTableWidgetItem("Y" if row["has_pdf"] else "")
            pdf_item.setTextAlignment(Qt.AlignmentFlag.AlignCenter)
            self._table.setItem(r, 4, pdf_item)

        self._table.setSortingEnabled(True)


# ── Main Window ───────────────────────────────────────────────────────────────

class MainWindow(QMainWindow):
    def __init__(self) -> None:
        super().__init__()
        self.setWindowTitle("linXiv — arXiv Paper Graph")
        self.resize(1500, 950)

        self._build_toolbar()
        self._build_central()

        # Debounce timer for text inputs
        self._filter_timer = QTimer(self)
        self._filter_timer.setSingleShot(True)
        self._filter_timer.setInterval(300)
        self._filter_timer.timeout.connect(self._apply_js_filter)

        self._load_all()

    # ── Construction ─────────────────────────────────────────────────────────

    def _build_toolbar(self) -> None:
        tb = QToolBar("Main toolbar", self)
        tb.setMovable(False)
        self.addToolBar(tb)

        refresh_action = QAction("Refresh", self)
        refresh_action.setToolTip("Reload graph data from database")
        refresh_action.triggered.connect(self.refresh)
        tb.addAction(refresh_action)

        clear_hl_action = QAction("Clear filters", self)
        clear_hl_action.setToolTip("Clear all filters and reset opacities")
        clear_hl_action.triggered.connect(self._clear_highlight)
        tb.addAction(clear_hl_action)

        self._show_filters_action = QAction("Show filters", self)
        self._show_filters_action.setToolTip("Show the filter panel")
        self._show_filters_action.triggered.connect(self._show_dock)
        self._show_filters_action.setVisible(False)
        tb.addAction(self._show_filters_action)

    def _build_central(self) -> None:
        # Central widget: graph (top) + paper list (bottom)
        right_split = QSplitter(Qt.Orientation.Vertical)
        right_split.setChildrenCollapsible(False)

        self._graph_view = GraphView()
        right_split.addWidget(self._graph_view)

        self._paper_list = PaperListPanel()
        right_split.addWidget(self._paper_list)

        right_split.setStretchFactor(0, 3)
        right_split.setStretchFactor(1, 1)

        self.setCentralWidget(right_split)

        # Filter sidebar as a dock widget (movable / floatable)
        self._sidebar = FilterSidebar()
        self._dock = QDockWidget("Filters", self)
        self._dock.setWidget(self._sidebar)
        self._dock.setFeatures(
            QDockWidget.DockWidgetFeature.DockWidgetMovable
            | QDockWidget.DockWidgetFeature.DockWidgetFloatable
            | QDockWidget.DockWidgetFeature.DockWidgetClosable
        )
        self._dock.visibilityChanged.connect(self._on_dock_visibility)
        self.addDockWidget(Qt.DockWidgetArea.LeftDockWidgetArea, self._dock)

        # ── Wire signals ──────────────────────────────────────────────────
        self._sidebar.show_papers.stateChanged.connect(self._on_structural_change)
        self._sidebar.show_authors.stateChanged.connect(self._on_structural_change)
        self._sidebar.category_edit.textChanged.connect(self._schedule_filter)
        self._sidebar.tag_edit.textChanged.connect(self._schedule_filter)
        self._sidebar.has_pdf_check.stateChanged.connect(self._apply_js_filter)
        self._sidebar.highlight_edit.textChanged.connect(self._schedule_filter)
        self._sidebar.author_edit.textChanged.connect(self._schedule_filter)
        self._sidebar.date_from_edit.textChanged.connect(self._schedule_filter)
        self._sidebar.date_to_edit.textChanged.connect(self._schedule_filter)
        self._sidebar.isolate_btn.toggled.connect(self._apply_js_filter)

        self._paper_list.table.currentCellChanged.connect(self._on_paper_selected)

    # ── Data loading ──────────────────────────────────────────────────────────

    def _load_all(self) -> None:
        self._load_graph()
        self._load_paper_list()
        self._load_dropdowns()

    def _load_graph(self) -> None:
        nodes, edges = get_graph_data()
        self._graph_view.set_graph_data(nodes, edges)

    def _load_paper_list(self) -> None:
        papers = list_papers(latest_only=True)
        self._paper_list.load_papers(papers)

    def _load_dropdowns(self) -> None:
        categories = get_categories()
        tags = get_tags()
        self._sidebar.populate_dropdowns(categories, tags)

    # ── Refresh ───────────────────────────────────────────────────────────────

    def refresh(self) -> None:
        self._load_all()

    # ── Dock visibility ───────────────────────────────────────────────────────

    def _on_dock_visibility(self, visible: bool) -> None:
        self._show_filters_action.setVisible(not visible)

    def _show_dock(self) -> None:
        self._dock.show()

    # ── Filter logic ──────────────────────────────────────────────────────────

    def _schedule_filter(self) -> None:
        self._filter_timer.start()

    def _on_structural_change(self) -> None:
        """Show/hide authors/papers is structural — reload graph data filtered."""
        show_authors = self._sidebar.show_authors.isChecked()
        show_papers  = self._sidebar.show_papers.isChecked()

        if not show_authors or not show_papers:
            nodes, edges = get_graph_data()
            if not show_authors:
                author_ids = {n["id"] for n in nodes if n["type"] == "author"}
                nodes = [n for n in nodes if n["type"] != "author"]
                edges = [e for e in edges
                         if e["source"] not in author_ids and e["target"] not in author_ids]
            if not show_papers:
                paper_ids = {n["id"] for n in nodes if n["type"] == "paper"}
                nodes = [n for n in nodes if n["type"] != "paper"]
                edges = [e for e in edges
                         if e["source"] not in paper_ids and e["target"] not in paper_ids]
            self._graph_view.set_graph_data(nodes, edges)
        else:
            self._load_graph()

        QTimer.singleShot(200, self._apply_js_filter)

    def _apply_js_filter(self) -> None:
        """Push current filter state to JS filterGraph()."""
        cat_text  = self._sidebar.category_edit.text().strip()
        tag_text  = self._sidebar.tag_edit.text().strip()
        hl_text   = self._sidebar.highlight_edit.text().strip()
        auth_text = self._sidebar.author_edit.text().strip()
        date_from = self._sidebar.date_from_edit.text().strip()
        date_to   = self._sidebar.date_to_edit.text().strip()
        isolate   = self._sidebar.isolate_btn.isChecked()

        opts = {
            "showAuthors":  self._sidebar.show_authors.isChecked(),
            "showPapers":   self._sidebar.show_papers.isChecked(),
            "category":     cat_text if cat_text else None,
            "tag":          tag_text if tag_text else None,
            "hasPdf":       self._sidebar.has_pdf_check.isChecked(),
            "highlight":    hl_text if hl_text else None,
            "authorFilter": auth_text if auth_text else None,
            "dateFrom":     date_from if date_from else None,
            "dateTo":       date_to if date_to else None,
            "isolate":      isolate,
        }
        self._graph_view.filter_graph(opts)

    def _clear_highlight(self) -> None:
        """Clear all filters and reset node opacities."""
        for w in (
            self._sidebar.highlight_edit,
            self._sidebar.author_edit,
            self._sidebar.date_from_edit,
            self._sidebar.date_to_edit,
            self._sidebar.category_edit,
            self._sidebar.tag_edit,
        ):
            w.clear()
        self._sidebar.has_pdf_check.setChecked(False)
        self._sidebar.show_papers.setChecked(True)
        self._sidebar.show_authors.setChecked(True)
        self._sidebar.isolate_btn.setChecked(False)
        self._graph_view.highlight_node(None)

    # ── Paper list → graph interaction ────────────────────────────────────────

    def _on_paper_selected(self, current_row: int, _current_col: int,
                           _prev_row: int, _prev_col: int) -> None:
        if current_row < 0:
            return
        paper_id = self._paper_list.paper_id_for_row(current_row)
        if paper_id:
            self._graph_view.highlight_node(paper_id)
