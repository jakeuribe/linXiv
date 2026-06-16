from __future__ import annotations

from pathlib import Path

from config import data_dir


def db_path() -> Path:
    return data_dir() / "papers.db"


def pdf_dir() -> Path:
    return data_dir() / "pdfs"


def vault_dir() -> Path:
    """Root of the embedded-editor LaTeX vaults. One subdir per editor project,
    keyed by its note id (see service/vault.py vault_root)."""
    return data_dir() / "vaults"


# Legacy PDF location — only used for migration
def old_pdf_dir() -> Path:
    return data_dir() / "gui" / "pdfs"
