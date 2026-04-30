"""Shared QPushButton stylesheet constants.

Import these instead of redefining locally:

    from gui.qt_assets.styles import BTN_PRIMARY, BTN_MUTED, BTN_SUCCESS
    from gui.qt_assets.styles import BTN_PANEL, BTN_OUTLINE, BTN_DANGER
    from gui.qt_assets.styles import BTN_PDF_OPEN, BTN_PDF_DOWNLOAD, BTN_PDF_LINK, BTN_PDF_ERROR
    from gui.qt_assets.styles import BTN_NOTE_OPEN, BTN_NOTE_EDIT, BTN_NOTE_DELETE
    from gui.qt_assets.styles import btn_colored_outline
"""
from __future__ import annotations

from gui.theme import ACCENT, BORDER, MUTED, PANEL, TEXT
from gui.theme import FONT_BODY, FONT_SECONDARY, FONT_TERTIARY
from gui.theme import RADIUS_MD, RADIUS_SM, SPACE_SM

# ── Primary / dialog buttons ──────────────────────────────────────────────────

BTN_PRIMARY = f"""
    QPushButton {{
        background: {ACCENT}; border: none; border-radius: {RADIUS_MD}px;
        color: #fff; font-size: {FONT_BODY}px; font-weight: 600; padding: {SPACE_SM}px 20px;
    }}
    QPushButton:hover   {{ background: #7aa3f5; }}
    QPushButton:pressed {{ background: #4a7add; }}
    QPushButton:disabled {{ background: #2a2a4a; color: {MUTED}; }}
"""

BTN_MUTED = f"""
    QPushButton {{
        background: transparent; border: 1px solid {BORDER}; border-radius: {RADIUS_MD}px;
        color: {MUTED}; font-size: {FONT_BODY}px; padding: {SPACE_SM}px 20px;
    }}
    QPushButton:hover {{ border-color: {TEXT}; color: {TEXT}; }}
"""

BTN_SUCCESS = f"""
    QPushButton {{
        background: #4caf7d; border: none; border-radius: {RADIUS_MD}px;
        color: #fff; font-size: {FONT_BODY}px; font-weight: 600; padding: {SPACE_SM}px 20px;
    }}
    QPushButton:hover   {{ background: #5dcc8f; }}
    QPushButton:pressed {{ background: #3a9e60; }}
    QPushButton:disabled {{ background: #2a2a4a; color: {MUTED}; }}
"""

# ── General outline buttons ───────────────────────────────────────────────────

BTN_PANEL = f"""
    QPushButton {{
        background: {PANEL}; border: 1px solid {BORDER};
        border-radius: {RADIUS_MD}px; color: {TEXT}; font-size: {FONT_SECONDARY}px; padding: 4px 14px;
    }}
    QPushButton:hover {{ background: #2a2a4a; }}
"""

BTN_OUTLINE = f"""
    QPushButton {{
        background: transparent; border: 1px solid {BORDER};
        border-radius: {RADIUS_MD}px; color: {TEXT}; font-size: {FONT_SECONDARY}px; padding: 4px 14px;
    }}
    QPushButton:hover {{ background: #2a2a4a; }}
"""

BTN_DANGER = f"""
    QPushButton {{
        background: transparent; border: 1px solid #e05c5c;
        border-radius: {RADIUS_MD}px; color: #e05c5c; font-size: {FONT_SECONDARY}px; padding: 4px 14px;
    }}
    QPushButton:hover {{ background: #2a1a1a; }}
"""

# ── PDF card button states (RADIUS_SM, FONT_TERTIARY, no padding) ─────────────

BTN_PDF_OPEN = f"""
    QPushButton {{ background: transparent; border: 1px solid #4caf7d;
        border-radius: {RADIUS_SM}px; color: #4caf7d; font-size: {FONT_TERTIARY}px; }}
    QPushButton:hover {{ background: #1a2e1f; }}
"""

BTN_PDF_DOWNLOAD = f"""
    QPushButton {{ background: transparent; border: 1px solid #5b8dee;
        border-radius: {RADIUS_SM}px; color: #5b8dee; font-size: {FONT_TERTIARY}px; }}
    QPushButton:hover {{ background: #1a1f2e; }}
"""

BTN_PDF_LINK = f"""
    QPushButton {{ background: transparent; border: 1px solid {BORDER};
        border-radius: {RADIUS_SM}px; color: {MUTED}; font-size: {FONT_TERTIARY}px; }}
    QPushButton:hover {{ background: #1a1a2a; }}
"""

BTN_PDF_ERROR = f"""
    QPushButton {{ background: transparent; border: 1px solid #e05c5c;
        border-radius: {RADIUS_SM}px; color: #e05c5c; font-size: {FONT_TERTIARY}px; }}
"""

# ── Note / annotation buttons (RADIUS_SM, FONT_TERTIARY) ─────────────────────

BTN_NOTE_OPEN = f"""
    QPushButton {{
        background: transparent; border: 1px solid {BORDER}; border-radius: {RADIUS_SM}px;
        color: {MUTED}; font-size: {FONT_TERTIARY}px; padding: 3px 10px;
    }}
    QPushButton:hover {{ border-color: {ACCENT}; color: {ACCENT}; }}
"""

BTN_NOTE_EDIT = f"""
    QPushButton {{
        background: transparent; border: 1px solid {BORDER}; border-radius: {RADIUS_SM}px;
        color: {MUTED}; font-size: {FONT_TERTIARY}px; padding: 2px 8px;
    }}
    QPushButton:hover {{ border-color: {ACCENT}; color: {ACCENT}; }}
"""

BTN_NOTE_DELETE = f"""
    QPushButton {{
        background: transparent; border: 1px solid #e05c5c; border-radius: {RADIUS_SM}px;
        color: #e05c5c; font-size: {FONT_TERTIARY}px; padding: 2px 8px;
    }}
    QPushButton:hover {{ border-color: #ff7070; color: #ff7070; }}
"""

# ── Helper for dynamically colored outline buttons ────────────────────────────

def btn_colored_outline(color: str, *, hover_bg: str = "rgba(91,141,238,0.08)") -> str:
    """Transparent-bg button with a single color for both border and text."""
    return f"""
        QPushButton {{ background: transparent; border: 1px solid {color};
            border-radius: {RADIUS_MD}px; color: {color}; font-size: {FONT_SECONDARY}px; padding: 4px 14px; }}
        QPushButton:hover {{ background: {hover_bg}; }}
    """
