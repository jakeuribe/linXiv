"""Tests for formats/json_fmt.py — import/export round-trips and edge cases."""

from __future__ import annotations

import datetime
import json
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from formats.json_fmt import JSONFormat, _parse_date, _parse_list

_fmt = JSONFormat()


# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

class TestParseDate:
    def test_iso_string(self):
        assert _parse_date("2023-06-15") == datetime.date(2023, 6, 15)

    def test_iso_string_with_time(self):
        assert _parse_date("2023-06-15T10:00:00") == datetime.date(2023, 6, 15)

    def test_date_object_passthrough(self):
        d = datetime.date(2021, 3, 1)
        assert _parse_date(d) == d

    def test_invalid_string_falls_back(self):
        assert _parse_date("not-a-date") == datetime.date(1900, 1, 1)

    def test_empty_string_falls_back(self):
        assert _parse_date("") == datetime.date(1900, 1, 1)

    def test_none_falls_back(self):
        assert _parse_date(None) == datetime.date(1900, 1, 1)


class TestParseList:
    def test_list_passthrough(self):
        assert _parse_list(["a", "b"]) == ["a", "b"]

    def test_semicolon_separated_string(self):
        assert _parse_list("a; b; c") == ["a", "b", "c"]

    def test_comma_separated_string(self):
        assert _parse_list("a,b,c", sep=",") == ["a", "b", "c"]

    def test_empty_string_returns_empty(self):
        assert _parse_list("") == []

    def test_none_returns_empty(self):
        assert _parse_list(None) == []


# ---------------------------------------------------------------------------
# export_papers
# ---------------------------------------------------------------------------

def _make_paper(**overrides) -> dict:
    base = {
        "paper_id":   "2301.00001",
        "version":    1,
        "title":      "Test Paper",
        "authors":    ["Alice Author", "Bob Builder"],
        "published":  datetime.date(2023, 1, 1),
        "summary":    "A test abstract.",
        "category":   "cs.LG",
        "tags":       ["ml", "nlp"],
        "doi":        None,
        "journal_ref": None,
        "url":        None,
        "source":     "test",
    }
    base.update(overrides)
    return base


class TestExportPapers:
    def test_output_is_valid_json(self):
        out = _fmt.export_papers([_make_paper()])
        parsed = json.loads(out)
        assert "papers" in parsed

    def test_single_paper_fields(self):
        out = _fmt.export_papers([_make_paper()])
        p = json.loads(out)["papers"][0]
        assert p["paper_id"] == "2301.00001"
        assert p["title"] == "Test Paper"
        assert p["authors"] == ["Alice Author", "Bob Builder"]
        assert p["published"] == "2023-01-01"

    def test_date_serialized_as_iso_string(self):
        out = _fmt.export_papers([_make_paper(published=datetime.date(2022, 5, 10))])
        p = json.loads(out)["papers"][0]
        assert p["published"] == "2022-05-10"

    def test_multiple_papers(self):
        papers = [_make_paper(paper_id="a"), _make_paper(paper_id="b")]
        out = _fmt.export_papers(papers)
        assert len(json.loads(out)["papers"]) == 2

    def test_empty_list(self):
        out = _fmt.export_papers([])
        assert json.loads(out) == {"papers": []}


# ---------------------------------------------------------------------------
# import_file
# ---------------------------------------------------------------------------

def _write_temp_json(data: dict) -> str:
    f = tempfile.NamedTemporaryFile(suffix=".json", mode="w", delete=False, encoding="utf-8")
    json.dump(data, f)
    f.close()
    return f.name


class TestImportFile:
    def test_basic_fields(self):
        path = _write_temp_json({"papers": [{
            "paper_id": "2301.00001",
            "title": "Hello World",
            "authors": ["A. Author"],
            "published": "2023-01-01",
            "summary": "Abstract here.",
            "category": "cs.AI",
            "source": "test",
        }]})
        try:
            papers = _fmt.import_file(path)
            assert len(papers) == 1
            p = papers[0]
            assert p.paper_id == "2301.00001"
            assert p.title == "Hello World"
            assert p.authors == ["A. Author"]
            assert p.published == datetime.date(2023, 1, 1)
            assert p.summary == "Abstract here."
            assert p.category == "cs.AI"
        finally:
            os.unlink(path)

    def test_flat_list_without_papers_key(self):
        path = _write_temp_json([{"paper_id": "x", "title": "T", "authors": []}])
        try:
            papers = _fmt.import_file(path)
            assert len(papers) == 1
            assert papers[0].paper_id == "x"
        finally:
            os.unlink(path)

    def test_tags_parsed(self):
        path = _write_temp_json({"papers": [{
            "paper_id": "1", "title": "T", "authors": [], "tags": ["a", "b"],
        }]})
        try:
            papers = _fmt.import_file(path)
            assert papers[0].tags == ["a", "b"]
        finally:
            os.unlink(path)

    def test_missing_optional_fields_are_none(self):
        path = _write_temp_json({"papers": [{"paper_id": "1", "title": "T", "authors": []}]})
        try:
            papers = _fmt.import_file(path)
            p = papers[0]
            assert p.doi is None
            assert p.url is None
            assert p.tags is None
        finally:
            os.unlink(path)

    def test_multiple_entries(self):
        path = _write_temp_json({"papers": [
            {"paper_id": "1", "title": "A", "authors": []},
            {"paper_id": "2", "title": "B", "authors": []},
        ]})
        try:
            papers = _fmt.import_file(path)
            assert len(papers) == 2
        finally:
            os.unlink(path)


# ---------------------------------------------------------------------------
# round-trip
# ---------------------------------------------------------------------------

class TestRoundTrip:
    def _round_trip(self, paper: dict):
        exported = _fmt.export_papers([paper])
        path = tempfile.NamedTemporaryFile(suffix=".json", mode="w", delete=False, encoding="utf-8")
        path.write(exported)
        path.close()
        try:
            return _fmt.import_file(path.name)
        finally:
            os.unlink(path.name)

    def test_preserves_paper_id(self):
        assert self._round_trip(_make_paper())[0].paper_id == "2301.00001"

    def test_preserves_title(self):
        assert self._round_trip(_make_paper())[0].title == "Test Paper"

    def test_preserves_authors(self):
        rt = self._round_trip(_make_paper())
        assert rt[0].authors == ["Alice Author", "Bob Builder"]

    def test_preserves_published_date(self):
        rt = self._round_trip(_make_paper())
        assert rt[0].published == datetime.date(2023, 1, 1)

    def test_preserves_tags(self):
        rt = self._round_trip(_make_paper())
        assert rt[0].tags == ["ml", "nlp"]

    def test_preserves_category(self):
        assert self._round_trip(_make_paper())[0].category == "cs.LG"

    def test_two_papers_round_trip(self):
        papers = [_make_paper(paper_id="a", title="A"), _make_paper(paper_id="b", title="B")]
        exported = _fmt.export_papers(papers)
        path = tempfile.NamedTemporaryFile(suffix=".json", mode="w", delete=False, encoding="utf-8")
        path.write(exported)
        path.close()
        try:
            rt = _fmt.import_file(path.name)
        finally:
            os.unlink(path.name)
        assert {p.paper_id for p in rt} == {"a", "b"}
