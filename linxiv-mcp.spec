# -*- mode: python ; coding: utf-8 -*-
#
# PyInstaller spec for the linXiv MCP server sidecar.
#
# Build via:  npm run build:sidecar
# Output:     dist/linxiv-mcp  (--onefile)
#
# The MCP server is called directly by Claude Desktop / Cursor / Windsurf
# using the path registered in their config files.

from pathlib import Path

ROOT = Path(".").resolve()

a = Analysis(
    [str(ROOT / "linxiv_mcp.py")],
    pathex=[str(ROOT)],
    binaries=[],
    datas=[
        (str(ROOT / "formats"),                    "formats"),
        (str(ROOT / "storage" / "config" / "sql"), "storage/config/sql"),
    ],
    hiddenimports=[
        # mcp / fastmcp internals
        "mcp",
        "mcp.server",
        "mcp.server.fastmcp",
        "mcp.server.stdio",
        "mcp.server.sse",
        "anyio",
        "anyio._backends._asyncio",
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
        "sources.openalex_source",
        "formats",
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
        "fastapi", "uvicorn",
        "matplotlib", "numpy", "pandas",
        "torch", "tensorflow",
        # AI SDKs (only used by AI_tools, which the MCP server does not import)
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
    name="linxiv-mcp",
    debug=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=True,
    argv_emulation=False,
    target_arch=None,
)
