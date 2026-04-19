from PyQt6.QtWidgets import (
    QWidget, QHBoxLayout, QLabel, QLineEdit,
    QComboBox, QPushButton, QListWidget, QListWidgetItem, QCheckBox,
)
from PyQt6.QtCore import Qt, pyqtSignal
from gui.theme import SPACE_SM, SPACE_XS, BTN_H_SM, RADIUS_SM

_FIELD_OPTIONS = [
    ("Author",     "au:"),
    ("Title",      "ti:"),
    ("Abstract",   "abs:"),
    ("Category",   "cat:"),
    ("Comment",    "co:"),
    ("Journal Ref","jr:"),
    ("All fields", ""),
]


class _ClauseRow(QWidget):
    changed = pyqtSignal()
    remove_requested = pyqtSignal(object)

    def __init__(self, show_operator: bool = False, parent=None):
        super().__init__(parent)
        self._layout = QHBoxLayout(self)
        self._layout.setContentsMargins(0, 0, 0, 0)
        self._layout.setSpacing(SPACE_SM)

        self._op_combo = QComboBox()
        self._op_combo.addItems(["AND", "OR", "ANDNOT"])
        self._op_combo.setFixedWidth(80)  # TODO: Make more customizable
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
        rm.setFixedWidth(BTN_H_SM)
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
    def __init__(self, title: str, source: str = "", parent=None):
        super().__init__(parent)
        layout = QHBoxLayout(self)
        layout.setContentsMargins(SPACE_XS, 2, SPACE_XS, 2)
        layout.setSpacing(SPACE_SM)

        if source and source != "arxiv":
            badge = QLabel(source)
            badge.setFixedWidth(70)  # TODO: Make more customizable
            badge.setAlignment(Qt.AlignmentFlag.AlignCenter)
            badge.setStyleSheet(
                f"background: #2a4a3a; color: #4caf7d; border-radius: {RADIUS_SM}px;"
                f" font-size: 10px; padding: 1px {SPACE_XS}px;"
            )
            layout.addWidget(badge, alignment=Qt.AlignmentFlag.AlignTop)

        self._label = QLabel(title)
        self._label.setWordWrap(True)
        layout.addWidget(self._label, stretch=1)

        self._checkbox = QCheckBox("Save")
        self._checkbox.setFixedWidth(60)  # TODO: Make more customizable
        layout.addWidget(self._checkbox, alignment=Qt.AlignmentFlag.AlignTop)

    @property
    def checked(self) -> bool:
        return self._checkbox.isChecked()

    def set_checked(self, value: bool) -> None:
        self._checkbox.setChecked(value)
