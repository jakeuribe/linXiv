import sys

from PyQt6.QtWidgets import QApplication, QStyleFactory

from gui.shell import AppShell
from gui.home import HomePage
from gui.graph import GraphPage
from gui.library import LibraryPage
from gui.projects import ProjectsPage
from gui.setup import SetupPage
from gui.doi import DoiPage
from gui.search import SearchPage
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
    search_page = SearchPage()
    shell.add_page("Search", search_page)
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

    shell.register_on_close(search_page.cleanup_pdfs)
    app.aboutToQuit.connect(search_page.cleanup_pdfs)

    shell.show()
    sys.exit(app.exec())
