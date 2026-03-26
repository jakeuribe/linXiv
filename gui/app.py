import sys
from PyQt6.QtWidgets import QApplication
from gui.main_window import MainWindow
from db import init_db


def run() -> None:
    init_db()
    app = QApplication(sys.argv)
    window = MainWindow()
    window.show()
    sys.exit(app.exec())
