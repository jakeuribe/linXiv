"""Tests for formats/markdown.py — import/export round-trips and edge cases."""

from __future__ import annotations

import datetime
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from formats.markdown import MarkdownFormat, ObsidianFormat, _paper_id_from_url

_md  = MarkdownFormat()
_obs = ObsidianFormat()


# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

def _make_paper(**overrides) -> dict:
    base = {
        "paper_id": "2301.00001",
        "title":    "Test Paper",
        "authors":  ["Alice Author", "Bob Builder"],
        "category": "cs.LG",
        "tags":     ["ml", "nlp"],
        "published": datetime.date(2023, 1, 1),
    }
    base.update(overrides)
    return base


# ---------------------------------------------------------------------------
# _paper_id_from_url
# ---------------------------------------------------------------------------

class TestPaperIdFromUrl:
    def test_arxiv_url(self):
        assert _paper_id_from_url("https://arxiv.org/abs/2301.00001") == "2301.00001"

    def test_http_arxiv_url(self):
        assert _paper_id_from_url("http://arxiv.org/abs/1234.56789") == "1234.56789"

    def test_non_arxiv_url_returns_full(self):
        assert _paper_id_from_url("https://example.com/paper") == "https://example.com/paper"

    def test_bare_id_returned_as_is(self):
        assert _paper_id_from_url("2301.00001") == "2301.00001"


# ---------------------------------------------------------------------------
# MarkdownFormat — export
# ---------------------------------------------------------------------------

class TestMarkdownExport:
    def test_contains_title(self):
        out = _md.export_papers([_make_paper()])
        assert "Test Paper" in out

    def test_contains_arxiv_url(self):
        out = _md.export_papers([_make_paper()])
        assert "arxiv.org/abs/2301.00001" in out

    def test_contains_authors(self):
        out = _md.export_papers([_make_paper()])
        assert "Alice Author" in out
        assert "Bob Builder" in out

    def test_contains_category(self):
        out = _md.export_papers([_make_paper()])
        assert "cs.LG" in out

    def test_contains_tags(self):
        out = _md.export_papers([_make_paper()])
        assert "ml" in out
        assert "nlp" in out

    def test_no_authors_skips_authors_line(self):
        out = _md.export_papers([_make_paper(authors=[])])
        assert "Authors:" not in out

    def test_no_category_skips_category_line(self):
        out = _md.export_papers([_make_paper(category="")])
        assert "Category:" not in out

    def test_no_tags_skips_tags_line(self):
        out = _md.export_papers([_make_paper(tags=[])])
        assert "Tags:" not in out

    def test_multiple_papers(self):
        papers = [_make_paper(paper_id="a", title="A"), _make_paper(paper_id="b", title="B")]
        out = _md.export_papers(papers)
        assert "A" in out and "B" in out

    def test_empty_list_has_header(self):
        out = _md.export_papers([])
        assert "# Selected Papers" in out


# ---------------------------------------------------------------------------
# MarkdownFormat — import_string
# ---------------------------------------------------------------------------

class TestMarkdownImport:
    def test_basic_import(self):
        out = _md.export_papers([_make_paper()])
        papers = _md.import_string(out)
        assert len(papers) == 1

    def test_paper_id_extracted_from_url(self):
        out = _md.export_papers([_make_paper()])
        p = _md.import_string(out)[0]
        assert p.paper_id == "2301.00001"

    def test_title_preserved(self):
        out = _md.export_papers([_make_paper()])
        assert _md.import_string(out)[0].title == "Test Paper"

    def test_authors_preserved(self):
        out = _md.export_papers([_make_paper()])
        p = _md.import_string(out)[0]
        assert "Alice Author" in p.authors
        assert "Bob Builder" in p.authors

    def test_category_preserved(self):
        out = _md.export_papers([_make_paper()])
        assert _md.import_string(out)[0].category == "cs.LG"

    def test_tags_preserved(self):
        out = _md.export_papers([_make_paper()])
        assert _md.import_string(out)[0].tags == ["ml", "nlp"]

    def test_multiple_papers(self):
        papers = [_make_paper(paper_id="2301.00001", title="A"),
                  _make_paper(paper_id="2301.00002", title="B")]
        out = _md.export_papers(papers)
        rt = _md.import_string(out)
        assert len(rt) == 2
        assert {p.paper_id for p in rt} == {"2301.00001", "2301.00002"}

    def test_empty_export_returns_empty(self):
        out = _md.export_papers([])
        assert _md.import_string(out) == []

    def test_source_is_import(self):
        out = _md.export_papers([_make_paper()])
        assert _md.import_string(out)[0].source == "import"


# ---------------------------------------------------------------------------
# MarkdownFormat — import_file
# ---------------------------------------------------------------------------

class TestMarkdownImportFile:
    def test_import_from_file(self):
        out = _md.export_papers([_make_paper()])
        with tempfile.NamedTemporaryFile(suffix=".md", mode="w", delete=False, encoding="utf-8") as f:
            f.write(out)
            path = f.name
        try:
            papers = _md.import_file(path)
            assert len(papers) == 1
            assert papers[0].paper_id == "2301.00001"
        finally:
            os.unlink(path)


# ---------------------------------------------------------------------------
# MarkdownFormat — round-trip
# ---------------------------------------------------------------------------

class TestMarkdownRoundTrip:
    def _rt(self, paper: dict):
        return _md.import_string(_md.export_papers([paper]))

    def test_paper_id(self):
        assert self._rt(_make_paper())[0].paper_id == "2301.00001"

    def test_title(self):
        assert self._rt(_make_paper())[0].title == "Test Paper"

    def test_authors(self):
        rt = self._rt(_make_paper())
        assert rt[0].authors == ["Alice Author", "Bob Builder"]

    def test_category(self):
        assert self._rt(_make_paper())[0].category == "cs.LG"

    def test_tags(self):
        assert self._rt(_make_paper())[0].tags == ["ml", "nlp"]

    def test_no_tags_round_trip(self):
        rt = self._rt(_make_paper(tags=[]))
        assert not rt[0].tags

    def test_special_chars_in_title(self):
        rt = self._rt(_make_paper(title="Transformers: A Survey (2023)"))
        assert rt[0].title == "Transformers: A Survey (2023)"


# ---------------------------------------------------------------------------
# ObsidianFormat — export
# ---------------------------------------------------------------------------

class TestObsidianExport:
    def test_has_yaml_frontmatter(self):
        out = _obs.export_papers([_make_paper()])
        assert out.startswith("---")
        assert "papers: 1" in out

    def test_frontmatter_contains_tags(self):
        out = _obs.export_papers([_make_paper()])
        assert "  - ml" in out
        assert "  - nlp" in out

    def test_frontmatter_tags_are_deduplicated(self):
        papers = [_make_paper(tags=["ml"]), _make_paper(paper_id="x", tags=["ml"])]
        out = _obs.export_papers(papers)
        assert out.count("  - ml") == 1

    def test_frontmatter_tags_sorted(self):
        out = _obs.export_papers([_make_paper(tags=["zzz", "aaa"])])
        aaa_pos = out.index("  - aaa")
        zzz_pos = out.index("  - zzz")
        assert aaa_pos < zzz_pos

    def test_no_tags_omits_tags_key(self):
        out = _obs.export_papers([_make_paper(tags=[])])
        assert "tags:" not in out

    def test_contains_h2_section_per_paper(self):
        papers = [_make_paper(paper_id="a", title="A"), _make_paper(paper_id="b", title="B")]
        out = _obs.export_papers(papers)
        assert out.count("## [") == 2

    def test_section_contains_bold_authors(self):
        out = _obs.export_papers([_make_paper()])
        assert "**Authors:**" in out
        assert "Alice Author" in out

    def test_section_contains_bold_category(self):
        out = _obs.export_papers([_make_paper()])
        assert "**Category:** cs.LG" in out

    def test_section_contains_bold_tags(self):
        out = _obs.export_papers([_make_paper()])
        assert "**Tags:**" in out


# ---------------------------------------------------------------------------
# ObsidianFormat — import_string
# ---------------------------------------------------------------------------

class TestObsidianImport:
    def test_basic_import(self):
        out = _obs.export_papers([_make_paper()])
        papers = _obs.import_string(out)
        assert len(papers) == 1

    def test_paper_id_from_url(self):
        out = _obs.export_papers([_make_paper()])
        assert _obs.import_string(out)[0].paper_id == "2301.00001"

    def test_title_preserved(self):
        out = _obs.export_papers([_make_paper()])
        assert _obs.import_string(out)[0].title == "Test Paper"

    def test_authors_preserved(self):
        out = _obs.export_papers([_make_paper()])
        p = _obs.import_string(out)[0]
        assert "Alice Author" in p.authors
        assert "Bob Builder" in p.authors

    def test_category_preserved(self):
        out = _obs.export_papers([_make_paper()])
        assert _obs.import_string(out)[0].category == "cs.LG"

    def test_tags_preserved(self):
        out = _obs.export_papers([_make_paper()])
        assert _obs.import_string(out)[0].tags == ["ml", "nlp"]

    def test_multiple_papers(self):
        papers = [_make_paper(paper_id="2301.00001", title="A"),
                  _make_paper(paper_id="2301.00002", title="B")]
        rt = _obs.import_string(_obs.export_papers(papers))
        assert len(rt) == 2
        assert {p.paper_id for p in rt} == {"2301.00001", "2301.00002"}

    def test_empty_export_returns_empty(self):
        assert _obs.import_string(_obs.export_papers([])) == []

    def test_source_is_import(self):
        out = _obs.export_papers([_make_paper()])
        assert _obs.import_string(out)[0].source == "import"


# ---------------------------------------------------------------------------
# ObsidianFormat — import_file
# ---------------------------------------------------------------------------

class TestObsidianImportFile:
    def test_import_from_file(self):
        out = _obs.export_papers([_make_paper()])
        with tempfile.NamedTemporaryFile(suffix=".md", mode="w", delete=False, encoding="utf-8") as f:
            f.write(out)
            path = f.name
        try:
            papers = _obs.import_file(path)
            assert len(papers) == 1
            assert papers[0].paper_id == "2301.00001"
        finally:
            os.unlink(path)


# ---------------------------------------------------------------------------
# ObsidianFormat — round-trip
# ---------------------------------------------------------------------------

class TestObsidianRoundTrip:
    def _rt(self, paper: dict):
        return _obs.import_string(_obs.export_papers([paper]))

    def test_paper_id(self):
        assert self._rt(_make_paper())[0].paper_id == "2301.00001"

    def test_title(self):
        assert self._rt(_make_paper())[0].title == "Test Paper"

    def test_authors(self):
        rt = self._rt(_make_paper())
        assert rt[0].authors == ["Alice Author", "Bob Builder"]

    def test_category(self):
        assert self._rt(_make_paper())[0].category == "cs.LG"

    def test_tags(self):
        assert self._rt(_make_paper())[0].tags == ["ml", "nlp"]

    def test_no_tags_round_trip(self):
        rt = self._rt(_make_paper(tags=[]))
        assert not rt[0].tags

    def test_special_chars_in_title(self):
        rt = self._rt(_make_paper(title="Attention Is All You Need"))
        assert rt[0].title == "Attention Is All You Need"

    def test_two_papers_ids(self):
        papers = [_make_paper(paper_id="2301.00001"), _make_paper(paper_id="2301.00002")]
        rt = _obs.import_string(_obs.export_papers(papers))
        assert {p.paper_id for p in rt} == {"2301.00001", "2301.00002"}
