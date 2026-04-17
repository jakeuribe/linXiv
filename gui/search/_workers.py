from pathlib import Path

from PyQt6.QtCore import QThread, pyqtSignal
import arxiv
from fetch_paper_metadata import search_papers
from sources import ArxivSource, OpenAlexSource
from downloads import download_pdf

_PDF_DIR = Path(__file__).parent.parent.parent / "pdfs"


class _SearchWorker(QThread):
    done = pyqtSignal(list)

    def __init__(self, query: str, max_results: int,
                 sort_by: arxiv.SortCriterion, sort_order: arxiv.SortOrder):
        super().__init__()
        self.query = query
        self.max_results = max_results
        self.sort_by = sort_by
        self.sort_order = sort_order

    def run(self) -> None:
        results = search_papers(
            self.query,
            max_results=self.max_results,
            sort_by=self.sort_by,
            sort_order=self.sort_order,
        )
        self.done.emit(results)


class _SourceSearchWorker(QThread):
    """Search worker for any PaperSource (arXiv or OpenAlex)."""
    done = pyqtSignal(list)

    def __init__(self, source_name: str, query: str, max_results: int):
        super().__init__()
        self._source_name = source_name
        self.query = query
        self.max_results = max_results

    def run(self) -> None:
        if self._source_name == "openalex":
            source = OpenAlexSource()
        else:
            source = ArxivSource()
        results = source.search(self.query, max_results=self.max_results)
        self.done.emit(results)


class _PdfWorker(QThread):
    done = pyqtSignal(str)

    def __init__(self, paper: arxiv.Result):
        super().__init__()
        self.paper = paper

    def run(self) -> None:
        _PDF_DIR.mkdir(parents=True, exist_ok=True)
        path = download_pdf(self.paper, dirpath=str(_PDF_DIR))
        self.done.emit(path)
