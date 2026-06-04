"""Tests for the author_paper_counts view and list_authors_with_paper_count filtering.

save_paper_metadata auto-creates AUTHOR rows (deduped by name) and links them to
each saved paper version, so author counts are driven through ``meta.authors``.
"""
from __future__ import annotations

import datetime

import pytest

import storage.db as db
import storage.authors as authors
from sources.base import PaperMetadata


def _meta(source_id: str, version: int = 1, names: list[str] | None = None) -> PaperMetadata:
    return PaperMetadata.model_validate(dict(
        source_id=source_id,
        version=version,
        title=f"Paper {source_id} v{version}",
        authors=names if names is not None else ["Jane Doe"],
        published=datetime.date(2024, 6, 1),
        summary="Abstract.",
        source="openalex",
    ))


@pytest.mark.usefixtures("tmp_db")
class TestAuthorPaperCounts:
    def _counts(self, min_papers: int = 0) -> dict[str, int]:
        return {
            a["full_name"]: a["paper_count"]
            for a in authors.list_authors_with_paper_count(min_papers=min_papers)
        }

    def test_counts_distinct_active_papers(self):
        db.save_paper_metadata(_meta("W1", names=["Solo Writer", "Prolific Person"]))
        db.save_paper_metadata(_meta("W2", names=["Prolific Person"]))
        assert self._counts() == {"Solo Writer": 1, "Prolific Person": 2}

    def test_versions_do_not_double_count(self):
        # Same author on two versions of the SAME paper root counts once.
        db.save_paper_metadata(_meta("W1", version=1, names=["Versioned Author"]))
        db.save_paper_metadata(_meta("W1", version=2, names=["Versioned Author"]))
        assert self._counts()["Versioned Author"] == 1

    def test_deleted_papers_excluded_from_count(self):
        db.save_paper_metadata(_meta("W1", names=["Ghost Author"]))
        db.soft_delete_paper("W1")
        # Author row survives soft-delete but the deleted paper no longer counts.
        assert self._counts().get("Ghost Author") == 0

    def test_default_keeps_zero_and_single_paper_authors(self):
        db.save_paper_metadata(_meta("W1", names=["Solo Writer"]))
        no_papers = authors.create_author("No Papers")  # never linked to any paper
        assert no_papers is not None
        names = self._counts(min_papers=0)
        assert names.get("No Papers") == 0
        assert names.get("Solo Writer") == 1

    def test_min_papers_two_excludes_single_paper_authors(self):
        db.save_paper_metadata(_meta("W1", names=["Solo Writer", "Prolific Person"]))
        db.save_paper_metadata(_meta("W2", names=["Prolific Person"]))
        authors.create_author("No Papers")  # 0 papers — also excluded
        names = self._counts(min_papers=2)
        assert list(names) == ["Prolific Person"]
