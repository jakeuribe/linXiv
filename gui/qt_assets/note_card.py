from __future__ import annotations

from PyQt6.QtWidgets import QFrame, QHBoxLayout, QLabel, QPushButton, QVBoxLayout

from gui.theme import ACCENT as _ACCENT, BORDER as _BORDER, MUTED as _MUTED, PANEL as _PANEL
from gui.theme import CARD_PAD_H, CARD_PAD_V, FONT_TERTIARY, NOTE_HEIGHT, RADIUS_LG, SPACE_XS
from gui.qt_assets.styles import BTN_NOTE_ACCENT, BTN_NOTE_DELETE
from gui.views import MarkdownView


def note_card(self, note, proj_names: dict[int, str], *, on_delete=None) -> QFrame:
    card = QFrame()
    card.setStyleSheet(f"""
        QFrame {{ background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: {RADIUS_LG}px; }}
        QLabel {{ border: none; background: transparent; }}
    """)
    col = QVBoxLayout(card)
    col.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)
    col.setSpacing(SPACE_XS)

    hdr = QHBoxLayout()
    if note.project_id is not None and note.project_id in proj_names:
        chip = QLabel(f"📁 {proj_names[note.project_id]}")
        chip.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_ACCENT};")
        hdr.addWidget(chip)
    elif note.project_id is None:
        standalone = QLabel("Standalone note")
        standalone.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
        hdr.addWidget(standalone)

    hdr.addStretch()
    if note.created_at:
        date_lbl = QLabel(note.created_at.strftime("%Y-%m-%d"))
        date_lbl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
        hdr.addWidget(date_lbl)

    edit_btn = QPushButton("Edit")
    edit_btn.setStyleSheet(BTN_NOTE_ACCENT)
    edit_btn.clicked.connect(lambda _, n=note: self._edit_note(n))
    hdr.addWidget(edit_btn)

    if on_delete is not None:
        del_btn = QPushButton("Delete")
        del_btn.setStyleSheet(BTN_NOTE_DELETE)
        del_btn.clicked.connect(lambda _, fn=on_delete: fn())
        hdr.addWidget(del_btn)

    col.addLayout(hdr)

    md = MarkdownView()
    md.set_title(note.title or "Untitled")
    md.set_content(note.content or "")
    md.setFixedHeight(NOTE_HEIGHT)
    col.addWidget(md)

    return card
