from PyQt6.QtCore import QThread, pyqtSignal
import arxiv
from storage.paths import pdf_dir
from sources import search_papers
from sources import ArxivSource, OpenAlexSource
from sources.arxiv_downloads import download_pdf

_PDF_DIR = pdf_dir()


class _SearchWorker(QThread):
    done = pyqtSignal(list)
    error = pyqtSignal(str)

    def __init__(self, query: str, max_results: int,
                 sort_by: arxiv.SortCriterion, sort_order: arxiv.SortOrder):
        super().__init__()
        self.query = query
        self.max_results = max_results
        self.sort_by = sort_by
        self.sort_order = sort_order

    def run(self) -> None:
        try:
            results = search_papers(
                self.query,
                max_results=self.max_results,
                sort_by=self.sort_by,
                sort_order=self.sort_order,
            )
            self.done.emit(results)
        except Exception as e:
            msg = "arXiv rate limit — wait ~60 s and retry." if "429" in str(e) else str(e)
            self.error.emit(msg)


class _SourceSearchWorker(QThread):
    """Search worker for any PaperSource (arXiv or OpenAlex)."""
    done = pyqtSignal(list)
    error = pyqtSignal(str)

    def __init__(self, source_name: str, query: str, max_results: int):
        super().__init__()
        self._source_name = source_name
        self.query = query
        self.max_results = max_results

    def run(self) -> None:
        try:
            if self._source_name == "openalex":
                source = OpenAlexSource()
            else:
                source = ArxivSource()
            results = source.search(self.query, max_results=self.max_results)
            self.done.emit(results)
        except Exception as e:
            msg = "arXiv rate limit — wait ~60 s and retry." if "429" in str(e) else str(e)
            self.error.emit(msg)


class _PdfWorker(QThread):
    done = pyqtSignal(str)
    error = pyqtSignal(str)

    def __init__(self, paper: arxiv.Result):
        super().__init__()
        self.paper = paper

    def run(self) -> None:
        try:
            _PDF_DIR.mkdir(parents=True, exist_ok=True)
            path = download_pdf(self.paper, dirpath=str(_PDF_DIR))
            self.done.emit(path)
        except Exception as e:
            msg = "arXiv rate limit — wait ~60 s and retry." if "429" in str(e) else str(e)
            self.error.emit(msg)
