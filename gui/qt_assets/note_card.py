from __future__ import annotations

from PyQt6.QtWidgets import QFrame, QHBoxLayout, QLabel, QPushButton, QVBoxLayout

from gui.theme import ACCENT as _ACCENT, BORDER as _BORDER, MUTED as _MUTED, PANEL as _PANEL
from gui.theme import CARD_PAD_H, CARD_PAD_V, FONT_TERTIARY, NOTE_HEIGHT, RADIUS_LG, SPACE_XS
from gui.qt_assets.styles import BTN_GHOST
from gui.views import MarkdownView


def _note_card(self, note, proj_names: dict[int, str]) -> QFrame:
        card = QFrame()
        card.setStyleSheet(f"""
            QFrame {{ background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: {RADIUS_LG}px; }}
            QLabel {{ border: none; background: transparent; }}
        """)
        col = QVBoxLayout(card)
        col.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)
        col.setSpacing(SPACE_XS)

        # Note header: project chip + date + edit button
        hdr = QHBoxLayout()
        if note.project_id is not None:
            proj_name = proj_names.get(note.project_id, f"Project {note.project_id}")
            chip = QLabel(f"📁 {proj_name}")
            chip.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_ACCENT};")
            hdr.addWidget(chip)
        else:
            standalone = QLabel("Standalone note")
            standalone.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
            hdr.addWidget(standalone)

        hdr.addStretch()
        if note.created_at:
            date_lbl = QLabel(note.created_at.strftime("%Y-%m-%d"))
            date_lbl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
            hdr.addWidget(date_lbl)

        edit_btn = QPushButton("Edit")
        edit_btn.setStyleSheet(BTN_GHOST)
        edit_btn.clicked.connect(lambda _, n=note: self._edit_note(n))
        hdr.addWidget(edit_btn)
        col.addLayout(hdr)

        md = MarkdownView()
        md.set_title(note.title or "Untitled")
        md.set_content(note.content or "")
        md.setFixedHeight(NOTE_HEIGHT)
        col.addWidget(md)

        return card
