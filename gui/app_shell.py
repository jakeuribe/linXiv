import sys

from PyQt6.QtWidgets import QApplication, QStyleFactory

from gui.shell import AppShell
from gui.home_page import HomePage
from gui.graph_page import GraphPage
from gui.library_page import LibraryPage
from gui.projects_page import ProjectsPage
from gui.setup_page import SetupPage
from gui.doi_page import DoiPage
from gui.search import SearchWindow
from storage.db import init_db


def run_shell() -> None:
    init_db()
    app = QApplication(sys.argv)
    app.setStyle(QStyleFactory.create("Fusion"))

    shell = AppShell()
    shell.add_page("Home", HomePage())
    library_page  = LibraryPage()
    projects_page = ProjectsPage()
    graph_page    = GraphPage()
    shell.add_page("Library", library_page)
    shell.add_page("Graph", graph_page)
    shell.add_page("Projects", projects_page)
    shell.add_page("Add by DOI", DoiPage())
    shell.add_page("Setup", SetupPage())

    def _on_navigate_to_project(project) -> None:
        shell.go_to_widget(projects_page)
        projects_page.open_project(project)

    library_page.navigate_to_project.connect(_on_navigate_to_project)

    def _on_paper_right_clicked(paper_id: str) -> None:
        shell.go_to_widget(library_page)
        library_page.open_paper(paper_id, on_back=lambda: shell.go_to_widget(graph_page))

    graph_page.paper_right_clicked.connect(_on_paper_right_clicked)

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
