import os
import sys
from pathlib import Path

_PROJECT_ROOT = Path(__file__).resolve().parent
_APP_IDENTIFIER = "com.linxiv.app"  # must match src-tauri/tauri.conf.json "identifier"


def repo_dir() -> Path:
    """The source-tree root. For developer/repo artifacts only (e.g. the dev .env).
    NOT for runtime user data — that is data_dir()."""
    return _PROJECT_ROOT


def _default_data_dir() -> Path:
    """OS per-user app-data dir for com.linxiv.app, matching Tauri's app_data_dir()
    so a directly-launched CLI/MCP/API uses the same location as the packaged app.

      Linux:   $XDG_DATA_HOME or ~/.local/share, then /com.linxiv.app
      macOS:   ~/Library/Application Support/com.linxiv.app
      Windows: %APPDATA% (Roaming) or ~/AppData/Roaming, then /com.linxiv.app

    Used only when LINXIV_DATA_DIR is unset (dev / CLI / MCP launched without Tauri).
    """
    if sys.platform == "darwin":
        base = Path.home() / "Library" / "Application Support"
    elif sys.platform == "win32":
        base = Path(os.environ.get("APPDATA") or (Path.home() / "AppData" / "Roaming"))
    else:
        base = Path(os.environ.get("XDG_DATA_HOME") or (Path.home() / ".local" / "share"))
    return base / _APP_IDENTIFIER


def data_dir() -> Path:
    """Runtime data dir: DB, PDFs, user settings, Obsidian vault, arXiv rate-limit file.

    The single source of truth is $LINXIV_DATA_DIR (Tauri sets it on the subprocess).
    When unset (dev / CLI / MCP launched without Tauri) it falls back to the OS
    app-data dir — never the repo root. Resolved on every call so it tracks the env
    var dynamically; call init_data_dir() once at startup to persist and create it.
    """
    d = os.environ.get("LINXIV_DATA_DIR")
    return Path(d) if d else _default_data_dir()


def init_data_dir() -> Path:
    """Resolve, persist, and create the data dir. Call once per run at startup, before
    any DB/PDF/vault access. Idempotent.

    Writes the resolved path back to $LINXIV_DATA_DIR so the value is stable for the
    whole process (and inherited by child processes), then creates the directory. This
    is what makes "initialize the data dir on any run" hold even when launched directly
    without Tauri.

    Side effect: once called, $LINXIV_DATA_DIR is pinned for the lifetime of the
    process. Tests that redirect the data dir must set/restore os.environ
    (pytest's monkeypatch.setenv does this automatically).
    """
    path = data_dir()
    os.environ["LINXIV_DATA_DIR"] = str(path)
    path.mkdir(parents=True, exist_ok=True)
    return path


def resources_dir() -> Path:
    """Read-only bundled resources: SQL schemas, graph assets, format templates.
    In a PyInstaller frozen build these are extracted to sys._MEIPASS.
    In development they live at the project root.
    """
    if getattr(sys, "frozen", False) and hasattr(sys, "_MEIPASS"):
        return Path(sys._MEIPASS)  # type: ignore[attr-defined]
    return _PROJECT_ROOT


# .env file location — developer convenience for dev API keys. Lives in the repo (a
# source artifact), NOT in data_dir(). In production Tauri injects env vars directly;
# this path just needs to exist.
ENV_PATH = repo_dir() / ".env"
