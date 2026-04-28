from __future__ import annotations

from pathlib import Path


def project_root() -> Path:
    return Path(__file__).resolve().parent.parent


def db_path() -> Path:
    return project_root() / "papers.db"


def pdf_dir() -> Path:
    return project_root() / "pdfs"
