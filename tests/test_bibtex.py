"""Tests for formats/bibtex.py — import/export round-trips and edge cases."""

from __future__ import annotations

import datetime
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from formats.bibtex import BibTeXFormat, _parse_year

_fmt = BibTeXFormat()

# ---------------------------------------------------------------------------
# _parse_year
# ---------------------------------------------------------------------------

class TestParseYear:
    def test_valid_year(self):
        assert _parse_year({"year": "2023"}) == datetime.date(2023, 1, 1)

    def test_missing_year_falls_back(self):
        assert _parse_year({}) == datetime.date(1900, 1, 1)

    def test_non_numeric_year_falls_back(self):
        assert _parse_year({"year": "forthcoming"}) == datetime.date(1900, 1, 1)

    def test_empty_string_falls_back(self):
        assert _parse_year({"year": ""}) == datetime.date(1900, 1, 1)


# ---------------------------------------------------------------------------
# import_string
# ---------------------------------------------------------------------------

_ARTICLE_BIB = """\
@article{smith2020,
  author  = {Smith, John},
  title   = {A Great Paper},
  year    = {2020},
  journal = {Nature},
  doi     = {10.1234/test},
  abstract = {This is the abstract.},
  url     = {https://example.com/paper},
}
"""

_NO_DOI_BIB = """\
@inproceedings{doe2019,
  author    = {Doe, Jane},
  title     = {Proceedings Paper},
  year      = {2019},
  booktitle = {NeurIPS},
}
"""

_MULTI_AUTHOR_BIB = """\
@article{multi2021,
  author = {Alice, A. and Bob, B. and Carol, C.},
  title  = {Multi-author Work},
  year   = {2021},
}
"""


class TestImportString:
    def test_basic_article_fields(self):
        papers = _fmt.import_string(_ARTICLE_BIB)
        assert len(papers) == 1
        p = papers[0]
        assert p.title == "A Great Paper"
        assert p.published == datetime.date(2020, 1, 1)
        assert p.doi == "10.1234/test"
        assert p.paper_id == "10.1234/test"
        assert p.journal_ref == "Nature"
        assert p.summary == "This is the abstract."
        assert p.url == "https://example.com/paper"
        assert p.source == "bibtex"
        assert p.version == 1

    def test_authors_parsed(self):
        papers = _fmt.import_string(_ARTICLE_BIB)
        assert len(papers[0].authors) == 1
        assert "Smith" in papers[0].authors[0]

    def test_multi_author(self):
        papers = _fmt.import_string(_MULTI_AUTHOR_BIB)
        assert len(papers[0].authors) == 3

    def test_no_doi_uses_key_as_paper_id(self):
        papers = _fmt.import_string(_NO_DOI_BIB)
        assert papers[0].paper_id == "doe2019"
        assert papers[0].doi is None

    def test_booktitle_used_as_journal_ref(self):
        papers = _fmt.import_string(_NO_DOI_BIB)
        assert papers[0].journal_ref == "NeurIPS"

    def test_multiple_entries(self):
        combined = _ARTICLE_BIB + _NO_DOI_BIB
        papers = _fmt.import_string(combined)
        assert len(papers) == 2

    def test_missing_abstract_is_empty_string(self):
        papers = _fmt.import_string(_NO_DOI_BIB)
        assert papers[0].summary == ""

    def test_missing_url_is_none(self):
        papers = _fmt.import_string(_NO_DOI_BIB)
        assert papers[0].url is None


# ---------------------------------------------------------------------------
# import_file
# ---------------------------------------------------------------------------

class TestImportFile:
    def test_import_from_file(self):
        with tempfile.NamedTemporaryFile(suffix=".bib", mode="w", delete=False) as f:
            f.write(_ARTICLE_BIB)
            path = f.name
        try:
            papers = _fmt.import_file(path)
            assert len(papers) == 1
            assert papers[0].title == "A Great Paper"
        finally:
            os.unlink(path)

    def test_import_multi_entry_file(self):
        with tempfile.NamedTemporaryFile(suffix=".bib", mode="w", delete=False) as f:
            f.write(_ARTICLE_BIB + _NO_DOI_BIB)
            path = f.name
        try:
            papers = _fmt.import_file(path)
            assert len(papers) == 2
        finally:
            os.unlink(path)


# ---------------------------------------------------------------------------
# export_papers
# ---------------------------------------------------------------------------

class TestExportPapers:
    def _make_paper(self, **overrides):
        base = {
            "paper_id": "2301.00001",
            "title": "Test Export",
            "summary": "An abstract.",
            "published": datetime.date(2023, 1, 1),
            "doi": None,
            "journal_ref": None,
            "url": None,
        }
        base.update(overrides)
        return base

    def test_export_contains_title(self):
        out = _fmt.export_papers([self._make_paper()])
        assert "Test Export" in out

    def test_export_contains_year(self):
        out = _fmt.export_papers([self._make_paper()])
        assert "2023" in out

    def test_export_contains_doi(self):
        out = _fmt.export_papers([self._make_paper(doi="10.9999/x")])
        assert "10.9999/x" in out

    def test_export_contains_journal(self):
        out = _fmt.export_papers([self._make_paper(journal_ref="Science")])
        assert "Science" in out

    def test_export_contains_url(self):
        out = _fmt.export_papers([self._make_paper(url="https://example.com")])
        assert "https://example.com" in out

    def test_export_multiple_papers(self):
        papers = [self._make_paper(paper_id="a"), self._make_paper(paper_id="b")]
        out = _fmt.export_papers(papers)
        assert out.count("@article") == 2

    def test_export_empty_list(self):
        out = _fmt.export_papers([])
        assert isinstance(out, str)


# ---------------------------------------------------------------------------
# round-trip: import then export
# ---------------------------------------------------------------------------

class TestRoundTrip:
    def test_import_export_preserves_title(self):
        papers = _fmt.import_string(_ARTICLE_BIB)
        exported = _fmt.export_papers([p.model_dump() for p in papers])
        reimported = _fmt.import_string(exported)
        assert reimported[0].title == papers[0].title

    def test_import_export_preserves_year(self):
        papers = _fmt.import_string(_ARTICLE_BIB)
        exported = _fmt.export_papers([p.model_dump() for p in papers])
        reimported = _fmt.import_string(exported)
        assert reimported[0].published.year == 2020
