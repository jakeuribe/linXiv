"""Headless CLI for linXiv — search, fetch, list, tag, and manage projects without the GUI."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any

import db
from sources.arxiv_source import ArxivSource
from sources.openalex_source import OpenAlexSource
from sources.base import PaperMetadata, PaperSource


def _source_for(name: str) -> PaperSource:
    if name == "openalex":
        return OpenAlexSource()
    return ArxivSource()


def _meta_to_dict(m: PaperMetadata) -> dict[str, Any]:
    d = m.model_dump()
    d["published"] = str(d["published"])
    if d.get("updated"):
        d["updated"] = str(d["updated"])
    return d


def _row_to_dict(row) -> dict[str, Any]:
    return {k: row[k] for k in row.keys()}


def _output(data: Any) -> None:
    json.dump(data, sys.stdout, indent=2, default=str)
    sys.stdout.write("\n")


# ── Commands 

def cmd_search(args: argparse.Namespace) -> None:
    source = _source_for(args.source)
    results = source.search(args.query, max_results=args.max)
    _output([_meta_to_dict(r) for r in results])


def cmd_fetch(args: argparse.Namespace) -> None:
    source = _source_for(args.source)
    meta = source.fetch_by_id(args.paper_id)
    db.save_paper_metadata(meta)
    _output(_meta_to_dict(meta))


def cmd_list(args: argparse.Namespace) -> None:
    rows = db.list_papers(limit=args.limit, offset=args.offset)
    papers = [_row_to_dict(r) for r in rows]
    if args.category:
        papers = [p for p in papers if p.get("category") == args.category]
    _output(papers)


def cmd_tag(args: argparse.Namespace) -> None:
    from AI_tools import PaperContent, tag

    row = db.get_paper(args.paper_id)
    if row is None:
        print(json.dumps({"error": f"Paper {args.paper_id} not found in DB"}), file=sys.stderr)
        sys.exit(1)
    content = PaperContent(abstract=row["summary"] or "")
    tags = tag(content)
    _output({"paper_id": args.paper_id, "tags": tags})


def cmd_project_list(args: argparse.Namespace) -> None:
    from projects import ensure_projects_db, filter_projects, Status, Q

    ensure_projects_db()
    if args.status:
        projects = filter_projects(Q("status = ?", Status(args.status)))
    else:
        projects = filter_projects(~Q("status = ?", Status.DELETED))
    _output([{
        "id": p.id, "name": p.name, "description": p.description,
        "status": p.status.value, "paper_count": len(p.paper_ids),
    } for p in projects])


def cmd_project_create(args: argparse.Namespace) -> None:
    from projects import ensure_projects_db, Project

    ensure_projects_db()
    p = Project(name=args.name, description=args.description or "")
    p.save()
    _output({"id": p.id, "name": p.name, "status": p.status.value})


def cmd_project_add_paper(args: argparse.Namespace) -> None:
    from projects import ensure_projects_db, get_project

    ensure_projects_db()
    p = get_project(args.project_id)
    if p is None:
        print(json.dumps({"error": f"Project {args.project_id} not found"}), file=sys.stderr)
        sys.exit(1)
    if args.paper_id not in p.paper_ids:
        p.paper_ids.append(args.paper_id)
        p.save()
    _output({"project_id": p.id, "paper_id": args.paper_id, "paper_count": len(p.paper_ids)})


# Argument parsing 

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="arxiv_cli", description="linXiv headless CLI")
    sub = parser.add_subparsers(dest="command", required=True)

    # search
    p_search = sub.add_parser("search", help="Search for papers")
    p_search.add_argument("query", help="Search query string")
    p_search.add_argument("--source", choices=["arxiv", "openalex"], default="arxiv")
    p_search.add_argument("--max", type=int, default=10, help="Max results")
    p_search.set_defaults(func=cmd_search)

    # fetch
    p_fetch = sub.add_parser("fetch", help="Fetch and save a paper by ID")
    p_fetch.add_argument("paper_id", help="Paper ID (e.g. 2204.12985 or W3123456789)")
    p_fetch.add_argument("--source", choices=["arxiv", "openalex"], default="arxiv")
    p_fetch.set_defaults(func=cmd_fetch)

    # list
    p_list = sub.add_parser("list", help="List papers in the database")
    p_list.add_argument("--limit", type=int, default=None, help="Max papers to return")
    p_list.add_argument("--offset", type=int, default=0, help="Offset for pagination")
    p_list.add_argument("--category", type=str, default=None, help="Filter by category")
    p_list.set_defaults(func=cmd_list)

    # tag
    p_tag = sub.add_parser("tag", help="Generate AI tags for a paper")
    p_tag.add_argument("paper_id", help="Paper ID to tag")
    p_tag.set_defaults(func=cmd_tag)

    # project
    p_proj = sub.add_parser("project", help="Manage projects")
    proj_sub = p_proj.add_subparsers(dest="project_command", required=True)

    p_proj_list = proj_sub.add_parser("list", help="List projects")
    p_proj_list.add_argument("--status", choices=["active", "archived", "deleted"], default=None)
    p_proj_list.set_defaults(func=cmd_project_list)

    p_proj_create = proj_sub.add_parser("create", help="Create a project")
    p_proj_create.add_argument("name", help="Project name")
    p_proj_create.add_argument("--description", help="Project description", default="")
    p_proj_create.set_defaults(func=cmd_project_create)

    p_proj_add = proj_sub.add_parser("add-paper", help="Add a paper to a project")
    p_proj_add.add_argument("project_id", type=int, help="Project ID")
    p_proj_add.add_argument("paper_id", help="Paper ID to add")
    p_proj_add.set_defaults(func=cmd_project_add_paper)

    return parser


def main(argv: list[str] | None = None) -> None:
    db.init_db()
    parser = build_parser()
    args = parser.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main()
