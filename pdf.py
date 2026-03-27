import sys
from PyQt6.QtWidgets import QApplication, QFileDialog
from gui.pdf_window import PdfWindow


if __name__ == "__main__":
    app = QApplication(sys.argv)
    window = PdfWindow()
    path, _ = QFileDialog.getOpenFileName(None, "Open PDF", "", "PDF Files (*.pdf)")
    if path:
        window.load_pdf(path)
        sys.exit(app.exec())
