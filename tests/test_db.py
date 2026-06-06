"""Tests for db.py — pure functions and DB round-trips."""
import datetime
import sys
import os

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import arxiv
import storage.db as db


# ---------------------------------------------------------------------------
# parse_entry_id — pure function, no DB needed
# ---------------------------------------------------------------------------

class TestParseEntryId:
    def test_full_url_with_version(self):
        source_id, version = db.parse_entry_id("http://arxiv.org/abs/2204.12985v4")
        assert source_id == "2204.12985"
        assert version == 4

    def test_https_url_with_version(self):
        source_id, version = db.parse_entry_id("https://arxiv.org/abs/2301.00001v1")
        assert source_id == "2301.00001"
        assert version == 1

    def test_bare_id_with_version(self):
        source_id, version = db.parse_entry_id("2204.12985v2")
        assert source_id == "2204.12985"
        assert version == 2

    def test_bare_id_no_version_defaults_to_1(self):
        source_id, version = db.parse_entry_id("2204.12985")
        assert source_id == "2204.12985"
        assert version == 1

    def test_old_style_url_takes_last_segment(self):
        # parse_entry_id uses split('/')[-1], so for URLs like
        # "http://arxiv.org/abs/hep-th/9901001v1" the category prefix is
        # dropped and only the numeric part is kept.
        source_id, version = db.parse_entry_id("http://arxiv.org/abs/hep-th/9901001v1")
        assert source_id == "9901001"
        assert version == 1

    def test_old_style_url_higher_version(self):
        source_id, version = db.parse_entry_id("http://arxiv.org/abs/math/0501234v3")
        assert source_id == "0501234"
        assert version == 3

    def test_old_style_bare_takes_last_segment(self):
        # Bare old-style ID — split('/') picks "9901001", no version → default 1
        source_id, version = db.parse_entry_id("hep-th/9901001")
        assert source_id == "9901001"
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

@pytest.mark.usefixtures("tmp_db")
class TestSavePaper:
    def test_save_and_get_by_id(self):
        result = _make_result("2204.12985v1", title="Attention Is All You Need")
        db.save_paper(result)
        row = db.get_paper("2204.12985")
        assert row
        assert row["source_id"] == "2204.12985"
        assert row["title"] == "Attention Is All You Need"
        assert row["version"] == 1

    def test_save_and_get_specific_version(self):
        result = _make_result("2204.12985v3", title="Paper v3")
        db.save_paper(result)
        row = db.get_paper("2204.12985", version=3)
        assert row
        assert row["version"] == 3

    def test_save_stores_authors(self):
        result = _make_result(
            "2301.00001v1",
            authors=["Alice Smith", "Bob Jones"],
        )
        db.save_paper(result)
        row = db.get_paper("2301.00001")
        assert row
        assert "Alice Smith" in row["authors"]
        assert "Bob Jones" in row["authors"]

    def test_save_stores_category(self):
        result = _make_result("2301.00002v1", primary_category="math.CO")
        db.save_paper(result)
        row = db.get_paper("2301.00002")
        assert row
        assert row["category"] == "math.CO"

    def test_save_returns_source_id_and_version(self):
        result = _make_result("2204.12985v2")
        source_id, version = db.save_paper(result)
        assert source_id == "2204.12985"
        assert version == 2

    def test_get_paper_missing_returns_none(self):
        row = db.get_paper("0000.00000")
        assert row is None

    def test_save_with_tags(self):
        result = _make_result("2204.12985v1")
        db.save_paper(result, tags=["ml", "transformers"])
        row = db.get_paper("2204.12985")
        assert row
        assert "ml" in row["tags"]
        assert "transformers" in row["tags"]


# ---------------------------------------------------------------------------
# save_paper_metadata (source-agnostic)
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestSavePaperMetadata:
    def _make_meta(self, **kwargs):
        from sources.base import PaperMetadata
        defaults = dict(
            source_id="W3123456789",
            version=1,
            title="OpenAlex Paper",
            authors=["Jane Doe"],
            published=datetime.date(2024, 6, 1),
            summary="An abstract from OpenAlex.",
            source="openalex",
        )
        defaults.update(kwargs)
        return PaperMetadata.model_validate(defaults)

    def test_save_and_get_by_id(self):
        meta = self._make_meta()
        db.save_paper_metadata(meta)
        row = db.get_paper("W3123456789")
        assert row
        assert row["title"] == "OpenAlex Paper"
        assert row["source"] == "openalex"

    def test_save_stores_source_field(self):
        meta = self._make_meta(source="openalex")
        db.save_paper_metadata(meta)
        row = db.get_paper("W3123456789")
        assert row
        assert row["source"] == "openalex"

    def test_arxiv_save_defaults_source_to_arxiv(self):
        result = _make_result("2204.12985v1")
        db.save_paper(result)
        row = db.get_paper("2204.12985")
        assert row
        assert row["source"] == "arxiv"

    def test_save_metadata_with_tags(self):
        meta = self._make_meta()
        db.save_paper_metadata(meta, tags=["physics", "ml"])
        row = db.get_paper("W3123456789")
        assert row
        assert "physics" in row["tags"]
        assert "ml" in row["tags"]

    def test_save_metadata_returns_id_and_version(self):
        meta = self._make_meta(source_id="W999", version=1)
        source_id, version = db.save_paper_metadata(meta)
        assert source_id == "W999"
        assert version == 1


# ---------------------------------------------------------------------------
# list_papers
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestListPapers:
    def test_empty_db_returns_empty_list(self):
        rows = db.list_papers()
        assert rows == []

    def test_saved_paper_appears_in_list(self):
        result = _make_result("2204.12985v1", title="Listed Paper")
        db.save_paper(result)
        rows = db.list_papers()
        assert len(rows) == 1
        assert rows[0]["title"] == "Listed Paper"

    def test_multiple_papers_all_listed(self):
        db.save_paper(_make_result("2204.12985v1", title="Paper A"))
        db.save_paper(_make_result("2301.00001v1", title="Paper B"))
        rows = db.list_papers()
        titles = {row["title"] for row in rows}
        assert "Paper A" in titles
        assert "Paper B" in titles

    def test_latest_only_returns_one_per_paper(self):
        db.save_paper(_make_result("2204.12985v1", title="Paper v1"))
        db.save_paper(_make_result("2204.12985v2", title="Paper v2"))
        rows = db.list_papers(latest_only=True)
        assert len(rows) == 1
        assert rows[0]["version"] == 2

    def test_not_latest_only_returns_all_versions(self):
        db.save_paper(_make_result("2204.12985v1", title="Paper v1"))
        db.save_paper(_make_result("2204.12985v2", title="Paper v2"))
        rows = db.list_papers(latest_only=False)
        assert len(rows) == 2


# ---------------------------------------------------------------------------
# set_full_text / search_full_text (FTS)
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestFullTextSearch:
    def test_set_full_text_stores_text(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.set_full_text("This paper studies transformers.", None, "2204.12985", 1)
        row = db.get_paper("2204.12985")
        assert row
        assert row["full_text"] == "This paper studies transformers."
        assert row["downloaded_source"] == True

    def test_set_full_text_marks_downloaded_source(self):
        db.save_paper(_make_result("2204.12985v1"))
        row_before = db.get_paper("2204.12985")
        assert row_before
        assert not row_before["downloaded_source"]

        db.set_full_text("Some TeX content.", None, "2204.12985", 1)
        row_after = db.get_paper("2204.12985")
        assert row_after
        assert row_after["downloaded_source"]  # set_full_text should mark this True

# ---------------------------------------------------------------------------
# extract_source (TeX extraction)
# ---------------------------------------------------------------------------

class TestExtractSource:
    def test_extract_from_tar(self, tmp_path):
        import tarfile
        from sources.arxiv_downloads import extract_source

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
        from sources.arxiv_downloads import extract_source

        # Create an empty .tar.gz (no .tex files)
        tar_path = str(tmp_path / "empty.tar.gz")
        with tarfile.open(tar_path, "w:gz") as tar:
            pass

        result = extract_source(tar_path)
        assert result == ""

    def test_extract_invalid_tar(self, tmp_path):
        from sources.arxiv_downloads import extract_source

        bad_path = str(tmp_path / "bad.tar.gz")
        (tmp_path / "bad.tar.gz").write_text("not a tarball")

        result = extract_source(bad_path)
        assert result == ""


# ---------------------------------------------------------------------------
# set_has_pdf
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestSetHasPdf:
    def test_set_has_pdf_true(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.set_has_pdf("2204.12985", 1, True)
        row = db.get_paper("2204.12985", version=1)
        assert row
        assert bool(row["has_pdf"]) is True

    def test_set_has_pdf_false(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.set_has_pdf("2204.12985", 1, True)
        db.set_has_pdf("2204.12985", 1, False)
        row = db.get_paper("2204.12985", version=1)
        assert row
        assert bool(row["has_pdf"]) is False

    def test_set_has_pdf_only_affects_target_version(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.save_paper(_make_result("2204.12985v2"))
        db.set_has_pdf("2204.12985", 1, True)
        row_v2 = db.get_paper("2204.12985", version=2)
        assert row_v2
        assert not bool(row_v2["has_pdf"])

    def test_set_has_pdf_default_is_false(self):
        db.save_paper(_make_result("2204.12985v1"))
        row = db.get_paper("2204.12985", version=1)
        assert row
        assert not bool(row["has_pdf"])


# ---------------------------------------------------------------------------
# set_pdf_path
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestSetPdfPath:
    def test_set_pdf_path_single_version(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.set_pdf_path("2204.12985", "/tmp/paper.pdf")
        row = db.get_paper("2204.12985", version=1)
        assert row
        assert row["pdf_path"] == "/tmp/paper.pdf"

    def test_set_pdf_path_all_versions(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.save_paper(_make_result("2204.12985v2"))
        db.set_pdf_path("2204.12985", "/tmp/paper.pdf")
        row_v1 = db.get_paper("2204.12985", version=1)
        row_v2 = db.get_paper("2204.12985", version=2)
        assert row_v1 and row_v1["pdf_path"] == "/tmp/paper.pdf"
        assert row_v2 and row_v2["pdf_path"] == "/tmp/paper.pdf"

    def test_set_pdf_path_overwrites_previous(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.set_pdf_path("2204.12985", "/tmp/old.pdf")
        db.set_pdf_path("2204.12985", "/tmp/new.pdf")
        row = db.get_paper("2204.12985", version=1)
        assert row
        assert row["pdf_path"] == "/tmp/new.pdf"

    def test_set_pdf_path_does_not_affect_other_papers(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.save_paper(_make_result("2301.00001v1"))
        db.set_pdf_path("2204.12985", "/tmp/paper.pdf")
        other = db.get_paper("2301.00001", version=1)
        assert other
        assert other["pdf_path"] is None


# ---------------------------------------------------------------------------
# get_all_versions
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGetAllVersions:
    def test_returns_empty_for_unknown_paper(self):
        assert db.get_all_versions("0000.00000") == []

    def test_returns_single_version(self):
        db.save_paper(_make_result("2204.12985v1"))
        rows = db.get_all_versions("2204.12985")
        assert len(rows) == 1
        assert rows[0]["version"] == 1

    def test_returns_multiple_versions_oldest_first(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.save_paper(_make_result("2204.12985v3"))
        db.save_paper(_make_result("2204.12985v2"))
        rows = db.get_all_versions("2204.12985")
        versions = [r["version"] for r in rows]
        assert versions == sorted(versions)

    def test_does_not_return_other_papers(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.save_paper(_make_result("2301.00001v1"))
        rows = db.get_all_versions("2204.12985")
        assert all(r["source_id"] == "2204.12985" for r in rows)


# ---------------------------------------------------------------------------
# get_graph_data
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGetGraphData:
    def test_empty_db_returns_empty_nodes_and_edges(self):
        nodes, edges = db.get_graph_data()
        assert nodes == []
        assert edges == []

    def test_paper_node_present(self):
        db.save_paper(_make_result("2204.12985v1", title="My Paper"))
        nodes, edges = db.get_graph_data()
        paper_nodes = [n for n in nodes if n.get("type") == "paper"]
        assert len(paper_nodes) == 1
        assert isinstance(paper_nodes[0]["id"], int)
        assert paper_nodes[0]["label"] == "My Paper"

    def test_author_node_present(self):
        db.save_paper(_make_result("2204.12985v1", authors=["Alice Smith"]))
        nodes, edges = db.get_graph_data()
        author_nodes = [n for n in nodes if n.get("type") == "author"]
        assert any(n["label"] == "Alice Smith" for n in author_nodes)

    def test_edge_connects_paper_to_author(self):
        db.save_paper(_make_result("2204.12985v1", authors=["Alice Smith"]))
        nodes, edges = db.get_graph_data()
        paper_nodes = [n for n in nodes if n.get("type") == "paper"]
        source_id = paper_nodes[0]["id"]
        assert any(e["source"] == source_id and "Alice Smith" in e["target"] for e in edges)

    def test_shared_author_has_single_node(self):
        db.save_paper(_make_result("2204.12985v1", authors=["Shared Author"]))
        db.save_paper(_make_result("2301.00001v1", authors=["Shared Author"]))
        nodes, edges = db.get_graph_data()
        author_nodes = [n for n in nodes if n.get("type") == "author" and n["label"] == "Shared Author"]
        assert len(author_nodes) == 1

    def test_paper_node_has_expected_fields(self):
        db.save_paper(_make_result("2204.12985v1", primary_category="cs.LG"))
        nodes, _ = db.get_graph_data()
        paper_node = next(n for n in nodes if n.get("type") == "paper")
        for key in ("id", "label", "type", "category", "tags", "has_pdf", "published"):
            assert key in paper_node

    def test_tags_defaults_to_empty_list_when_none(self):
        db.save_paper(_make_result("2204.12985v1"))  # no tags
        nodes, _ = db.get_graph_data()
        paper_node = next(n for n in nodes if n.get("type") == "paper")
        assert paper_node["tags"] == []

    def test_default_keeps_single_paper_authors(self):
        db.save_paper(_make_result("2204.12985v1", authors=["Solo Author"]))
        nodes, _ = db.get_graph_data()
        assert any(n.get("type") == "author" and n["label"] == "Solo Author" for n in nodes)

    def test_exclude_single_authors_drops_single_paper_author(self):
        db.save_paper(_make_result("2204.12985v1", authors=["Solo Author", "Shared Author"]))
        db.save_paper(_make_result("2301.00001v1", authors=["Shared Author"]))
        nodes, edges = db.get_graph_data(exclude_single_authors=True)
        author_labels = {n["label"] for n in nodes if n.get("type") == "author"}
        assert "Shared Author" in author_labels       # 2 papers — kept
        assert "Solo Author" not in author_labels      # 1 paper — dropped
        # The dropped author's edge is gone too (match the exact node id).
        assert not any(e["target"] == "author::Solo Author" for e in edges)

    def test_exclude_single_authors_counts_distinct_papers_not_versions(self):
        # Same author on two versions of ONE paper root is still "single-paper".
        db.save_paper(_make_result("2204.12985v1", authors=["Versioned Author"]))
        db.save_paper(_make_result("2204.12985v2", authors=["Versioned Author"]))
        nodes, _ = db.get_graph_data(exclude_single_authors=True)
        author_labels = {n["label"] for n in nodes if n.get("type") == "author"}
        assert "Versioned Author" not in author_labels


# ---------------------------------------------------------------------------
# get_categories
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGetCategories:
    def test_empty_db_returns_empty_list(self):
        assert db.get_categories() == []

    def test_returns_distinct_categories(self):
        db.save_paper(_make_result("2204.12985v1", primary_category="cs.LG"))
        db.save_paper(_make_result("2301.00001v1", primary_category="math.CO"))
        db.save_paper(_make_result("2301.00002v1", primary_category="cs.LG"))  # duplicate
        cats = db.get_categories()
        assert cats.count("cs.LG") == 1
        assert "math.CO" in cats

    def test_returns_sorted_list(self):
        db.save_paper(_make_result("2204.12985v1", primary_category="stat.ML"))
        db.save_paper(_make_result("2301.00001v1", primary_category="cs.LG"))
        db.save_paper(_make_result("2301.00002v1", primary_category="math.CO"))
        cats = db.get_categories()
        assert cats == sorted(cats)

    def test_only_latest_version_category_counted(self):
        db.save_paper(_make_result("2204.12985v1", primary_category="cs.AI"))
        db.save_paper(_make_result("2204.12985v2", primary_category="cs.LG"))
        cats = db.get_categories()
        # latest version is v2 with cs.LG; cs.AI should not appear
        assert "cs.LG" in cats
        assert "cs.AI" not in cats


# ---------------------------------------------------------------------------
# get_tags
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestGetTags:
    def test_empty_db_returns_empty_list(self):
        assert db.get_tags() == []

    def test_returns_distinct_tags(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["ml", "transformers"])
        db.save_paper(_make_result("2301.00001v1"), tags=["ml", "diffusion"])
        tags = db.get_tags()
        assert tags.count("ml") == 1
        assert "transformers" in tags
        assert "diffusion" in tags

    def test_returns_sorted_list(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["zoo", "alpha", "beta"])
        tags = db.get_tags()
        assert tags == sorted(tags)

    def test_paper_without_tags_not_included(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["ml"])
        db.save_paper(_make_result("2301.00001v1"))  # no tags
        tags = db.get_tags()
        assert tags == ["ml"]


# ---------------------------------------------------------------------------
# add_paper_tags
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestAddPaperTags:
    def test_add_tags_to_paper_without_tags(self):
        db.save_paper(_make_result("2204.12985v1"))
        result = db.add_paper_tags("2204.12985", ["ml", "transformers"])
        assert "ml" in result
        assert "transformers" in result

    def test_add_tags_persisted_to_db(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.add_paper_tags("2204.12985", ["ml"])
        row = db.get_paper("2204.12985")
        assert row
        assert "ml" in row["tags"]

    def test_add_tags_deduplicates(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["ml"])
        result = db.add_paper_tags("2204.12985", ["ml", "new_tag"])
        assert result.count("ml") == 1
        assert "new_tag" in result

    def test_add_tags_preserves_existing(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["existing"])
        result = db.add_paper_tags("2204.12985", ["new"])
        assert "existing" in result
        assert "new" in result

    def test_add_tags_returns_updated_list(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["a"])
        result = db.add_paper_tags("2204.12985", ["b", "c"])
        assert set(result) == {"a", "b", "c"}

    def test_add_tags_raises_for_unknown_paper(self):
        with pytest.raises(KeyError):
            db.add_paper_tags("0000.00000", ["ml"])


# ---------------------------------------------------------------------------
# remove_paper_tags
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestRemovePaperTags:
    def test_remove_tag_from_paper(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["ml", "transformers"])
        result = db.remove_paper_tags("2204.12985", ["ml"])
        assert "ml" not in result
        assert "transformers" in result

    def test_remove_tag_persisted_to_db(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["ml", "transformers"])
        db.remove_paper_tags("2204.12985", ["ml"])
        row = db.get_paper("2204.12985")
        assert row
        assert "ml" not in row["tags"]

    def test_remove_nonexistent_tag_is_noop(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["ml"])
        result = db.remove_paper_tags("2204.12985", ["nonexistent"])
        assert result == ["ml"]

    def test_remove_multiple_tags(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["a", "b", "c"])
        result = db.remove_paper_tags("2204.12985", ["a", "c"])
        assert result == ["b"]

    def test_remove_all_tags_leaves_empty_list(self):
        db.save_paper(_make_result("2204.12985v1"), tags=["x"])
        result = db.remove_paper_tags("2204.12985", ["x"])
        assert result == []

    def test_remove_tags_raises_for_unknown_paper(self):
        with pytest.raises(KeyError):
            db.remove_paper_tags("0000.00000", ["ml"])


# ---------------------------------------------------------------------------
# search_full_text
# ---------------------------------------------------------------------------

@pytest.mark.usefixtures("tmp_db")
class TestSearchFullText:
    def test_returns_empty_for_no_indexed_content(self):
        db.save_paper(_make_result("2204.12985v1"))
        # No full text set, FTS index empty
        results = db.search_full_text("transformers")
        assert results == []

    def test_finds_paper_with_matching_content(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.set_full_text("This paper studies transformers in NLP.", None, "2204.12985", 1)
        results = db.search_full_text("transformers")
        assert len(results) >= 1
        assert any(r["source_id"] == "2204.12985" for r in results)

    def test_does_not_return_non_matching_paper(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.save_paper(_make_result("2301.00001v1"))
        db.set_full_text("This paper studies transformers.", None, "2204.12985", 1)
        db.set_full_text("This paper is about diffusion models.", None, "2301.00001", 1)
        results = db.search_full_text("transformers")
        ids = [r["source_id"] for r in results]
        assert "2204.12985" in ids
        assert "2301.00001" not in ids

    def test_limit_parameter_respected(self):
        for i in range(5):
            sid = f"2204.1298{i}v1"
            db.save_paper(_make_result(sid, title=f"Paper {i}"))
            db.set_full_text("quantum computing research paper", None, f"2204.1298{i}", 1)
        results = db.search_full_text("quantum", limit=3)
        assert len(results) <= 3

    def test_multi_word_query(self):
        db.save_paper(_make_result("2204.12985v1"))
        db.set_full_text("attention mechanisms in neural networks.", None, "2204.12985", 1)
        results = db.search_full_text("attention neural")
        assert any(r["source_id"] == "2204.12985" for r in results)


# ---------------------------------------------------------------------------
# Migration-path test
# ---------------------------------------------------------------------------

class TestMigrationPath:
    def test_init_db_adds_missing_columns(self, tmp_path, monkeypatch):
        pytest.skip("migration tests not yet implemented for new schema")
