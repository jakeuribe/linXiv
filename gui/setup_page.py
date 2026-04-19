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
from gui.theme import (
    FONT_TITLE, FONT_HEADING, FONT_SUBHEADING, FONT_BODY, FONT_SECONDARY,
    SPACE_XS, SPACE_SM, SPACE_MD,
    RADIUS_MD, RADIUS_LG,
    CARD_PAD_H, CARD_PAD_V, DIALOG_PAD, PAGE_MARGIN_H,
)

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
        background: #0f0f1a; border: 1px solid {_BORDER}; border-radius: {RADIUS_MD}px;
        color: {_TEXT}; font-size: {FONT_BODY}px; padding: {SPACE_SM}px 10px;
    }}
    QLineEdit:focus {{ border-color: {_ACCENT}; }}
"""
_COMBO_STYLE = f"""
    QComboBox {{
        background: #0f0f1a; border: 1px solid {_BORDER}; border-radius: {RADIUS_MD}px;
        color: {_TEXT}; font-size: {FONT_BODY}px; padding: {SPACE_XS}px 10px;
    }}
    QComboBox:focus {{ border-color: {_ACCENT}; }}
    QComboBox::drop-down {{ border: none; }}
    QComboBox QAbstractItemView {{
        background: {_PANEL}; color: {_TEXT}; selection-background-color: {_ACCENT};
    }}
"""
_BTN_STYLE = f"""
    QPushButton {{
        background: {_ACCENT}; border: none; border-radius: {RADIUS_MD}px;
        color: #fff; font-size: {FONT_BODY}px; font-weight: 600; padding: {SPACE_SM}px 20px;
    }}
    QPushButton:hover   {{ background: #7aa3f5; }}
    QPushButton:pressed {{ background: #4a7add; }}
    QPushButton:disabled {{ background: #2a2a4a; color: {_MUTED}; }}
"""
_BTN_MUTED_STYLE = f"""
    QPushButton {{
        background: transparent; border: 1px solid {_BORDER}; border-radius: {RADIUS_MD}px;
        color: {_MUTED}; font-size: {FONT_BODY}px; padding: {SPACE_SM}px 20px;
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
        inner.setContentsMargins(PAGE_MARGIN_H, 40, PAGE_MARGIN_H, PAGE_MARGIN_H)
        inner.setSpacing(0)

        def add(w: QWidget, space_after: int = 0) -> None:
            inner.addWidget(w)
            if space_after:
                inner.addSpacing(space_after)

        # ── Title ─────────────────────────────────────────────────────────────
        add(_h("Setup", FONT_TITLE, _ACCENT), SPACE_XS)
        add(_p("Configure API keys so linXiv's AI features work correctly.", _MUTED, FONT_BODY), 24)

        # ── Security warning ──────────────────────────────────────────────────
        add(_security_warning(), 32)

        # ── Status banner ─────────────────────────────────────────────────────
        add(self._status_banner(), 32)

        # ── AI provider config ───────────────────────────────────────────────
        add(_h("AI Provider", FONT_HEADING, _ACCENT), SPACE_MD)
        add(_p("Choose your AI provider and enter the API key.", _MUTED, FONT_BODY), SPACE_MD)
        add(self._build_provider_config(), 32)

        # ── Step 1 ────────────────────────────────────────────────────────────
        add(_h("1 · Get a Google Gemini API key", FONT_HEADING, _TEXT), SPACE_SM)
        add(_p(
            "linXiv uses the <b>Google Gemini API</b> (gemini-2.0-flash) to generate tags, "
            "summarise papers, and find related work. You need a free API key from Google AI Studio.",
            _TEXT, FONT_BODY,
        ), SPACE_MD)
        add(_link_card(
            "Google AI Studio",
            "aistudio.google.com/app/apikey",
            "Create or copy an existing key from the API keys page.",
        ), 24)

        # ── Step 2 ────────────────────────────────────────────────────────────
        add(_h("2 · Create a <code>.env</code> file", FONT_HEADING, _TEXT), SPACE_SM)
        add(_p(
            "In the <b>project root</b> (the same folder as <code>main.py</code>), "
            "create a file named <code>.env</code> and add the line below:",
            _TEXT, FONT_BODY,
        ), SPACE_MD)
        add(_code_block("GENAI_API_KEY_TAG_GEN=your_api_key_here"), 8)
        add(_p(
            "Replace <code>your_api_key_here</code> with the key you copied from AI Studio. "
            "Do <b>not</b> add quotes around the value.",
            _MUTED, FONT_SECONDARY,
        ), 24)

        # ── Step 3 ────────────────────────────────────────────────────────────
        add(_h("3 · Restart linXiv", FONT_HEADING, _TEXT), SPACE_SM)
        add(_p(
            "The <code>.env</code> file is loaded at startup. Close and reopen the app, "
            "then return to this page — the status banner above will turn green when the key is detected.",
            _TEXT, FONT_BODY,
        ), 32)

        # ── Features table ────────────────────────────────────────────────────
        add(_h("What uses this key", FONT_HEADING, _TEXT), SPACE_MD)
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

    # ── Provider config ────────────────────────────────────────────────────

    def _build_provider_config(self) -> QFrame:
        frame = QFrame()
        frame.setStyleSheet(f"""
            QFrame {{
                background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: {RADIUS_LG}px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """)
        col = QVBoxLayout(frame)
        col.setContentsMargins(DIALOG_PAD, CARD_PAD_H, DIALOG_PAD, CARD_PAD_H)
        col.setSpacing(SPACE_MD)

        # Provider dropdown
        prov_row = QHBoxLayout()
        prov_lbl = QLabel("Provider")
        prov_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; color: {_MUTED}; font-weight: 600; min-width: 80px;")  # TODO: Make more customizable (min-width)
        self._provider_combo = QComboBox()
        self._provider_combo.addItems(list(_PROVIDERS.keys()))
        self._provider_combo.setStyleSheet(_COMBO_STYLE)
        self._provider_combo.currentTextChanged.connect(self._on_provider_changed)
        prov_row.addWidget(prov_lbl)
        prov_row.addWidget(self._provider_combo, stretch=1)
        col.addLayout(prov_row)

        # API key input
        key_row = QHBoxLayout()
        key_lbl = QLabel("API Key")
        key_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; color: {_MUTED}; font-weight: 600; min-width: 80px;")  # TODO: Make more customizable (min-width)
        self._key_input = QLineEdit()
        self._key_input.setEchoMode(QLineEdit.EchoMode.Password)
        self._key_input.setPlaceholderText("Paste your API key here")
        self._key_input.setStyleSheet(_INPUT_STYLE)
        key_row.addWidget(key_lbl)
        key_row.addWidget(self._key_input, stretch=1)
        col.addLayout(key_row)

        # Buttons row
        btn_row = QHBoxLayout()
        btn_row.setSpacing(8)

        self._test_btn = QPushButton("Test connection")
        self._test_btn.setStyleSheet(_BTN_MUTED_STYLE)
        self._test_btn.clicked.connect(self._on_test)
        btn_row.addWidget(self._test_btn)

        self._save_btn = QPushButton("Save")
        self._save_btn.setStyleSheet(_BTN_STYLE)
        self._save_btn.clicked.connect(self._on_save_provider)
        btn_row.addWidget(self._save_btn)

        btn_row.addStretch()
        col.addLayout(btn_row)

        # Status label
        self._provider_status = QLabel("")
        self._provider_status.setWordWrap(True)
        self._provider_status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED};")
        col.addWidget(self._provider_status)

        # Load current state from .env
        self._load_provider_from_env()

        return frame

    def _load_provider_from_env(self) -> None:
        """Read .env to determine current provider and pre-fill API key."""
        active = os.getenv("AI_PROVIDER", "Gemini")
        idx = self._provider_combo.findText(active)
        if idx >= 0:
            self._provider_combo.setCurrentIndex(idx)

        env_var = _PROVIDERS.get(self._provider_combo.currentText(), "")
        current_key = os.getenv(env_var, "")
        if current_key:
            self._key_input.setText(current_key)
            self._provider_status.setText(f"Key loaded from environment ({env_var})")
            self._provider_status.setStyleSheet(f"font-size: 12px; color: {_GREEN};")

    def _on_provider_changed(self, provider_name: str) -> None:
        """Update key field when provider selection changes."""
        env_var = _PROVIDERS.get(provider_name, "")
        current_key = os.getenv(env_var, "")
        self._key_input.setText(current_key)
        self._provider_status.setText("")
        self._provider_status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED};")

    def _on_test(self) -> None:
        """Test connection with the selected provider and entered key."""
        provider_name = self._provider_combo.currentText()
        env_var = _PROVIDERS[provider_name]
        key = self._key_input.text().strip()

        if not key:
            self._provider_status.setText("Please enter an API key first.")
            self._provider_status.setStyleSheet(f"font-size: 12px; color: {_AMBER};")
            return

        self._provider_status.setText("Testing connection...")
        self._provider_status.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED};")
        self._test_btn.setEnabled(False)

        # Set key in env temporarily for provider init
        old_val = os.environ.get(env_var, "")
        os.environ[env_var] = key

        try:
            from AI_tools import GeminiProvider, OpenAIProvider, PaperContent, set_provider
            if provider_name == "Gemini":
                provider = GeminiProvider()
            else:
                provider = OpenAIProvider()
            # Try a minimal call to verify the key works
            test_content = PaperContent(abstract="Test paper: Introduction to Machine Learning")
            provider.tag(test_content)
            set_provider(provider)
            self._provider_status.setText(f"{provider_name} connection successful.")
            self._provider_status.setStyleSheet(f"font-size: 12px; color: {_GREEN};")
        except Exception as exc:
            self._provider_status.setText(f"Connection failed: {exc}")
            self._provider_status.setStyleSheet(f"font-size: 12px; color: #e05c5c;")
            os.environ[env_var] = old_val
        finally:
            self._test_btn.setEnabled(True)

    def _on_save_provider(self) -> None:
        """Save provider choice and API key to .env file."""
        provider_name = self._provider_combo.currentText()
        env_var = _PROVIDERS[provider_name]
        key = self._key_input.text().strip()

        if not key:
            self._provider_status.setText("Please enter an API key to save.")
            self._provider_status.setStyleSheet(f"font-size: 12px; color: {_AMBER};")
            return

        # Read existing .env, update or add the relevant lines
        lines: list[str] = []
        if os.path.isfile(_ENV_PATH):
            with open(_ENV_PATH, "r", encoding="utf-8") as f:
                lines = f.readlines()

        updated_key = False
        updated_provider = False
        for i, line in enumerate(lines):
            stripped = line.strip()
            if stripped.startswith(f"{env_var}="):
                lines[i] = f"{env_var}={key}\n"
                updated_key = True
            elif stripped.startswith("AI_PROVIDER="):
                lines[i] = f"AI_PROVIDER={provider_name}\n"
                updated_provider = True

        if not updated_key:
            lines.append(f"{env_var}={key}\n")
        if not updated_provider:
            lines.append(f"AI_PROVIDER={provider_name}\n")

        with open(_ENV_PATH, "w", encoding="utf-8") as f:
            f.writelines(lines)

        # Update environment
        os.environ[env_var] = key
        os.environ["AI_PROVIDER"] = provider_name

        # Wire up the provider
        try:
            from AI_tools import GeminiProvider, OpenAIProvider, set_provider
            if provider_name == "Gemini":
                set_provider(GeminiProvider())
            else:
                set_provider(OpenAIProvider())
        except Exception:
            pass  # Provider will be initialized on next use

        self._provider_status.setText(f"Saved {provider_name} config to .env")
        self._provider_status.setStyleSheet(f"font-size: 12px; color: {_GREEN};")

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
                border-radius: {RADIUS_LG}px;
                padding: 0px;
            }}
            QLabel {{ border: none; background: transparent; }}
        """)
        row = QVBoxLayout(frame)
        row.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)

        lbl = QLabel(f"{icon}  {text}")
        lbl.setStyleSheet(f"color: {color}; font-size: {FONT_BODY}px;")
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
            border-radius: {RADIUS_LG}px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """)
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(DIALOG_PAD, CARD_PAD_H, DIALOG_PAD, CARD_PAD_H)
    lay.setSpacing(SPACE_SM)

    icon_lbl = QLabel("⚠  Keep your API key safe")
    icon_lbl.setStyleSheet(
        f"font-size: {FONT_SUBHEADING}px; font-weight: bold; color: {_RED}; letter-spacing: 0.02em;"
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
    body.setStyleSheet(f"font-size: {FONT_BODY}px; color: #ddbbbb; line-height: 1.5;")

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
            border-radius: {RADIUS_MD}px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """)
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(CARD_PAD_H, SPACE_MD, CARD_PAD_H, SPACE_MD)
    lbl = QLabel(text)
    lbl.setStyleSheet(f"font-family: 'Consolas', monospace; font-size: {FONT_BODY}px; color: {_GREEN};")
    lay.addWidget(lbl)
    return frame


def _link_card(title: str, url: str, desc: str) -> QFrame:
    frame = QFrame()
    frame.setStyleSheet(f"""
        QFrame {{
            background: {_PANEL};
            border: 1px solid {_BORDER};
            border-radius: {RADIUS_LG}px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """)
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)
    lay.setSpacing(SPACE_XS)

    title_lbl = QLabel(title)
    title_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; font-weight: 600; color: {_ACCENT};")

    url_lbl = QLabel(url)
    url_lbl.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED}; font-family: 'Consolas', monospace;")

    desc_lbl = QLabel(desc)
    desc_lbl.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_TEXT};")

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
            border-radius: {RADIUS_MD}px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """)
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(CARD_PAD_H, SPACE_SM, CARD_PAD_H, SPACE_SM)
    lay.setSpacing(SPACE_XS)

    name_lbl = QLabel(name)
    name_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; font-weight: 600; color: {_TEXT};")

    desc_lbl = QLabel(desc)
    desc_lbl.setStyleSheet(f"font-size: {FONT_SECONDARY}px; color: {_MUTED};")
    desc_lbl.setWordWrap(True)

    lay.addWidget(name_lbl)
    lay.addWidget(desc_lbl)
    return frame
