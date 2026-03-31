from __future__ import annotations

import csv
import datetime
import io
import json

from PyQt6.QtCore import Qt, QTimer
from PyQt6.QtWidgets import (
    QFileDialog,
    QHeaderView,
    QHBoxLayout,
    QLabel,
    QMenu,
    QPushButton,
    QSplitter,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from .graph_view import GraphView
from db import get_categories, get_graph_data, get_tags, list_papers


# ── Helpers ───────────────────────────────────────────────────────────────────

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


# ── Graph page ────────────────────────────────────────────────────────────────

class GraphPage(QWidget):
    """Graph + paper list, embeddable as a page inside AppShell."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        self._build_btn_bar(layout)
        self._build_split(layout)
        self._load_all()
        QTimer.singleShot(100, self._toggle_paper_list)

    # ── Construction ──────────────────────────────────────────────────────────

    def _build_btn_bar(self, layout: QVBoxLayout) -> None:
        bar = QHBoxLayout()
        bar.setContentsMargins(6, 4, 6, 4)
        bar.setSpacing(6)

        refresh_btn = QPushButton("Refresh")
        refresh_btn.setToolTip("Reload graph data from database")
        refresh_btn.clicked.connect(self.refresh)
        bar.addWidget(refresh_btn)

        clear_btn = QPushButton("Clear filters")
        clear_btn.setToolTip("Reset all graph filters")
        clear_btn.clicked.connect(self._clear_filters)
        bar.addWidget(clear_btn)

        toggle_btn = QPushButton("Toggle list")
        toggle_btn.setToolTip("Show / hide the paper list panel")
        toggle_btn.clicked.connect(self._toggle_paper_list)
        bar.addWidget(toggle_btn)

        bar.addStretch()

        self._selection_lbl = QLabel("0 selected")
        self._selection_lbl.setStyleSheet("color: #7777aa; font-size: 12px;")
        bar.addWidget(self._selection_lbl)

        self._export_btn = QPushButton("Export selected")
        self._export_btn.setToolTip("Export selected papers to file")
        self._export_btn.setEnabled(False)
        self._export_btn.clicked.connect(self._show_export_menu)
        bar.addWidget(self._export_btn)

        layout.addLayout(bar)

    def _build_split(self, layout: QVBoxLayout) -> None:
        split = QSplitter(Qt.Orientation.Vertical)
        self._split = split
        split.setChildrenCollapsible(False)

        self._graph_view = GraphView()
        split.addWidget(self._graph_view)

        self._paper_list = PaperListPanel()
        split.addWidget(self._paper_list)

        split.setStretchFactor(0, 3)
        split.setStretchFactor(1, 1)

        layout.addWidget(split)

        self._paper_list.table.currentCellChanged.connect(self._on_paper_selected)
        self._graph_view.node_clicked.connect(self._on_graph_node_clicked)
        self._graph_view.selection_changed.connect(self._on_selection_changed)

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

    # ── Button actions ────────────────────────────────────────────────────────

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

    def _on_graph_node_clicked(self, paper_id: str) -> None:
        """Graph paper node clicked — select matching row in the paper list."""
        for row in range(self._paper_list.table.rowCount()):
            if self._paper_list.paper_id_for_row(row) == paper_id:
                self._paper_list.table.setCurrentCell(row, 0)
                break
