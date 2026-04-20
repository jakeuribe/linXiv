import sys
from PyQt6.QtWidgets import QApplication
from gui.search import SearchPage
from storage.db import init_db


if __name__ == "__main__":
    init_db()
    app = QApplication(sys.argv)
    window = SearchPage()
    window.show()
    sys.exit(app.exec())
