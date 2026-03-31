from __future__ import annotations

import datetime

from PyQt6.QtCore import Qt, QTimer
from PyQt6.QtGui import QAction
from PyQt6.QtWidgets import (
    QHeaderView,
    QMainWindow,
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


# ── Paper list panel ──────────────────────────────────────────────────────────

_COLUMNS = ["Title", "Category", "Published", "Tags", "PDF"]
_COL_WIDTHS = [320, 80, 90, 180, 40]


class PaperListPanel(QWidget):
    """Bottom panel showing saved papers as a table."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)

        self._table = QTableWidget(0, len(_COLUMNS))
        self._table.setHorizontalHeaderLabels(_COLUMNS)
        self._table.setSelectionBehavior(QTableWidget.SelectionBehavior.SelectRows)
        self._table.setEditTriggers(QTableWidget.EditTrigger.NoEditTriggers)
        self._table.setAlternatingRowColors(True)
        self._table.verticalHeader().setVisible(False)  # pyright: ignore[reportOptionalMemberAccess]
        self._table.setSortingEnabled(True)

        hdr = self._table.horizontalHeader()
        for i, w in enumerate(_COL_WIDTHS):
            self._table.setColumnWidth(i, w)
        hdr.setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)  # pyright: ignore[reportOptionalMemberAccess]

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
        self._load_all()
        QTimer.singleShot(100, self._toggle_paper_list)
    # ── Construction ─────────────────────────────────────────────────────────

    def _build_toolbar(self) -> None:
        tb = QToolBar("Main toolbar", self)
        tb.setMovable(False)
        self.addToolBar(tb)

        refresh_action = QAction("Refresh", self)
        refresh_action.setToolTip("Reload graph data from database")
        refresh_action.triggered.connect(self.refresh)
        tb.addAction(refresh_action)

        clear_action = QAction("Clear filters", self)
        clear_action.setToolTip("Reset all graph filters")
        clear_action.triggered.connect(self._clear_filters)
        tb.addAction(clear_action)

        toggle_list_action = QAction("Toggle list", self)
        toggle_list_action.setToolTip("Show / hide the paper list panel")
        toggle_list_action.triggered.connect(self._toggle_paper_list)
        tb.addAction(toggle_list_action)

    def _build_central(self) -> None:
        split = QSplitter(Qt.Orientation.Vertical)
        self._split = split
        split.setChildrenCollapsible(False)

        self._graph_view = GraphView()
        split.addWidget(self._graph_view)

        self._paper_list = PaperListPanel()
        split.addWidget(self._paper_list)

        split.setStretchFactor(0, 3)
        split.setStretchFactor(1, 1)

        self.setCentralWidget(split)

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
        self._graph_view.set_filter_options(categories, tags)

    # ── Toolbar actions ───────────────────────────────────────────────────────

    def refresh(self) -> None:
        self._load_all()

    def _clear_filters(self) -> None:
        self._graph_view.clear_filters()
        self._graph_view.highlight_node(None)

    def _toggle_paper_list(self) -> None:
        if self._paper_list.isVisible():
            self._list_sizes = self._split.sizes()
            self._paper_list.hide()
        else:
            self._paper_list.show()
            if hasattr(self, '_list_sizes'):
                self._split.setSizes(self._list_sizes)

    # ── Paper list → graph interaction ────────────────────────────────────────

    def _on_paper_selected(self, current_row: int, _current_col: int,
                           _prev_row: int, _prev_col: int) -> None:
        if current_row < 0:
            return
        paper_id = self._paper_list.paper_id_for_row(current_row)
        if paper_id:
            self._graph_view.highlight_node(paper_id)
