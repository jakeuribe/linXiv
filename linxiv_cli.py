"""Headless CLI for linXiv — search, fetch, list, tag, and manage projects without the GUI."""

from __future__ import annotations

from dotenv import load_dotenv
from config import ENV_PATH, init_data_dir
load_dotenv(ENV_PATH)

import argparse
import dataclasses
import datetime
from importlib.metadata import version, PackageNotFoundError
import json
from pathlib import Path
import re
import sys
from typing import Any

try:
    __version__ = version("linxiv")
except PackageNotFoundError:
    __version__ = "unknown"

from formats.bibtex import BibTeXFormat
from formats.markdown import ObsidianFormat
from sources.arxiv_source import ArxivSource
from sources.base import PaperMetadata, PaperSource
from sources.crossref_source import CrossRefSource
from sources.doi_resolve import resolve_doi
from sources.openalex_source import OpenAlexSource
from storage.projects import remove_paper_from_all_projects as _remove_paper_from_all_projects
import user_settings

import service.author as svc_author
import service.paper as svc_paper
import service.tag as svc_tag
import service.project as svc_project
import service.note as svc_note
import service.files as svc_files
import service.export_import as svc_ei
from service.author import Author
from service.paper import Paper, Papers
from service.tag import Tag, TagIn
from service.project import Project, Projects, ProjectIn, Status, UNSET
from service.note import Note, Notes, NoteIn, NoteUpdateIn

_FORMATS_DIR = Path(__file__).parent / "formats"

_ARXIV_ID_RE = re.compile(r"^\d{4}\.\d{4,5}(v\d+)?$|^[a-z\-]+(\.[A-Z]{2})?/\d{7}(v\d+)?$")


def _validate_arxiv_id(source_id: str) -> str:
    if not _ARXIV_ID_RE.match(source_id):
        print(json.dumps({"error": f"Invalid arXiv ID format: {source_id!r}"}), file=sys.stderr)
        sys.exit(1)
    return source_id


def _as_source_id(raw: str, source: str = "arxiv") -> str:
    """Prefix a bare paper ID with its namespace; already-prefixed IDs are returned unchanged."""
    return raw if ":" in raw else f"{source}:{raw}"


def _render_paper(meta: PaperMetadata) -> str | None:
    template_path = _FORMATS_DIR / f"{meta.source}_paper.md"
    if not template_path.exists():
        return None
    template = template_path.read_text(encoding="utf-8")
    data = meta.model_dump(mode="json")
    data["authors_inline"] = ", ".join(meta.authors)
    return template.format_map(data)


_SOURCES: dict[str, type[PaperSource]] = {
    "arxiv":    ArxivSource,
    "openalex": OpenAlexSource,
    "crossref": CrossRefSource,
}


def _source_for(name: str) -> PaperSource:
    cls = _SOURCES.get(name)
    if cls is None:
        raise ValueError(f"Unknown source {name!r}. Available: {list(_SOURCES)}")
    return cls()


def _output(data: Any) -> None:
    json.dump(data, sys.stdout, indent=2, default=str)
    sys.stdout.write("\n")


def _details_to_dict(obj: Any) -> dict[str, Any]:
    return dataclasses.asdict(obj)


def _resolve_paper_or_exit(source_id: str) -> Any:
    details = svc_paper.get(Paper(source_id=source_id))
    if details is None:
        print(json.dumps({"error": f"Paper {source_id!r} not found in DB"}), file=sys.stderr)
        sys.exit(1)
    return details


def _resolve_project_or_exit(project_id: int) -> Any:
    details = svc_project.get(Project(project_fk=project_id))
    if details is None:
        print(json.dumps({"error": f"Project {project_id} not found"}), file=sys.stderr)
        sys.exit(1)
    return details


# ---------------------------------------------------------------------------
# Commands — search / fetch / list
# ---------------------------------------------------------------------------

def cmd_search(args: argparse.Namespace) -> None:
    source = _source_for(args.source)
    try:
        results = source.search(args.query, max_results=args.max)
    except Exception as e:
        print(f"[search] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    _output([m.model_dump(mode="json") for m in results])


def cmd_fetch(args: argparse.Namespace) -> None:
    if args.source == "arxiv":
        _validate_arxiv_id(args.source_id)
    source = _source_for(args.source)
    try:
        meta = source.fetch_by_id(args.source_id)
    except Exception as e:
        print(f"[fetch] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    svc_paper.save_paper_metadata(meta, None)
    rendered = _render_paper(meta)
    if rendered:
        sys.stdout.write(rendered + "\n")
    else:
        _output(meta.model_dump(mode="json"))


def cmd_list(args: argparse.Namespace) -> None:
    if args.category:
        rows = svc_paper.list_papers(limit=None, offset=0)
        papers = [p for p in [{k: row[k] for k in row.keys()} for row in rows]
                  if p.get("category") == args.category]
        if args.offset:
            papers = papers[args.offset:]
        if args.limit is not None:
            papers = papers[:args.limit]
    else:
        rows = svc_paper.list_papers(limit=args.limit, offset=args.offset)
        papers = [{k: row[k] for k in row.keys()} for row in rows]
    _output(papers)


# ---------------------------------------------------------------------------
# Commands — paper subgroup
# ---------------------------------------------------------------------------

def cmd_paper_get(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    details = _resolve_paper_or_exit(source_id)
    _output(_details_to_dict(details))


def cmd_paper_delete(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    _resolve_paper_or_exit(source_id)
    svc_paper.delete(svc_paper.Paper(source_id=source_id))
    _output({"deleted": source_id})


def cmd_paper_versions(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    all_versions = svc_paper.get_all(Paper(source_id=source_id))
    if all_versions is None:
        print(json.dumps({"error": f"Paper {source_id!r} not found in DB"}), file=sys.stderr)
        sys.exit(1)
    _output(_details_to_dict(all_versions))


def cmd_paper_repair(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    root = svc_paper.get_paper_root(source_id)
    if root is None:
        print(json.dumps({"error": f"Paper {source_id!r} not found"}), file=sys.stderr)
        sys.exit(1)
    source_fk = int(root["SOURCE_FK"])
    existing = svc_paper.get(Paper(source_id=source_id))
    version = existing.version if existing is not None else 1
    try:
        published = datetime.date.fromisoformat(args.published)
    except ValueError:
        print(json.dumps({"error": f"Invalid date {args.published!r}; use YYYY-MM-DD"}), file=sys.stderr)
        sys.exit(1)
    meta = PaperMetadata(
        source_id=source_id,
        version=version,
        title=args.title,
        authors=args.authors,
        published=published,
        summary=args.summary or "",
        category=args.category,
        doi=args.doi,
        url=args.url,
        tags=args.tags or None,
        source=None,
    )
    svc_paper.repair_paper(source_fk, meta)
    _output({"repaired": source_id})


def _do_paper_restore(source_id: str) -> dict:
    if not svc_paper.is_paper_deleted(source_id):
        print(json.dumps({"error": f"Paper {source_id!r} not found in trash"}), file=sys.stderr)
        sys.exit(1)
    pdf_path, project_fks = svc_paper.restore(Paper(source_id=source_id))
    return {"restored": source_id, "pdf_path": pdf_path, "project_fks": project_fks}


def _do_paper_hard_delete(source_id: str) -> dict:
    root = svc_paper.get_paper_root(source_id)
    if root is None:
        print(json.dumps({"error": f"Paper {source_id!r} not found"}), file=sys.stderr)
        sys.exit(1)
    svc_paper.hard_delete(Paper(source_id=source_id))
    return {"hard_deleted": source_id}


def cmd_paper_restore(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    _output(_do_paper_restore(source_id))


def cmd_paper_hard_delete(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    _output(_do_paper_hard_delete(source_id))


def cmd_paper_search(args: argparse.Namespace) -> None:
    results = svc_paper.search_papers(args.query, limit=args.limit)
    _output([_details_to_dict(r) for r in results])


def cmd_paper_remove_from_all(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    root = svc_paper.get_paper_root(source_id)
    if root is None:
        print(json.dumps({"error": f"Paper {source_id!r} not found"}), file=sys.stderr)
        sys.exit(1)
    source_fk = int(root["SOURCE_FK"])
    removed = _remove_paper_from_all_projects(source_fk)
    _output({"source_id": source_id, "removed_from_projects": removed})


# ---------------------------------------------------------------------------
# Commands — trash subgroup
# ---------------------------------------------------------------------------

def cmd_trash_list(args: argparse.Namespace) -> None:
    papers = svc_paper.list_deleted()
    projects = svc_project.list_deleted()
    _output({
        "papers": [_details_to_dict(p) for p in papers],
        "projects": [_details_to_dict(p) for p in projects],
    })


def cmd_trash_restore(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    _output(_do_paper_restore(source_id))


def cmd_trash_hard_delete(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    if not svc_paper.is_paper_deleted(source_id):
        print(json.dumps({"error": f"Paper {source_id!r} not found in trash"}), file=sys.stderr)
        sys.exit(1)
    _output(_do_paper_hard_delete(source_id))


def cmd_trash_restore_project(args: argparse.Namespace) -> None:
    details = _resolve_project_or_exit(args.project_id)
    if details.status != Status.DELETED:
        print(json.dumps({"error": f"Project {args.project_id} is not in trash"}), file=sys.stderr)
        sys.exit(1)
    svc_project.restore(Project(project_fk=args.project_id))
    _output({"restored_project_id": args.project_id})


def cmd_trash_hard_delete_project(args: argparse.Namespace) -> None:
    details = _resolve_project_or_exit(args.project_id)
    if details.status != Status.DELETED:
        print(json.dumps({"error": f"Project {args.project_id} is not in trash"}), file=sys.stderr)
        sys.exit(1)
    svc_project.hard_delete(Project(project_fk=args.project_id))
    _output({"hard_deleted_project_id": args.project_id})


# ---------------------------------------------------------------------------
# Commands — doi subgroup
# ---------------------------------------------------------------------------

def cmd_doi_resolve(args: argparse.Namespace) -> None:
    try:
        meta = resolve_doi(args.doi)
    except Exception as e:
        print(f"[doi] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    _output(meta.model_dump(mode="json"))


def cmd_doi_save(args: argparse.Namespace) -> None:
    try:
        meta = resolve_doi(args.doi)
    except Exception as e:
        print(f"[doi] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    source_id, ver = svc_paper.save_paper_metadata(meta)
    _output({"source_id": source_id, "version": ver, "title": meta.title})


# ---------------------------------------------------------------------------
# Commands — author subgroup
# ---------------------------------------------------------------------------

def cmd_author_list(args: argparse.Namespace) -> None:
    authors = svc_author.list_with_paper_count()
    _output([_details_to_dict(a) for a in authors])


def cmd_author_get(args: argparse.Namespace) -> None:
    author = svc_author.get(Author(author_id=args.author_id))
    if author is None:
        print(json.dumps({"error": f"Author {args.author_id} not found"}), file=sys.stderr)
        sys.exit(1)
    previews = svc_author.get_paper_previews(args.author_id)
    result = _details_to_dict(author)
    result["papers"] = [_details_to_dict(p) for p in previews]
    _output(result)


def cmd_author_update(args: argparse.Namespace) -> None:
    if svc_author.get(Author(author_id=args.author_id)) is None:
        print(json.dumps({"error": f"Author {args.author_id} not found"}), file=sys.stderr)
        sys.exit(1)
    if args.full_name is None and args.first_name is None and args.last_name is None and args.orcid is None:
        print(json.dumps({"error": "at least one of --full-name, --first-name, --last-name, or --orcid must be provided"}), file=sys.stderr)
        sys.exit(1)
    svc_author.update_fields(
        author_id=args.author_id,
        full_name=args.full_name,
        first_name=args.first_name,
        last_name=args.last_name,
        orcid=args.orcid,
    )
    _output({"updated_author_id": args.author_id})


def cmd_author_delete(args: argparse.Namespace) -> None:
    link_count = svc_author.count_paper_links(args.author_id)
    if link_count > 0:
        print(
            json.dumps({"error": f"Author {args.author_id} is linked to {link_count} paper(s); unlink first"}),
            file=sys.stderr,
        )
        sys.exit(1)
    svc_author.delete_author(args.author_id)
    _output({"deleted_author_id": args.author_id})


# ---------------------------------------------------------------------------
# Commands — tag subgroup
# ---------------------------------------------------------------------------

def cmd_tag_add(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    try:
        updated = svc_tag.add_paper_tags(source_id, args.tags)
    except KeyError:
        print(json.dumps({"error": f"Paper {source_id} not found in DB"}), file=sys.stderr)
        sys.exit(1)
    _output({"source_id": source_id, "tags": updated})


def cmd_tag_remove(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    try:
        updated = svc_tag.remove_paper_tags(source_id, args.tags)
    except KeyError:
        print(json.dumps({"error": f"Paper {source_id} not found in DB"}), file=sys.stderr)
        sys.exit(1)
    _output({"source_id": source_id, "tags": updated})


def cmd_tag_list(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    tags = svc_tag.get_paper_tags(source_id)
    _output({"source_id": source_id, "tags": tags})


def cmd_tag_list_all(args: argparse.Namespace) -> None:
    _output(svc_tag.list_all_tags())


def cmd_tag_create(args: argparse.Namespace) -> None:
    tag_id = svc_tag.upsert(TagIn(label=args.label))
    _output({"tag_id": tag_id, "label": args.label})


def cmd_tag_delete(args: argparse.Namespace) -> None:
    svc_tag.delete(Tag(tag_id=args.tag_id))
    _output({"deleted_tag_id": args.tag_id})


def cmd_tag_add_project(args: argparse.Namespace) -> None:
    _resolve_project_or_exit(args.project_id)
    updated = svc_tag.add_project_tags(args.project_id, args.tags)
    _output({"project_id": args.project_id, "tags": updated})


def cmd_tag_remove_project(args: argparse.Namespace) -> None:
    _resolve_project_or_exit(args.project_id)
    updated = svc_tag.remove_project_tags(args.project_id, args.tags)
    _output({"project_id": args.project_id, "tags": updated})


def cmd_tag_list_project(args: argparse.Namespace) -> None:
    details = _resolve_project_or_exit(args.project_id)
    _output({"project_id": args.project_id, "tags": details.project_tags})


# ---------------------------------------------------------------------------
# Commands — project subgroup
# ---------------------------------------------------------------------------

def cmd_project_list(args: argparse.Namespace) -> None:
    status = Status(args.status) if args.status else None
    projects = svc_project.get_many(Projects(status=status))
    if status is None:
        projects = [p for p in projects if p.status != Status.DELETED]
    _output([{
        "id": p.id,
        "name": p.name,
        "description": p.description,
        "status": p.status.value,
        "paper_count": len(p.source_fks),
        "color": p.color,
        "project_tags": p.project_tags,
    } for p in projects])


def cmd_project_get(args: argparse.Namespace) -> None:
    details = _resolve_project_or_exit(args.project_id)
    _output(_details_to_dict(details))


def cmd_project_create(args: argparse.Namespace) -> None:
    color = svc_project.color_from_hex(args.color) if args.color else None
    tags = args.tags or []
    fk = svc_project.upsert(ProjectIn(
        name=args.name,
        description=args.description or "",
        color=color,
        tags=tags,
    ))
    _output({"id": fk, "name": args.name, "status": "active"})


def cmd_project_update(args: argparse.Namespace) -> None:
    _resolve_project_or_exit(args.project_id)
    try:
        color: Any = UNSET
        if args.color is not None:
            color = svc_project.color_from_hex(args.color)
        status = Status(args.status) if args.status else None
        svc_project.update(
            project_fk=args.project_id,
            name=args.name,
            description=args.description,
            color=color,
            project_tags=args.tags,
            status=status,
        )
    except (LookupError, ValueError) as e:
        print(f"[project] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    updated = svc_project.get(Project(project_fk=args.project_id))
    _output(_details_to_dict(updated))


def cmd_project_delete(args: argparse.Namespace) -> None:
    _resolve_project_or_exit(args.project_id)
    svc_project.delete(Project(project_fk=args.project_id))
    _output({"deleted_project_id": args.project_id})


def cmd_project_archive(args: argparse.Namespace) -> None:
    _resolve_project_or_exit(args.project_id)
    svc_project.archive(Project(project_fk=args.project_id))
    _output({"archived_project_id": args.project_id})


def cmd_project_restore(args: argparse.Namespace) -> None:
    _resolve_project_or_exit(args.project_id)
    svc_project.restore(Project(project_fk=args.project_id))
    _output({"restored_project_id": args.project_id})


def cmd_project_hard_delete(args: argparse.Namespace) -> None:
    _resolve_project_or_exit(args.project_id)
    svc_project.hard_delete(Project(project_fk=args.project_id))
    _output({"hard_deleted_project_id": args.project_id})


def cmd_project_add_paper(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    details = _resolve_project_or_exit(args.project_id)
    root = svc_paper.get_paper_root(source_id)
    if root is None:
        print(json.dumps({"error": f"Paper {source_id} not found in database"}), file=sys.stderr)
        sys.exit(1)
    source_fk = int(root["SOURCE_FK"])
    if source_fk not in details.source_fks:
        svc_project.upsert(ProjectIn(
            name=details.name,
            description=details.description,
            color=details.color,
            tags=details.project_tags,
            source_fks=details.source_fks + [source_fk],
        ), project_fk=args.project_id)
    _output({"project_id": args.project_id, "source_id": source_id})


def cmd_project_remove_paper(args: argparse.Namespace) -> None:
    source_id = _as_source_id(args.source_id)
    details = _resolve_project_or_exit(args.project_id)
    root = svc_paper.get_paper_root(source_id)
    if root is None:
        print(json.dumps({"error": f"Paper {source_id} not found in database"}), file=sys.stderr)
        sys.exit(1)
    source_fk = int(root["SOURCE_FK"])
    svc_project.upsert(ProjectIn(
        name=details.name,
        description=details.description,
        color=details.color,
        tags=details.project_tags,
        source_fks=[fk for fk in details.source_fks if fk != source_fk],
    ), project_fk=args.project_id)
    _output({"project_id": args.project_id, "source_id": source_id, "removed": True})


# ---------------------------------------------------------------------------
# Commands — project export / import
# ---------------------------------------------------------------------------

def cmd_project_export(args: argparse.Namespace) -> None:
    try:
        out = svc_ei.export_project(args.project_id, Path(args.dest), include_pdfs=args.pdfs)
    except Exception as e:
        print(f"[export] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    _output({"path": str(out), "project_id": args.project_id})


def cmd_project_import(args: argparse.Namespace) -> None:
    zip_path = Path(args.zip_path)
    if args.preview:
        try:
            preview = svc_ei.preview_import(zip_path)
        except Exception as e:
            print(f"[import] {e}", file=sys.stderr)
            print(json.dumps({"error": str(e)}), file=sys.stderr)
            sys.exit(1)
        _output(_details_to_dict(preview))
    else:
        try:
            fk = svc_ei.commit_import(zip_path, on_conflict=args.on_conflict)
        except Exception as e:
            print(f"[import] {e}", file=sys.stderr)
            print(json.dumps({"error": str(e)}), file=sys.stderr)
            sys.exit(1)
        _output({"project_id": fk})


def cmd_project_export_bibtex(args: argparse.Namespace) -> None:
    details = _resolve_project_or_exit(args.project_id)
    papers = svc_paper.get_many(Papers(source_fks=details.source_fks)) if details.source_fks else []
    bibtex_str = BibTeXFormat().export_papers([_details_to_dict(p) for p in papers])
    dest = Path(args.dest)
    if not dest.suffix:
        dest = dest.with_suffix(".bib")
    dest.write_text(bibtex_str, encoding="utf-8")
    _output({"path": str(dest), "project_id": args.project_id})


def cmd_project_export_obsidian(args: argparse.Namespace) -> None:
    details = _resolve_project_or_exit(args.project_id)
    papers = svc_paper.get_many(Papers(source_fks=details.source_fks)) if details.source_fks else []
    md_str = ObsidianFormat().export_papers([_details_to_dict(p) for p in papers])
    dest = Path(args.dest)
    if not dest.suffix:
        dest = dest.with_suffix(".md")
    dest.write_text(md_str, encoding="utf-8")
    _output({"path": str(dest), "project_id": args.project_id})


# ---------------------------------------------------------------------------
# Commands — note subgroup
# ---------------------------------------------------------------------------

def cmd_note_create(args: argparse.Namespace) -> None:
    if args.project_id is not None:
        _resolve_project_or_exit(args.project_id)
    source_id = _as_source_id(args.source_id)
    root = svc_paper.get_paper_root(source_id)
    if root is None:
        print(json.dumps({"error": f"Paper {source_id} not found in DB"}), file=sys.stderr)
        sys.exit(1)
    source_fk = int(root["SOURCE_FK"])
    note_id = svc_note.create(NoteIn(
        source_fk=source_fk,
        title=args.title or "",
        content=args.content,
        project_fk=args.project_id,
    ))
    _output({"id": note_id, "source_fk": source_fk, "project_id": args.project_id, "title": args.title or ""})


def cmd_note_get(args: argparse.Namespace) -> None:
    details = svc_note.get(Note(note_id=args.note_id))
    if details is None:
        print(json.dumps({"error": f"Note {args.note_id} not found"}), file=sys.stderr)
        sys.exit(1)
    _output(_details_to_dict(details))


def cmd_note_list(args: argparse.Namespace) -> None:
    source_fk = None
    if args.source_id:
        sid = _as_source_id(args.source_id)
        root = svc_paper.get_paper_root(sid)
        if root is None:
            print(json.dumps({"error": f"Paper {sid!r} not found in DB"}), file=sys.stderr)
            sys.exit(1)
        source_fk = int(root["SOURCE_FK"])
    if source_fk is None and args.project_id is None:
        notes = svc_note.list_all()
    else:
        notes = svc_note.get_many(Notes(source_fk=source_fk, project_fk=args.project_id))
    _output([_details_to_dict(n) for n in notes])


def cmd_note_update(args: argparse.Namespace) -> None:
    if svc_note.get(Note(note_id=args.note_id)) is None:
        print(json.dumps({"error": f"Note {args.note_id} not found"}), file=sys.stderr)
        sys.exit(1)
    if args.title is None and args.content is None:
        print(json.dumps({"error": "at least one of --title or --content must be provided"}), file=sys.stderr)
        sys.exit(1)
    svc_note.update(NoteUpdateIn(note_id=args.note_id, title=args.title, content=args.content))
    _output({"id": args.note_id, "updated": True})


def cmd_note_delete(args: argparse.Namespace) -> None:
    details = svc_note.get(Note(note_id=args.note_id))
    if details is None:
        print(json.dumps({"error": f"Note {args.note_id} not found"}), file=sys.stderr)
        sys.exit(1)
    svc_note.delete(Note(note_id=args.note_id))
    _output({"deleted_note_id": args.note_id})


# ---------------------------------------------------------------------------
# Commands — pdf subgroup
# ---------------------------------------------------------------------------

def cmd_pdf_path(args: argparse.Namespace) -> None:
    paper = _resolve_paper_or_exit(_as_source_id(args.source_id))
    version = args.version if args.version else paper.version
    path = svc_files.pdf_path(paper.source_id, version, paper.pdf_path)
    _output({"source_id": paper.source_id, "version": version, "path": path})


def cmd_pdf_download(args: argparse.Namespace) -> None:
    paper = _resolve_paper_or_exit(_as_source_id(args.source_id))
    version = args.version if args.version else paper.version
    try:
        path = svc_files.download_pdf(paper.source_id, version, args.url)
    except Exception as e:
        print(f"[pdf] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    if path is None:
        print(json.dumps({"error": "Download failed"}), file=sys.stderr)
        sys.exit(1)
    _output({"source_id": paper.source_id, "version": version, "path": path})


def cmd_pdf_storage(args: argparse.Namespace) -> None:
    mb = svc_files.pdf_storage_mb()
    _output({"storage_mb": round(mb, 3), "pdf_dir": svc_files.managed_pdf_dir()})


def cmd_pdf_import(args: argparse.Namespace) -> None:
    if args.project_id is not None:
        _resolve_project_or_exit(args.project_id)
    pdf_path = Path(args.file)
    try:
        content = pdf_path.read_bytes()
        result = svc_paper.import_pdf(content, args.project_id)
    except Exception as e:
        print(f"[pdf-import] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    _output({"source_id": result.source_id, "title": result.title})


# ---------------------------------------------------------------------------
# Commands — bibtex subgroup
# ---------------------------------------------------------------------------

def cmd_bibtex_import(args: argparse.Namespace) -> None:
    bib_path = Path(args.file)
    if args.project_id is not None:
        _resolve_project_or_exit(args.project_id)
    try:
        metas = BibTeXFormat().import_file(str(bib_path))
    except Exception as e:
        print(f"[bibtex-import] {e}", file=sys.stderr)
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)
    results = svc_paper.save_papers_metadata(metas)
    if args.project_id is not None:
        details = _resolve_project_or_exit(args.project_id)
        existing_fks: set[int] = set(details.source_fks)
        new_fks: list[int] = []
        for source_id, _ in results:
            root = svc_paper.get_paper_root(source_id)
            if root:
                fk = int(root["SOURCE_FK"])
                if fk not in existing_fks:
                    new_fks.append(fk)
                    existing_fks.add(fk)
        if new_fks:
            svc_project.upsert(ProjectIn(
                name=details.name,
                description=details.description,
                color=details.color,
                tags=details.project_tags,
                source_fks=details.source_fks + new_fks,
            ), project_fk=args.project_id)
    _output({"imported": len(results), "papers": [{"source_id": s, "version": v} for s, v in results]})


# ---------------------------------------------------------------------------
# Commands — stats / categories / settings
# ---------------------------------------------------------------------------

def cmd_stats(args: argparse.Namespace) -> None:
    papers = svc_paper.list_paper_details(latest_only=True)
    categories = svc_paper.get_categories()
    all_tags = svc_tag.list_all_tags()
    pdf_count = sum(1 for p in papers if p.has_pdf)
    _output({
        "paper_count": len(papers),
        "tag_count": len(all_tags),
        "category_count": len(categories),
        "pdf_count": pdf_count,
    })


def cmd_categories(args: argparse.Namespace) -> None:
    _output(svc_paper.get_categories())


def cmd_settings_get(args: argparse.Namespace) -> None:
    _output(user_settings.all_settings())


def cmd_settings_update(args: argparse.Namespace) -> None:
    try:
        value: Any = json.loads(args.value)
    except json.JSONDecodeError:
        value = args.value
    user_settings.set(args.key, value)
    _output({args.key: value})


# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="linxiv", description="linXiv headless CLI")
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    sub = parser.add_subparsers(dest="command", required=True)

    _source_choices = list(_SOURCES)

    # ── search ──────────────────────────────────────────────────────────────
    p_search = sub.add_parser("search", help="Search for papers")
    p_search.add_argument("query", help="Search query string")
    p_search.add_argument("--source", choices=_source_choices, default="arxiv")
    p_search.add_argument("--max", type=int, default=10, help="Max results")
    p_search.set_defaults(func=cmd_search)

    # ── fetch ────────────────────────────────────────────────────────────────
    p_fetch = sub.add_parser("fetch", help="Fetch and save a paper by ID")
    p_fetch.add_argument("source_id", help="Paper ID (e.g. 2204.12985 or W3123456789)")
    p_fetch.add_argument("--source", choices=_source_choices, default="arxiv")
    p_fetch.set_defaults(func=cmd_fetch)

    # ── list ─────────────────────────────────────────────────────────────────
    p_list = sub.add_parser("list", help="List papers in the database")
    p_list.add_argument("--limit", type=int, default=None, help="Max papers to return")
    p_list.add_argument("--offset", type=int, default=0, help="Offset for pagination")
    p_list.add_argument("--category", type=str, default=None, help="Filter by category")
    p_list.set_defaults(func=cmd_list)

    # ── paper ────────────────────────────────────────────────────────────────
    p_paper = sub.add_parser("paper", help="Manage individual papers")
    paper_sub = p_paper.add_subparsers(dest="paper_command", required=True)

    p_paper_get = paper_sub.add_parser("get", help="Get full details for a paper")
    p_paper_get.add_argument("source_id", help="Paper source ID")
    p_paper_get.set_defaults(func=cmd_paper_get)

    p_paper_del = paper_sub.add_parser("delete", help="Soft-delete a paper")
    p_paper_del.add_argument("source_id", help="Paper source ID")
    p_paper_del.set_defaults(func=cmd_paper_delete)

    p_paper_ver = paper_sub.add_parser("versions", help="List all stored versions of a paper")
    p_paper_ver.add_argument("source_id", help="Paper source ID")
    p_paper_ver.set_defaults(func=cmd_paper_versions)

    p_paper_repair = paper_sub.add_parser("repair", help="Overwrite paper metadata in-place")
    p_paper_repair.add_argument("source_id", help="Paper source ID")
    p_paper_repair.add_argument("--title", required=True, help="New title")
    p_paper_repair.add_argument("--authors", nargs="+", required=True, help="Author names")
    p_paper_repair.add_argument("--published", required=True, help="Publication date (YYYY-MM-DD)")
    p_paper_repair.add_argument("--summary", default="", help="Abstract / summary")
    p_paper_repair.add_argument("--category", default=None, help="Category")
    p_paper_repair.add_argument("--doi", default=None, help="DOI")
    p_paper_repair.add_argument("--url", default=None, help="URL")
    p_paper_repair.add_argument("--tags", nargs="*", default=None, help="Tags")
    p_paper_repair.set_defaults(func=cmd_paper_repair)

    p_paper_restore = paper_sub.add_parser("restore", help="Restore a soft-deleted paper")
    p_paper_restore.add_argument("source_id", help="Paper source ID")
    p_paper_restore.set_defaults(func=cmd_paper_restore)

    p_paper_hd = paper_sub.add_parser("hard-delete", help="Permanently delete a paper")
    p_paper_hd.add_argument("source_id", help="Paper source ID")
    p_paper_hd.set_defaults(func=cmd_paper_hard_delete)

    p_paper_search = paper_sub.add_parser("search", help="Full-text search within local library")
    p_paper_search.add_argument("query", help="Search query")
    p_paper_search.add_argument("--limit", type=int, default=50, help="Max results")
    p_paper_search.set_defaults(func=cmd_paper_search)

    p_paper_rmall = paper_sub.add_parser(
        "remove-from-all-projects", help="Remove a paper from every project"
    )
    p_paper_rmall.add_argument("source_id", help="Paper source ID")
    p_paper_rmall.set_defaults(func=cmd_paper_remove_from_all)

    # ── tag ──────────────────────────────────────────────────────────────────
    p_tag = sub.add_parser("tag", help="Manage tags")
    tag_sub = p_tag.add_subparsers(dest="tag_command", required=True)

    p_tag_add = tag_sub.add_parser("add", help="Add tags to a paper")
    p_tag_add.add_argument("source_id", help="Paper source ID")
    p_tag_add.add_argument("tags", nargs="+", help="Tags to add")
    p_tag_add.set_defaults(func=cmd_tag_add)

    p_tag_remove = tag_sub.add_parser("remove", help="Remove tags from a paper")
    p_tag_remove.add_argument("source_id", help="Paper source ID")
    p_tag_remove.add_argument("tags", nargs="+", help="Tags to remove")
    p_tag_remove.set_defaults(func=cmd_tag_remove)

    p_tag_list = tag_sub.add_parser("list", help="List tags on a paper")
    p_tag_list.add_argument("source_id", help="Paper source ID")
    p_tag_list.set_defaults(func=cmd_tag_list)

    p_tag_list_all = tag_sub.add_parser("list-all", help="List all tags in the database")
    p_tag_list_all.set_defaults(func=cmd_tag_list_all)

    p_tag_create = tag_sub.add_parser("create", help="Create a tag")
    p_tag_create.add_argument("label", help="Tag label")
    p_tag_create.set_defaults(func=cmd_tag_create)

    p_tag_delete = tag_sub.add_parser("delete", help="Delete a tag by ID")
    p_tag_delete.add_argument("tag_id", type=int, help="Tag ID")
    p_tag_delete.set_defaults(func=cmd_tag_delete)

    p_tag_add_proj = tag_sub.add_parser("add-project", help="Add tags to a project")
    p_tag_add_proj.add_argument("project_id", type=int, help="Project ID")
    p_tag_add_proj.add_argument("tags", nargs="+", help="Tags to add")
    p_tag_add_proj.set_defaults(func=cmd_tag_add_project)

    p_tag_remove_proj = tag_sub.add_parser("remove-project", help="Remove tags from a project")
    p_tag_remove_proj.add_argument("project_id", type=int, help="Project ID")
    p_tag_remove_proj.add_argument("tags", nargs="+", help="Tags to remove")
    p_tag_remove_proj.set_defaults(func=cmd_tag_remove_project)

    p_tag_list_proj = tag_sub.add_parser("list-project", help="List tags on a project")
    p_tag_list_proj.add_argument("project_id", type=int, help="Project ID")
    p_tag_list_proj.set_defaults(func=cmd_tag_list_project)

    # ── project ───────────────────────────────────────────────────────────────
    p_proj = sub.add_parser("project", help="Manage projects")
    proj_sub = p_proj.add_subparsers(dest="project_command", required=True)

    p_proj_list = proj_sub.add_parser("list", help="List projects")
    p_proj_list.add_argument("--status", choices=["active", "archived", "deleted"], default=None)
    p_proj_list.set_defaults(func=cmd_project_list)

    p_proj_get = proj_sub.add_parser("get", help="Get project details")
    p_proj_get.add_argument("project_id", type=int, help="Project ID")
    p_proj_get.set_defaults(func=cmd_project_get)

    p_proj_create = proj_sub.add_parser("create", help="Create a project")
    p_proj_create.add_argument("name", help="Project name")
    p_proj_create.add_argument("--description", default="", help="Project description")
    p_proj_create.add_argument("--color", default=None, help="Hex color (e.g. #4f86f7)")
    p_proj_create.add_argument("--tags", nargs="*", default=None, help="Project tags")
    p_proj_create.set_defaults(func=cmd_project_create)

    p_proj_update = proj_sub.add_parser("update", help="Update project fields")
    p_proj_update.add_argument("project_id", type=int, help="Project ID")
    p_proj_update.add_argument("--name", default=None, help="New name")
    p_proj_update.add_argument("--description", default=None, help="New description")
    p_proj_update.add_argument("--color", default=None, help="Hex color (e.g. #4f86f7)")
    p_proj_update.add_argument("--tags", nargs="*", default=None,
                               help="Project tags (replaces existing; pass no values to clear)")
    p_proj_update.add_argument("--status", choices=["active", "archived", "deleted"], default=None)
    p_proj_update.set_defaults(func=cmd_project_update)

    p_proj_delete = proj_sub.add_parser("delete", help="Soft-delete a project")
    p_proj_delete.add_argument("project_id", type=int, help="Project ID")
    p_proj_delete.set_defaults(func=cmd_project_delete)

    p_proj_archive = proj_sub.add_parser("archive", help="Archive an active project")
    p_proj_archive.add_argument("project_id", type=int, help="Project ID")
    p_proj_archive.set_defaults(func=cmd_project_archive)

    p_proj_restore = proj_sub.add_parser("restore", help="Restore an archived or deleted project")
    p_proj_restore.add_argument("project_id", type=int, help="Project ID")
    p_proj_restore.set_defaults(func=cmd_project_restore)

    p_proj_hd = proj_sub.add_parser("hard-delete", help="Permanently delete a project")
    p_proj_hd.add_argument("project_id", type=int, help="Project ID")
    p_proj_hd.set_defaults(func=cmd_project_hard_delete)

    p_proj_add = proj_sub.add_parser("add-paper", help="Add a paper to a project")
    p_proj_add.add_argument("project_id", type=int, help="Project ID")
    p_proj_add.add_argument("source_id", help="Paper source ID")
    p_proj_add.set_defaults(func=cmd_project_add_paper)

    p_proj_rem = proj_sub.add_parser("remove-paper", help="Remove a paper from a project")
    p_proj_rem.add_argument("project_id", type=int, help="Project ID")
    p_proj_rem.add_argument("source_id", help="Paper source ID")
    p_proj_rem.set_defaults(func=cmd_project_remove_paper)

    p_proj_export = proj_sub.add_parser("export", help="Export a project to a .lxproj archive")
    p_proj_export.add_argument("project_id", type=int, help="Project ID")
    p_proj_export.add_argument("dest", help="Destination path (.lxproj extension added automatically)")
    p_proj_export.add_argument("--pdfs", action="store_true", default=False,
                               help="Include bundled PDFs in the archive")
    p_proj_export.set_defaults(func=cmd_project_export)

    p_proj_import = proj_sub.add_parser("import", help="Import a project from a .lxproj archive")
    p_proj_import.add_argument("zip_path", help="Path to .lxproj archive")
    p_proj_import.add_argument("--preview", action="store_true", default=False,
                               help="Show archive summary without modifying the database")
    p_proj_import.add_argument("--on-conflict", choices=["merge", "overwrite"], default="merge",
                               dest="on_conflict",
                               help="How to handle papers that already exist (default: merge)")
    p_proj_import.set_defaults(func=cmd_project_import)

    p_proj_export_bib = proj_sub.add_parser("export-bibtex", help="Export project papers as BibTeX")
    p_proj_export_bib.add_argument("project_id", type=int, help="Project ID")
    p_proj_export_bib.add_argument("dest", help="Output file path (.bib added if no extension)")
    p_proj_export_bib.set_defaults(func=cmd_project_export_bibtex)

    p_proj_export_obs = proj_sub.add_parser("export-obsidian",
                                             help="Export project papers as Obsidian markdown")
    p_proj_export_obs.add_argument("project_id", type=int, help="Project ID")
    p_proj_export_obs.add_argument("dest", help="Output file path (.md added if no extension)")
    p_proj_export_obs.set_defaults(func=cmd_project_export_obsidian)

    # ── note ─────────────────────────────────────────────────────────────────
    p_note = sub.add_parser("note", help="Manage notes")
    note_sub = p_note.add_subparsers(dest="note_command", required=True)

    p_note_create = note_sub.add_parser("create", help="Create a note on a paper")
    p_note_create.add_argument("source_id", help="Paper source ID")
    p_note_create.add_argument("content", help="Note body text")
    p_note_create.add_argument("--title", default="", help="Note title")
    p_note_create.add_argument("--project-id", type=int, dest="project_id", default=None,
                               help="Associate note with a project")
    p_note_create.set_defaults(func=cmd_note_create)

    p_note_get = note_sub.add_parser("get", help="Get a note by ID")
    p_note_get.add_argument("note_id", type=int, help="Note ID")
    p_note_get.set_defaults(func=cmd_note_get)

    p_note_list = note_sub.add_parser("list", help="List notes")
    p_note_list.add_argument("--paper-id", dest="source_id", default=None,
                             help="Filter by paper source ID")
    p_note_list.add_argument("--project-id", type=int, dest="project_id", default=None,
                             help="Filter by project ID")
    p_note_list.set_defaults(func=cmd_note_list)

    p_note_update = note_sub.add_parser("update", help="Update note title or content")
    p_note_update.add_argument("note_id", type=int, help="Note ID")
    p_note_update.add_argument("--title", default=None, help="New title")
    p_note_update.add_argument("--content", default=None, help="New content")
    p_note_update.set_defaults(func=cmd_note_update)

    p_note_del = note_sub.add_parser("delete", help="Delete a note by ID")
    p_note_del.add_argument("note_id", type=int, help="Note ID")
    p_note_del.set_defaults(func=cmd_note_delete)

    # ── pdf ──────────────────────────────────────────────────────────────────
    p_pdf = sub.add_parser("pdf", help="Manage PDFs")
    pdf_sub = p_pdf.add_subparsers(dest="pdf_command", required=True)

    p_pdf_path = pdf_sub.add_parser("path", help="Show local PDF path for a paper")
    p_pdf_path.add_argument("source_id", help="Paper source ID")
    p_pdf_path.add_argument("--version", type=int, default=None,
                            help="Paper version (defaults to latest)")
    p_pdf_path.set_defaults(func=cmd_pdf_path)

    p_pdf_dl = pdf_sub.add_parser("download", help="Download PDF for a paper")
    p_pdf_dl.add_argument("source_id", help="Paper source ID")
    p_pdf_dl.add_argument("url", help="PDF download URL")
    p_pdf_dl.add_argument("--version", type=int, default=None,
                          help="Paper version (defaults to latest)")
    p_pdf_dl.set_defaults(func=cmd_pdf_download)

    p_pdf_storage = pdf_sub.add_parser("storage", help="Report total PDF storage usage")
    p_pdf_storage.set_defaults(func=cmd_pdf_storage)

    p_pdf_import = pdf_sub.add_parser("import", help="Import a local PDF (extract metadata)")
    p_pdf_import.add_argument("file", help="Path to PDF file")
    p_pdf_import.add_argument("--project-id", type=int, dest="project_id", default=None,
                              help="Link imported paper to a project")
    p_pdf_import.set_defaults(func=cmd_pdf_import)

    # ── trash ─────────────────────────────────────────────────────────────────
    p_trash = sub.add_parser("trash", help="Manage soft-deleted items")
    trash_sub = p_trash.add_subparsers(dest="trash_command", required=True)

    p_trash_list = trash_sub.add_parser("list", help="List soft-deleted papers and projects")
    p_trash_list.set_defaults(func=cmd_trash_list)

    p_trash_restore = trash_sub.add_parser("restore", help="Restore a soft-deleted paper")
    p_trash_restore.add_argument("source_id", help="Paper source ID")
    p_trash_restore.set_defaults(func=cmd_trash_restore)

    p_trash_hd = trash_sub.add_parser("hard-delete", help="Permanently delete a paper")
    p_trash_hd.add_argument("source_id", help="Paper source ID")
    p_trash_hd.set_defaults(func=cmd_trash_hard_delete)

    p_trash_rp = trash_sub.add_parser("restore-project", help="Restore a soft-deleted project")
    p_trash_rp.add_argument("project_id", type=int, help="Project ID")
    p_trash_rp.set_defaults(func=cmd_trash_restore_project)

    p_trash_hdp = trash_sub.add_parser("hard-delete-project", help="Permanently delete a project")
    p_trash_hdp.add_argument("project_id", type=int, help="Project ID")
    p_trash_hdp.set_defaults(func=cmd_trash_hard_delete_project)

    # ── doi ───────────────────────────────────────────────────────────────────
    p_doi = sub.add_parser("doi", help="Resolve and save papers by DOI")
    doi_sub = p_doi.add_subparsers(dest="doi_command", required=True)

    p_doi_resolve = doi_sub.add_parser("resolve", help="Resolve DOI to metadata (no save)")
    p_doi_resolve.add_argument("doi", help="DOI string")
    p_doi_resolve.set_defaults(func=cmd_doi_resolve)

    p_doi_save = doi_sub.add_parser("save", help="Resolve DOI and save paper to library")
    p_doi_save.add_argument("doi", help="DOI string")
    p_doi_save.set_defaults(func=cmd_doi_save)

    # ── author ────────────────────────────────────────────────────────────────
    p_author = sub.add_parser("author", help="Manage authors")
    author_sub = p_author.add_subparsers(dest="author_command", required=True)

    p_author_list = author_sub.add_parser("list", help="List all authors with paper counts")
    p_author_list.set_defaults(func=cmd_author_list)

    p_author_get = author_sub.add_parser("get", help="Get author details and paper list")
    p_author_get.add_argument("author_id", type=int, help="Author ID")
    p_author_get.set_defaults(func=cmd_author_get)

    p_author_update = author_sub.add_parser("update", help="Update author fields")
    p_author_update.add_argument("author_id", type=int, help="Author ID")
    p_author_update.add_argument("--full-name", dest="full_name", default=None)
    p_author_update.add_argument("--first-name", dest="first_name", default=None)
    p_author_update.add_argument("--last-name", dest="last_name", default=None)
    p_author_update.add_argument("--orcid", default=None)
    p_author_update.set_defaults(func=cmd_author_update)

    p_author_delete = author_sub.add_parser(
        "delete", help="Delete an author (blocked if linked to papers)"
    )
    p_author_delete.add_argument("author_id", type=int, help="Author ID")
    p_author_delete.set_defaults(func=cmd_author_delete)

    # ── bibtex ────────────────────────────────────────────────────────────────
    p_bibtex = sub.add_parser("bibtex", help="BibTeX import")
    bibtex_sub = p_bibtex.add_subparsers(dest="bibtex_command", required=True)

    p_bibtex_import = bibtex_sub.add_parser("import", help="Import papers from a .bib file")
    p_bibtex_import.add_argument("file", help="Path to .bib file")
    p_bibtex_import.add_argument("--project-id", type=int, dest="project_id", default=None,
                                  help="Link imported papers to a project")
    p_bibtex_import.set_defaults(func=cmd_bibtex_import)

    # ── stats ─────────────────────────────────────────────────────────────────
    p_stats = sub.add_parser("stats", help="Library statistics")
    p_stats.set_defaults(func=cmd_stats)

    # ── categories ────────────────────────────────────────────────────────────
    p_cats = sub.add_parser("categories", help="List all paper categories in the library")
    p_cats.set_defaults(func=cmd_categories)

    # ── settings ──────────────────────────────────────────────────────────────
    p_settings = sub.add_parser("settings", help="View and update user settings")
    settings_sub = p_settings.add_subparsers(dest="settings_command", required=True)

    p_settings_get = settings_sub.add_parser("get", help="Show all current settings")
    p_settings_get.set_defaults(func=cmd_settings_get)

    p_settings_update = settings_sub.add_parser("update", help="Set a setting value")
    p_settings_update.add_argument("key", help="Setting key")
    p_settings_update.add_argument("value", help="New value (JSON-parsed if valid JSON, else string)")
    p_settings_update.set_defaults(func=cmd_settings_update)

    return parser


def main(argv: list[str] | None = None) -> None:
    init_data_dir()
    svc_paper.init_db()
    svc_project.ensure_projects_db()
    svc_note.ensure_notes_db()
    parser = build_parser()
    args = parser.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main()
