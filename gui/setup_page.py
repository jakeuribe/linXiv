from __future__ import annotations

import os

from PyQt6.QtCore import Qt
from PyQt6.QtWidgets import (
    QComboBox,
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from gui.theme import BG as _BG, PANEL as _PANEL, BORDER as _BORDER
from gui.theme import ACCENT as _ACCENT, TEXT as _TEXT, MUTED as _MUTED

_GREEN  = "#4caf7d"
_AMBER  = "#e8a838"
_CODE   = "#0a0a14"

_ENV_PATH = os.path.join(os.path.dirname(__file__), "..", ".env")

_PROVIDERS = {
    "Gemini": "GENAI_API_KEY_TAG_GEN",
    "OpenAI": "OPENAI_API_KEY",
}

_INPUT_STYLE = f"""
    QLineEdit {{
        background: #0f0f1a; border: 1px solid {_BORDER}; border-radius: 6px;
        color: {_TEXT}; font-size: 13px; padding: 8px 10px;
    }}
    QLineEdit:focus {{ border-color: {_ACCENT}; }}
"""
_COMBO_STYLE = f"""
    QComboBox {{
        background: #0f0f1a; border: 1px solid {_BORDER}; border-radius: 6px;
        color: {_TEXT}; font-size: 13px; padding: 6px 10px;
    }}
    QComboBox:focus {{ border-color: {_ACCENT}; }}
    QComboBox::drop-down {{ border: none; }}
    QComboBox QAbstractItemView {{
        background: {_PANEL}; color: {_TEXT}; selection-background-color: {_ACCENT};
    }}
"""
_BTN_STYLE = f"""
    QPushButton {{
        background: {_ACCENT}; border: none; border-radius: 6px;
        color: #fff; font-size: 13px; font-weight: 600; padding: 8px 20px;
    }}
    QPushButton:hover   {{ background: #7aa3f5; }}
    QPushButton:pressed {{ background: #4a7add; }}
    QPushButton:disabled {{ background: #2a2a4a; color: {_MUTED}; }}
"""
_BTN_MUTED_STYLE = f"""
    QPushButton {{
        background: transparent; border: 1px solid {_BORDER}; border-radius: 6px;
        color: {_MUTED}; font-size: 13px; padding: 8px 20px;
    }}
    QPushButton:hover {{ border-color: {_TEXT}; color: {_TEXT}; }}
"""


def _env_present() -> bool:
    return os.path.isfile(_ENV_PATH)


def _key_set() -> bool:
    val = os.getenv("GENAI_API_KEY_TAG_GEN", "")
    return bool(val)


class SetupPage(QWidget):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setStyleSheet(f"background: {_BG}; color: {_TEXT};")

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setStyleSheet(f"QScrollArea {{ border: none; background: {_BG}; }}")

        content = QWidget()
        content.setStyleSheet(f"background: {_BG};")
        inner = QVBoxLayout(content)
        inner.setContentsMargins(48, 40, 48, 48)
        inner.setSpacing(0)

        def add(w: QWidget, space_after: int = 0) -> None:
            inner.addWidget(w)
            if space_after:
                inner.addSpacing(space_after)

        # ── Title ─────────────────────────────────────────────────────────────
        add(_h(f"Setup", 34, _ACCENT), 4)
        add(_p("Configure API keys so linXiv's AI features work correctly.", _MUTED, 13), 24)

        # ── Security warning ──────────────────────────────────────────────────
        add(_security_warning(), 32)

        # ── Status banner ─────────────────────────────────────────────────────
        add(self._status_banner(), 32)

        # ── AI provider config ───────────────────────────────────────────────
        add(_h("AI Provider", 20, _ACCENT), 12)
        add(_p("Choose your AI provider and enter the API key.", _MUTED, 13), 12)
        add(self._build_provider_config(), 32)

        # ── Step 1 ────────────────────────────────────────────────────────────
        add(_h("1 · Get a Google Gemini API key", 17, _TEXT), 10)
        add(_p(
            "linXiv uses the <b>Google Gemini API</b> (gemini-2.0-flash) to generate tags, "
            "summarise papers, and find related work. You need a free API key from Google AI Studio.",
            _TEXT, 13,
        ), 12)
        add(_link_card(
            "Google AI Studio",
            "aistudio.google.com/app/apikey",
            "Create or copy an existing key from the API keys page.",
        ), 24)

        # ── Step 2 ────────────────────────────────────────────────────────────
        add(_h("2 · Create a <code>.env</code> file", 17, _TEXT), 10)
        add(_p(
            "In the <b>project root</b> (the same folder as <code>main.py</code>), "
            "create a file named <code>.env</code> and add the line below:",
            _TEXT, 13,
        ), 12)
        add(_code_block("GENAI_API_KEY_TAG_GEN=your_api_key_here"), 8)
        add(_p(
            "Replace <code>your_api_key_here</code> with the key you copied from AI Studio. "
            "Do <b>not</b> add quotes around the value.",
            _MUTED, 12,
        ), 24)

        # ── Step 3 ────────────────────────────────────────────────────────────
        add(_h("3 · Restart linXiv", 17, _TEXT), 10)
        add(_p(
            "The <code>.env</code> file is loaded at startup. Close and reopen the app, "
            "then return to this page — the status banner above will turn green when the key is detected.",
            _TEXT, 13,
        ), 32)

        # ── Features table ────────────────────────────────────────────────────
        add(_h("What uses this key", 17, _TEXT), 14)
        for fn, desc in (
            ("Tag generation",    "Automatically suggests 3–5 Obsidian-style tags from a paper's content."),
            ("Summarisation",     "Produces a one-sentence TL;DR and a list of key contributions."),
            ("Related papers",    "Finds conceptually similar papers from your saved collection."),
        ):
            add(_feature_row(fn, desc), 8)

        inner.addStretch()
        scroll.setWidget(content)

        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(scroll)

    # ── Widgets ───────────────────────────────────────────────────────────────

    def _status_banner(self) -> QFrame:
        if _key_set():
            color, icon, text = _GREEN, "✓", "API key detected — AI features are active."
        elif _env_present():
            color, icon, text = _AMBER, "⚠", (
                f"<b>.env</b> file found at <code>{os.path.normpath(_ENV_PATH)}</code> "
                "but <code>GENAI_API_KEY_TAG_GEN</code> is not set or empty."
            )
        else:
            color, icon, text = _AMBER, "⚠", (
                f"No <b>.env</b> file found. Expected at "
                f"<code>{os.path.normpath(_ENV_PATH)}</code>."
            )

        frame = QFrame()
        frame.setStyleSheet(f"""
            QFrame {{
                background: transparent;
                border: 1px solid {color};
                border-radius: 8px;
                padding: 0px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """)
        row = QVBoxLayout(frame)
        row.setContentsMargins(16, 12, 16, 12)

        lbl = QLabel(f"{icon}  {text}")
        lbl.setStyleSheet(f"color: {color}; font-size: 13px;")
        lbl.setWordWrap(True)
        lbl.setTextFormat(Qt.TextFormat.RichText)
        row.addWidget(lbl)
        return frame


# ── Warning card ─────────────────────────────────────────────────────────────

_RED        = "#e05c5c"
_RED_BG     = "#1f0f0f"
_RED_BORDER = "#7a2020"

def _security_warning() -> QFrame:
    frame = QFrame()
    frame.setStyleSheet(f"""
        QFrame {{
            background: {_RED_BG};
            border: 2px solid {_RED_BORDER};
            border-radius: 10px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """)
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(20, 16, 20, 16)
    lay.setSpacing(8)

    icon_lbl = QLabel("⚠  Keep your API key safe")
    icon_lbl.setStyleSheet(
        f"font-size: 16px; font-weight: bold; color: {_RED}; letter-spacing: 0.02em;"
    )

    body = QLabel(
        "Your API key grants direct access to your Google account's quota and billing. "
        "Treat it like a password — <b>never share it, commit it to git, or paste it anywhere you don't fully trust.</b>"
        "<br><br>"
        "This application is open-source software. If you have any doubt about how your key is being used, "
        "<b>do not add it</b> — linXiv works without it and all AI features are strictly opt-in."
    )
    body.setTextFormat(Qt.TextFormat.RichText)
    body.setWordWrap(True)
    body.setStyleSheet(f"font-size: 13px; color: #ddbbbb; line-height: 1.5;")

    lay.addWidget(icon_lbl)
    lay.addWidget(body)
    return frame


# ── Reusable label helpers ────────────────────────────────────────────────────

def _h(text: str, size: int, color: str) -> QLabel:
    lbl = QLabel(text)
    lbl.setTextFormat(Qt.TextFormat.RichText)
    lbl.setStyleSheet(
        f"font-size: {size}px; font-weight: bold; color: {color}; background: transparent;"
    )
    return lbl


def _p(text: str, color: str, size: int = 13) -> QLabel:
    lbl = QLabel(text)
    lbl.setTextFormat(Qt.TextFormat.RichText)
    lbl.setWordWrap(True)
    lbl.setStyleSheet(f"font-size: {size}px; color: {color}; background: transparent;")
    return lbl


def _code_block(text: str) -> QFrame:
    frame = QFrame()
    frame.setStyleSheet(f"""
        QFrame {{
            background: {_CODE};
            border: 1px solid {_BORDER};
            border-radius: 6px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """)
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(16, 10, 16, 10)
    lbl = QLabel(text)
    lbl.setStyleSheet(f"font-family: 'Consolas', monospace; font-size: 13px; color: {_GREEN};")
    lay.addWidget(lbl)
    return frame


def _link_card(title: str, url: str, desc: str) -> QFrame:
    frame = QFrame()
    frame.setStyleSheet(f"""
        QFrame {{
            background: {_PANEL};
            border: 1px solid {_BORDER};
            border-radius: 8px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """)
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(16, 12, 16, 12)
    lay.setSpacing(4)

    title_lbl = QLabel(title)
    title_lbl.setStyleSheet(f"font-size: 13px; font-weight: 600; color: {_ACCENT};")

    url_lbl = QLabel(url)
    url_lbl.setStyleSheet(f"font-size: 12px; color: {_MUTED}; font-family: 'Consolas', monospace;")

    desc_lbl = QLabel(desc)
    desc_lbl.setStyleSheet(f"font-size: 12px; color: {_TEXT};")

    lay.addWidget(title_lbl)
    lay.addWidget(url_lbl)
    lay.addWidget(desc_lbl)
    return frame


def _feature_row(name: str, desc: str) -> QFrame:
    frame = QFrame()
    frame.setStyleSheet(f"""
        QFrame {{
            background: {_PANEL};
            border: 1px solid {_BORDER};
            border-radius: 6px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """)
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(14, 9, 14, 9)
    lay.setSpacing(2)

    name_lbl = QLabel(name)
    name_lbl.setStyleSheet(f"font-size: 13px; font-weight: 600; color: {_TEXT};")

    desc_lbl = QLabel(desc)
    desc_lbl.setStyleSheet(f"font-size: 12px; color: {_MUTED};")
    desc_lbl.setWordWrap(True)

    lay.addWidget(name_lbl)
    lay.addWidget(desc_lbl)
    return frame
