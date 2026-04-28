from __future__ import annotations

import os
from pathlib import Path

from PyQt6.QtCore import Qt, QThread, pyqtSignal
from PyQt6.QtGui import QFontMetrics
from PyQt6.QtWidgets import (
    QCheckBox,
    QFileDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSizePolicy,
    QVBoxLayout,
    QWidget,
)

from gui.theme import ACCENT as _ACCENT, BORDER as _BORDER, MUTED as _MUTED
from gui.theme import PANEL as _PANEL, TEXT as _TEXT
from gui.theme import (
    BTN_H_SM,
    CARD_PAD_H, CARD_PAD_V,
    FONT_BODY, FONT_TERTIARY,
    RADIUS_LG, RADIUS_SM,
    SPACE_MD, SPACE_XS,
)
from storage.db import set_has_pdf, set_pdf_path

_GREEN = "#4caf7d"
_BLUE  = "#5b8dee"
_RED   = "#e05c5c"

_PDF_DIR = Path(__file__).parent.parent / "pdfs"

CAT_COLORS: dict[str, str] = {
    "cs":      "#4a7add",
    "physics": "#bf8a5b",
    "math":    "#bf5b8a",
    "stat":    "#8abf5b",
    "eess":    "#9b6de0",
    "econ":    "#a0b05b",
    "q-bio":   "#5ba080",
    "q-fin":   "#b0905b",
}


# ── Download worker ───────────────────────────────────────────────────────────

class _DownloadWorker(QThread):
    finished     = pyqtSignal(str, int, str)  # paper_id, version, local_path
    failed       = pyqtSignal(str, int, str)  # paper_id, version, error
    rate_limited = pyqtSignal(str, int)       # paper_id, version — emitted before each retry sleep

    _RETRY_DELAYS = (5, 15, 30)  # seconds before each retry on HTTP 429

    def __init__(self, paper_id: str, version: int) -> None:
        super().__init__()
        self.paper_id = paper_id
        self.version  = version

    def run(self) -> None:
        import time
        import urllib.error
        import urllib.request
        from sources.fetch_paper_metadata import _check_ratelimit, _record_ratelimit
        _PDF_DIR.mkdir(parents=True, exist_ok=True)
        dest = _PDF_DIR / f"{self.paper_id}v{self.version}.pdf"
        url  = f"https://arxiv.org/pdf/{self.paper_id}v{self.version}"
        req  = urllib.request.Request(url, headers={"User-Agent": "linXiv/1.0"})
        _check_ratelimit()  # wait out any rate limit recorded by prior API calls
        for delay in (None, *self._RETRY_DELAYS):
            if delay is not None:
                self.rate_limited.emit(self.paper_id, self.version)
                time.sleep(delay)
            try:
                with urllib.request.urlopen(req, timeout=30) as resp:
                    dest.write_bytes(resp.read())
                self.finished.emit(self.paper_id, self.version, str(dest))
                return
            except urllib.error.HTTPError as e:
                if e.code != 429:
                    self.failed.emit(self.paper_id, self.version, str(e))
                    return
                _record_ratelimit()  # inform API calls that arXiv is rate limiting
            except Exception as e:
                self.failed.emit(self.paper_id, self.version, str(e))
                return
        self.failed.emit(self.paper_id, self.version, "rate limited")


# ── Elided label (row / compact modes) ───────────────────────────────────────

class ElidedLabel(QLabel):
    _MAX_LINES = 3

    def __init__(self, text: str = "", parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._full_text = text
        self.setWordWrap(False)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        self._relayout()

    def setText(self, text: str) -> None:
        self._full_text = text
        self._relayout()

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        self._relayout()

    def _relayout(self) -> None:
        if not self._full_text:
            super().setText("")
            return
        w = self.width()
        if w <= 0:
            super().setText(self._full_text)
            return
        fm = self.fontMetrics()
        lines = self._wrap(self._full_text, fm, w)
        if len(lines) > self._MAX_LINES:
            kept = lines[: self._MAX_LINES - 1]
            remaining = " ".join(lines[self._MAX_LINES - 1 :])
            kept.append(fm.elidedText(remaining, Qt.TextElideMode.ElideRight, w))
            lines = kept
        lines = [
            fm.elidedText(ln, Qt.TextElideMode.ElideRight, w)
            if fm.horizontalAdvance(ln) > w else ln
            for ln in lines
        ]
        super().setText("\n".join(lines))

    @staticmethod
    def _wrap(text: str, fm: QFontMetrics, width: int) -> list[str]:
        words = text.split()
        if not words:
            return []
        lines, current = [], words[0]
        for word in words[1:]:
            candidate = current + " " + word
            if fm.horizontalAdvance(candidate) <= width:
                current = candidate
            else:
                lines.append(current)
                current = word
        lines.append(current)
        return lines


# ── Paper card ────────────────────────────────────────────────────────────────

class PaperCard(QFrame):
    """Paper widget with three modes driven by constructor kwargs:

    card    (pdf_window set)  — checkbox, stripe, full metadata, PDF button
    row     (project_id set)  — elided title, notes button
    compact (neither set)     — elided title, date/category line, no action
    """

    selection_toggled = pyqtSignal(str, bool)   # paper_id, is_selected — card mode only
    clicked           = pyqtSignal(object)       # emits DB row — all modes
    double_clicked    = pyqtSignal(object)       # emits DB row — row and compact modes

    _base_style = f"""
        QFrame#paperCard {{
            background: {_PANEL}; border: 1px solid {_BORDER}; border-radius: {RADIUS_LG}px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """
    _sel_style = f"""
        QFrame#paperCard {{
            background: {_PANEL}; border: 1px solid {_ACCENT}; border-radius: {RADIUS_LG}px;
        }}
        QLabel {{ border: none; background: transparent; }}
    """

    def __init__(
        self,
        row,
        *,
        pdf_window=None,
        project_id: int | None = None,
        parent: QWidget | None = None,
    ) -> None:
        super().__init__(parent)
        self._row        = row
        self._pdf_window = pdf_window
        self._project_id = project_id
        self._worker: _DownloadWorker | None = None
        self._pdf_btn:  QPushButton | None = None
        self._selected  = False
        self._row_mode  = project_id is not None
        self._card_mode = not self._row_mode and pdf_window is not None

        self.setObjectName("paperCard")
        self.setStyleSheet(self._base_style)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        if not self._card_mode:
            self.setCursor(Qt.CursorShape.PointingHandCursor)

        outer = QHBoxLayout(self)
        outer.setSpacing(0)

        if self._card_mode:
            outer.setContentsMargins(10, 0, CARD_PAD_H - 10, 0)
            self._build_card(outer, row)
        elif self._row_mode:
            outer.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)
            outer.setSpacing(SPACE_MD)
            self._build_row(outer, row)
        else:
            outer.setContentsMargins(CARD_PAD_H, CARD_PAD_V, CARD_PAD_H, CARD_PAD_V)
            outer.setSpacing(SPACE_XS)
            self._build_compact(outer, row)

    # ── Mode builders ─────────────────────────────────────────────────────────

    def _build_card(self, outer: QHBoxLayout, row) -> None:
        outer.addSpacing(CARD_PAD_H)

        self._chk = QCheckBox()
        self._chk.setFixedSize(44, 44)
        self._chk.setStyleSheet(
            "QCheckBox { background: transparent; }"
            " QCheckBox::indicator { width: 14px; height: 14px; }"
        )
        self._chk.stateChanged.connect(self._on_checkbox)
        outer.addWidget(self._chk)

        cat   = (row["category"] or "").split(".")[0] if row["category"] else ""
        color = CAT_COLORS.get(row["category"] or "", CAT_COLORS.get(cat, _ACCENT))
        stripe = QWidget()
        stripe.setFixedWidth(4)
        stripe.setStyleSheet(f"background: {color}; border-radius: 0;")
        outer.addWidget(stripe)

        body = QVBoxLayout()
        body.setContentsMargins(CARD_PAD_H, CARD_PAD_V, 0, CARD_PAD_V)
        body.setSpacing(SPACE_XS)

        title_lbl = QLabel(row["title"] or "(untitled)")
        title_lbl.setWordWrap(True)
        title_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; font-weight: 600; color: {_TEXT};")
        body.addWidget(title_lbl)

        authors: list[str] = row["authors"] or []
        if authors:
            shown = ", ".join(authors[:3])
            if len(authors) > 3:
                shown += f" +{len(authors) - 3} more"
            auth_lbl = QLabel(shown)
            auth_lbl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
            body.addWidget(auth_lbl)

        date_str = row["published"].isoformat() if row["published"] else ""
        cat_str  = row["category"] or ""
        meta     = "  ·  ".join(filter(None, [date_str, cat_str]))
        if meta:
            ml = QLabel(meta)
            ml.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
            body.addWidget(ml)

        tags: list[str] = row["tags"] or []
        if tags:
            tl = QLabel("  ".join(f"#{t}" for t in tags[:6]))
            tl.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_ACCENT};")
            body.addWidget(tl)

        outer.addLayout(body, stretch=1)

        self._pdf_btn = QPushButton()
        self._pdf_btn.setFixedSize(116, BTN_H_SM)
        self._pdf_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._refresh_pdf_btn()
        self._pdf_btn.clicked.connect(self._on_pdf_action)
        outer.addWidget(self._pdf_btn, alignment=Qt.AlignmentFlag.AlignVCenter)

    def _build_row(self, outer: QHBoxLayout, row) -> None:
        title_lbl = ElidedLabel(row["title"] or "(untitled)")
        title_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; color: {_TEXT};")
        outer.addWidget(title_lbl, stretch=1)

        self._note_btn = QPushButton(self._note_label())
        self._note_btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self._note_btn.setStyleSheet(f"""
            QPushButton {{
                background: transparent; border: 1px solid {_BORDER}; border-radius: {RADIUS_SM}px;
                color: {_MUTED}; font-size: {FONT_TERTIARY}px; padding: 3px 10px;
            }}
            QPushButton:hover {{ border-color: {_ACCENT}; color: {_ACCENT}; }}
        """)
        self._note_btn.clicked.connect(self._on_open_notes)
        outer.addWidget(self._note_btn)

        if self._pdf_window is not None:
            self._pdf_btn = QPushButton()
            self._pdf_btn.setFixedSize(116, BTN_H_SM)
            self._pdf_btn.setCursor(Qt.CursorShape.PointingHandCursor)
            self._refresh_pdf_btn()
            self._pdf_btn.clicked.connect(self._on_pdf_action)
            outer.addWidget(self._pdf_btn, alignment=Qt.AlignmentFlag.AlignVCenter)

    def _build_compact(self, outer: QHBoxLayout, row) -> None:
        body = QVBoxLayout()
        body.setContentsMargins(0, 0, 0, 0)
        body.setSpacing(SPACE_XS)

        title_lbl = ElidedLabel(row["title"] or "(untitled)")
        title_lbl.setStyleSheet(f"font-size: {FONT_BODY}px; font-weight: 600; color: {_TEXT};")
        body.addWidget(title_lbl)

        date_str = row["published"].isoformat() if row["published"] else ""
        cat_str  = row["category"] or ""
        meta     = "  ·  ".join(filter(None, [date_str, cat_str]))
        if meta:
            ml = QLabel(meta)
            ml.setStyleSheet(f"font-size: {FONT_TERTIARY}px; color: {_MUTED};")
            body.addWidget(ml)

        outer.addLayout(body, stretch=1)

    # ── Selection (card mode) ─────────────────────────────────────────────────

    def set_selected(self, selected: bool) -> None:
        if self._selected == selected:
            return
        self._selected = selected
        if self._card_mode:
            self._chk.blockSignals(True)
            self._chk.setChecked(selected)
            self._chk.blockSignals(False)
        self.setStyleSheet(self._sel_style if selected else self._base_style)

    def is_selected(self) -> bool:
        return self._selected

    def _on_checkbox(self, state: int) -> None:
        checked = state == Qt.CheckState.Checked.value
        self._selected = checked
        self.setStyleSheet(self._sel_style if checked else self._base_style)
        self.selection_toggled.emit(self._row["paper_id"], checked)

    def mouseDoubleClickEvent(self, event) -> None:
        if event.button() == Qt.MouseButton.LeftButton and not self._card_mode:
            self.double_clicked.emit(self._row)
        super().mouseDoubleClickEvent(event)

    def mousePressEvent(self, event) -> None:
        if event.button() == Qt.MouseButton.LeftButton:
            if event.modifiers() & Qt.KeyboardModifier.ControlModifier:
                self.set_selected(not self._selected)
                self.selection_toggled.emit(self._row["paper_id"], self._selected)
                return
            self.clicked.emit(self._row)
        super().mousePressEvent(event)

    # ── Accessors ─────────────────────────────────────────────────────────────

    def paper_id(self) -> str:
        return self._row["paper_id"]

    def is_arxiv(self) -> bool:
        src = self._row["source"] if "source" in self._row.keys() else "arxiv"
        return (src or "arxiv") == "arxiv"

    # ── PDF (card mode) ───────────────────────────────────────────────────────

    def local_pdf_path(self) -> str | None:
        p = self._row["pdf_path"] if "pdf_path" in self._row.keys() else None
        if p and os.path.isfile(p):
            return p
        std = _PDF_DIR / f"{self._row['paper_id']}v{self._row['version']}.pdf"
        return str(std) if std.is_file() else None

    def _refresh_pdf_btn(self) -> None:
        if self._pdf_btn is None:
            return
        path = self.local_pdf_path()
        if path:
            self._pdf_btn.setText("Open PDF")
            self._pdf_btn.setStyleSheet(f"""
                QPushButton {{ background: transparent; border: 1px solid {_GREEN};
                    border-radius: {RADIUS_SM}px; color: {_GREEN}; font-size: {FONT_TERTIARY}px; }}
                QPushButton:hover {{ background: #1a2e1f; }}
            """)
        elif self.is_arxiv():
            self._pdf_btn.setText("Download PDF")
            self._pdf_btn.setStyleSheet(f"""
                QPushButton {{ background: transparent; border: 1px solid {_BLUE};
                    border-radius: {RADIUS_SM}px; color: {_BLUE}; font-size: {FONT_TERTIARY}px; }}
                QPushButton:hover {{ background: #1a1f2e; }}
            """)
        else:
            self._pdf_btn.setText("Link PDF")
            self._pdf_btn.setStyleSheet(f"""
                QPushButton {{ background: transparent; border: 1px solid {_BORDER};
                    border-radius: {RADIUS_SM}px; color: {_MUTED}; font-size: {FONT_TERTIARY}px; }}
                QPushButton:hover {{ background: #1a1a2a; }}
            """)

    def _on_pdf_action(self) -> None:
        if self._pdf_window is None:
            return
        path = self.local_pdf_path()
        if path:
            self._pdf_window.load_pdf(path)
        elif self.is_arxiv():
            self._start_download()
        else:
            self._link_pdf()

    def start_download_if_needed(self) -> None:
        """Called by bulk download. No-op if not card mode, already has PDF, or not arXiv."""
        if not self._card_mode or self.local_pdf_path() or not self.is_arxiv():
            return
        self._start_download()

    def _start_download(self) -> None:
        if self._pdf_btn is None:
            return
        self._pdf_btn.setText("Downloading…")
        self._pdf_btn.setEnabled(False)
        pid, ver = self._row["paper_id"], self._row["version"]
        self._worker = _DownloadWorker(pid, ver)
        self._worker.finished.connect(self._on_download_done)
        self._worker.failed.connect(self._on_download_failed)
        self._worker.rate_limited.connect(self._on_download_rate_limited)
        self._worker.start()

    def _on_download_done(self, paper_id: str, version: int, path: str) -> None:
        set_pdf_path(paper_id, path)
        set_has_pdf(paper_id, version, True)
        if self._pdf_btn is not None:
            self._pdf_btn.setEnabled(True)
        self._refresh_pdf_btn()
        if self._pdf_window is not None:
            self._pdf_window.load_pdf(path)

    def _on_download_rate_limited(self, _pid: str, _ver: int) -> None:
        if self._pdf_btn is not None:
            self._pdf_btn.setText("Rate limited — retrying…")

    def _on_download_failed(self, _pid: str, _ver: int, err: str) -> None:
        if self._pdf_btn is None:
            return
        self._pdf_btn.setEnabled(True)
        self._pdf_btn.setText("Rate limited — retry?" if err == "rate limited" else "Failed — retry?")
        self._pdf_btn.setStyleSheet(f"""
            QPushButton {{ background: transparent; border: 1px solid {_RED};
                border-radius: {RADIUS_SM}px; color: {_RED}; font-size: {FONT_TERTIARY}px; }}
        """)

    def _link_pdf(self) -> None:
        path, _ = QFileDialog.getOpenFileName(self, "Link PDF", "", "PDF Files (*.pdf)")
        if not path:
            return
        pid, ver = self._row["paper_id"], self._row["version"]
        set_pdf_path(pid, path)
        set_has_pdf(pid, ver, True)
        self._refresh_pdf_btn()

    # ── Notes (row mode) ──────────────────────────────────────────────────────

    def _note_count(self) -> int:
        if self._project_id is None:
            return 0
        try:
            from storage.notes import count_paper_notes
            return count_paper_notes(self._row["paper_id"], self._project_id)
        except Exception:
            return 0

    def _note_label(self) -> str:
        n = self._note_count()
        return f"📝 {n} {'note' if n == 1 else 'notes'}"

    def _on_open_notes(self) -> None:
        if self._project_id is None:
            return
        from gui.projects.page import NotesDialog
        host = self.window()
        dlg = NotesDialog(
            self._row["paper_id"],
            self._project_id,
            self._row["title"] or self._row["paper_id"],
            host,
        )
        dlg.exec()
        if host is not None:
            host.raise_()
            host.activateWindow()
        self._note_btn.setText(self._note_label())
