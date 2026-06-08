"""Editor projects = the note-link half of the embedded-editor "vault + note-link" model.

A linXiv NOTE is general: it can stand alone (a paper annotation) OR reference an outside
object. An *editor project* is the latter — a NOTE whose content carries a small frontmatter
block declaring it owns an on-disk LaTeX vault:

    ---
    linxiv-editor-vault: true
    projectName: My Draft
    mainFile: main.tex
    ---
    <optional human-readable note body>

The vault itself lives at ``vault_dir()/note_<NOTE_SK>/`` (see service/vault.py). The NOTE's
SOURCE_FK pins it to a paper (required by the schema); editor projects with no real paper
attach to a sentinel paper root (``texbrain:local``). An optional PROJECT_FK scopes it to a
linXiv project. No schema migration — the link is pure frontmatter convention.
"""

from __future__ import annotations

import datetime

from service.models.note import NoteDetails
import service.note as _note
from service.paper import ensure_paper_root
import service.vault as _vault

# Sentinel paper root for standalone editor projects (those not about a specific paper).
STANDALONE_SOURCE_ID = "texbrain:local"

VAULT_FLAG = "linxiv-editor-vault"

DEFAULT_TEX = """\\documentclass{article}

\\title{Untitled Document}
\\author{Author}
\\date{\\today}

\\begin{document}
\\maketitle

\\section{Introduction}
Hello, world!

\\end{document}
"""


# ── frontmatter parse / build ───────────────────────────────────────────────────

def parse_frontmatter(content: str | None) -> tuple[dict[str, str], str]:
    """Split a note body into (frontmatter dict, remaining body). A note with no leading
    ``---`` block yields ({}, content)."""
    if not content:
        return {}, ""
    lines = content.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}, content
    end = next((i for i in range(1, len(lines)) if lines[i].strip() == "---"), None)
    if end is None:
        return {}, content
    meta: dict[str, str] = {}
    for line in lines[1:end]:
        if ":" in line:
            k, v = line.split(":", 1)
            meta[k.strip()] = v.strip()
    return meta, "\n".join(lines[end + 1:])


def _sanitize_line(s: str | None) -> str:
    """Collapse CR/LF to spaces and trim. Frontmatter is line-oriented, so a newline in a
    value would terminate/forge the block (see review findings); never let one through."""
    return (s or "").replace("\r", " ").replace("\n", " ").strip()


def build_content(project_name: str, main_file: str, body: str = "") -> str:
    # Sanitize again at the serializer (defense in depth) so a stray newline can never
    # break or inject into the frontmatter fence regardless of caller.
    fm = (
        "---\n"
        f"{VAULT_FLAG}: true\n"
        f"projectName: {_sanitize_line(project_name)}\n"
        f"mainFile: {_sanitize_line(main_file)}\n"
        "---\n"
    )
    return fm + (body or "")


def _is_editor_project(meta: dict[str, str]) -> bool:
    return str(meta.get(VAULT_FLAG, "")).strip().lower() == "true"


def _to_summary(note: NoteDetails, meta: dict[str, str]) -> dict:
    return {
        "noteId": note.note_id,
        "projectName": meta.get("projectName") or note.title or f"project {note.note_id}",
        "mainFile": meta.get("mainFile") or "main.tex",
        "sourceFk": note.source_fk,
        "projectId": note.project_id,
        "updatedAt": (
            note.updated_at.isoformat()
            if isinstance(note.updated_at, datetime.datetime)
            else note.updated_at
        ),
    }


# ── operations used by the /api/editor routes ────────────────────────────────────

def list_projects(project_id: int | None = None) -> list[dict]:
    """Editor-project notes (those flagged in frontmatter), newest first, optionally
    scoped to a linXiv project."""
    out: list[dict] = []
    for note in _note.list_all():
        meta, _ = parse_frontmatter(note.content)
        if not _is_editor_project(meta):
            continue
        if project_id is not None and note.project_id != project_id:
            continue
        out.append(_to_summary(note, meta))
    out.sort(key=lambda p: p.get("updatedAt") or "", reverse=True)
    return out


def create_project(
    project_name: str,
    main_file: str = "main.tex",
    source_id: str | None = None,
    project_id: int | None = None,
) -> dict:
    """Create an editor-project note + scaffold its vault with a starter main file."""
    name = _sanitize_line(project_name) or "Untitled"
    main = _sanitize_line(main_file) or "main.tex"
    # Validate the main file is a safe, contained relative path BEFORE creating the note,
    # so a bad name yields a clean 400 (via the route) rather than a 500 with an orphaned
    # note that has no vault. Raises ValueError on traversal/absolute paths.
    _vault._safe_path(0, main)
    source_fk = ensure_paper_root((source_id or STANDALONE_SOURCE_ID).strip())
    note_id = _note.create(_note.NoteIn(
        source_fk=source_fk,
        project_fk=project_id,
        title=name,
        content=build_content(name, main),
    ))
    # Scaffold the vault so the first doc:open + list() find a real main file. If this
    # fails, roll back the note so we never leave a flagged project with no vault.
    try:
        _vault.write_file(note_id, main, DEFAULT_TEX, binary=False)
    except Exception:
        _note.delete(_note.Note(note_id=note_id))
        raise
    return {"noteId": note_id, "projectName": name, "mainFile": main}


def get_meta(note_id: int) -> tuple[NoteDetails, dict[str, str]] | None:
    """Return (note, frontmatter) for an editor-project note, or None if the note is
    missing or is not an editor project."""
    note = _note.get(_note.Note(note_id=note_id))
    if note is None:
        return None
    meta, _ = parse_frontmatter(note.content)
    if not _is_editor_project(meta):
        return None
    return note, meta


def get_doc(note_id: int) -> dict | None:
    """Assemble the DocOpenPayload {mainFile, files, projectName} the host pushes to the
    editor. `files` is empty: the guest mounts the vault as its projectHandle and pulls
    every file (text + binary) lazily through the FS RPC, so shipping the whole tree as
    text here would be redundant work. We only resolve a sane main file."""
    found = get_meta(note_id)
    if found is None:
        return None
    note, meta = found
    main = meta.get("mainFile") or "main.tex"
    # The recorded main file may be stale (e.g. renamed in-editor). Fall back to a present
    # .tex so the project never opens empty.
    existing = _vault.list_files(note_id)
    if main not in existing:
        tex = sorted(p for p in existing if p.lower().endswith(".tex"))
        if tex:
            main = "main.tex" if "main.tex" in tex else tex[0]
    return {
        "mainFile": main,
        "files": {},
        "projectName": meta.get("projectName") or note.title or f"project {note_id}",
    }
