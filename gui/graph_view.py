import os
import json
from PyQt6.QtWebEngineCore import QWebEnginePage
from PyQt6.QtWebEngineWidgets import QWebEngineView
from PyQt6.QtCore import QUrl, pyqtSignal


class _GraphPage(QWebEnginePage):
    """Custom page that intercepts JS console messages for the graph bridge."""

    console_message_received = pyqtSignal(str)

    def javaScriptConsoleMessage(self, level, message, line, source):  # pyright: ignore[reportIncompatibleMethodOverride] — Qt override
        self.console_message_received.emit(message)


class GraphView(QWebEngineView):
    node_clicked = pyqtSignal(str)  # emits paper_id when a paper node is clicked
    selection_changed = pyqtSignal(int)  # emits count of selected nodes

    def __init__(self, parent=None):
        super().__init__(parent)
        self._loaded = False
        self._pending_nodes: list = []
        self._pending_edges: list = []
        self._pending_categories: list = []
        self._pending_tags: list = []

        self._page = _GraphPage(self)
        self._page.console_message_received.connect(self._on_console_message)
        self.setPage(self._page)

        self.loadFinished.connect(self._on_load_finished)
        html_path = os.path.abspath(os.path.join(os.path.dirname(__file__), "web", "graph.html"))
        self.load(QUrl.fromLocalFile(html_path))

    def set_graph_data(self, nodes: list, edges: list) -> None:
        self._pending_nodes = nodes
        self._pending_edges = edges
        if self._loaded:
            self._push()

    def set_filter_options(self, categories: list[str], tags: list[str]) -> None:
        """Populate the in-graph filter datalists with available categories and tags."""
        self._pending_categories = categories
        self._pending_tags = tags
        if self._loaded:
            self._push_filter_options()

    def _on_load_finished(self, ok: bool) -> None:
        if ok:
            self._loaded = True
            self._push()
            self._push_filter_options()

    def _push(self) -> None:
        data = json.dumps({"nodes": self._pending_nodes, "edges": self._pending_edges})
        self.page().runJavaScript(f"loadGraph({data})")  # pyright: ignore[reportOptionalMemberAccess]

    def _push_filter_options(self) -> None:
        cats = json.dumps(self._pending_categories)
        tags = json.dumps(self._pending_tags)
        self.page().runJavaScript(f"setFilterOptions({cats}, {tags})")  # pyright: ignore[reportOptionalMemberAccess]

    def run_js(self, code: str) -> None:
        """Run arbitrary JavaScript in the graph page."""
        if self._loaded:
            self.page().runJavaScript(code)  # pyright: ignore[reportOptionalMemberAccess]

    def filter_graph(self, opts: dict) -> None:
        """Call JS filterGraph with the given options dict."""
        self.run_js(f"filterGraph({json.dumps(opts)})")

    def highlight_node(self, node_id: str | None) -> None:
        """Highlight a single node by id, dimming all others."""
        self.run_js(f"highlightNode({json.dumps(node_id)})")

    def clear_filters(self) -> None:
        """Reset all in-graph filters to their default state."""
        self.run_js("clearFilters()")

    def select_all_papers(self) -> None:
        """Select all currently visible paper nodes."""
        self.run_js("selectAllPapers()")

    def clear_selection(self) -> None:
        """Deselect all paper nodes."""
        self.run_js("clearSelection()")

    def get_selected_paper_data(self, callback) -> None:
        """Retrieve data for selected papers from JS. Calls callback(dict) with result."""
        if self._loaded:
            self.page().runJavaScript(  # pyright: ignore[reportOptionalMemberAccess]
                "getSelectedPaperData()",
                lambda result: callback(json.loads(result) if isinstance(result, str) else {"papers": [], "edges": []}),
            )

    def _on_console_message(self, message: str) -> None:
        """Parse JS console messages and emit signals for graph events."""
        if message.startswith("GRAPHVIEW_PAPER_CLICKED:"):
            paper_id = message[len("GRAPHVIEW_PAPER_CLICKED:"):]
            self.node_clicked.emit(paper_id)
        elif message.startswith("GRAPHVIEW_SELECTION_COUNT:"):
            count = int(message[len("GRAPHVIEW_SELECTION_COUNT:"):])
            self.selection_changed.emit(count)
