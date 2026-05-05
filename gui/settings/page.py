from __future__ import annotations

import json
import os
import re

from pathlib import Path

from PyQt6.QtCore import Qt
from PyQt6.QtGui import QColor, QFont
from PyQt6.QtWidgets import (
    QColorDialog,
    QDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QProgressBar,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

import user_settings as _user_settings
from storage.paths import pdf_dir as _pdf_dir
from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED
from gui.theme import (
    FONT_TITLE, FONT_HEADING, FONT_BODY, FONT_SECONDARY,
    SPACE_XL, SPACE_MD, SPACE_SM, SPACE_XS,
    RADIUS_LG, RADIUS_MD, RADIUS_SM, CARD_PAD_H, CARD_PAD_V, PAGE_MARGIN_H, DIALOG_PAD,
    SPACE_LG,
)
from gui.qt_assets.styles import BTN_PRIMARY as _BTN_PRIMARY, BTN_MUTED as _BTN_MUTED


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


def _normalize_hex3(template: str) -> str:
    """Expand 3-digit hex colors (#rgb → #rrggbb) in a stylesheet template."""
    def _expand(m: re.Match) -> str:
        h = m.group(0)
        return '#' + h[1] * 2 + h[2] * 2 + h[3] * 2
    return re.sub(r'#([0-9a-fA-F]{3})(?![0-9a-fA-F])', _expand, template)


def _parse_template_colors(template: str) -> list[tuple[str, int, str]]:
    """Return (label, position_in_template, '#rrggbb') for each hardcoded hex color."""
    results: list[tuple[str, int, str]] = []
    block_re = re.compile(r'(QPushButton(?::[\w-]+)?)\s*\{\{(.*?)\}\}', re.DOTALL)
    color_re = re.compile(r'#[0-9a-fA-F]{6}(?![0-9a-fA-F])')
    prop_re  = re.compile(r'([\w-]+)\s*:')

    for bm in block_re.finditer(template):
        selector = bm.group(1)
        state = selector[len('QPushButton'):]
        block_body = bm.group(2)
        block_offset = bm.start(2)

        for cm in color_re.finditer(block_body):
            hex_val = cm.group(0)
            before = block_body[:cm.start()]
            props = list(prop_re.finditer(before))
            prop = props[-1].group(1) if props else 'color'
            label = f"{state.lstrip(':')} · {prop}" if state else prop
            results.append((label, block_offset + cm.start(), hex_val))

    return results


class _ColorSwatch(QPushButton):
    """Clickable color swatch that opens QColorDialog and updates a bound hex label."""

    def __init__(self, hex_color: str, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setFixedSize(32, 24)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        self._color = hex_color
        self._lbl: QLabel | None = None
        self._refresh()
        self.clicked.connect(self._pick)

    def _refresh(self) -> None:
        self.setStyleSheet(
            f"QPushButton {{ background: {self._color}; border: 1px solid {_BORDER}; "
            f"border-radius: {RADIUS_SM}px; }}"
            f"QPushButton:hover {{ border-color: {_TEXT}; }}"
        )

    def _pick(self) -> None:
        color = QColorDialog.getColor(QColor(self._color), self, "Pick Color")
        if color.isValid():
            self._color = color.name()
            self._refresh()
            if self._lbl is not None:
                self._lbl.setText(self._color)

    def bind(self, lbl: QLabel) -> None:
        self._lbl = lbl

    def hex_color(self) -> str:
        return self._color


class _JsonEditorDialog(QDialog):
    def __init__(self, path: Path, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle(f"Edit {path.name}")
        self.setMinimumWidth(620)
        self.resize(720, 620)
        self.setStyleSheet(f"background: {_PANEL}; color: {_TEXT};")
        self._path = path

        raw: dict[str, str] = json.loads(path.read_text())
        # key → (normalized_template, [(label, abs_pos, swatch)])
        self._state: dict[str, tuple[str, list[tuple[str, int, _ColorSwatch]]]] = {}

        outer = QVBoxLayout(self)
        outer.setContentsMargins(DIALOG_PAD, DIALOG_PAD, DIALOG_PAD, DIALOG_PAD)
        outer.setSpacing(SPACE_SM)

        heading = QLabel(f"Edit {path.stem}")
        heading.setStyleSheet(
            f"font-size: {FONT_HEADING}px; font-weight: bold; color: {_ACCENT};"
        )
        outer.addWidget(heading)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setStyleSheet("background: transparent;")

        content = QWidget()
        content.setStyleSheet("background: transparent;")
        clay = QVBoxLayout(content)
        clay.setContentsMargins(0, SPACE_SM, 0, 0)
        clay.setSpacing(SPACE_LG)

        mono = QFont("Monospace", 10)

        for key, template in raw.items():
            norm = _normalize_hex3(template)
            colors = _parse_template_colors(norm)

            key_lbl = QLabel(key)
            key_lbl.setStyleSheet(
                f"color: {_MUTED}; font-size: {FONT_SECONDARY}px; font-weight: 600;"
            )
            clay.addWidget(key_lbl)

            card = QFrame()
            card.setStyleSheet(
                f"QFrame {{ background: {_BG}; border: 1px solid {_BORDER}; "
                f"border-radius: {RADIUS_MD}px; }}"
            )
            c_lay = QVBoxLayout(card)
            c_lay.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)
            c_lay.setSpacing(SPACE_SM)

            swatches: list[tuple[str, int, _ColorSwatch]] = []
            if colors:
                for label, pos, hex_val in colors:
                    row_w = QWidget()
                    row_w.setStyleSheet("background: transparent;")
                    row = QHBoxLayout(row_w)
                    row.setContentsMargins(0, 0, 0, 0)
                    row.setSpacing(SPACE_SM)

                    prop_lbl = QLabel(label)
                    prop_lbl.setStyleSheet(
                        f"color: {_TEXT}; font-size: {FONT_SECONDARY}px;"
                    )
                    prop_lbl.setMinimumWidth(160)

                    swatch = _ColorSwatch(hex_val)

                    hex_lbl = QLabel(hex_val)
                    hex_lbl.setFont(mono)
                    hex_lbl.setStyleSheet(f"color: {_MUTED}; font-size: {FONT_SECONDARY}px;")
                    swatch.bind(hex_lbl)

                    row.addWidget(prop_lbl)
                    row.addWidget(swatch)
                    row.addWidget(hex_lbl)
                    row.addStretch()
                    c_lay.addWidget(row_w)
                    swatches.append((label, pos, swatch))
            else:
                note = QLabel("Colors inherited from theme variables")
                note.setStyleSheet(
                    f"color: {_MUTED}; font-size: {FONT_SECONDARY}px; font-style: italic;"
                )
                c_lay.addWidget(note)

            clay.addWidget(card)
            self._state[key] = (norm, swatches)

        clay.addStretch()
        scroll.setWidget(content)
        outer.addWidget(scroll, stretch=1)

        btn_row = QHBoxLayout()
        cancel_btn = QPushButton("Cancel")
        cancel_btn.setStyleSheet(_BTN_MUTED)
        cancel_btn.clicked.connect(self.reject)
        save_btn = QPushButton("Save")
        save_btn.setStyleSheet(_BTN_PRIMARY)
        save_btn.clicked.connect(self._save)
        btn_row.addWidget(cancel_btn)
        btn_row.addStretch()
        btn_row.addWidget(save_btn)
        outer.addLayout(btn_row)

    def _save(self) -> None:
        out: dict[str, str] = {}
        for key, (template, swatches) in self._state.items():
            t = template
            for _, pos, swatch in sorted(swatches, key=lambda x: x[1], reverse=True):
                t = t[:pos] + swatch.hex_color() + t[pos + 7:]
            out[key] = t
        self._path.write_text(json.dumps(out, indent=2))
        self.accept()


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

        inner.addSpacing(SPACE_XL)

        # ── Appearance section ────────────────────────────────────────────────
        inner.addWidget(_section_label("Appearance"))
        inner.addSpacing(SPACE_SM)

        _styles_path = Path(__file__).resolve().parent.parent.parent / "formats" / "styles.json"

        open_styles_btn = QPushButton("Edit")
        open_styles_btn.setFixedWidth(80)
        open_styles_btn.setStyleSheet(
            f"QPushButton {{ background: {_PANEL}; border: 1px solid {_BORDER}; "
            f"border-radius: 6px; color: {_TEXT}; font-size: {FONT_BODY}px; padding: 4px 10px; }}"
            f"QPushButton:hover {{ border-color: {_TEXT}; }}"
        )
        open_styles_btn.clicked.connect(
            lambda: _JsonEditorDialog(_styles_path, self).exec()
        )

        inner.addWidget(_Card([
            _SettingRow(
                "Button styles",
                f"Edit QPushButton stylesheet templates — changes take effect on next launch.  {_styles_path.name}",
                open_styles_btn,
            )
        ]))

        inner.addStretch()
        scroll.setWidget(content)

        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(scroll)
