from __future__ import annotations

from dataclasses import dataclass
from typing import Optional


@dataclass
class BasicAuthorDetails:
    author_id:  int
    orcid:      str | None = None
    full_name:  str | None = None
    first_name: str | None = None
    last_name:  str | None = None

    def to_dict(self) -> dict:
        return {
            "author_id":  self.author_id,
            "orcid":      self.orcid,
            "full_name":  self.full_name,
            "first_name": self.first_name,
            "last_name":  self.last_name,
        }


@dataclass
class FullAuthorDetails(BasicAuthorDetails):
    paper_ids: Optional[list[int]] = None

    def to_dict(self) -> dict:
        d = super().to_dict()
        d["paper_ids"] = self.paper_ids
        return d


@dataclass
class AuthorWithCount(BasicAuthorDetails):
    paper_count: int = 0

    def to_dict(self) -> dict:
        d = super().to_dict()
        d["paper_count"] = self.paper_count
        return d


@dataclass
class AuthorPaperPreview:
    paper_id:  int
    source_id: str
    source_fk: int
    version:   int
    title:     str | None = None

    def to_dict(self) -> dict:
        return {
            "paper_id":  self.paper_id,
            "source_id": self.source_id,
            "source_fk": self.source_fk,
            "version":   self.version,
            "title":     self.title,
        }
