from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

import storage.db as db
import storage.tags as _tags_storage
from storage.projects import get_project
from service.models.tag import TagDetails


# ---------------------------------------------------------------------------
# GUI-facing models
# ---------------------------------------------------------------------------

@dataclass
class Tag:
    tag_id:     int | None = None  # TAG_FK
    label:      str | None = None  # look up by label when tag_id absent


@dataclass
class Tags:
    paper_id:   int | None = None
    project_id: int | None = None
    label:      str | None = None


@dataclass
class TagIn:
    label: str


# ---------------------------------------------------------------------------
# Master functions
# ---------------------------------------------------------------------------

def get(tag: Tag) -> Optional[TagDetails]:
    """Fetch a single tag. Resolution order: tag_id → label."""
    if tag.tag_id is not None:
        return _get_tag(tag.tag_id)
    if tag.label is not None:
        all_labels = list_all_tags()
        for label in all_labels:
            if label.lower() == tag.label.lower():
                return TagDetails(tag_id=-1, label=label)
    return None


def get_many(tags: Tags) -> list[TagDetails]:
    """Fetch tags matching any combination of Tags filter fields."""
    rows = _list_tags(
        paper_id   = tags.paper_id,
        project_id = tags.project_id,
        label      = tags.label,
    )
    if rows:
        return rows

    all_labels = list_all_tags()
    results: list[TagDetails] = []

    if tags.paper_id is not None:
        paper_labels = get_paper_tags_by_id(tags.paper_id)
        all_labels = [l for l in all_labels if l in paper_labels]

    if tags.project_id is not None:
        project_labels = get_project_tags(tags.project_id)
        all_labels = [l for l in all_labels if l in project_labels]

    if tags.label is not None:
        all_labels = [l for l in all_labels if l.lower() == tags.label.lower()]

    for label in all_labels:
        results.append(TagDetails(tag_id=-1, label=label))
    return results


def upsert(tag: TagIn) -> int | None:
    """Insert a new tag or return the existing TAG_FK if label already exists."""
    all_labels = list_all_tags()
    for label in all_labels:
        if label.lower() == tag.label.lower():
            rows = _list_tags(label=label)
            if rows:
                return rows[0].tag_id
            return -1
    return create_tag(tag.label)


def delete(tag: Tag) -> None:
    if tag.tag_id is not None:
        delete_tag(tag.tag_id)


# ---------------------------------------------------------------------------
# Internal helpers (pending storage/tags.py)
# ---------------------------------------------------------------------------

def _get_tag(tag_id: int) -> Optional[TagDetails]:
    return _tags_storage.get_tag(tag_id)


def _list_tags(
    paper_id:   int | None = None,
    project_id: int | None = None,
    label:      str | None = None,
) -> list[TagDetails]:
    return _tags_storage.list_tags(paper_id=paper_id, project_id=project_id, label=label)


# ---------------------------------------------------------------------------
# Low-level reads
# ---------------------------------------------------------------------------

def get_tag_details(tag: Tag) -> Optional[TagDetails]:
    if tag.tag_id is None:
        return None
    row = _get_tag(tag.tag_id)
    if row is None:
        return None
    return row


def get_tags(tags: Tags) -> list[TagDetails]:
    rows = _list_tags(
        paper_id   = tags.paper_id,
        project_id = tags.project_id,
        label      = tags.label,
    )
    return rows


def list_all_tags() -> list[str]:
    return db.get_tags()


# ---------------------------------------------------------------------------
# Tag entity writes
# ---------------------------------------------------------------------------

def create_tag(label: str) -> int | None:
    return _tags_storage.create_tag(label)

def delete_tag(tag_id: int) -> None:
    _tags_storage.delete_tag(tag_id)


# ---------------------------------------------------------------------------
# Tags on papers (PAPER_TO_TAG)
# ---------------------------------------------------------------------------

def get_paper_tags(source_id: str) -> list[str]:
    row = db.get_paper(source_id)
    if row is None:
        return []
    tags = row["tags"]
    if tags is None:
        return []
    return list(tags)


def get_paper_tags_by_id(paper_id: int) -> list[str]:
    # paper_id here is a PAPER_ID integer; not directly supported by db.get_paper
    # which takes source_id strings. Fall back to empty list.
    return []


def add_paper_tags(source_id: str, tags: list[str]) -> list[str]:
    return db.add_paper_tags(source_id, tags)


def remove_paper_tags(source_id: str, tags: list[str]) -> list[str]:
    return db.remove_paper_tags(source_id, tags)


# ---------------------------------------------------------------------------
# Tags on projects (PROJECT_TO_TAG)
# ---------------------------------------------------------------------------

def get_project_tags(project_id: int) -> list[str]:
    project = get_project(project_id)
    if project is None:
        return []
    return list(project.project_tags)


def add_project_tags(project_id: int, tags: list[str]) -> None:
    project = get_project(project_id)
    if project is None:
        return
    existing = set(project.project_tags)
    for tag in tags:
        if tag not in existing:
            project.project_tags.append(tag)
            existing.add(tag)
    project.save()


def remove_project_tags(project_id: int, tags: list[str]) -> None:
    project = get_project(project_id)
    if project is None:
        return
    remove = set(tags)
    project.project_tags = [t for t in project.project_tags if t not in remove]
    project.save()