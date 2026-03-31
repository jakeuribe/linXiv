import os
import json
from PyQt6.QtWebEngineWidgets import QWebEngineView
from PyQt6.QtGui import QColor
from PyQt6.QtCore import QUrl


class TexView(QWebEngineView):
    """QWebEngineView that renders text containing LaTeX math via KaTeX."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self._loaded = False
        self._pending = ""
        self.page().setBackgroundColor(QColor("transparent"))  # pyright: ignore[reportOptionalMemberAccess]
        self.loadFinished.connect(self._on_load_finished)
        html_path = os.path.abspath(
            os.path.join(os.path.dirname(__file__), "web", "tex_view.html")
        )
        self.load(QUrl.fromLocalFile(html_path))

    def set_content(self, text: str) -> None:
        self._pending = text
        if self._loaded:
            self._push()

    def _on_load_finished(self, ok: bool) -> None:
        if ok:
            self._loaded = True
            self._push()

    def _push(self) -> None:
        self.page().runJavaScript(f"setContent({json.dumps(self._pending)})")  # pyright: ignore[reportOptionalMemberAccess]
