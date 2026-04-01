import os
import json
from PyQt6.QtWebEngineWidgets import QWebEngineView
from PyQt6.QtGui import QColor
from PyQt6.QtCore import QUrl

try:
    import markdown as _md

    def _render(text: str) -> str:
        return _md.markdown(text, extensions=["fenced_code", "nl2br"])
except ImportError:
    import html as _html

    def _render(text: str) -> str:
        return "<pre>" + _html.escape(text) + "</pre>"


class MarkdownView(QWebEngineView):
    """QWebEngineView that renders Markdown content as styled HTML."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self._loaded = False
        self._pending = ""
        self.page().setBackgroundColor(QColor("transparent"))  # pyright: ignore[reportOptionalMemberAccess]
        self.loadFinished.connect(self._on_load_finished)
        html_path = os.path.abspath(
            os.path.join(os.path.dirname(__file__), "web", "markdown.html")
        )
        self.load(QUrl.fromLocalFile(html_path))

    def set_content(self, text: str) -> None:
        """Convert Markdown text to HTML and display."""
        self._pending = _render(text or "")
        if self._loaded:
            self._push()

    def _on_load_finished(self, ok: bool) -> None:
        if ok:
            self._loaded = True
            self._push()

    def _push(self) -> None:
        self.page().runJavaScript(f"setContent({json.dumps(self._pending)})")  # pyright: ignore[reportOptionalMemberAccess]
