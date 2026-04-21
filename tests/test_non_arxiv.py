"""Round-trip and correctness tests for papers from non-arXiv/non-OpenAlex sources.

Covers BibTeX, JSON, CSV, Markdown, and Obsidian for papers whose paper_id is a
DOI, a BibTeX cite-key, or any other non-arXiv identifier.
"""

from __future__ import annotations

import datetime
import io
import csv
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from formats.bibtex import BibTeXFormat
from formats.csv_fmt import CSVFormat
from formats.json_fmt import JSONFormat
from formats.markdown import (
    MarkdownFormat, ObsidianFormat,
    _is_arxiv_id, _paper_url,
)

_bib = BibTeXFormat()
_csv = CSVFormat()
_jsn = JSONFormat()
_md  = MarkdownFormat()
_obs = ObsidianFormat()


# ---------------------------------------------------------------------------
# Shared fixtures
# ---------------------------------------------------------------------------

def _doi_paper(**overrides) -> dict:
    """A paper whose paper_id is a DOI (e.g. from an ACM/IEEE/Springer import)."""
    base = {
        "paper_id":    "10.1145/3290605.3300741",
        "version":     1,
        "title":       "Deep Learning for HCI",
        "authors":     ["Jane Doe", "Bob Smith"],
        "published":   datetime.date(2019, 5, 4),
        "summary":     "An ACM CHI paper.",
        "category":    None,
        "tags":        ["hci", "deep-learning"],
        "doi":         "10.1145/3290605.3300741",
        "journal_ref": "CHI 2019",
        "url":         "https://dl.acm.org/doi/10.1145/3290605.3300741",
        "source":      "bibtex",
    }
    base.update(overrides)
    return base


def _key_paper(**overrides) -> dict:
    """A paper whose paper_id is a bare BibTeX cite-key (no DOI)."""
    base = {
        "paper_id":    "vaswani2017attention",
        "version":     1,
        "title":       "Attention Is All You Need",
        "authors":     ["Vaswani, Ashish", "Shazeer, Noam"],
        "published":   datetime.date(2017, 6, 12),
        "summary":     "The transformer paper.",
        "category":    None,
        "tags":        ["transformers", "nlp"],
        "doi":         None,
        "journal_ref": "NeurIPS 2017",
        "url":         None,
        "source":      "bibtex",
    }
    base.update(overrides)
    return base


def _arxiv_paper(**overrides) -> dict:
    """A normal arXiv paper for mixed-source tests."""
    base = {
        "paper_id":  "2301.00001",
        "version":   1,
        "title":     "An arXiv Paper",
        "authors":   ["Alice Arxiv"],
        "published": datetime.date(2023, 1, 1),
        "summary":   "",
        "category":  "cs.LG",
        "tags":      ["ml"],
        "doi":       None,
        "url":       None,
        "source":    "arxiv",
    }
    base.update(overrides)
    return base


# ---------------------------------------------------------------------------
# _is_arxiv_id
# ---------------------------------------------------------------------------

class TestIsArxivId:
    def test_new_format(self):
        assert _is_arxiv_id("2301.00001")

    def test_new_format_with_version(self):
        assert _is_arxiv_id("2301.00001v2")

    def test_new_format_five_digits(self):
        assert _is_arxiv_id("2301.12345")

    def test_old_format(self):
        assert _is_arxiv_id("cs/0612047")

    def test_doi_is_not_arxiv(self):
        assert not _is_arxiv_id("10.1145/3290605.3300741")

    def test_bibtex_key_is_not_arxiv(self):
        assert not _is_arxiv_id("vaswani2017attention")

    def test_url_is_not_arxiv(self):
        assert not _is_arxiv_id("https://arxiv.org/abs/2301.00001")

    def test_empty_is_not_arxiv(self):
        assert not _is_arxiv_id("")


# ---------------------------------------------------------------------------
# _paper_url
# ---------------------------------------------------------------------------

class TestPaperUrl:
    def test_stored_url_takes_priority(self):
        assert _paper_url("2301.00001", "https://example.com") == "https://example.com"

    def test_arxiv_id_without_stored_url(self):
        assert _paper_url("2301.00001", None) == "https://arxiv.org/abs/2301.00001"

    def test_doi_without_stored_url_returns_empty(self):
        assert _paper_url("10.1145/xyz", None) == ""

    def test_bibtex_key_without_stored_url_returns_empty(self):
        assert _paper_url("smith2020", None) == ""

    def test_doi_with_stored_url(self):
        assert _paper_url("10.1145/xyz", "https://dl.acm.org/doi/10.1145/xyz") == \
               "https://dl.acm.org/doi/10.1145/xyz"


# ---------------------------------------------------------------------------
# BibTeX — non-arXiv import
# ---------------------------------------------------------------------------

_ACM_BIB = """\
@inproceedings{doe2019deep,
  author    = {Doe, Jane and Smith, Bob},
  title     = {Deep Learning for HCI},
  year      = {2019},
  booktitle = {CHI 2019},
  doi       = {10.1145/3290605.3300741},
  url       = {https://dl.acm.org/doi/10.1145/3290605.3300741},
  abstract  = {An ACM CHI paper.},
}
"""

_NO_DOI_BIB = """\
@article{vaswani2017attention,
  author  = {Vaswani, Ashish and Shazeer, Noam},
  title   = {Attention Is All You Need},
  year    = {2017},
  journal = {NeurIPS 2017},
}
"""

_MIXED_BIB = """\
@article{arxiv2023,
  author = {Alice, A.},
  title  = {An arXiv Paper},
  year   = {2023},
  doi    = {10.48550/arXiv.2301.00001},
}
@inproceedings{doe2019deep,
  author    = {Doe, Jane},
  title     = {Deep Learning for HCI},
  year      = {2019},
  booktitle = {CHI 2019},
  doi       = {10.1145/3290605.3300741},
}
"""


class TestBibTeXNonArxiv:
    def test_doi_used_as_paper_id(self):
        papers = _bib.import_string(_ACM_BIB)
        assert papers[0].paper_id == "10.1145/3290605.3300741"

    def test_doi_field_preserved(self):
        papers = _bib.import_string(_ACM_BIB)
        assert papers[0].doi == "10.1145/3290605.3300741"

    def test_url_preserved(self):
        papers = _bib.import_string(_ACM_BIB)
        assert papers[0].url == "https://dl.acm.org/doi/10.1145/3290605.3300741"

    def test_booktitle_as_journal_ref(self):
        papers = _bib.import_string(_ACM_BIB)
        assert papers[0].journal_ref == "CHI 2019"

    def test_title_preserved(self):
        papers = _bib.import_string(_ACM_BIB)
        assert papers[0].title == "Deep Learning for HCI"

    def test_year_parsed(self):
        papers = _bib.import_string(_ACM_BIB)
        assert papers[0].published.year == 2019

    def test_source_is_bibtex(self):
        papers = _bib.import_string(_ACM_BIB)
        assert papers[0].source == "bibtex"

    def test_no_doi_uses_cite_key(self):
        papers = _bib.import_string(_NO_DOI_BIB)
        assert papers[0].paper_id == "vaswani2017attention"
        assert papers[0].doi is None

    def test_mixed_doi_and_no_doi(self):
        papers = _bib.import_string(_MIXED_BIB)
        ids = {p.paper_id for p in papers}
        assert "10.1145/3290605.3300741" in ids
        assert "10.48550/arXiv.2301.00001" in ids

    def test_export_doi_paper_contains_doi(self):
        out = _bib.export_papers([_doi_paper()])
        assert "10.1145/3290605.3300741" in out

    def test_export_doi_paper_contains_title(self):
        out = _bib.export_papers([_doi_paper()])
        assert "Deep Learning for HCI" in out

    def test_export_key_paper_contains_title(self):
        out = _bib.export_papers([_key_paper()])
        assert "Attention Is All You Need" in out

    def test_bibtex_round_trip_doi_paper(self):
        out = _bib.export_papers([_doi_paper()])
        reimported = _bib.import_string(out)
        assert reimported[0].title == "Deep Learning for HCI"
        assert reimported[0].doi == "10.1145/3290605.3300741"


# ---------------------------------------------------------------------------
# JSON — non-arXiv round-trip
# ---------------------------------------------------------------------------

def _json_round_trip(paper: dict) -> list:
    exported = _jsn.export_papers([paper])
    path = tempfile.NamedTemporaryFile(suffix=".json", mode="w", delete=False, encoding="utf-8")
    path.write(exported)
    path.close()
    try:
        return _jsn.import_file(path.name)
    finally:
        os.unlink(path.name)


class TestJSONNonArxiv:
    def test_doi_paper_id_preserved(self):
        assert _json_round_trip(_doi_paper())[0].paper_id == "10.1145/3290605.3300741"

    def test_key_paper_id_preserved(self):
        assert _json_round_trip(_key_paper())[0].paper_id == "vaswani2017attention"

    def test_doi_field_preserved(self):
        rt = _json_round_trip(_doi_paper())
        assert rt[0].doi == "10.1145/3290605.3300741"

    def test_url_preserved(self):
        rt = _json_round_trip(_doi_paper())
        assert rt[0].url == "https://dl.acm.org/doi/10.1145/3290605.3300741"

    def test_title_preserved(self):
        assert _json_round_trip(_doi_paper())[0].title == "Deep Learning for HCI"

    def test_authors_preserved(self):
        rt = _json_round_trip(_doi_paper())
        assert rt[0].authors == ["Jane Doe", "Bob Smith"]

    def test_tags_preserved(self):
        rt = _json_round_trip(_doi_paper())
        assert rt[0].tags == ["hci", "deep-learning"]

    def test_mixed_sources_in_one_file(self):
        papers = [_doi_paper(), _arxiv_paper()]
        exported = _jsn.export_papers(papers)
        path = tempfile.NamedTemporaryFile(suffix=".json", mode="w", delete=False, encoding="utf-8")
        path.write(exported)
        path.close()
        try:
            rt = _jsn.import_file(path.name)
        finally:
            os.unlink(path.name)
        ids = {p.paper_id for p in rt}
        assert "10.1145/3290605.3300741" in ids
        assert "2301.00001" in ids


# ---------------------------------------------------------------------------
# CSV — non-arXiv round-trip
# ---------------------------------------------------------------------------

def _csv_round_trip(paper: dict) -> list:
    exported = _csv.export_papers([paper])
    f = tempfile.NamedTemporaryFile(suffix=".csv", mode="w", delete=False,
                                    encoding="utf-8", newline="")
    f.write(exported)
    f.close()
    try:
        return _csv.import_file(f.name)
    finally:
        os.unlink(f.name)


class TestCSVNonArxiv:
    def test_doi_paper_id_preserved(self):
        assert _csv_round_trip(_doi_paper())[0].paper_id == "10.1145/3290605.3300741"

    def test_key_paper_id_preserved(self):
        assert _csv_round_trip(_key_paper())[0].paper_id == "vaswani2017attention"

    def test_title_preserved(self):
        assert _csv_round_trip(_doi_paper())[0].title == "Deep Learning for HCI"

    def test_authors_preserved(self):
        rt = _csv_round_trip(_doi_paper())
        assert rt[0].authors == ["Jane Doe", "Bob Smith"]

    def test_tags_preserved(self):
        rt = _csv_round_trip(_doi_paper())
        assert rt[0].tags == ["hci", "deep-learning"]

    def test_doi_with_slash_in_paper_id(self):
        # DOIs contain slashes — must survive CSV quoting
        rt = _csv_round_trip(_doi_paper())
        assert "/" in rt[0].paper_id

    def test_mixed_sources_row_count(self):
        exported = _csv.export_papers([_doi_paper(), _arxiv_paper()])
        rows = list(csv.DictReader(io.StringIO(exported)))
        assert len(rows) == 2
        ids = {r["paper_id"] for r in rows}
        assert "10.1145/3290605.3300741" in ids
        assert "2301.00001" in ids


# ---------------------------------------------------------------------------
# Markdown — non-arXiv export correctness
# ---------------------------------------------------------------------------

class TestMarkdownNonArxivExport:
    def test_does_not_use_fake_arxiv_url_for_doi_paper(self):
        out = _md.export_papers([_doi_paper()])
        assert "arxiv.org/abs/10.1145" not in out

    def test_uses_stored_url_for_doi_paper(self):
        out = _md.export_papers([_doi_paper()])
        assert "dl.acm.org" in out

    def test_paper_id_line_written_for_doi_paper(self):
        out = _md.export_papers([_doi_paper()])
        assert "Paper-ID: 10.1145/3290605.3300741" in out

    def test_paper_id_line_written_for_key_paper(self):
        out = _md.export_papers([_key_paper()])
        assert "Paper-ID: vaswani2017attention" in out

    def test_no_paper_id_line_for_arxiv_paper(self):
        out = _md.export_papers([_arxiv_paper()])
        assert "Paper-ID:" not in out

    def test_arxiv_paper_uses_arxiv_url(self):
        out = _md.export_papers([_arxiv_paper()])
        assert "arxiv.org/abs/2301.00001" in out

    def test_key_paper_no_url_has_empty_link(self):
        out = _md.export_papers([_key_paper()])
        assert "]()" in out

    def test_title_present_for_doi_paper(self):
        out = _md.export_papers([_doi_paper()])
        assert "Deep Learning for HCI" in out


# ---------------------------------------------------------------------------
# Markdown — non-arXiv round-trip
# ---------------------------------------------------------------------------

class TestMarkdownNonArxivRoundTrip:
    def _rt(self, paper: dict):
        return _md.import_string(_md.export_papers([paper]))

    def test_doi_paper_id_preserved(self):
        assert self._rt(_doi_paper())[0].paper_id == "10.1145/3290605.3300741"

    def test_key_paper_id_preserved(self):
        assert self._rt(_key_paper())[0].paper_id == "vaswani2017attention"

    def test_arxiv_paper_id_preserved(self):
        assert self._rt(_arxiv_paper())[0].paper_id == "2301.00001"

    def test_doi_title_preserved(self):
        assert self._rt(_doi_paper())[0].title == "Deep Learning for HCI"

    def test_key_title_preserved(self):
        assert self._rt(_key_paper())[0].title == "Attention Is All You Need"

    def test_doi_authors_preserved(self):
        rt = self._rt(_doi_paper())
        assert rt[0].authors == ["Jane Doe", "Bob Smith"]

    def test_doi_tags_preserved(self):
        assert self._rt(_doi_paper())[0].tags == ["hci", "deep-learning"]

    def test_doi_url_preserved(self):
        rt = self._rt(_doi_paper())
        assert rt[0].url == "https://dl.acm.org/doi/10.1145/3290605.3300741"

    def test_mixed_sources_both_ids_correct(self):
        papers = [_doi_paper(), _arxiv_paper()]
        rt = _md.import_string(_md.export_papers(papers))
        ids = {p.paper_id for p in rt}
        assert "10.1145/3290605.3300741" in ids
        assert "2301.00001" in ids

    def test_three_sources_mixed(self):
        papers = [_doi_paper(), _key_paper(), _arxiv_paper()]
        rt = _md.import_string(_md.export_papers(papers))
        ids = {p.paper_id for p in rt}
        assert ids == {"10.1145/3290605.3300741", "vaswani2017attention", "2301.00001"}


# ---------------------------------------------------------------------------
# Obsidian — non-arXiv export correctness
# ---------------------------------------------------------------------------

class TestObsidianNonArxivExport:
    def test_does_not_use_fake_arxiv_url_for_doi_paper(self):
        out = _obs.export_papers([_doi_paper()])
        assert "arxiv.org/abs/10.1145" not in out

    def test_uses_stored_url_for_doi_paper(self):
        out = _obs.export_papers([_doi_paper()])
        assert "dl.acm.org" in out

    def test_paper_id_line_written_for_doi_paper(self):
        out = _obs.export_papers([_doi_paper()])
        assert "**Paper-ID:** 10.1145/3290605.3300741" in out

    def test_paper_id_line_written_for_key_paper(self):
        out = _obs.export_papers([_key_paper()])
        assert "**Paper-ID:** vaswani2017attention" in out

    def test_no_paper_id_line_for_arxiv_paper(self):
        out = _obs.export_papers([_arxiv_paper()])
        assert "**Paper-ID:**" not in out

    def test_arxiv_paper_uses_arxiv_url(self):
        out = _obs.export_papers([_arxiv_paper()])
        assert "arxiv.org/abs/2301.00001" in out


# ---------------------------------------------------------------------------
# Obsidian — non-arXiv round-trip
# ---------------------------------------------------------------------------

class TestObsidianNonArxivRoundTrip:
    def _rt(self, paper: dict):
        return _obs.import_string(_obs.export_papers([paper]))

    def test_doi_paper_id_preserved(self):
        assert self._rt(_doi_paper())[0].paper_id == "10.1145/3290605.3300741"

    def test_key_paper_id_preserved(self):
        assert self._rt(_key_paper())[0].paper_id == "vaswani2017attention"

    def test_arxiv_paper_id_preserved(self):
        assert self._rt(_arxiv_paper())[0].paper_id == "2301.00001"

    def test_doi_title_preserved(self):
        assert self._rt(_doi_paper())[0].title == "Deep Learning for HCI"

    def test_doi_authors_preserved(self):
        rt = self._rt(_doi_paper())
        assert rt[0].authors == ["Jane Doe", "Bob Smith"]

    def test_doi_tags_preserved(self):
        assert self._rt(_doi_paper())[0].tags == ["hci", "deep-learning"]

    def test_doi_url_preserved(self):
        rt = self._rt(_doi_paper())
        assert rt[0].url == "https://dl.acm.org/doi/10.1145/3290605.3300741"

    def test_mixed_sources_both_ids_correct(self):
        papers = [_doi_paper(), _arxiv_paper()]
        rt = _obs.import_string(_obs.export_papers(papers))
        ids = {p.paper_id for p in rt}
        assert "10.1145/3290605.3300741" in ids
        assert "2301.00001" in ids

    def test_three_sources_mixed(self):
        papers = [_doi_paper(), _key_paper(), _arxiv_paper()]
        rt = _obs.import_string(_obs.export_papers(papers))
        ids = {p.paper_id for p in rt}
        assert ids == {"10.1145/3290605.3300741", "vaswani2017attention", "2301.00001"}


# ---------------------------------------------------------------------------
# Graph export contract — getSelectedPaperData() shape
#
# graph.js builds paper dicts with a fixed set of fields. These tests use that
# exact shape to ensure the format layer handles missing/null url correctly and
# that Paper-ID round-trips survive even when url is absent from the payload.
# ---------------------------------------------------------------------------

def _graph_paper(paper_id: str, title: str, url=None, doi=None, **kwargs) -> dict:
    """Mirrors the shape emitted by getSelectedPaperData() in graph.js."""
    return {
        "paper_id":  paper_id,
        "title":     title,
        "category":  kwargs.get("category", ""),
        "tags":      kwargs.get("tags", []),
        "has_pdf":   kwargs.get("has_pdf", False),
        "published": kwargs.get("published", "2023-01-01"),
        "authors":   kwargs.get("authors", []),
        "url":       url,
        "doi":       doi,
        "summary":   kwargs.get("summary", ""),
    }


class TestGraphExportContract:
    """Ensure format exporters handle the exact dict shape graph.js produces."""

    # arXiv paper — url is None but paper_id is an arXiv ID
    def test_arxiv_paper_no_url_markdown(self):
        p = _graph_paper("2301.00001", "A Paper")
        out = _md.export_papers([p])
        assert "arxiv.org/abs/2301.00001" in out
        assert "Paper-ID:" not in out

    def test_arxiv_paper_no_url_obsidian(self):
        p = _graph_paper("2301.00001", "A Paper")
        out = _obs.export_papers([p])
        assert "arxiv.org/abs/2301.00001" in out
        assert "**Paper-ID:**" not in out

    def test_arxiv_paper_round_trip_markdown(self):
        p = _graph_paper("2301.00001", "A Paper")
        rt = _md.import_string(_md.export_papers([p]))
        assert rt[0].paper_id == "2301.00001"

    def test_arxiv_paper_round_trip_obsidian(self):
        p = _graph_paper("2301.00001", "A Paper")
        rt = _obs.import_string(_obs.export_papers([p]))
        assert rt[0].paper_id == "2301.00001"

    # DOI paper — url is populated by graph.js
    def test_doi_paper_with_url_markdown(self):
        p = _graph_paper("10.1145/3290605.3300741", "CHI Paper",
                         url="https://dl.acm.org/doi/10.1145/3290605.3300741")
        out = _md.export_papers([p])
        assert "dl.acm.org" in out
        assert "arxiv.org/abs/10.1145" not in out

    def test_doi_paper_with_url_round_trip_markdown(self):
        p = _graph_paper("10.1145/3290605.3300741", "CHI Paper",
                         url="https://dl.acm.org/doi/10.1145/3290605.3300741")
        rt = _md.import_string(_md.export_papers([p]))
        assert rt[0].paper_id == "10.1145/3290605.3300741"
        assert rt[0].url == "https://dl.acm.org/doi/10.1145/3290605.3300741"

    def test_doi_paper_with_url_round_trip_obsidian(self):
        p = _graph_paper("10.1145/3290605.3300741", "CHI Paper",
                         url="https://dl.acm.org/doi/10.1145/3290605.3300741")
        rt = _obs.import_string(_obs.export_papers([p]))
        assert rt[0].paper_id == "10.1145/3290605.3300741"

    # DOI paper — url is None (shouldn't happen after our db fix, but defensive)
    def test_doi_paper_null_url_markdown_no_fake_arxiv_link(self):
        p = _graph_paper("10.1145/3290605.3300741", "CHI Paper", url=None)
        out = _md.export_papers([p])
        assert "arxiv.org/abs/10.1145" not in out
        assert "Paper-ID: 10.1145/3290605.3300741" in out

    def test_doi_paper_null_url_round_trip_markdown(self):
        p = _graph_paper("10.1145/3290605.3300741", "CHI Paper", url=None)
        rt = _md.import_string(_md.export_papers([p]))
        assert rt[0].paper_id == "10.1145/3290605.3300741"

    def test_doi_paper_null_url_round_trip_obsidian(self):
        p = _graph_paper("10.1145/3290605.3300741", "CHI Paper", url=None)
        rt = _obs.import_string(_obs.export_papers([p]))
        assert rt[0].paper_id == "10.1145/3290605.3300741"

    # Mixed arXiv + non-arXiv from graph export
    def test_mixed_graph_export_markdown(self):
        papers = [
            _graph_paper("2301.00001", "arXiv Paper"),
            _graph_paper("10.1145/xyz", "ACM Paper",
                         url="https://dl.acm.org/doi/10.1145/xyz"),
        ]
        rt = _md.import_string(_md.export_papers(papers))
        ids = {p.paper_id for p in rt}
        assert ids == {"2301.00001", "10.1145/xyz"}

    def test_mixed_graph_export_obsidian(self):
        papers = [
            _graph_paper("2301.00001", "arXiv Paper"),
            _graph_paper("10.1145/xyz", "ACM Paper",
                         url="https://dl.acm.org/doi/10.1145/xyz"),
        ]
        rt = _obs.import_string(_obs.export_papers(papers))
        ids = {p.paper_id for p in rt}
        assert ids == {"2301.00001", "10.1145/xyz"}
