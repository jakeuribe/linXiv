from dataclasses import dataclass
from typing import Optional


@dataclass
class Author:
    author_id:  int
    orcid:      Optional[str]


@dataclass
class Authors:
    paper_id:  int | None           = None
    name:      Optional[list[str]]  = None


@dataclass
class BasicAuthorDetails(Author):
    full_name:  str | None = None
    first_name: str | None = None
    last_name:  str | None = None
    orcid:      str | None = None

@dataclass
class FullAuthorDetails(BasicAuthorDetails):
    paper_ids:  Optional[list[int]] = None


def _get_author(author_id: int) -> None:
    # TODO: implement once storage/authors.py exists
    return None


def _list_authors(paper_id: int | None = None, name: str | None = None) -> list:
    # TODO: implement once storage/authors.py exists
    return []


def _get_author_papers(author_id: int) -> list:
    # TODO: implement once storage/authors.py exists
    return []


def get_author_details(author: Author) -> Optional[BasicAuthorDetails]:
    row = _get_author(author.author_id)
    if row is None:
        return None
    return BasicAuthorDetails(
        author_id  = row["author_id"],
        orcid      = row["orcid"],
        full_name  = row["full_name"],
        first_name = row["first_name"],
        last_name  = row["last_name"],
    )


def get_full_author_details(author: Author) -> Optional[FullAuthorDetails]:
    row = _get_author(author.author_id)
    if row is None:
        return None
    paper_ids = [p["paper_id"] for p in _get_author_papers(author.author_id)]
    return FullAuthorDetails(
        author_id  = row["author_id"],
        orcid      = row["orcid"],
        full_name  = row["full_name"],
        first_name = row["first_name"],
        last_name  = row["last_name"],
        paper_ids  = paper_ids,
    )


def get_authors(authors: Authors) -> list[BasicAuthorDetails]:
    name_filter = authors.name[0] if authors.name and len(authors.name) == 1 else None
    rows = _list_authors(paper_id=authors.paper_id, name=name_filter)
    if authors.name and len(authors.name) > 1:
        rows = [r for r in rows if r["full_name"] in authors.name]
    return [
        BasicAuthorDetails(
            author_id  = row["author_id"],
            orcid      = row["orcid"],
            full_name  = row["full_name"],
            first_name = row["first_name"],
            last_name  = row["last_name"],
        )
        for row in rows
    ]
