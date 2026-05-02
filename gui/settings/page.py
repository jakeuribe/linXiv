from __future__ import annotations

import os

from PyQt6.QtCore import Qt
from PyQt6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QProgressBar,
    QScrollArea,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

import user_settings as _user_settings
from storage.paths import pdf_dir as _pdf_dir
from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import TEXT as _TEXT, MUTED as _MUTED
from gui.theme import (
    FONT_TITLE, FONT_BODY, FONT_SECONDARY,
    SPACE_XL, SPACE_MD, SPACE_SM, SPACE_XS,
    RADIUS_LG, CARD_PAD_H, CARD_PAD_V, PAGE_MARGIN_H,
)


def _section_label(text: str) -> QLabel:
    lbl = QLabel(text)
    lbl.setStyleSheet(
        f"color: {_MUTED}; font-size: {FONT_SECONDARY}px; font-weight: 600; "
        f"letter-spacing: 1px; text-transform: uppercase;"
    )
    return lbl


def _divider() -> QFrame:
    line = QFrame()
    line.setFrameShape(QFrame.Shape.HLine)
    line.setStyleSheet(f"color: {_BORDER};")
    return line


class _SettingRow(QWidget):
    """A labeled setting row — subclass or replace the right-hand widget."""

    def __init__(self, label: str, description: str, control: QWidget) -> None:
        super().__init__()
        self.setStyleSheet(f"background: transparent;")
        h = QHBoxLayout(self)
        h.setContentsMargins(0, 0, 0, 0)
        h.setSpacing(SPACE_MD)

        text_col = QVBoxLayout()
        text_col.setSpacing(SPACE_XS)
        name = QLabel(label)
        name.setStyleSheet(f"color: {_TEXT}; font-size: {FONT_BODY}px;")
        desc = QLabel(description)
        desc.setStyleSheet(f"color: {_MUTED}; font-size: {FONT_SECONDARY}px;")
        desc.setWordWrap(True)
        text_col.addWidget(name)
        text_col.addWidget(desc)

        h.addLayout(text_col, stretch=1)
        h.addWidget(control, alignment=Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)


class _Card(QFrame):
    """Rounded panel that groups related setting rows."""

    def __init__(self, rows: list[_SettingRow]) -> None:
        super().__init__()
        self.setStyleSheet(
            f"QFrame {{ background: {_PANEL}; border: 1px solid {_BORDER}; "
            f"border-radius: {RADIUS_LG}px; }}"
        )
        layout = QVBoxLayout(self)
        layout.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)
        layout.setSpacing(0)

        for i, row in enumerate(rows):
            layout.addWidget(row)
            if i < len(rows) - 1:
                layout.addWidget(_divider())


class SettingsPage(QWidget):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setStyleSheet(f"QScrollArea {{ border: none; background: {_BG}; }}")

        content = QWidget()
        content.setStyleSheet(f"background: {_BG};")
        inner = QVBoxLayout(content)
        inner.setContentsMargins(PAGE_MARGIN_H, 40, PAGE_MARGIN_H, PAGE_MARGIN_H)
        inner.setSpacing(0)

        title = QLabel("Settings")
        title.setStyleSheet(f"color: {_TEXT}; font-size: {FONT_TITLE}px; font-weight: 700;")
        inner.addWidget(title)
        inner.addSpacing(SPACE_XL)

        # ── Metadata Sources section ──────────────────────────────────────────
        inner.addWidget(_section_label("Metadata Sources"))
        inner.addSpacing(SPACE_SM)

        mailto_field = QLineEdit()
        mailto_field.setPlaceholderText("your@email.com")
        mailto_field.setText(os.environ.get("CROSSREF_MAILTO", ""))
        mailto_field.setFixedWidth(240)
        mailto_field.setStyleSheet(
            f"background: {_PANEL}; border: 1px solid {_BORDER}; "
            f"border-radius: 6px; color: {_TEXT}; font-size: {FONT_BODY}px; "
            f"padding: 4px 10px;"
        )

        def _save_mailto() -> None:
            from config import ENV_PATH
            from dotenv import set_key
            value = mailto_field.text().strip()
            os.environ["CROSSREF_MAILTO"] = value
            set_key(str(ENV_PATH), "CROSSREF_MAILTO", value)

        mailto_field.editingFinished.connect(_save_mailto)

        inner.addWidget(_Card([
            _SettingRow(
                "CrossRef Email (mailto)",
                "Your email for CrossRef's polite pool — faster, more reliable metadata responses.",
                mailto_field,
            )
        ]))

        inner.addSpacing(SPACE_XL)

        # ── Storage section ───────────────────────────────────────────────────
        inner.addWidget(_section_label("Storage"))
        inner.addSpacing(SPACE_SM)

        def _pdf_used_mb() -> float:
            d = _pdf_dir()
            if not d.is_dir():
                return 0.0
            return sum(
                os.path.getsize(d / f)
                for f in os.listdir(d)
                if f.lower().endswith(".pdf") and os.path.isfile(d / f)
            ) / 1024 ** 2

        limit_spin = QSpinBox()
        limit_spin.setRange(100, 100_000)
        limit_spin.setSingleStep(100)
        limit_spin.setSuffix(" MB")
        limit_spin.setValue(_user_settings.get("pdf_save_limit_mb"))
        limit_spin.setFixedWidth(130)
        limit_spin.setStyleSheet(
            f"background: {_PANEL}; border: 1px solid {_BORDER}; "
            f"border-radius: 6px; color: {_TEXT}; font-size: {FONT_BODY}px; "
            f"padding: 4px 10px;"
        )

        used_mb = _pdf_used_mb()
        limit_mb_init = _user_settings.get("pdf_save_limit_mb")
        pct_init = min(100, int(used_mb / limit_mb_init * 100)) if limit_mb_init else 0

        usage_bar = QProgressBar()
        usage_bar.setRange(0, 100)
        usage_bar.setValue(pct_init)
        usage_bar.setTextVisible(False)
        usage_bar.setFixedWidth(100)
        usage_bar.setFixedHeight(8)
        usage_bar.setStyleSheet(
            f"QProgressBar {{ background: {_BORDER}; border-radius: 4px; border: none; }}"
            f"QProgressBar::chunk {{ background: {'#c0392b' if pct_init >= 90 else _MUTED}; border-radius: 4px; }}"
        )

        usage_label = QLabel(f"{used_mb:.1f} / {limit_mb_init} MB  ({pct_init}%)")
        usage_label.setStyleSheet(f"color: {_MUTED}; font-size: {FONT_SECONDARY}px;")

        usage_widget = QWidget()
        usage_widget.setStyleSheet("background: transparent;")
        usage_row = QHBoxLayout(usage_widget)
        usage_row.setContentsMargins(0, 0, 0, 0)
        usage_row.setSpacing(SPACE_SM)
        usage_row.addWidget(usage_bar)
        usage_row.addWidget(usage_label)

        def _refresh_usage() -> None:
            mb = _pdf_used_mb()
            lim = limit_spin.value()
            pct = min(100, int(mb / lim * 100)) if lim else 0
            usage_bar.setValue(pct)
            usage_label.setText(f"{mb:.1f} / {lim} MB  ({pct}%)")
            color = "#c0392b" if pct >= 90 else _MUTED
            usage_bar.setStyleSheet(
                f"QProgressBar {{ background: {_BORDER}; border-radius: 4px; border: none; }}"
                f"QProgressBar::chunk {{ background: {color}; border-radius: 4px; }}"
            )

        def _save_pdf_limit() -> None:
            _user_settings.set("pdf_save_limit_mb", limit_spin.value())

        limit_spin.editingFinished.connect(_save_pdf_limit)
        limit_spin.valueChanged.connect(_refresh_usage)

        inner.addWidget(_Card([
            _SettingRow(
                "PDF save limit",
                "Maximum total size of PDFs kept on disk across all sessions.",
                limit_spin,
            ),
            _SettingRow(
                "PDF storage used",
                "Current size of all PDFs in the local downloads folder.",
                usage_widget,
            ),
        ]))

        inner.addStretch()
        scroll.setWidget(content)

        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(scroll)
