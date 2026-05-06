from dataclasses import dataclass
from datetime import datetime
from typing import Optional


@dataclass
class Tag:
    tag_id: int  # TAG_FK


@dataclass
class Tags:
    paper_id:   int | None = None
    project_id: int | None = None
    label:      str | None = None


@dataclass
class TagDetails(Tag):
    label:      str | None      = None
    created_at: datetime | None = None
    updated_at: datetime | None = None


def _get_tag(tag_id: int) -> None:
    # TODO: implement once storage/tags.py exposes get_tag_by_id
    return None


def _list_tags(
    paper_id:   int | None = None,
    project_id: int | None = None,
    label:      str | None = None,
) -> list:
    # TODO: implement once storage/tags.py exists
    return []


def get_tag_details(tag: Tag) -> Optional[TagDetails]:
    row = _get_tag(tag.tag_id)
    if row is None:
        return None
    return TagDetails(
        tag_id     = row["TAG_FK"],
        label      = row["TAG"],
        created_at = row["CREATED_AT"],
        updated_at = row["UPDATED_AT"],
    )


def get_tags(tags: Tags) -> list[TagDetails]:
    rows = _list_tags(
        paper_id   = tags.paper_id,
        project_id = tags.project_id,
        label      = tags.label,
    )
    return [
        TagDetails(
            tag_id     = row["TAG_FK"],
            label      = row["TAG"],
            created_at = row["CREATED_AT"],
            updated_at = row["UPDATED_AT"],
        )
        for row in rows
    ]