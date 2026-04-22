from PyQt6.QtWidgets import (
    QFrame, QWidget, QVBoxLayout, QHBoxLayout,
    QLabel, QPushButton, QComboBox, QSpinBox,
)
from PyQt6.QtCore import Qt, pyqtSignal
import arxiv

from gui.theme import SPACE_XS, SPACE_SM, SPACE_MD
from gui.search._widgets import _ClauseRow

_SORT_BY_OPTIONS = [
    ("Relevance",      arxiv.SortCriterion.Relevance),
    ("Submitted Date", arxiv.SortCriterion.SubmittedDate),
    ("Last Updated",   arxiv.SortCriterion.LastUpdatedDate),
]

_SORT_ORDER_OPTIONS = [
    ("Descending", arxiv.SortOrder.Descending),
    ("Ascending",  arxiv.SortOrder.Ascending),
]


class _QueryBuilderPanel(QFrame):
    query_inserted = pyqtSignal(str)  # emitted when "Insert Query →" clicked
    toggled = pyqtSignal(bool)        # emitted when panel visibility changes

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFrameShape(QFrame.Shape.StyledPanel)
        self._clauses: list[_ClauseRow] = []

        outer = QVBoxLayout(self)
        outer.setContentsMargins(SPACE_SM, SPACE_XS, SPACE_SM, SPACE_XS)
        outer.setSpacing(SPACE_SM)

        self._clause_container = QWidget()
        self._clause_layout = QVBoxLayout(self._clause_container)
        self._clause_layout.setContentsMargins(0, 0, 0, 0)
        self._clause_layout.setSpacing(SPACE_XS)
        outer.addWidget(self._clause_container)

        add_btn = QPushButton("+ Add clause")
        add_btn.setFixedWidth(110)  # TODO: Make more customizable
        add_btn.clicked.connect(self._add_clause)
        outer.addWidget(add_btn, alignment=Qt.AlignmentFlag.AlignLeft)

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
        outer.addLayout(preview_row)

        line = QFrame()
        line.setFrameShape(QFrame.Shape.HLine)
        line.setFrameShadow(QFrame.Shadow.Sunken)
        outer.addWidget(line)

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
        outer.addLayout(opts_row)

        self.setVisible(False)
        self._add_clause()

    def toggle(self) -> None:
        visible = not self.isVisible()
        self.setVisible(visible)
        self.toggled.emit(visible)

    def sort_by(self) -> arxiv.SortCriterion:
        return _SORT_BY_OPTIONS[self._sort_combo.currentIndex()][1]

    def sort_order(self) -> arxiv.SortOrder:
        return _SORT_ORDER_OPTIONS[self._order_combo.currentIndex()][1]

    def max_results(self) -> int:
        return self._max_spin.value()

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
            self.query_inserted.emit(q)
        self.setVisible(False)
        self.toggled.emit(False)
