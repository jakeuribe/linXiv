from PyQt6.QtWidgets import QMainWindow
from .graph_view import GraphView
from db import get_graph_data


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("arXiv Paper Graph")
        self.resize(1400, 900)
        self._graph_view = GraphView(self)
        self.setCentralWidget(self._graph_view)
        self._load_graph()

    def _load_graph(self) -> None:
        nodes, edges = get_graph_data()
        self._graph_view.set_graph_data(nodes, edges)

    def refresh(self) -> None:
        """Reload graph data from the DB."""
        self._load_graph()
