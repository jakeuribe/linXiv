"""
HTTP JSON API for linXiv → http://127.0.0.1:8000

- OpenAPI: http://127.0.0.1:8000/docs
- External frontends call ``/api/...`` on this origin.

CORS: set env ``CORS_ORIGINS`` to a comma-separated list of frontend origins, e.g.
``http://localhost:5180``. If unset, all origins are allowed (no credentials).

Run with venv:
    python -m api
"""

from __future__ import annotations

import os
import sys
from pathlib import Path
import uvicorn


def run() -> None:
    # reload=True requires the source tree — disabled in frozen PyInstaller builds
    should_reload = not getattr(sys, "frozen", False)
    raw_port = os.environ.get("LINXIV_PORT", "8000")
    try:
        port = int(raw_port)
    except ValueError:
        sys.stderr.write(f"[linxiv] LINXIV_PORT={raw_port!r} is not a valid integer\n")
        sys.exit(2)
    uvicorn.run(
        "api.app:app",
        host="127.0.0.1",
        port=port,
        reload=should_reload,
        reload_dirs=[str(Path(__file__).resolve().parent.parent)] if should_reload else None,
    )


if __name__ == "__main__":
    run()
