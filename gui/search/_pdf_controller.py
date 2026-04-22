import os

from PyQt6.QtWidgets import QPushButton, QCheckBox, QLabel, QFileDialog, QApplication
import arxiv

from storage.db import get_paper, set_has_pdf, set_pdf_path, parse_entry_id, list_papers
from sources.arxiv_downloads import cleanup_pdfs as _cleanup_pdfs, saved_pdfs_size
from gui.views import PdfWindow
from gui.search._workers import _PdfWorker, _PDF_DIR


class _PdfController:
    def __init__(
        self,
        pdf_btn: QPushButton,
        save_pdf_btn: QCheckBox,
        link_pdf_btn: QPushButton,
        linked_indicator: QLabel,
        status_label: QLabel,
        pdf_window: PdfWindow,
    ):
        self._pdf_btn = pdf_btn
        self._save_pdf_btn = save_pdf_btn
        self._link_pdf_btn = link_pdf_btn
        self._linked_indicator = linked_indicator
        self._status = status_label
        self._pdf_window = pdf_window

        self._saved_papers: set[tuple[str, int]] = set()
        self._paper_pdf_paths: dict[tuple[str, int], str] = {}
        # TODO: Make configurable in user specific settings
        self._save_limit_bytes: int = 1 * 1024 ** 3  # 1 GB cap
        self._pdf_worker: _PdfWorker | None = None

    def on_view_pdf(
        self,
        key: tuple[str, int] | None,
        results: list[arxiv.Result],
        current_row: int,
    ) -> None:
        # Check for linked external PDF first
        if key:
            db_row = get_paper(key[0], key[1])
            if db_row and db_row["pdf_path"] and os.path.isfile(db_row["pdf_path"]):
                self._pdf_window.load_pdf(db_row["pdf_path"], is_external=True)
                return
        # Only arXiv results support direct PDF download
        if current_row < 0 or current_row >= len(results):
            return
        self._pdf_btn.setEnabled(False)
        self._pdf_btn.setText("Downloading…")
        self._pdf_worker = _PdfWorker(results[current_row])
        self._pdf_worker.done.connect(lambda path, k=key: self._on_pdf_ready(path, k))
        self._pdf_worker.start()

    def _on_pdf_ready(self, path: str, key: tuple[str, int] | None = None) -> None:
        self._pdf_btn.setEnabled(True)
        self._pdf_btn.setText("View PDF")
        if key:
            self._paper_pdf_paths[key] = path
            print(f"[pdf] downloaded {key} → {path}")

        # If this paper is marked to save, check size limit before displaying
        if key and key in self._saved_papers:
            saved_paths = {
                self._paper_pdf_paths.get(k) or self.pdf_path_for_key(k)
                for k in self._saved_papers
            }
            total = saved_pdfs_size(saved_paths)
            limit_mb = self._save_limit_bytes / 1024 ** 2
            total_mb = total / 1024 ** 2
            print(f"[size] saved total: {total_mb:.1f} MB / {limit_mb:.0f} MB limit")
            if total > self._save_limit_bytes:
                self._saved_papers.discard(key)
                self._save_pdf_btn.blockSignals(True)
                self._save_pdf_btn.setChecked(False)
                self._save_pdf_btn.blockSignals(False)
                self._status.setText(
                    f"Save limit reached ({total_mb:.0f} MB / {limit_mb:.0f} MB) — PDF not saved."
                )
                print(f"[size] limit exceeded — not saving {key}")
                return  # don't open viewer

        self._pdf_window.load_pdf(path)

    def on_save_pdf_toggled(self, checked: bool, key: tuple[str, int] | None) -> None:
        if key is None:
            return
        if checked:
            self._saved_papers.add(key)
            print(f"[save] marked {key} as saved | saved set: {self._saved_papers}")
        else:
            self._saved_papers.discard(key)
            print(f"[save] unmarked {key} | saved set: {self._saved_papers}")

    def on_link_pdf(self, key: tuple[str, int] | None, parent_widget) -> None:
        if key is None:
            return
        paper_id, version = key
        # Check if the paper is saved in the DB first
        row = get_paper(paper_id, version)
        if row is None:
            self._status.setText("Save the paper first before linking a PDF.")
            return
        path, _ = QFileDialog.getOpenFileName(
            parent_widget, "Link PDF to paper", "", "PDF Files (*.pdf)"
        )
        if not path:
            return
        set_pdf_path(paper_id, path)
        self._linked_indicator.setText("Linked")
        self._status.setText(f"Linked PDF: {os.path.basename(path)}")

    def sync_save_state(self, key: tuple[str, int]) -> None:
        """Sync the save checkbox to reflect whether key is saved. Called on result selection."""
        already_saved = key in self._saved_papers or os.path.isfile(self.pdf_path_for_key(key))
        if already_saved:
            self._saved_papers.add(key)
        self._save_pdf_btn.blockSignals(True)
        self._save_pdf_btn.setChecked(already_saved)
        self._save_pdf_btn.blockSignals(False)

    def cleanup_pdfs(self) -> list[str]:
        """Delete all unsaved PDFs. Always runs — no size condition for deletion."""
        self._pdf_window._doc.close()  # release Windows file lock before deleting
        QApplication.processEvents()   # flush handle release (required on Windows)
        if not _PDF_DIR.is_dir():
            return []
        keep = {
            self._paper_pdf_paths.get(key) or self.pdf_path_for_key(key)
            for key in self._saved_papers
        }
        # Also keep any PDF already recorded in the DB (e.g. downloaded via Library page)
        for row in list_papers():
            pdf_path = row["pdf_path"] if "pdf_path" in row.keys() else None
            if pdf_path and os.path.isfile(pdf_path):
                keep.add(pdf_path)
            if row["has_pdf"]:
                keep.add(self.pdf_path_for_key((row["paper_id"], row["version"])))
        deleted = _cleanup_pdfs(str(_PDF_DIR), keep=keep)

        # Update has_pdf flag in DB
        for key in self._saved_papers:
            path = self._paper_pdf_paths.get(key) or self.pdf_path_for_key(key)
            set_has_pdf(key[0], key[1], os.path.isfile(path))
        for path in deleted:
            fname = os.path.splitext(os.path.basename(path))[0]  # e.g. '2204.12985v4'
            key = parse_entry_id(fname)
            set_has_pdf(key[0], key[1], False)

        print(f"[cleanup] kept: {self._saved_papers} | deleted {len(deleted)} file(s): {deleted}")
        return deleted

    @staticmethod
    def pdf_path_for_key(key: tuple[str, int]) -> str:
        paper_id, version = key
        return str(_PDF_DIR / f"{paper_id}v{version}.pdf")
