from __future__ import annotations

from PyQt6.QtWidgets import QFileDialog, QMenu, QPushButton, QWidget

from .view import GraphView
from formats.bibtex import BibTeXFormat
from formats.csv_fmt import CSVFormat
from formats.json_fmt import JSONFormat
from formats.markdown import MarkdownFormat, ObsidianFormat

_bibtex_fmt   = BibTeXFormat()
_csv_fmt      = CSVFormat()
_markdown_fmt = MarkdownFormat()
_obsidian_fmt = ObsidianFormat()
_json_fmt     = JSONFormat()


class _ExportHandler:
    def __init__(self, graph_view: GraphView, parent: QWidget) -> None:
        self._graph_view = graph_view
        self._parent = parent

    def show_menu(self, anchor: QPushButton) -> None:
        menu = QMenu(self._parent)
        menu.addAction("Export as JSON",     self._export_json)
        menu.addAction("Export as CSV",      self._export_csv)
        menu.addAction("Export as BibTeX",   self._export_bibtex)
        menu.addSeparator()
        menu.addAction("Export as Markdown", self._export_markdown)
        menu.addAction("Export as Obsidian", self._export_obsidian)
        menu.exec(anchor.mapToGlobal(anchor.rect().bottomLeft()))

    def _export_json(self)     -> None: self._graph_view.get_selected_paper_data(self._save_json)
    def _export_csv(self)      -> None: self._graph_view.get_selected_paper_data(self._save_csv)
    def _export_bibtex(self)   -> None: self._graph_view.get_selected_paper_data(self._save_bibtex)
    def _export_markdown(self) -> None: self._graph_view.get_selected_paper_data(self._save_markdown)
    def _export_obsidian(self) -> None: self._graph_view.get_selected_paper_data(self._save_obsidian)

    def _save_json(self, data: dict) -> None:
        path, _ = QFileDialog.getSaveFileName(self._parent, "Export JSON", "selected_papers.json", "JSON (*.json)")
        if path:
            with open(path, "w", encoding="utf-8") as f:
                f.write(_json_fmt.export_papers(data.get("papers", [])))

    def _save_csv(self, data: dict) -> None:
        path, _ = QFileDialog.getSaveFileName(self._parent, "Export CSV", "selected_papers.csv", "CSV (*.csv)")
        if path:
            with open(path, "w", encoding="utf-8", newline="") as f:
                f.write(_csv_fmt.export_papers(data.get("papers", [])))

    def _save_bibtex(self, data: dict) -> None:
        path, _ = QFileDialog.getSaveFileName(self._parent, "Export BibTeX", "selected_papers.bib", "BibTeX (*.bib)")
        if path:
            with open(path, "w", encoding="utf-8") as f:
                f.write(_bibtex_fmt.export_papers(data.get("papers", [])))

    def _save_markdown(self, data: dict) -> None:
        path, _ = QFileDialog.getSaveFileName(self._parent, "Export Markdown", "selected_papers.md", "Markdown (*.md)")
        if path:
            with open(path, "w", encoding="utf-8") as f:
                f.write(_markdown_fmt.export_papers(data.get("papers", [])))

    def _save_obsidian(self, data: dict) -> None:
        path, _ = QFileDialog.getSaveFileName(self._parent, "Export Obsidian", "selected_papers.md", "Markdown (*.md)")
        if path:
            with open(path, "w", encoding="utf-8") as f:
                f.write(_obsidian_fmt.export_papers(data.get("papers", [])))
