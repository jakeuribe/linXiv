"""On-disk LaTeX vault backing the embedded TeXbrain editor's filesystem.

Each embedded-editor project owns one directory tree under ``vault_dir()`` keyed by
its note id (``vault_dir()/note_<NOTE_SK>/``; see service/editor_project.py for the
note-link). The TeXbrain editor, running in an iframe, drives its FileSystemDirectoryHandle
over a postMessage RPC; the linXiv host forwards each op to ``run_fs_op`` here. The wire
contract is the FsOp/FsResult shape vendored in src/lib/editorBridgeTypes.ts:

    FsOp    = list{path} | readFile{path} | writeFile{path,data,binary?}
            | mkdir{path} | remove{path,recursive?}
    FsResult= list{entries:[{name,kind}]} | readFile{data,binary} | ok

Path semantics (must match postmessage-fs.ts): paths are root-relative with NO leading
slash; ``""`` denotes the vault root. Binary file bytes ride the wire as base64 strings
with ``binary=true``; text rides as raw UTF-8 with ``binary=false``. The host decides
text-vs-binary by UTF-8 decodability on read.

Security: every op resolves its path against the vault root and asserts containment with
``Path.is_relative_to`` (the same guard service/files.py uses for managed PDFs); ``..``
and absolute paths are rejected before the join so the editor can never escape its vault.
"""

from __future__ import annotations

import base64
import shutil
from pathlib import Path

from storage.paths import vault_dir


# ── path resolution + containment guard ────────────────────────────────────────

def vault_root(note_id: int) -> Path:
    """Absolute directory backing the editor project's filesystem root."""
    return (vault_dir() / f"note_{int(note_id)}").resolve()


def _safe_path(note_id: int, relpath: str) -> Path:
    """Resolve a root-relative editor path inside the vault, or raise ValueError.

    Rejects absolute paths and any ``..`` traversal, then asserts the resolved path
    stays within the vault root. ``relpath == ""`` resolves to the vault root itself.
    """
    raw = (relpath or "").replace("\\", "/")
    if raw.startswith("/"):
        raise ValueError("absolute paths are not allowed")
    parts = [p for p in raw.split("/") if p not in ("", ".")]
    if any(p == ".." for p in parts):
        raise ValueError("path traversal is not allowed")
    root = vault_root(note_id)
    target = (root / Path(*parts)).resolve() if parts else root
    if target != root and not target.is_relative_to(root):
        raise ValueError("path escapes the vault root")
    return target


# ── individual ops (raise on error; the route maps to HTTP status) ──────────────

def list_dir(note_id: int, relpath: str) -> dict:
    """List immediate children (BASENAMES only — the guest re-joins to the parent)."""
    target = _safe_path(note_id, relpath)
    entries: list[dict] = []
    if target.is_dir():
        for child in sorted(target.iterdir(), key=lambda c: c.name):
            entries.append({
                "name": child.name,
                "kind": "directory" if child.is_dir() else "file",
            })
    # A missing dir lists as empty rather than erroring so a freshly-mounted vault
    # (or a not-yet-created subdir) does not break the editor's initial scan.
    return {"kind": "list", "entries": entries}


# The editor's own text/binary classification (tex-brain readDirRecursive). The host's
# `binary` flag MUST match how the guest reads each file: text extensions are read via
# getFile().text() (so they must arrive as a raw string), binary via arrayBuffer() (so
# they must arrive base64). Classifying by UTF-8-decodability instead would ship a
# latin-1 .tex as base64 and the guest's .text() would mojibake it.
_TEXT_EXTS = {
    "tex", "sty", "cls", "bib", "bst", "def", "cfg", "fd", "dtx", "ins",
    "ltx", "txt", "bbx", "cbx", "lbx",
}
_BINARY_EXTS = {
    "png", "jpg", "jpeg", "pdf", "eps", "svg", "gif", "bmp", "tfm", "pfb",
    "vf", "map", "enc", "otf", "ttf",
}


def _ext_is_text(relpath: str) -> bool | None:
    """True/False per the editor's extension sets; None for an unknown extension."""
    ext = Path(relpath).suffix.lower().lstrip(".")
    if ext in _TEXT_EXTS:
        return True
    if ext in _BINARY_EXTS:
        return False
    return None


def read_file(note_id: int, relpath: str) -> dict:
    """Read a file, classifying text-vs-binary by extension to match the editor.

    Text-extension files return a raw string (UTF-8, falling back to latin-1 so non-UTF-8
    LaTeX source is not corrupted into base64). Binary-extension files return base64.
    Unknown extensions fall back to UTF-8 decodability."""
    target = _safe_path(note_id, relpath)
    if not target.is_file():
        raise FileNotFoundError(relpath)
    raw = target.read_bytes()
    classified = _ext_is_text(relpath)
    if classified is True:
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            text = raw.decode("latin-1")  # round-trip normalizes to UTF-8 on save
        return {"kind": "readFile", "data": text, "binary": False}
    if classified is False:
        return {"kind": "readFile", "data": base64.b64encode(raw).decode("ascii"), "binary": True}
    try:
        return {"kind": "readFile", "data": raw.decode("utf-8"), "binary": False}
    except UnicodeDecodeError:
        return {"kind": "readFile", "data": base64.b64encode(raw).decode("ascii"), "binary": True}


def write_file(note_id: int, relpath: str, data: str, binary: bool) -> dict:
    """Write a file, creating parent dirs. ``data`` is base64 when binary else raw text.
    An empty string materializes a zero-length file (the editor's create-empty path)."""
    target = _safe_path(note_id, relpath)
    if target == vault_root(note_id):
        raise ValueError("cannot write the vault root as a file")
    target.parent.mkdir(parents=True, exist_ok=True)
    if binary:
        target.write_bytes(base64.b64decode(data or ""))
    else:
        target.write_text(data or "", encoding="utf-8")
    return {"kind": "ok"}


def make_dir(note_id: int, relpath: str) -> dict:
    target = _safe_path(note_id, relpath)
    target.mkdir(parents=True, exist_ok=True)
    return {"kind": "ok"}


def remove_entry(note_id: int, relpath: str, recursive: bool) -> dict:
    target = _safe_path(note_id, relpath)
    if target == vault_root(note_id):
        raise ValueError("cannot remove the vault root")
    if target.is_dir():
        if recursive:
            shutil.rmtree(target)
        else:
            target.rmdir()  # raises OSError if non-empty — mirrors removeEntry(recursive:false)
    elif target.exists():
        target.unlink()
    else:
        raise FileNotFoundError(relpath)
    return {"kind": "ok"}


# ── op dispatch (one entry point for the /fs route) ─────────────────────────────

def run_fs_op(note_id: int, op: dict) -> dict:
    """Dispatch one FsOp to the matching disk op and return the FsResult dict.

    Raises ValueError for bad input (-> HTTP 400) and FileNotFoundError for missing
    files (-> HTTP 404). The host bridge serializes any error into a failed RPC.
    """
    kind = op.get("kind")
    path = op.get("path", "")
    if kind == "list":
        return list_dir(note_id, path)
    if kind == "readFile":
        return read_file(note_id, path)
    if kind == "writeFile":
        return write_file(note_id, path, op.get("data") or "", bool(op.get("binary", False)))
    if kind == "mkdir":
        return make_dir(note_id, path)
    if kind == "remove":
        return remove_entry(note_id, path, bool(op.get("recursive", False)))
    raise ValueError(f"unknown fs op kind: {kind!r}")


def list_files(note_id: int) -> list[str]:
    """Every file in the vault as root-relative posix paths (no content read). Used to
    resolve/repair the project's main file; the editor pulls actual content via the FS RPC."""
    root = vault_root(note_id)
    if not root.is_dir():
        return []
    return sorted(
        p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()
    )


def delete_vault(note_id: int) -> None:
    """Remove an editor project's entire vault tree (best-effort)."""
    root = vault_root(note_id)
    if root.is_dir():
        shutil.rmtree(root, ignore_errors=True)
