"""Export and import projects as .lxproj archive files.

File format: a zip archive with the extension .lxproj containing:
  manifest.json          — project metadata, papers, and notes (no local IDs)
  pdfs/<name>_v<n>.pdf   — optional; only present when exported with include_pdfs=True
"""

from __future__ import annotations

import datetime
import json
import shutil
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

import service.note as _note
import service.paper as _paper
import service.project as _project
from sources.base import PaperMetadata
from storage.paths import pdf_dir


_FORMAT_VERSION = 1


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------

class ProjectImportError(Exception):
    """Raised when commit_import fails mid-way. The partially-created project
    is deleted before this is raised, so the DB is left clean."""


# ---------------------------------------------------------------------------
# Public result types
# ---------------------------------------------------------------------------

@dataclass
class ImportPreview:
    """Summary read from a .lxproj file without modifying the database."""
    project_name: str
    description: str
    paper_count: int
    note_count: int
    has_pdfs: bool
    format_version: int


# ---------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------

def export_project(
    project_fk: int,
    dest_path: Path,
    include_pdfs: bool = False,
) -> Path:
    """Write a project to a .lxproj file at dest_path.

    dest_path may omit the extension — .lxproj is appended regardless.
    Returns the path of the written archive.
    """
    details = _project.get(_project.Project(project_fk=project_fk))
    if details is None:
        raise ValueError(f"Project {project_fk} not found")

    papers = _paper.get_many(_paper.Papers(source_fks=details.source_fks))
    notes  = _note.get_many(_note.Notes(project_fk=project_fk))

    paper_dicts = [_serialize_paper(p) for p in papers]

    note_dicts = []
    for n in notes:
        source_id = _paper.get_source_id(n.source_fk)
        if source_id is None:
            continue
        version = None
        if n.paper_id_fk:
            pinned = _paper.get(_paper.Paper(paper_id=n.paper_id_fk))
            if pinned:
                version = pinned.version
        note_dicts.append({
            "paper_source_id": source_id,
            "paper_version":   version,
            "title":           n.title,
            "content":         n.content,
        })

    pdf_files: dict[str, Path] = {}  # archive entry name → local path
    if include_pdfs:
        pdf_root = pdf_dir()
        for p in papers:
            if not p.pdf_path:
                continue
            local = Path(p.pdf_path)
            if not local.is_absolute():
                local = pdf_root / local
            if local.exists():
                pdf_files[f"pdfs/{p.source_id}_v{p.version}.pdf"] = local

    color_hex = _project.color_to_hex(details.color) if details.color else None

    manifest = {
        "format_version": _FORMAT_VERSION,
        "exported_at": datetime.datetime.now().isoformat(),
        "summary": {
            "paper_count": len(paper_dicts),
            "note_count":  len(note_dicts),
            "has_pdfs":    bool(pdf_files),
        },
        "project": {
            "name":        details.name,
            "description": details.description,
            "color_hex":   color_hex,
            "tags":        details.project_tags,
        },
        "papers": paper_dicts,
        "notes":  note_dicts,
    }

    dest_path = dest_path.with_suffix(".lxproj")
    with zipfile.ZipFile(dest_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("manifest.json", json.dumps(manifest, indent=2))
        for archive_name, local_path in pdf_files.items():
            zf.write(local_path, archive_name)

    return dest_path


# ---------------------------------------------------------------------------
# Import — two-phase
# ---------------------------------------------------------------------------

def preview_import(zip_path: Path) -> ImportPreview:
    """Parse a .lxproj archive and return a preview without touching the database."""
    manifest = _read_manifest(zip_path)
    proj    = manifest["project"]
    summary = manifest.get("summary", {})
    return ImportPreview(
        project_name   = proj["name"],
        description    = proj.get("description", ""),
        paper_count    = summary.get("paper_count", len(manifest.get("papers", []))),
        note_count     = summary.get("note_count",  len(manifest.get("notes",  []))),
        has_pdfs       = summary.get("has_pdfs", False),
        format_version = manifest.get("format_version", 1),
    )


def commit_import(
    zip_path: Path,
    *,
    on_conflict: Literal["merge", "overwrite"] = "merge",
) -> int:
    """Import a .lxproj archive. Returns the new project_fk.

    on_conflict="merge"     — skip re-saving paper metadata when source_id already
                              exists; link the existing paper to the imported project.
    on_conflict="overwrite" — re-save all paper metadata from the archive, replacing
                              what is in the database.
    """
    manifest  = _read_manifest(zip_path)
    proj_data = manifest["project"]
    paper_dicts = manifest.get("papers", [])
    note_dicts  = manifest.get("notes",  [])

    color_int = None
    if proj_data.get("color_hex"):
        color_int = _project.color_from_hex(proj_data["color_hex"])

    project_fk = _project.upsert(
        _project.ProjectIn(
            name        = proj_data["name"],
            description = proj_data.get("description", ""),
            color       = color_int,
            tags        = proj_data.get("tags", []),
            source_fks  = [],
        )
    )

    try:
        _commit_import_body(project_fk, paper_dicts, note_dicts, zip_path, on_conflict)
    except Exception as exc:
        print(f"[export_import] import failed, rolling back project {project_fk}: {exc}")
        _project.delete(_project.Project(project_fk=project_fk))
        raise ProjectImportError(str(exc)) from exc

    return project_fk


def _commit_import_body(
    project_fk: int,
    paper_dicts: list[dict],
    note_dicts: list[dict],
    zip_path: Path,
    on_conflict: str,
) -> None:
    # Import papers, resolving each to a SOURCE_FK
    source_id_to_fk: dict[str, int] = {}
    for pd in paper_dicts:
        source_id    = pd["source_id"]
        existing_root = _paper.get_paper_root(source_id)

        if existing_root and on_conflict == "merge":
            source_fk = int(existing_root["SOURCE_FK"])
            if str(existing_root["STATUS"]) == "deleted":
                _paper.restore(_paper.Paper(source_fk=source_fk))
            # Union any tags from the import that the existing paper doesn't have
            if pd.get("tags"):
                _paper.add_paper_tags(source_id, pd["tags"])
        elif existing_root:  # on_conflict == "overwrite"
            source_fk = int(existing_root["SOURCE_FK"])
            if str(existing_root["STATUS"]) == "deleted":
                _paper.restore(_paper.Paper(source_fk=source_fk))
            meta = _deserialize_paper(pd)
            _paper.repair_paper(source_fk, meta)
            if pd.get("tags"):
                _paper.add_paper_tags(source_id, pd["tags"])
        else:
            meta = _deserialize_paper(pd)
            _paper.save_paper_metadata(meta, pd.get("tags") or None)
            source_fk = _paper.ensure_paper_root(source_id)

        source_id_to_fk[source_id] = source_fk

    # Link all papers to the newly created project in order. Every id was
    # saved or matched above, so an unresolved id means the save step and the
    # membership lookup disagree on id form — fail the import (the caller
    # rolls the project back) rather than produce a partially linked project.
    if source_id_to_fk:
        failed = _project.add_papers(project_fk, list(source_id_to_fk.keys()))
        if failed:
            raise RuntimeError(
                f"{len(failed)} imported paper(s) could not be linked to the project: {failed[:5]}"
            )

    # Copy any bundled PDFs into the local pdf directory
    _import_pdfs(zip_path, source_id_to_fk)

    # Import notes, re-resolving paper references by source_id
    for nd in note_dicts:
        paper_source_id = nd.get("paper_source_id")
        if not paper_source_id:
            continue
        source_fk = source_id_to_fk.get(paper_source_id)
        if source_fk is None:
            continue

        paper_id = None
        pinned_version = nd.get("paper_version")
        if pinned_version:
            row = _paper.get_paper(paper_source_id, pinned_version)
            if row:
                paper_id = row["paper_id"]

        _note.create(
            _note.NoteIn(
                source_fk  = source_fk,
                title      = nd.get("title", ""),
                content    = nd.get("content", ""),
                paper_id   = paper_id,
                project_fk = project_fk,
            )
        )


# ---------------------------------------------------------------------------
# Private helpers
# ---------------------------------------------------------------------------

def _read_manifest(zip_path: Path) -> dict:
    with zipfile.ZipFile(zip_path, "r") as zf:
        if "manifest.json" not in zf.namelist():
            raise ValueError(f"{zip_path.name} is not a valid .lxproj file: manifest.json missing")
        return json.loads(zf.read("manifest.json"))


def _serialize_paper(p) -> dict:
    return {
        "source_id":   p.source_id,
        "version":     p.version,
        "title":       p.title,
        "authors":     p.authors or [],
        "published":   p.published.isoformat() if p.published else None,
        "updated":     p.updated.isoformat()   if p.updated   else None,
        "summary":     p.summary or "",
        "category":    p.category,
        "categories":  p.categories,
        "doi":         p.doi,
        "journal_ref": p.journal_ref,
        "comment":     p.comment,
        "url":         p.url,
        "tags":        p.tags or [],
        "source":      p.source,
    }


def _deserialize_paper(pd: dict) -> PaperMetadata:
    published_raw = pd.get("published")
    updated_raw   = pd.get("updated")
    return PaperMetadata(
        source_id   = pd["source_id"],
        version     = pd.get("version", 1),
        title       = pd["title"],
        authors     = pd.get("authors", []),
        published   = datetime.date.fromisoformat(published_raw) if published_raw else datetime.date.today(),
        updated     = datetime.date.fromisoformat(updated_raw)   if updated_raw   else None,
        summary     = pd.get("summary", ""),
        category    = pd.get("category"),
        categories  = pd.get("categories"),
        doi         = pd.get("doi"),
        journal_ref = pd.get("journal_ref"),
        comment     = pd.get("comment"),
        url         = pd.get("url"),
        tags        = pd.get("tags") or None,
        source      = pd.get("source"),
    )


def _import_pdfs(zip_path: Path, source_id_to_fk: dict[str, int]) -> None:
    """Extract bundled PDFs from the archive into the local pdf directory."""
    dest_dir = pdf_dir()
    dest_dir.mkdir(parents=True, exist_ok=True)

    with zipfile.ZipFile(zip_path, "r") as zf:
        pdf_entries = [n for n in zf.namelist() if n.startswith("pdfs/") and n.endswith(".pdf")]
        for entry in pdf_entries:
            basename = Path(entry).name          # e.g. "2204.12985_v1.pdf"
            stem     = basename[:-4]             # strip ".pdf"
            sep_idx  = stem.rfind("_v")          # last "_v" separates source_id from version
            if sep_idx == -1:
                continue

            source_id   = stem[:sep_idx]
            version_str = stem[sep_idx + 2:]

            if source_id not in source_id_to_fk:
                continue

            dest_path = dest_dir / basename
            with zf.open(entry) as src, open(dest_path, "wb") as dst:
                shutil.copyfileobj(src, dst)

            try:
                version = int(version_str)
            except ValueError:
                version = 1

            try:
                _paper.mark_pdf_saved(source_id, str(dest_path), version)
            except RuntimeError as exc:
                # Bundled PDF names a version that wasn't imported; skip it
                # and remove the file already extracted to disk.
                dest_path.unlink(missing_ok=True)
                print(f"[import] skipping PDF for {basename}: {exc}")
