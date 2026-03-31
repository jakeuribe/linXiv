"""Tests for db.py — pure functions and DB round-trips."""
import datetime
import sys
import os

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import arxiv
import db


# ---------------------------------------------------------------------------
# parse_entry_id — pure function, no DB needed
# ---------------------------------------------------------------------------

class TestParseEntryId:
    def test_full_url_with_version(self):
        paper_id, version = db.parse_entry_id("http://arxiv.org/abs/2204.12985v4")
        assert paper_id == "2204.12985"
        assert version == 4

    def test_https_url_with_version(self):
        paper_id, version = db.parse_entry_id("https://arxiv.org/abs/2301.00001v1")
        assert paper_id == "2301.00001"
        assert version == 1

    def test_bare_id_with_version(self):
        paper_id, version = db.parse_entry_id("2204.12985v2")
        assert paper_id == "2204.12985"
        assert version == 2

    def test_bare_id_no_version_defaults_to_1(self):
        paper_id, version = db.parse_entry_id("2204.12985")
        assert paper_id == "2204.12985"
        assert version == 1

    def test_old_style_url_takes_last_segment(self):
        # parse_entry_id uses split('/')[-1], so for URLs like
        # "http://arxiv.org/abs/hep-th/9901001v1" the category prefix is
        # dropped and only the numeric part is kept.
        paper_id, version = db.parse_entry_id("http://arxiv.org/abs/hep-th/9901001v1")
        assert paper_id == "9901001"
        assert version == 1

    def test_old_style_url_higher_version(self):
        paper_id, version = db.parse_entry_id("http://arxiv.org/abs/math/0501234v3")
        assert paper_id == "0501234"
        assert version == 3

    def test_old_style_bare_takes_last_segment(self):
        # Bare old-style ID — split('/') picks "9901001", no version → default 1
        paper_id, version = db.parse_entry_id("hep-th/9901001")
        assert paper_id == "9901001"
        assert version == 1


# ---------------------------------------------------------------------------
# Helper: build a minimal arxiv.Result for testing
# ---------------------------------------------------------------------------

def _make_result(
    arxiv_id: str = "2204.12985v1",
    title: str = "Test Paper Title",
    summary: str = "An abstract.",
    authors: list[str] | None = None,
    primary_category: str = "cs.LG",
    categories: list[str] | None = None,
) -> arxiv.Result:
    """Build a minimal arxiv.Result for use in tests."""
    now = datetime.datetime(2024, 1, 15, 12, 0, 0, tzinfo=datetime.timezone.utc)
    result = arxiv.Result(
        entry_id=f"http://arxiv.org/abs/{arxiv_id}",
        title=title,
        summary=summary,
        authors=[arxiv.Result.Author(name) for name in (authors or ["Alice Smith"])],
        published=now,
        updated=now,
        primary_category=primary_category,
        categories=categories or [primary_category],
        doi="",
        comment="",
        journal_ref="",
        links=[],
    )
    return result


# ---------------------------------------------------------------------------
# save_paper / get_paper round-trip
# ---------------------------------------------------------------------------

class TestSavePaper:
    def test_save_and_get_by_id(self, tmp_db):
        result = _make_result("2204.12985v1", title="Attention Is All You Need")
        db.save_paper(result)
        row = db.get_paper("2204.12985")
        assert row is not None
        assert row["paper_id"] == "2204.12985"
        assert row["title"] == "Attention Is All You Need"
        assert row["version"] == 1

    def test_save_and_get_specific_version(self, tmp_db):
        result = _make_result("2204.12985v3", title="Paper v3")
        db.save_paper(result)
        row = db.get_paper("2204.12985", version=3)
        assert row is not None
        assert row["version"] == 3

    def test_save_stores_authors(self, tmp_db):
        result = _make_result(
            "2301.00001v1",
            authors=["Alice Smith", "Bob Jones"],
        )
        db.save_paper(result)
        row = db.get_paper("2301.00001")
        assert row is not None
        assert "Alice Smith" in row["authors"]
        assert "Bob Jones" in row["authors"]

    def test_save_stores_category(self, tmp_db):
        result = _make_result("2301.00002v1", primary_category="math.CO")
        db.save_paper(result)
        row = db.get_paper("2301.00002")
        assert row is not None
        assert row["category"] == "math.CO"

    def test_save_returns_paper_id_and_version(self, tmp_db):
        result = _make_result("2204.12985v2")
        paper_id, version = db.save_paper(result)
        assert paper_id == "2204.12985"
        assert version == 2

    def test_get_paper_missing_returns_none(self, tmp_db):
        row = db.get_paper("0000.00000")
        assert row is None

    def test_save_with_tags(self, tmp_db):
        result = _make_result("2204.12985v1")
        db.save_paper(result, tags=["ml", "transformers"])
        row = db.get_paper("2204.12985")
        assert row is not None
        assert "ml" in row["tags"]
        assert "transformers" in row["tags"]


# ---------------------------------------------------------------------------
# save_paper_metadata (source-agnostic)
# ---------------------------------------------------------------------------

class TestSavePaperMetadata:
    def _make_meta(self, **kwargs):
        from sources.base import PaperMetadata
        defaults = dict(
            paper_id="W3123456789",
            version=1,
            title="OpenAlex Paper",
            authors=["Jane Doe"],
            published=datetime.date(2024, 6, 1),
            summary="An abstract from OpenAlex.",
            source="openalex",
        )
        defaults.update(kwargs)
        return PaperMetadata(**defaults)

    def test_save_and_get_by_id(self, tmp_db):
        meta = self._make_meta()
        db.save_paper_metadata(meta)
        row = db.get_paper("W3123456789")
        assert row is not None
        assert row["title"] == "OpenAlex Paper"
        assert row["source"] == "openalex"

    def test_save_stores_source_field(self, tmp_db):
        meta = self._make_meta(source="openalex")
        db.save_paper_metadata(meta)
        row = db.get_paper("W3123456789")
        assert row is not None
        assert row["source"] == "openalex"

    def test_arxiv_save_defaults_source_to_arxiv(self, tmp_db):
        result = _make_result("2204.12985v1")
        db.save_paper(result)
        row = db.get_paper("2204.12985")
        assert row is not None
        assert row["source"] == "arxiv"

    def test_save_metadata_with_tags(self, tmp_db):
        meta = self._make_meta()
        db.save_paper_metadata(meta, tags=["physics", "ml"])
        row = db.get_paper("W3123456789")
        assert row is not None
        assert "physics" in row["tags"]
        assert "ml" in row["tags"]

    def test_save_metadata_returns_id_and_version(self, tmp_db):
        meta = self._make_meta(paper_id="W999", version=1)
        paper_id, version = db.save_paper_metadata(meta)
        assert paper_id == "W999"
        assert version == 1


# ---------------------------------------------------------------------------
# list_papers
# ---------------------------------------------------------------------------

class TestListPapers:
    def test_empty_db_returns_empty_list(self, tmp_db):
        rows = db.list_papers()
        assert rows == []

    def test_saved_paper_appears_in_list(self, tmp_db):
        result = _make_result("2204.12985v1", title="Listed Paper")
        db.save_paper(result)
        rows = db.list_papers()
        assert len(rows) == 1
        assert rows[0]["title"] == "Listed Paper"

    def test_multiple_papers_all_listed(self, tmp_db):
        db.save_paper(_make_result("2204.12985v1", title="Paper A"))
        db.save_paper(_make_result("2301.00001v1", title="Paper B"))
        rows = db.list_papers()
        titles = {row["title"] for row in rows}
        assert "Paper A" in titles
        assert "Paper B" in titles

    def test_latest_only_returns_one_per_paper(self, tmp_db):
        db.save_paper(_make_result("2204.12985v1", title="Paper v1"))
        db.save_paper(_make_result("2204.12985v2", title="Paper v2"))
        rows = db.list_papers(latest_only=True)
        assert len(rows) == 1
        assert rows[0]["version"] == 2

    def test_not_latest_only_returns_all_versions(self, tmp_db):
        db.save_paper(_make_result("2204.12985v1", title="Paper v1"))
        db.save_paper(_make_result("2204.12985v2", title="Paper v2"))
        rows = db.list_papers(latest_only=False)
        assert len(rows) == 2


# ---------------------------------------------------------------------------
# set_full_text / search_full_text (FTS)
# ---------------------------------------------------------------------------

class TestFullTextSearch:
    def test_set_full_text_stores_text(self, tmp_db):
        db.save_paper(_make_result("2204.12985v1"))
        db.set_full_text("2204.12985", 1, "This paper studies transformers.")
        row = db.get_paper("2204.12985")
        assert row is not None
        assert row["full_text"] == "This paper studies transformers."
        assert row["downloaded_source"] == True

    def test_set_full_text_marks_downloaded_source(self, tmp_db):
        db.save_paper(_make_result("2204.12985v1"))
        row_before = db.get_paper("2204.12985")
        assert not row_before["downloaded_source"]
        db.set_full_text("2204.12985", 1, "Some TeX content.")
        row_after = db.get_paper("2204.12985")
        assert row_after["downloaded_source"]


# ---------------------------------------------------------------------------
# extract_source (TeX extraction)
# ---------------------------------------------------------------------------

class TestExtractSource:
    def test_extract_from_tar(self, tmp_path):
        import tarfile
        from downloads import extract_source

        # Create a fake .tex file
        tex_content = r"""
\documentclass{article}
\usepackage{amsmath}
% This is a comment
\begin{document}
Hello world of transformers.
\end{document}
"""
        tex_file = tmp_path / "main.tex"
        tex_file.write_text(tex_content)

        # Create a .tar.gz
        tar_path = str(tmp_path / "source.tar.gz")
        with tarfile.open(tar_path, "w:gz") as tar:
            tar.add(str(tex_file), arcname="main.tex")

        result = extract_source(tar_path)
        assert "Hello world of transformers" in result
        # Comments should be stripped
        assert "This is a comment" not in result

    def test_extract_empty_tar(self, tmp_path):
        import tarfile
        from downloads import extract_source

        # Create an empty .tar.gz (no .tex files)
        tar_path = str(tmp_path / "empty.tar.gz")
        with tarfile.open(tar_path, "w:gz") as tar:
            pass

        result = extract_source(tar_path)
        assert result == ""

    def test_extract_invalid_tar(self, tmp_path):
        from downloads import extract_source

        bad_path = str(tmp_path / "bad.tar.gz")
        (tmp_path / "bad.tar.gz").write_text("not a tarball")

        result = extract_source(bad_path)
        assert result == ""
