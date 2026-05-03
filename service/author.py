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
