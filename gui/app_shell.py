import sys
from pathlib import Path

from PyQt6.QtGui import QIcon
from PyQt6.QtWidgets import QApplication, QStyleFactory

from gui.shell import AppShell
from gui.home import HomePage
from gui.graph import GraphPage
from gui.library import LibraryPage
from gui.projects import ProjectsPage
from gui.setup import SetupPage
from gui.settings import SettingsPage
from gui.doi import DoiPage
from gui.search import SearchPage
from storage.db import init_db


def run_shell() -> None:
    init_db()
    app = QApplication(sys.argv)
    app.setApplicationName("Linxiv")
    app.setStyle(QStyleFactory.create("Fusion"))
    _icon = QIcon(str(Path(__file__).parent.parent / "assets" / "app_icon.png"))
    app.setWindowIcon(_icon)

    shell = AppShell()
    shell.setWindowIcon(_icon)
    home_page     = HomePage()
    library_page  = LibraryPage()
    shell.add_page("Home", home_page)
    projects_page = ProjectsPage()
    graph_page    = GraphPage()
    shell.add_page("Library", library_page)
    shell.add_page("Graph", graph_page)
    shell.add_page("Projects", projects_page)
    search_page = SearchPage()
    shell.add_page("Search", search_page)
    shell.add_page("Add by DOI", DoiPage())
    shell.add_page("Setup", SetupPage())
    shell.add_page("Settings", SettingsPage())

    def _on_navigate_to_project(project) -> None:
        paper_id = library_page.take_paper_id_for_project_return()
        shell.go_to_widget(projects_page)
        projects_page.open_project(
            project,
            opened_from_other_shell_tab=True,
            return_to_library_paper_id=paper_id,
        )

    library_page.navigate_to_project.connect(_on_navigate_to_project)

    def _on_navigate_to_paper(paper_id: str) -> None:
        shell.go_to_widget(library_page)
        library_page.open_paper(paper_id)

    # Home double-click, Projects double-click, Graph right-click all open Library detail.
    home_page.navigate_to_paper.connect(_on_navigate_to_paper)
    projects_page.navigate_to_paper.connect(_on_navigate_to_paper)
    graph_page.paper_right_clicked.connect(_on_navigate_to_paper)

    library_page.attach_app_shell(shell)
    projects_page.attach_app_shell(shell)
    projects_page.attach_library_page(library_page)

    def _on_shell_page_changed(idx: int) -> None:
        if idx == shell._stack.indexOf(library_page):
            library_page.show_library_list()
        if idx == shell._stack.indexOf(projects_page):
            projects_page.show_project_list()

    shell._stack.currentChanged.connect(_on_shell_page_changed)

    shell.register_on_close(search_page.cleanup_pdfs)
    app.aboutToQuit.connect(search_page.cleanup_pdfs)

    shell.showMaximized()
    sys.exit(app.exec())
