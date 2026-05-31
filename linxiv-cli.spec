# -*- mode: python ; coding: utf-8 -*-
#
# PyInstaller spec for the linXiv CLI sidecar.
#
# Build via:  npm run build:sidecar
# Output:     dist/linxiv-cli  (--onefile)

from pathlib import Path

ROOT = Path(".").resolve()

a = Analysis(
    [str(ROOT / "linxiv_cli.py")],
    pathex=[str(ROOT)],
    binaries=[],
    datas=[
        (str(ROOT / "formats"),                    "formats"),
        (str(ROOT / "storage" / "config" / "sql"), "storage/config/sql"),
    ],
    hiddenimports=[
        # our packages
        "config",
        "user_settings",
        "service",
        "service.paper",
        "service.project",
        "service.note",
        "service.tag",
        "service.author",
        "service.files",
        "service.export_import",
        "service.models",
        "service.models.paper",
        "service.models.project",
        "service.models.note",
        "service.models.tag",
        "service.models.author",
        "storage",
        "storage.db",
        "storage.notes",
        "storage.projects",
        "storage.tags",
        "storage.authors",
        "storage.paths",
        "storage.config",
        "storage.config.core",
        "storage.config.queries",
        "sources",
        "sources.arxiv_source",
        "sources.crossref_source",
        "sources.openalex_source",
        "sources.doi_resolve",
        "sources.fetch_paper_metadata",
        "sources.arxiv_downloads",
        "sources.pdf_metadata",
        "formats",
        "formats.bibtex",
        "formats.csv_fmt",
        "formats.json_fmt",
        "formats.markdown",
        # deps
        "arxiv",
        "httpx",
        "httpcore",
        "dotenv",
        "pydantic",
        "pydantic.deprecated",
        "email.mime.text",
        "email.mime.multipart",
    ],
    hookspath=[],
    runtime_hooks=[],
    excludes=[
        "PyQt6", "PyQt5", "tkinter",
        "fastapi", "uvicorn", "starlette",
        "matplotlib", "numpy", "pandas",
        "torch", "tensorflow",
        # AI SDKs (only used by AI_tools, which the CLI does not import)
        "openai", "google", "google.genai",
        "pytest",
    ],
    noarchive=False,
    optimize=0,
)

pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name="linxiv-cli",
    debug=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=True,
    argv_emulation=False,
    target_arch=None,
)
