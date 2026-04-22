from __future__ import annotations

import datetime

from PyQt6.QtCore import Qt, QTimer, pyqtSignal
from PyQt6.QtWidgets import (
    QHeaderView,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSplitter,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from ._export_handler import _ExportHandler
from .view import GraphView
from gui.theme import FONT_SECONDARY, FONT_TERTIARY, SPACE_XS, SPACE_SM
from storage.db import get_categories, get_graph_data, get_tags, list_papers

# ── Helpers ───────────────────────────────────────────────────────────────────
# TODO: Break down into smaller chunks
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


# ── Paper list panel ──

_COLUMNS = ["Title", "Category", "Published", "Tags", "PDF"]
_COL_WIDTHS = [320, 80, 90, 180, 40]
_PAGE_SIZE = 50


class PaperListPanel(QWidget):
    """Bottom panel showing saved papers as a table with lazy-loading pagination."""

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._all_rows: list = []
        self._loaded_count = 0

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(2)

        self._table = QTableWidget(0, len(_COLUMNS))
        self._table.setHorizontalHeaderLabels(_COLUMNS)
        self._table.setSelectionBehavior(QTableWidget.SelectionBehavior.SelectRows)
        self._table.setEditTriggers(QTableWidget.EditTrigger.NoEditTriggers)
        self._table.setAlternatingRowColors(False)
        self._table.verticalHeader().setVisible(False)  # pyright: ignore[reportOptionalMemberAccess]
        self._table.setSortingEnabled(True)

        hdr = self._table.horizontalHeader()
        for i, w in enumerate(_COL_WIDTHS):
            self._table.setColumnWidth(i, w)
        hdr.setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)  # pyright: ignore[reportOptionalMemberAccess]

        layout.addWidget(self._table)

        # Status bar
        self._status_lbl = QLabel("")
        self._status_lbl.setStyleSheet(f"color: #7777aa; font-size: {FONT_TERTIARY}px; padding: 2px 6px;")
        layout.addWidget(self._status_lbl)

        # Scroll detection for lazy loading
        vbar = self._table.verticalScrollBar()
        if vbar is not None:
            vbar.valueChanged.connect(self._on_scroll)

    @property
    def table(self) -> QTableWidget:
        return self._table

    def paper_id_for_row(self, row: int) -> str | None:
        item = self._table.item(row, 0)
        if item is None:
            return None
        return item.data(Qt.ItemDataRole.UserRole)

    def load_papers(self, rows) -> None:
        """Store all rows and display the first page."""
        self._all_rows = list(rows)
        self._loaded_count = 0
        self._table.setSortingEnabled(False)
        self._table.setRowCount(0)
        self._load_next_page()
        self._table.setSortingEnabled(True)

    def _load_next_page(self) -> None:
        """Append the next PAGE_SIZE rows to the table."""
        total = len(self._all_rows)
        end = min(self._loaded_count + _PAGE_SIZE, total)
        if self._loaded_count >= total:
            return

        was_sorting = self._table.isSortingEnabled()
        self._table.setSortingEnabled(False)

        for row_data in self._all_rows[self._loaded_count:end]:
            r = self._table.rowCount()
            self._table.insertRow(r)

            title_item = QTableWidgetItem(row_data["title"] or "")
            title_item.setData(Qt.ItemDataRole.UserRole, row_data["paper_id"])
            self._table.setItem(r, 0, title_item)
            self._table.setItem(r, 1, QTableWidgetItem(row_data["category"] or ""))
            self._table.setItem(r, 2, QTableWidgetItem(_fmt_date(row_data["published"])))
            self._table.setItem(r, 3, QTableWidgetItem(_fmt_tags(row_data["tags"])))
            pdf_item = QTableWidgetItem("Y" if row_data["has_pdf"] else "")
            pdf_item.setTextAlignment(Qt.AlignmentFlag.AlignCenter)
            self._table.setItem(r, 4, pdf_item)

        self._loaded_count = end
        self._table.setSortingEnabled(was_sorting)
        self._update_status()

    def _on_scroll(self, value: int) -> None:
        """Load more rows when scrolled near the bottom."""
        vbar = self._table.verticalScrollBar()
        if vbar is None:
            return
        if self._loaded_count >= len(self._all_rows):
            return
        # Trigger when within 20% of bottom
        if value >= vbar.maximum() * 0.8:
            self._load_next_page()

    def _update_status(self) -> None:
        total = len(self._all_rows)
        shown = self._loaded_count
        if total == 0:
            self._status_lbl.setText("")
        elif shown >= total:
            self._status_lbl.setText(f"Showing all {total} papers")
        else:
            self._status_lbl.setText(f"Showing {shown} of {total} papers (scroll for more)")


# ── Graph page ────────────────────────────────────────────────────────────────

class GraphPage(QWidget):
    """Graph + paper list, embeddable as a page inside AppShell."""
    paper_right_clicked = pyqtSignal(str)  # emits paper_id when a node is right-clicked

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        self._build_btn_bar(layout)
        self._build_split(layout)
        self._export_handler = _ExportHandler(self._graph_view, self)
        self._load_all()
        QTimer.singleShot(100, self._toggle_paper_list)

    # ── Construction ──────────────────────────────────────────────────────────

    def _build_btn_bar(self, layout: QVBoxLayout) -> None:
        bar = QHBoxLayout()
        bar.setContentsMargins(SPACE_SM, SPACE_XS, SPACE_SM, SPACE_XS)
        bar.setSpacing(SPACE_SM)

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
        self._selection_lbl.setStyleSheet(f"color: #7777aa; font-size: {FONT_SECONDARY}px;")
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
        self._graph_view.node_right_clicked.connect(self.paper_right_clicked)
        self._graph_view.selection_changed.connect(self._on_selection_changed)

    # ── Data loading ──────────────────────────────────────────────────────────

    def _load_all(self) -> None:
        self._load_graph()
        self._load_paper_list()
        self._load_dropdowns()

    def _load_graph(self) -> None:
        nodes, edges = get_graph_data()

        # Generate tag nodes from paper tags
        seen_tag_ids: set[str] = set()
        tag_edges: list[dict] = []
        for node in nodes:
            if node["type"] == "paper":
                for tag in (node.get("tags") or []):
                    tag_node_id = f"tag::{tag}"
                    if tag_node_id not in seen_tag_ids:
                        seen_tag_ids.add(tag_node_id)
                    tag_edges.append({"source": node["id"], "target": tag_node_id})
        tag_nodes = [{"id": tid, "label": tid[5:], "type": "tag"} for tid in seen_tag_ids]
        nodes = nodes + tag_nodes
        edges = edges + tag_edges

        # Augment paper nodes with project membership
        try:
            from storage.projects import filter_projects
            paper_to_projects: dict[str, list[int]] = {}
            for proj in filter_projects():
                if proj.id is not None:
                    for pid in (proj.paper_ids or []):
                        paper_to_projects.setdefault(pid, []).append(proj.id)
            for node in nodes:
                if node["type"] == "paper":
                    node["project_ids"] = paper_to_projects.get(node["id"], [])
        except Exception:
            pass

        self._graph_view.set_graph_data(nodes, edges)

    def _load_paper_list(self) -> None:
        papers = list_papers(latest_only=True)
        self._paper_list.load_papers(papers)

    def _load_dropdowns(self) -> None:
        categories = get_categories()
        tags = get_tags()
        proj_data: list[dict] = []
        try:
            from storage.projects import filter_projects, color_to_hex, Status
            for p in filter_projects():
                if p.id is not None and p.status != Status.DELETED:
                    proj_data.append({
                        "id":    p.id,
                        "name":  p.name,
                        "color": color_to_hex(p.color) if p.color else "#5b8dee",
                        "tags":  p.project_tags or [],
                    })
        except Exception:
            pass
        self._graph_view.set_filter_options(categories, tags, proj_data)

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
        # First search already-loaded rows
        for row in range(self._paper_list.table.rowCount()):
            if self._paper_list.paper_id_for_row(row) == paper_id:
                self._paper_list.table.setCurrentCell(row, 0)
                return
        # Paper might be in a not-yet-loaded page — load remaining pages and retry
        while self._paper_list._loaded_count < len(self._paper_list._all_rows):
            prev_count = self._paper_list.table.rowCount()
            self._paper_list._load_next_page()
            for row in range(prev_count, self._paper_list.table.rowCount()):
                if self._paper_list.paper_id_for_row(row) == paper_id:
                    self._paper_list.table.setCurrentCell(row, 0)
                    return

    # ── Selection & export ───────────────────────────────────────────────────

    def _on_selection_changed(self, count: int) -> None:
        self._selection_lbl.setText(f"{count} selected")
        self._export_btn.setEnabled(count > 0)

    def _show_export_menu(self) -> None:
        self._export_handler.show_menu(self._export_btn)
