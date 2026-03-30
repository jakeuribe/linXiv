import sys

from PyQt6.QtWidgets import QApplication

from gui.shell import AppShell
from gui.home_page import HomePage
from gui.graph_page import GraphPage
from gui.projects_page import ProjectsPage
from gui.setup_page import SetupPage
from gui.doi_page import DoiPage
from gui.search_window import SearchWindow
from db import init_db


def run_shell() -> None:
    init_db()
    app = QApplication(sys.argv)

    shell = AppShell()
    shell.add_page("Home", HomePage())
    shell.add_page("Graph", GraphPage())
    shell.add_page("Projects", ProjectsPage())
    shell.add_page("Add by DOI", DoiPage())
    shell.add_page("Setup", SetupPage())

    _sw: list[SearchWindow] = []

    def _open_search() -> None:
        if not _sw:
            _sw.append(SearchWindow())
        w = _sw[0]
        w.show()
        w.raise_()
        w.activateWindow()

    shell.add_launcher("Search", _open_search)
    shell.show()
    sys.exit(app.exec())
