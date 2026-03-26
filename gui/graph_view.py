import os
import json
from PyQt6.QtWebEngineWidgets import QWebEngineView
from PyQt6.QtCore import QUrl


class GraphView(QWebEngineView):
    def __init__(self, parent=None):
        super().__init__(parent)
        self._loaded = False
        self._pending_nodes: list = []
        self._pending_edges: list = []
        self.loadFinished.connect(self._on_load_finished)
        html_path = os.path.abspath(os.path.join(os.path.dirname(__file__), "web", "graph.html"))
        self.load(QUrl.fromLocalFile(html_path))

    def set_graph_data(self, nodes: list, edges: list) -> None:
        self._pending_nodes = nodes
        self._pending_edges = edges
        if self._loaded:
            self._push()

    def _on_load_finished(self, ok: bool) -> None:
        if ok:
            self._loaded = True
            self._push()

    def _push(self) -> None:
        data = json.dumps({"nodes": self._pending_nodes, "edges": self._pending_edges})
        self.page().runJavaScript(f"loadGraph({data})")
