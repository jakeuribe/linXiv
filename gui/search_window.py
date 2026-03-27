from PyQt6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QLineEdit, QPushButton, QListWidget, QListWidgetItem,
    QLabel, QSplitter
)
from PyQt6.QtCore import Qt, QThread, pyqtSignal
import arxiv
from fetch_paper_metadata import search_papers
from downloads import download_pdf
from .tex_view import TexView
from .pdf_window import PdfWindow


class _SearchWorker(QThread):
    done = pyqtSignal(list)

    def __init__(self, query: str, max_results: int):
        super().__init__()
        self.query = query
        self.max_results = max_results

    def run(self) -> None:
        results = search_papers(self.query, max_results=self.max_results)
        self.done.emit(results)


class _PdfWorker(QThread):
    done = pyqtSignal(str)

    def __init__(self, paper: arxiv.Result):
        super().__init__()
        self.paper = paper

    def run(self) -> None:
        import os
        dirpath = os.path.join(os.path.dirname(__file__), '..', 'pdfs')
        os.makedirs(dirpath, exist_ok=True)
        path = download_pdf(self.paper, dirpath=dirpath)
        self.done.emit(path)


class SearchWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("arXiv Search")
        self.resize(1000, 600)
        self._results: list[arxiv.Result] = []
        self._pdf_window = PdfWindow()
        self._build_ui()

    def _build_ui(self) -> None:
        root = QWidget()
        self.setCentralWidget(root)
        layout = QVBoxLayout(root)
        layout.setContentsMargins(12, 12, 12, 12)
        layout.setSpacing(8)

        row = QHBoxLayout()
        self._search_box = QLineEdit()
        self._search_box.setPlaceholderText("Search arXiv…")
        self._search_box.returnPressed.connect(self._on_search)
        self._search_btn = QPushButton("Search")
        self._search_btn.clicked.connect(self._on_search)
        row.addWidget(self._search_box)
        row.addWidget(self._search_btn)
        layout.addLayout(row)

        # Outer: top panels / abstract
        outer = QSplitter(Qt.Orientation.Vertical)
        layout.addWidget(outer)

        # Top: list | meta
        top = QSplitter(Qt.Orientation.Horizontal)

        left = QWidget()
        left_layout = QVBoxLayout(left)
        left_layout.setContentsMargins(0, 0, 0, 0)
        left_layout.setSpacing(4)
        self._list = QListWidget()
        self._list.currentRowChanged.connect(self._on_select)
        self._status = QLabel("")
        self._status.setStyleSheet("color: grey;")
        left_layout.addWidget(self._list)
        left_layout.addWidget(self._status)
        top.addWidget(left)

        meta = QWidget()
        meta_layout = QVBoxLayout(meta)
        meta_layout.setContentsMargins(8, 0, 0, 0)
        meta_layout.setSpacing(4)

        self._sidebar_title = TexView()
        self._sidebar_title.setFixedHeight(70)

        self._sidebar_meta = TexView()
        self._sidebar_meta.setFixedHeight(40)

        self._pdf_btn = QPushButton("View PDF")
        self._pdf_btn.setEnabled(False)
        self._pdf_btn.clicked.connect(self._on_view_pdf)

        meta_layout.addWidget(self._sidebar_title)
        meta_layout.addWidget(self._sidebar_meta)
        meta_layout.addWidget(self._pdf_btn)
        meta_layout.addStretch()
        top.addWidget(meta)

        top.setSizes([400, 600])
        outer.addWidget(top)

        # Bottom: abstract
        self._sidebar_abstract = TexView()
        outer.addWidget(self._sidebar_abstract)

        outer.setSizes([300, 300])

    def _on_search(self) -> None:
        query = self._search_box.text().strip()
        if not query:
            return
        self._set_busy(True)
        self._list.clear()
        self._results = []
        self._clear_sidebar()
        self._worker = _SearchWorker(query, max_results=25)
        self._worker.done.connect(self._on_done)
        self._worker.start()

    def _on_done(self, results: list) -> None:
        self._results = results
        for paper in results:
            self._list.addItem(QListWidgetItem(paper.title))
        self._set_busy(False)
        self._status.setText(f"{len(results)} results")

    def _on_select(self, row: int) -> None:
        if row < 0 or row >= len(self._results):
            self._clear_sidebar()
            return
        paper = self._results[row]
        authors = ", ".join(a.name for a in paper.authors[:5])
        if len(paper.authors) > 5:
            authors += f" +{len(paper.authors) - 5} more"
        self._sidebar_title.set_content(paper.title)
        self._sidebar_meta.set_content(
            f"{authors}  ·  {paper.published.strftime('%Y-%m-%d')}  ·  {paper.primary_category}"
        )
        self._sidebar_abstract.set_content(paper.summary)
        self._pdf_btn.setEnabled(True)

    def _on_view_pdf(self) -> None:
        row = self._list.currentRow()
        if row < 0 or row >= len(self._results):
            return
        self._pdf_btn.setEnabled(False)
        self._pdf_btn.setText("Downloading…")
        self._pdf_worker = _PdfWorker(self._results[row])
        self._pdf_worker.done.connect(self._on_pdf_ready)
        self._pdf_worker.start()

    def _on_pdf_ready(self, path: str) -> None:
        self._pdf_btn.setEnabled(True)
        self._pdf_btn.setText("View PDF")
        self._pdf_window.load_pdf(path)

    def _clear_sidebar(self) -> None:
        self._sidebar_title.set_content("")
        self._sidebar_meta.set_content("")
        self._sidebar_abstract.set_content("")
        self._pdf_btn.setEnabled(False)

    def _set_busy(self, busy: bool) -> None:
        self._search_btn.setEnabled(not busy)
        self._search_box.setEnabled(not busy)
        self._status.setText("Fetching…" if busy else "")
