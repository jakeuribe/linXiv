import sys
from PyQt6.QtWidgets import QApplication
from gui.search_window import SearchWindow
from db import init_db


if __name__ == "__main__":
    init_db()
    app = QApplication(sys.argv)
    window = SearchWindow()
    window.show()
    sys.exit(app.exec())
