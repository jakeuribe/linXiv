from PyQt6.QtWidgets import QMainWindow, QToolBar, QLabel, QPushButton, QWidget
from PyQt6.QtPdfWidgets import QPdfView
from PyQt6.QtPdf import QPdfDocument
from PyQt6.QtCore import Qt


class PdfWindow(QMainWindow):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("PDF Viewer")
        self.resize(900, 1100)

        self._doc = QPdfDocument(self)
        self._view = QPdfView(self)
        self._view.setDocument(self._doc)
        self._view.setPageMode(QPdfView.PageMode.MultiPage)
        self.setCentralWidget(self._view)

        self._build_toolbar()

    def _build_toolbar(self) -> None:
        bar = QToolBar()
        bar.setMovable(False)
        self.addToolBar(bar)

        zoom_out = QPushButton("−")
        zoom_out.setFixedWidth(28)
        zoom_out.clicked.connect(lambda: self._zoom(-0.15))

        zoom_in = QPushButton("+")
        zoom_in.setFixedWidth(28)
        zoom_in.clicked.connect(lambda: self._zoom(0.15))

        fit_btn = QPushButton("Fit")
        fit_btn.clicked.connect(self._fit_width)

        self._page_label = QLabel("  ")

        spacer = QWidget()
        spacer.setMinimumWidth(12)

        bar.addWidget(zoom_out)
        bar.addWidget(zoom_in)
        bar.addWidget(fit_btn)
        bar.addWidget(spacer)
        bar.addWidget(self._page_label)

        self._view.pageNavigator().currentPageChanged.connect(self._update_page_label)

    def load_pdf(self, path: str) -> None:
        self._doc.close()
        self._doc.load(path)
        self._update_page_label(0)
        self.setWindowTitle(f"PDF — {path.split('/')[-1]}")
        self.show()
        self.raise_()
        self.activateWindow()

    def _zoom(self, delta: float) -> None:
        self._view.setZoomFactor(max(0.1, self._view.zoomFactor() + delta))

    def _fit_width(self) -> None:
        self._view.setZoomMode(QPdfView.ZoomMode.FitToWidth)

    def _update_page_label(self, page: int) -> None:
        total = self._doc.pageCount()
        self._page_label.setText(f"Page {page + 1} / {total}" if total else "")
