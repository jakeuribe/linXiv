from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional

from storage.db import list_papers as _list_papers
from storage.notes import get_notes as _get_notes


def _get_paper(paper_id: int, version: int | None = None) -> None:
    # TODO: implement once paper_id is an int in the storage layer
    # Implementation will be to get all the details of a specific paper, not a specific version, title etc
    return None


@dataclass
class Paper:
    paper_id:                       int
    source_id_version_pair:         dict[str, int]| None


@dataclass
class Papers:
    paper_ids:   list[int] | None = None
    project_ids: list[int] | None = None
    tags:        list[str] | None = None  # not SELECT from, SELECT LIKE


@dataclass
class PaperDetails(Paper):
    abstract:          str | None        = None
    released:          datetime | None   = None
    updated_at_source: datetime | None   = None
    note_ids:          list[int | None]  = field(default_factory=list)


def get_paper_details(paper: Paper) -> Optional[PaperDetails]:

    row = _get_paper(paper.paper_id, paper.source_version) #TODO: UPDATE BEFORE USING
    if row is None:
        return None
    notes = _get_notes(paper.paper_id, all_projects=True)
    return PaperDetails(
        paper_id          = row["paper_id"],
        abstract          = row["summary"],
        released          = row["published"],
        updated_at_source = row["updated"],
        note_ids          = [n.id for n in notes],
    )


def get_papers(papers: Papers) -> list[PaperDetails]:
    rows = _list_papers(latest_only=True)
    if papers.paper_ids is not None:
        rows = [r for r in rows if r["paper_id"] in papers.paper_ids]
    if papers.tags is not None:
        rows = [
            r for r in rows
            if r["tags"] and any(t in r["tags"] for t in papers.tags)
        ]
    # TODO: filter by papers.project_ids via project_papers join
    return [
        PaperDetails(
            paper_id          = row["paper_id"],
            source_id         = row["source"],
            source_version    = row["version"],
            abstract          = row["summary"],
            released          = row["published"],
            updated_at_source = row["updated"],
        )
        for row in rows
    ]
