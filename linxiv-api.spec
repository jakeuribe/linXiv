# -*- mode: python ; coding: utf-8 -*-
#
# PyInstaller spec for the linXiv API sidecar.
#
# Build:
#   pyinstaller linxiv-api.spec --noconfirm
#   python scripts/stage_sidecar.py
#
# Output: dist/linxiv-api  (single executable, --onefile)
# Tauri bundles this as an externalBin via scripts/stage_sidecar.py.

from pathlib import Path

ROOT = Path(".").resolve()

a = Analysis(
    [str(ROOT / "api" / "__main__.py")],
    pathex=[str(ROOT)],
    binaries=[],
    datas=[
        # Bundled read-only resources accessed via resources_dir() / config.resources_dir()
        (str(ROOT / "formats"),                   "formats"),
        (str(ROOT / "storage" / "config" / "sql"), "storage/config/sql"),
    ],
    hiddenimports=[
        # uvicorn internals that PyInstaller misses
        "uvicorn.logging",
        "uvicorn.loops",
        "uvicorn.loops.auto",
        "uvicorn.loops.asyncio",
        "uvicorn.protocols",
        "uvicorn.protocols.http",
        "uvicorn.protocols.http.auto",
        "uvicorn.protocols.http.h11_impl",
        "uvicorn.protocols.websockets",
        "uvicorn.protocols.websockets.auto",
        "uvicorn.protocols.websockets.wsproto_impl",
        "uvicorn.lifespan",
        "uvicorn.lifespan.on",
        "uvicorn.lifespan.off",
        # anyio backends
        "anyio",
        "anyio._backends._asyncio",
        # starlette
        "starlette",
        "starlette.routing",
        "starlette.middleware",
        "starlette.middleware.cors",
        "starlette.staticfiles",
        "starlette.responses",
        # fastapi internals
        "fastapi",
        "fastapi.responses",
        "fastapi.staticfiles",
        # pydantic (v2 ships both interfaces)
        "pydantic",
        "pydantic.deprecated",
        "pydantic.v1",
        # email (used by various stdlib/third-party deps)
        "email.mime.text",
        "email.mime.multipart",
        "email.mime.nonmultipart",
        # python-dotenv
        "dotenv",
        # python-multipart (required by fastapi File/UploadFile and starlette form parsing)
        "multipart",
        "multipart.multipart",
        # httpx / httpcore (used by arxiv client)
        "httpx",
        "httpcore",
        "httpcore._async",
        "httpcore._sync",
        # h11 (used by uvicorn)
        "h11",
        # arxiv
        "arxiv",
        # our own packages (ensure collected even if not reached via static analysis)
        "api",
        "api.app",
        "api.graph_payload",
        "api.run_api",
        "service",
        "service.paper",
        "service.project",
        "service.note",
        "service.tag",
        "service.author",
        "service.files",
        "service.content",
        "service.export_import",
        "service.models",
        "service.models.paper",
        "service.models.project",
        "service.models.note",
        "service.models.tag",
        "service.models.author",
        "service.models.content",
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
        "config",
        "user_settings",
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[
        # GUI layer — not needed in the API sidecar
        "PyQt6",
        "PyQt5",
        "PySide6",
        "tkinter",
        "wx",
        # Heavy scientific libs not used by the API
        "matplotlib",
        "numpy",
        "pandas",
        "scipy",
        "PIL",
        "cv2",
        "torch",
        "tensorflow",
        # Test infrastructure
        "pytest",
        "pytest_asyncio",
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
    name="linxiv-api",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
